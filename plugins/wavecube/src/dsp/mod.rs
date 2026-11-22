//! The audio engine: a chain of up to 12 items. Every shaper lane has an LFO per band (linked or
//! not), reading its baked shape with swing and a left / right offset, plus an envelope
//! follower; the lane value drives the item's effect per band.
//!
//! Click safety: items fade in / out over 8 ms; offsets, mixes, knobs and gains are smoothed;
//! delay lines never read audio older than what was written since the item started.

pub mod blocks;
pub mod media;
pub mod noise;
pub mod reverb;

use crate::kinds::{length_beats, Kind};
use crate::wave::TABLE;
use crate::{AudioTrig, Shared, SlotSet, State, WaveCubeParams, N_BANDS, N_CUSTOM, N_KNOBS, N_LANES, N_SLOTS, RING_N, SCOPE_N};
use blocks::*;
use nih_plug::prelude::Param;
use noise::{Gen, NoiseCoefs};
use reverb::Reverb;
use std::f32::consts::PI;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// longest Time delay (seconds)
const TIME_MAX_S: f32 = 16.0;
const HAAS_MAX_S: f32 = 0.045;
const FLANGE_MAX_S: f32 = 0.02;

/// The engine's copy of the saved state (plain data: no allocation on the audio thread).
#[derive(Clone, Copy)]
struct Snap {
    sets: [SlotSet; N_SLOTS],
    chain: [usize; N_SLOTS],
    chain_len: usize,
    trig: AudioTrig,
    midi_channel: u8,
    grid: f32,
}

impl Snap {
    fn of(st: &State) -> Self {
        let mut n = Snap { sets: [SlotSet::default(); N_SLOTS], chain: [0; N_SLOTS], chain_len: 0, trig: st.trig, midi_channel: st.midi_channel, grid: st.grid.max(1) as f32 };
        for i in 0..N_SLOTS {
            n.sets[i] = st.slots.get(i).copied().unwrap_or_default();
        }
        let mut seen = [false; N_SLOTS];
        for &s in st.chain.iter() {
            let s = s as usize;
            if s < N_SLOTS && !seen[s] && n.chain_len < N_SLOTS {
                n.chain[n.chain_len] = s;
                n.chain_len += 1;
                seen[s] = true;
            }
        }
        n
    }
}

/// Reads `d` samples back from the newest sample `w` (cubic between samples).
#[inline]
fn time_read(buf: &[[f32; 2]], w: usize, d: f32, ch: usize, now: f32) -> f32 {
    if d < 1e-3 {
        now
    } else if d < 2.0 {
        ring_read(buf, w, d, ch)
    } else {
        let n = buf.len() as isize;
        let pos = w as f32 - d;
        let i = pos.floor();
        let t = pos - i;
        let i = i as isize;
        let at = |k: isize| buf[(i + k).rem_euclid(n) as usize][ch];
        let (xm1, x0, x1, x2) = (at(-1), at(0), at(1), at(2));
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }
}

pub struct Transport {
    pub tempo: f64,
    pub pos_beats: Option<f64>,
    pub playing: bool,
}

// ------------------------------------------------------------------------------------------
// Big buffers, made off the audio thread
// ------------------------------------------------------------------------------------------

pub struct Heavy {
    pub tag: u64,
    pub time: Vec<Vec<[f32; 2]>>,
    pub reverbs: Vec<Reverb>,
}

/// Which big buffers a slot needs (0 = none).
pub fn heavy_tag(s: &SlotSet, media_version: u32) -> u64 {
    match s.kind {
        Kind::Time => 1 | ((s.multiband as u64) << 4),
        Kind::Reverb => {
            let user = if s.r_type >= 1000 { media_version as u64 } else { 0 };
            2 | ((s.r_type as u64) << 8) | (user << 24)
        }
        _ => 0,
    }
}

pub fn build_heavy(s: &SlotSet, sr: f32, shared: &Shared) -> Option<Box<Heavy>> {
    let mv = shared.media_version.load(Ordering::Acquire);
    let tag = heavy_tag(s, mv);
    match s.kind {
        Kind::Time => Some(Box::new(Heavy { tag, time: (0..if s.multiband { N_BANDS } else { 1 }).map(|_| vec![[0.0; 2]; (TIME_MAX_S * sr) as usize + 8]).collect(), reverbs: vec![] })),
        Kind::Reverb => {
            let ir = if s.r_type >= 1000 { shared.irs.lock().get((s.r_type - 1000) as usize).map(|x| media::resample(x, sr)) } else { None };
            Some(Box::new(Heavy { tag, time: vec![], reverbs: (0..N_BANDS).map(|_| Reverb::new(sr, ir.as_deref())).collect() }))
        }
        _ => None,
    }
}

/// GUI side: hands the audio thread the big buffers the chain needs (call every frame).
/// `provided[slot]` remembers what was already sent.
pub fn provide_heavy(st: &State, shared: &Shared, provided: &mut [u64; N_SLOTS]) {
    let sr = shared.sr.load(Ordering::Relaxed);
    let mv = shared.media_version.load(Ordering::Acquire);
    for &s in &st.chain {
        let s = s as usize;
        let tag = heavy_tag(&st.slots[s], mv);
        if tag != 0 && provided[s] != tag {
            if let Some(h) = build_heavy(&st.slots[s], sr, shared) {
                let mut q = shared.heavy_in.lock();
                q.retain(|(slot, _)| *slot != s);
                if q.len() < q.capacity() {
                    q.push((s, h));
                    provided[s] = tag;
                }
            }
        }
    }
    // free what the audio thread handed back
    let old: Vec<Box<Heavy>> = std::mem::take(&mut *shared.heavy_out.lock());
    drop(old);
    shared.heavy_out.lock().reserve(64);
}

// ------------------------------------------------------------------------------------------
// Per-slot state
// ------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct LfoDsp {
    phase: f64,
    inc: f64,
    done: bool,
    y: [f32; 2],
    primed: bool,
    smooth_a: f32,
}

#[derive(Clone, Copy)]
struct EnvDsp {
    env: f32,
    hold: u32,
    peak: f32,
    hp: Svf,
    lp: Svf,
    // per-block coefficients
    a_att: f32,
    a_rel: f32,
    a_fast: f32,
    a_slow: f32,
    hold_n: u32,
    c_hp: SvfCoef,
    c_lp: SvfCoef,
}

impl Default for EnvDsp {
    fn default() -> Self {
        Self {
            env: 0.0,
            hold: 0,
            peak: 0.0,
            hp: Svf::default(),
            lp: Svf::default(),
            a_att: 1.0,
            a_rel: 1.0,
            a_fast: 1.0,
            a_slow: 1.0,
            hold_n: 0,
            c_hp: SvfCoef::new(20.0, 48000.0, 1.4),
            c_lp: SvfCoef::new(20000.0, 48000.0, 1.4),
        }
    }
}

struct BandFx {
    svf: [[Svf; 8]; 2],
    fc: [Option<(f32, f32, SvfCoef, SvfCoef)>; 2],
    hold: [f32; 2],
    count: f32,
    lp: [f32; 2],
    aa: [[f32; 2]; 2],
    dc: [(f32, f32); 2],
    grip: f32,
    haas: Vec<[f32; 2]>,
    haas_w: usize,
    flange: Vec<[f32; 2]>,
    flange_w: usize,
    fb: [f32; 2],
    comp: f32,
    comp_hold: u32,
    comp_peak: f32,
    comp_hp: Svf,
    comp_lp: Svf,
    /// cached: (attack ms, release ms, hold ms) -> (att, rel, fast, slow, hold samples)
    comp_k: (f32, f32, f32, [f32; 4], u32),
    /// cached phaser stage coefficients: (centre, k, spread, stages)
    ph_k: [(f32, f32, f32, [SvfCoef; 8]); 2],
    /// cached crush rate -> anti-alias coefficient
    aa_k: (f32, f32),
    /// cached crush resample lane value -> rate
    rate_k: (f32, f32),
    last_vol: f32,
    valid: usize,
    // Time: current and previous read position, crossfade left
    td: [f32; 2],
    told: [f32; 2],
    tx: [f32; 2],
    tprimed: bool,
}

impl BandFx {
    fn new(sr: f32) -> Self {
        Self {
            svf: Default::default(),
            fc: [None; 2],
            hold: [0.0; 2],
            count: 0.0,
            lp: [0.0; 2],
            aa: [[0.0; 2]; 2],
            dc: [(0.0, 0.0); 2],
            grip: 0.0,
            haas: vec![[0.0; 2]; (HAAS_MAX_S * sr) as usize + 8],
            haas_w: 0,
            flange: vec![[0.0; 2]; (FLANGE_MAX_S * sr) as usize + 8],
            flange_w: 0,
            fb: [0.0; 2],
            comp: 0.0,
            comp_hold: 0,
            comp_peak: 0.0,
            comp_hp: Svf::default(),
            comp_lp: Svf::default(),
            comp_k: (-1.0, -1.0, -1.0, [1.0; 4], 0),
            ph_k: [(-1.0, -1.0, -1.0, [SvfCoef::new(1000.0, sr, 1.0); 8]); 2],
            aa_k: (-1.0, 1.0),
            rate_k: (-1.0, 1.0),
            last_vol: 1.0,
            valid: 0,
            td: [0.0; 2],
            told: [0.0; 2],
            tx: [0.0; 2],
            tprimed: false,
        }
    }

    fn clear(&mut self) {
        self.svf = Default::default();
        self.fc = [None; 2];
        self.hold = [0.0; 2];
        self.count = 0.0;
        self.lp = [0.0; 2];
        self.aa = [[0.0; 2]; 2];
        self.dc = [(0.0, 0.0); 2];
        self.grip = 0.0;
        self.haas.iter_mut().for_each(|v| *v = [0.0; 2]);
        self.flange.iter_mut().for_each(|v| *v = [0.0; 2]);
        self.fb = [0.0; 2];
        self.comp = 0.0;
        self.comp_peak = 0.0;
        self.valid = 0;
        self.tx = [0.0; 2];
        self.tprimed = false;
    }
}

struct SlotDsp {
    kind: Kind,
    active: bool,
    fade: f32,
    lfo: [[LfoDsp; N_BANDS]; N_LANES],
    env: [[EnvDsp; N_BANDS]; N_LANES],
    /// lane value per band and channel (after envelope and offset)
    val: [[[f32; 2]; N_BANDS]; N_LANES],
    split: [Split3; 2],
    noise_split: [Split3; 2],
    c1: SvfCoef,
    c2: SvfCoef,
    fx: Vec<BandFx>,
    heavy: Option<Box<Heavy>>,
    time_w: usize,
    time_len: f32,
    gens: [Gen; 3],
    hum_ph: f32,
    nf: [[Svf; 2]; 2],
    nc: (SvfCoef, SvfCoef),
    user: Option<Arc<media::Sample>>,
    /// n_type the user sample was looked up for
    user_t: u16,
    user_pos: f64,
    // smoothed controls
    tile: [f32; N_LANES],
    mix: [f32; N_BANDS],
    knob: [f32; N_KNOBS],
    primed: bool,
    was_multi: bool,
    solo: u32,
    gr: f32,
    /// samples to run silently after switching on (delay lines fill up first)
    warm: u32,
}

impl SlotDsp {
    fn new(sr: f32) -> Self {
        Self {
            kind: Kind::Volume,
            active: false,
            fade: 0.0,
            lfo: [[LfoDsp::default(); N_BANDS]; N_LANES],
            env: [[EnvDsp::default(); N_BANDS]; N_LANES],
            val: [[[1.0; 2]; N_BANDS]; N_LANES],
            split: Default::default(),
            noise_split: Default::default(),
            c1: SvfCoef::new(200.0, sr, 1.4),
            c2: SvfCoef::new(2500.0, sr, 1.4),
            fx: (0..N_BANDS).map(|_| BandFx::new(sr)).collect(),
            heavy: None,
            time_w: 0,
            time_len: sr,
            gens: [Gen::default(); 3],
            hum_ph: 0.0,
            nf: Default::default(),
            nc: (SvfCoef::new(120.0, sr, 1.4), SvfCoef::new(16000.0, sr, 1.4)),
            user: None,
            user_t: 0,
            user_pos: 0.0,
            tile: [0.0; N_LANES],
            mix: [1.0; N_BANDS],
            knob: [0.5; N_KNOBS],
            primed: false,
            was_multi: false,
            solo: 0,
            gr: 0.0,
            warm: 0,
        }
    }

    /// Fresh state when the item starts running (nothing old comes back).
    fn activate(&mut self) {
        self.primed = false;
        self.split = Default::default();
        self.noise_split = Default::default();
        for f in self.fx.iter_mut() {
            f.clear();
        }
        for l in self.lfo.iter_mut().flatten() {
            l.primed = false;
        }
        for e in self.env.iter_mut().flatten() {
            *e = EnvDsp::default();
        }
        if let Some(h) = self.heavy.as_mut() {
            for r in h.reverbs.iter_mut() {
                r.clear();
            }
        }
    }
}

/// The automatable controls as plain values (read from the parameters once per block; tests
/// fill it directly).
#[derive(Clone, Copy)]
pub struct Controls {
    pub bypass: bool,
    pub in_db: f32,
    pub out_db: f32,
    pub master: f32,
    pub trig_db: f32,
    pub midi_switch: bool,
    pub safe_clip: bool,
    pub on: [bool; N_SLOTS],
    pub tile: [[f32; N_LANES]; N_SLOTS],
    pub mix: [f32; N_SLOTS],
    pub band_mix: [[f32; N_BANDS]; N_SLOTS],
    /// normalized knob values
    pub knob: [[f32; N_KNOBS]; N_SLOTS],
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            bypass: false,
            in_db: 0.0,
            out_db: 0.0,
            master: 1.0,
            trig_db: -24.0,
            midi_switch: false,
            safe_clip: false,
            on: [true; N_SLOTS],
            tile: [[0.0; N_LANES]; N_SLOTS],
            mix: [1.0; N_SLOTS],
            band_mix: [[1.0; N_BANDS]; N_SLOTS],
            knob: [[0.5; N_KNOBS]; N_SLOTS],
        }
    }
}

impl Controls {
    pub fn from_params(p: &WaveCubeParams) -> Self {
        let mut c = Self {
            bypass: p.bypass.value(),
            in_db: p.in_gain.value(),
            out_db: p.out_gain.value(),
            master: p.mix.value(),
            trig_db: p.trig_db.value(),
            midi_switch: p.midi_switch.value(),
            safe_clip: p.safe_clip.value(),
            ..Default::default()
        };
        for (s, sp) in p.slots.iter().enumerate() {
            c.on[s] = sp.on.value();
            c.tile[s] = [sp.tile_a.value(), sp.tile_b.value()];
            c.mix[s] = sp.mix.value();
            for b in 0..N_BANDS {
                c.band_mix[s][b] = sp.band_mix(b).value();
            }
            for k in 0..N_KNOBS {
                c.knob[s][k] = sp.knob(k).unmodulated_normalized_value();
            }
        }
        c
    }

    /// Knob defaults of a kind (normalized) into a slot.
    pub fn defaults_for(&mut self, slot: usize, kind: Kind) {
        for (i, d) in kind.knobs().iter().enumerate() {
            self.knob[slot][i] = d.default_norm();
        }
    }
}

/// Per-block plain values of a slot's automatable controls.
#[derive(Clone, Copy, Default)]
struct SlotCtl {
    on: bool,
    tile: [f32; N_LANES],
    mix: f32,
    band_mix: [f32; N_BANDS],
    /// plain knob values (in the kind's units)
    knob: [f32; N_KNOBS],
}

// ------------------------------------------------------------------------------------------
// Engine
// ------------------------------------------------------------------------------------------

pub struct Engine {
    sr: f32,
    tempo: f32,
    wave_version: u32,
    set_version: u32,
    media_version: u32,
    sets: [SlotSet; N_SLOTS],
    chain: [usize; N_SLOTS],
    chain_len: usize,
    in_chain: [bool; N_SLOTS],
    trig: AudioTrig,
    midi_channel: u8,
    grid: f32,
    tables: Box<[[[[f32; TABLE]; N_BANDS]; N_LANES]; N_SLOTS]>,
    custom: Box<[[f32; TABLE]; N_CUSTOM]>,
    custom_dest: [Option<(usize, usize, usize)>; N_CUSTOM],
    switch_count: u32,
    sl: Vec<SlotDsp>,
    ctl: [SlotCtl; N_SLOTS],
    rng: Rng,
    nco: NoiseCoefs,
    // global controls
    bypass: bool,
    in_gain: f32,
    out_gain: f32,
    master: f32,
    trig_lvl: f32,
    midi_switch: bool,
    safe_clip: bool,
    in_s: f32,
    out_s: f32,
    wet: f32,
    k_fade: f32,
    k_param: f32,
    /// Time: jump cross-fade step
    k_xf: f32,
    /// structural changes wait in `pending` while the output fades to dry, then fade back in
    pending: Option<Snap>,
    /// has made sound (before that, changes apply at once)
    started: bool,
    duck: f32,
    /// after a structural change: stay dry while the new items' delay lines fill
    duck_hold: u32,
    k_duck: f32,
    k_2500: f32,
    k_1200: f32,
    k_dc: f32,
    // audio trigger
    tr_fast: f32,
    tr_slow: f32,
    tr_hold: u32,
    tr_delay: u32,
    tr_f: [Svf; 2],
    tr_c: (SvfCoef, SvfCoef),
    follow: f32,
    note_hz: f64,
    /// some LFO in the chain is audio-triggered (else the detector sleeps)
    need_trig: bool,
    /// per-block trigger / follow coefficients: fast release, slow attack, slow release, follow attack, follow release
    k_tr: [f32; 5],
    // scope
    view: usize,
    view_lane: usize,
    view_band: usize,
    bin: usize,
    acc_in: f32,
    acc_out: f32,
    pk_in: f32,
    pk_out: f32,
    listen: f32,
    ring_buf: [[f32; 2]; 256],
    ring_n: usize,
    /// Scope tool: its own beat-synced phase
    tool_ph: f64,
    tool_inc: f64,
}

fn boxed<T: Clone, const N: usize>(v: T) -> Box<[T; N]> {
    let b: Box<[T]> = vec![v; N].into_boxed_slice();
    match b.try_into() {
        Ok(a) => a,
        Err(_) => unreachable!(),
    }
}

impl Engine {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            tempo: 120.0,
            wave_version: 0,
            set_version: 0,
            media_version: u32::MAX,
            sets: [SlotSet::default(); N_SLOTS],
            chain: [0; N_SLOTS],
            chain_len: 0,
            in_chain: [false; N_SLOTS],
            trig: AudioTrig::default(),
            midi_channel: 0,
            grid: 16.0,
            tables: boxed([[[1.0; TABLE]; N_BANDS]; N_LANES]),
            custom: boxed([1.0; TABLE]),
            custom_dest: [None; N_CUSTOM],
            switch_count: 0,
            sl: (0..N_SLOTS).map(|_| SlotDsp::new(sr)).collect(),
            ctl: [SlotCtl::default(); N_SLOTS],
            rng: Rng(0x9e3779b9),
            nco: NoiseCoefs::new(sr),
            bypass: false,
            in_gain: 1.0,
            out_gain: 1.0,
            master: 1.0,
            trig_lvl: 0.06,
            midi_switch: false,
            safe_clip: false,
            in_s: 1.0,
            out_s: 1.0,
            wet: 1.0,
            k_fade: 1.0 / (0.008 * sr),
            k_param: coef_ms(15.0, sr),
            k_xf: 1.0 / (0.008 * sr),
            pending: None,
            started: false,
            duck: 1.0,
            duck_hold: 0,
            k_duck: 1.0 / (0.004 * sr),
            k_2500: coef_hz(2500.0, sr),
            k_1200: coef_hz(1200.0, sr),
            k_dc: 1.0 - 2.0 * PI * 12.0 / sr,
            tr_fast: 0.0,
            tr_slow: 0.0,
            tr_hold: 0,
            tr_delay: 0,
            tr_f: Default::default(),
            tr_c: (SvfCoef::new(20.0, sr, 1.4), SvfCoef::new(20000.0, sr, 1.4)),
            follow: 0.0,
            note_hz: 440.0,
            k_tr: [0.0; 5],
            need_trig: false,
            view: 0,
            view_lane: 0,
            view_band: 0,
            bin: 0,
            acc_in: 0.0,
            acc_out: 0.0,
            pk_in: 0.0,
            pk_out: 0.0,
            listen: 0.0,
            ring_buf: [[0.0; 2]; 256],
            ring_n: 0,
            tool_ph: 0.0,
            tool_inc: 0.0,
        }
    }

    pub fn midi_channel(&self) -> u8 {
        self.midi_channel
    }

    pub fn reset(&mut self) {
        for s in self.sl.iter_mut() {
            s.activate();
            s.active = false;
            s.fade = 0.0;
        }
        self.tr_fast = 0.0;
        self.tr_slow = 0.0;
        self.tr_hold = 0;
    }

    /// Non-real-time: big buffers for the whole chain (initialize, tests).
    pub fn prepare(&mut self, st: &State, shared: &Shared) {
        self.pending = None;
        self.duck = 1.0;
        self.take_state(st);
        self.bake(st);
        for i in 0..N_SLOTS {
            self.sl[i].kind = st.slots[i].kind;
            if self.in_chain[i] {
                self.sl[i].heavy = build_heavy(&st.slots[i], self.sr, shared);
            }
        }
        let n = shared.noises.lock();
        for i in 0..N_SLOTS {
            let t = self.sets[i].n_type;
            self.sl[i].user = if t >= 1000 { n.get((t - 1000) as usize).cloned() } else { None };
        }
        self.media_version = shared.media_version.load(Ordering::Acquire);
    }

    fn take_state(&mut self, st: &State) {
        let snap = Snap::of(st);
        self.apply_snap(&snap);
    }

    /// Would going to `n` change the sound in a jump (chain, kinds, types, bands...)? Those changes
    /// wait for a short fade to dry; the rest (LFO, envelope, splits) apply at once.
    fn structural(&self, n: &Snap) -> bool {
        if n.chain_len != self.chain_len || n.chain[..n.chain_len] != self.chain[..self.chain_len] {
            return true;
        }
        n.chain[..n.chain_len].iter().any(|&s| {
            let (a, b) = (self.sets[s], n.sets[s]);
            let mut a2 = a;
            a2.lfo = b.lfo;
            a2.env = b.env;
            a2.link = b.link;
            a2.x_lo = b.x_lo;
            a2.x_hi = b.x_hi;
            a2.scope_len = b.scope_len;
            a2.scope_split = b.scope_split;
            a2.t_instant = b.t_instant;
            a2 != b
        })
    }

    fn apply_snap(&mut self, n: &Snap) {
        self.sets = n.sets;
        self.chain = n.chain;
        self.chain_len = n.chain_len;
        self.in_chain = [false; N_SLOTS];
        for &s in &n.chain[..n.chain_len] {
            self.in_chain[s] = true;
        }
        self.trig = n.trig;
        self.midi_channel = n.midi_channel;
        self.grid = n.grid;
        let sr = self.sr;
        self.tr_c = (SvfCoef::new(self.trig.hp, sr, 1.4), SvfCoef::new(self.trig.lp.max(self.trig.hp * 1.05), sr, 1.4));
    }

    fn bake(&mut self, st: &State) {
        for s in 0..N_SLOTS {
            for l in 0..N_LANES {
                for b in 0..N_BANDS {
                    if let Some(w) = st.waves.get(s).and_then(|w| w.get(l)).and_then(|w| w.get(b)) {
                        w.bake(&mut self.tables[s][l][b]);
                    }
                }
            }
        }
        for i in 0..N_CUSTOM {
            self.custom_dest[i] = None;
            if let Some(Some(c)) = st.custom.get(i) {
                c.wave.bake(&mut self.custom[i]);
                self.custom_dest[i] = Some((c.slot as usize, c.lane as usize, c.band as usize));
            }
        }
    }

    /// Takes the GUI's changes (never waits on a lock).
    pub fn sync(&mut self, params: &WaveCubeParams, shared: &Shared) {
        let sv = shared.set_version.load(Ordering::Acquire);
        let wv = shared.wave_version.load(Ordering::Acquire);
        if sv != self.set_version || wv != self.wave_version {
            if let Some(st) = params.state.try_read() {
                if sv != self.set_version {
                    let snap = Snap::of(&st);
                    if self.started && self.structural(&snap) {
                        // fade to dry first; reuse the box (no allocation after the first time)
                        self.pending = Some(snap);
                    } else {
                        self.apply_snap(&snap);
                        if self.pending.is_some() {
                            // a pending change is older than this state: this one wins
                            self.pending = Some(snap);
                        }
                    }
                    self.set_version = sv;
                }
                // new shapes wait for a pending structural change (presets switch as a whole)
                if wv != self.wave_version && self.pending.is_none() {
                    self.bake(&st);
                    self.wave_version = wv;
                }
            }
        }
        if self.duck <= 0.0 {
            if let Some(p) = self.pending.take() {
                self.apply_snap(&p);
                self.duck_hold = (0.02 * self.sr) as u32;
                if let Some(st) = params.state.try_read() {
                    self.bake(&st);
                    self.wave_version = shared.wave_version.load(Ordering::Acquire);
                }
            }
        }
        let mv = shared.media_version.load(Ordering::Acquire);
        // kind changes and big buffers
        for i in 0..N_SLOTS {
            let set = self.sets[i];
            if self.sl[i].kind != set.kind {
                self.sl[i].kind = set.kind;
                self.sl[i].activate();
            }
            if !self.in_chain[i] {
                continue;
            }
            let need = heavy_tag(&set, mv);
            let have = self.sl[i].heavy.as_ref().map(|h| h.tag).unwrap_or(0);
            if need != have && need != 0 {
                if let Some(mut q) = shared.heavy_in.try_lock() {
                    if let Some(pos) = q.iter().position(|(s, h)| *s == i && h.tag == need) {
                        let (_, h) = q.swap_remove(pos);
                        let old = self.sl[i].heavy.replace(h);
                        if let Some(o) = old {
                            if let Some(mut out) = shared.heavy_out.try_lock() {
                                if out.len() < out.capacity() {
                                    out.push(o);
                                } else {
                                    std::mem::forget(o);
                                }
                            } else {
                                std::mem::forget(o);
                            }
                        }
                        for b in self.sl[i].fx.iter_mut() {
                            b.valid = 0;
                        }
                    }
                }
            }
        }
        // user noise samples (new media, or a new choice)
        if mv != self.media_version || self.sets.iter().zip(self.sl.iter()).any(|(set, sl)| set.n_type != sl.user_t) {
            if let Some(n) = shared.noises.try_lock() {
                for i in 0..N_SLOTS {
                    let t = self.sets[i].n_type;
                    self.sl[i].user = if t >= 1000 { n.get((t - 1000) as usize).cloned() } else { None };
                    self.sl[i].user_t = t;
                }
                self.media_version = mv;
            }
        }
    }

    fn set_inc(&mut self, s: usize, l: usize, b: usize) {
        let lfo = self.sets[s].lfo_of(l, b);
        let sr = self.sr as f64;
        let d = &mut self.sl[s].lfo[l][b];
        d.inc = match lfo.mode {
            1 => lfo.hz as f64 / sr,
            2 => self.note_hz * 2f64.powf(lfo.transpose as f64 / 12.0) / sr,
            _ => self.tempo as f64 / 60.0 / sr / length_beats(lfo.length, lfo.feel).max(1e-3),
        };
    }

    pub fn begin_block(&mut self, p: &Controls, t: &Transport, shared: &Shared) {
        let tempo = t.tempo.clamp(20.0, 999.0);
        self.tempo = tempo as f32;
        shared.tempo.store(self.tempo, Ordering::Relaxed);
        self.bypass = p.bypass;
        self.in_gain = db2lin(p.in_db);
        self.out_gain = db2lin(p.out_db);
        self.master = p.master;
        self.trig_lvl = db2lin(p.trig_db);
        self.midi_switch = p.midi_switch;
        self.safe_clip = p.safe_clip;
        self.view = (shared.view.load(Ordering::Relaxed) as usize).min(N_SLOTS - 1);
        self.view_lane = (shared.view_lane.load(Ordering::Relaxed) as usize).min(N_LANES - 1);
        self.view_band = (shared.view_band.load(Ordering::Relaxed) as usize).min(N_BANDS - 1);
        let sr = self.sr;
        let (f_rel, s_att, s_rel) = match self.trig.algo {
            0 => (12.0, 30.0, 250.0),
            1 => (30.0, 60.0, 400.0),
            _ => (20.0, 120.0, 800.0),
        };
        self.k_tr = [coef_ms(f_rel, sr), coef_ms(s_att, sr), coef_ms(s_rel, sr), coef_ms(2.0, sr), coef_ms(80.0, sr)];
        self.need_trig = self.chain[..self.chain_len].iter().any(|&s| {
            let set = &self.sets[s];
            (0..set.kind.n_lanes()).any(|l| (0..N_BANDS).any(|b| set.lfo_of(l, b).trigger == 2))
        });
        for k in 0..self.chain_len {
            let s = self.chain[k];
            let set = self.sets[s];
            let kind = set.kind;
            let mut c = SlotCtl { on: p.on[s], tile: p.tile[s], mix: p.mix[s], band_mix: p.band_mix[s], ..Default::default() };
            for (i, d) in kind.knobs().iter().enumerate() {
                c.knob[i] = d.plain(p.knob[s][i]);
            }
            self.ctl[s] = c;
            let nb = if set.multiband { N_BANDS } else { 1 };
            for l in 0..kind.n_lanes() {
                for b in 0..nb {
                    self.set_inc(s, l, b);
                    let lfo = set.lfo_of(l, b);
                    let d = &mut self.sl[s].lfo[l][b];
                    d.smooth_a = if kind == Kind::Time {
                        // cut: jumps cross-fade in the effect itself; glide: the Smooth knob
                        if set.t_instant {
                            1.0
                        } else {
                            coef_ms(lfo.smooth.max(0.5), sr)
                        }
                    } else {
                        coef_ms(lfo.smooth, sr)
                    };
                    if lfo.mode == 0 && lfo.trigger == 0 && t.playing {
                        if let Some(pos) = t.pos_beats {
                            d.phase = (pos / length_beats(lfo.length, lfo.feel).max(1e-3)).rem_euclid(1.0);
                            // 1-shot in sync: each cycle plays up to the end marker and holds there
                            d.done = !lfo.looping && d.phase >= lfo.end as f64;
                        }
                    }
                    if lfo.looping {
                        d.done = false;
                    }
                    let env = set.env[l][b];
                    if env.on || kind == Kind::Compressor {
                        let ed = &mut self.sl[s].env[l][b];
                        ed.a_att = coef_ms(env.attack, sr);
                        ed.a_rel = coef_ms(env.release, sr);
                        ed.a_fast = coef_ms(env.release * 0.25, sr);
                        ed.a_slow = coef_ms(env.release * 1.5, sr);
                        ed.hold_n = (env.hold * 0.001 * sr) as u32;
                        ed.c_hp = SvfCoef::new(env.hp, sr, 1.4);
                        ed.c_lp = SvfCoef::new(env.lp.max(env.hp * 1.05), sr, 1.4);
                    }
                }
            }
            if kind == Kind::Compressor {
                let env = set.env[0][0];
                let ed = &mut self.sl[s].env[0][0];
                ed.c_hp = SvfCoef::new(env.hp, sr, 1.4);
                ed.c_lp = SvfCoef::new(env.lp.max(env.hp * 1.05), sr, 1.4);
            }
            let sd = &mut self.sl[s];
            sd.solo = if set.multiband { shared.solo[s].load(Ordering::Relaxed) & 7 } else { 0 };
            if set.multiband {
                sd.c1 = split_coef(set.x_lo, sr, set.slope);
                sd.c2 = split_coef(set.x_hi, sr, set.slope);
            }
            if set.multiband != sd.was_multi {
                sd.split = Default::default();
                sd.noise_split = Default::default();
                for b in sd.fx.iter_mut() {
                    b.clear();
                }
            }
            sd.was_multi = set.multiband;
            if kind == Kind::Time {
                let cyc = |l: usize| {
                    let lfo = set.lfo_of(0, l);
                    match lfo.mode {
                        1 => sr / lfo.hz.max(0.02),
                        2 => sr / (self.note_hz as f32).max(1.0),
                        _ => (length_beats(lfo.length, lfo.feel) * 60.0 / tempo) as f32 * sr,
                    }
                };
                let bar = 4.0 * 60.0 / self.tempo * sr;
                sd.time_len = match set.t_range {
                    0 => cyc(0),
                    1 => bar,
                    2 => 2.0 * bar,
                    3 => 4.0 * bar,
                    4 => 8.0 * bar,
                    5 => 0.0025 * sr,
                    6 => 0.010 * sr,
                    _ => 0.020 * sr,
                };
            }
            if kind == Kind::Scope && s == self.view {
                let beats = length_beats(set.scope_len, 0).max(1e-3);
                self.tool_inc = tempo as f64 / 60.0 / beats / sr as f64;
                if t.playing {
                    if let Some(pos) = t.pos_beats {
                        self.tool_ph = (pos / beats).rem_euclid(1.0);
                    }
                }
            }
            if kind == Kind::Noise {
                sd.nc = (SvfCoef::new(c.knob[1], sr, 1.4), SvfCoef::new(c.knob[2].max(c.knob[1] * 1.05), sr, 1.4));
            }
            sd.gr = 0.0;
        }
    }

    #[inline]
    fn lookup(&self, s: usize, l: usize, b: usize, ph: f32) -> f32 {
        let t = &self.tables[s][l][b];
        let pos = ph * TABLE as f32;
        let i = (pos as usize).min(TABLE - 1);
        let f = pos - i as f32;
        let a = t[i];
        if i + 1 >= TABLE {
            a
        } else {
            a + (t[i + 1] - a) * f
        }
    }

    /// One stereo frame. `sc`: sidechain input; `note`: a MIDI note started at this sample.
    pub fn process(&mut self, x: [f32; 2], sc: [f32; 2], stereo: bool, note: Option<u8>, shared: &Shared) -> [f32; 2] {
        let sr = self.sr;
        self.in_s += (self.in_gain - self.in_s) * self.k_param;
        self.out_s += (self.out_gain - self.out_s) * self.k_param;
        let input = [x[0] * self.in_s, x[1] * self.in_s];

        // ---- audio trigger: a fast envelope jumping over a slow one (filtered, optional sidechain)
        let mut audio_trig = false;
        if self.need_trig {
            let src = if self.trig.sidechain { sc } else { input };
            let m = 0.5 * (src[0] + src[1]);
            let (_, _, h) = self.tr_f[0].run(&self.tr_c.0, m);
            let (fl, _, _) = self.tr_f[1].run(&self.tr_c.1, h);
            let lv = fl.abs();
            let k = self.k_tr;
            let af = if lv > self.tr_fast { 0.3 } else { k[0] };
            self.tr_fast += (lv - self.tr_fast) * af;
            let as_ = if lv > self.tr_slow { k[1] } else { k[2] };
            self.tr_slow += (lv - self.tr_slow) * as_;
            let ratio = 1.3 + 3.0 * (1.0 - self.trig.detail);
            if self.tr_delay > 0 {
                self.tr_delay -= 1;
                if self.tr_delay == 0 {
                    audio_trig = true;
                }
            }
            if self.tr_hold > 0 {
                self.tr_hold -= 1;
            } else if self.tr_fast > self.trig_lvl && self.tr_fast > ratio * self.tr_slow + 1e-4 {
                self.tr_hold = (0.06 * sr) as u32;
                let d = (self.trig.shift * 0.001 * sr) as u32;
                if d == 0 {
                    audio_trig = true;
                } else {
                    self.tr_delay = d;
                }
            }
        }
        let lvl_in = input[0].abs().max(input[1].abs());
        let fa = if lvl_in > self.follow { self.k_tr[3] } else { self.k_tr[4] };
        self.follow += (lvl_in - self.follow) * fa;

        // ---- MIDI: switch custom waves (C# .. A) or trigger (any note; only C with MIDI Switch)
        let mut midi_trig = false;
        if let Some(n) = note {
            let pc = (n % 12) as usize;
            if self.midi_switch && (1..=N_CUSTOM).contains(&pc) {
                let slot = pc - 1;
                if let Some((s, l, b)) = self.custom_dest[slot] {
                    self.tables[s][l][b] = self.custom[slot];
                    self.switch_count = self.switch_count.wrapping_add(1);
                    shared.switched.store((self.switch_count << 8) | slot as u32, Ordering::Release);
                }
            } else if !self.midi_switch || pc == 0 {
                midi_trig = true;
                self.note_hz = 440.0 * 2f64.powf((n as f64 - 69.0) / 12.0);
            }
        }

        // ---- the chain
        let mut sig = input;
        let listen_slot = shared.env_listen.load(Ordering::Relaxed) as usize;
        let mut listening = false;
        for k in 0..self.chain_len {
            let s = self.chain[k];
            // SAFETY: `sets` only changes in `sync` / `apply_snap`, never while a sample is made;
            // a reference instead of a copy saves ~700 bytes of copying per item per sample
            let set: &SlotSet = unsafe { &*(&self.sets[s] as *const SlotSet) };
            let c = self.ctl[s];
            let kind = set.kind;
            // triggers / pitch
            let nb = if set.multiband { N_BANDS } else { 1 };
            for l in 0..kind.n_lanes() {
                for b in 0..nb {
                    let lfo = set.lfo_of(l, b);
                    if lfo.mode == 2 && midi_trig {
                        self.set_inc(s, l, b);
                    }
                    if (lfo.trigger == 1 && midi_trig) || (lfo.trigger == 2 && audio_trig) {
                        let d = &mut self.sl[s].lfo[l][b];
                        d.phase = 0.0;
                        d.done = false;
                    }
                }
            }
            let target = if c.on { 1.0 } else { 0.0 };
            {
                let sd = &mut self.sl[s];
                if target > 0.5 && !sd.active {
                    sd.activate();
                    sd.active = true;
                    sd.fade = 0.0;
                    sd.warm = (0.02 * sr) as u32;
                }
                if !sd.active {
                    continue;
                }
                if sd.warm > 0 {
                    sd.warm -= 1;
                } else if target > sd.fade {
                    sd.fade = (sd.fade + self.k_fade).min(1.0);
                } else if target < sd.fade {
                    sd.fade = (sd.fade - self.k_fade).max(0.0);
                }
                if target < 0.5 && sd.fade <= 0.0 {
                    sd.active = false;
                    continue;
                }
                // smoothed controls
                let kp = self.k_param;
                if !sd.primed {
                    sd.tile = c.tile;
                    sd.mix = if set.multiband { c.band_mix } else { [c.mix; N_BANDS] };
                    sd.knob = c.knob;
                    sd.primed = true;
                }
                for l in 0..N_LANES {
                    sd.tile[l] += (c.tile[l] - sd.tile[l]) * kp;
                }
                for b in 0..N_BANDS {
                    let m = if set.multiband { c.band_mix[b] * c.mix } else { c.mix };
                    sd.mix[b] += (m - sd.mix[b]) * kp;
                }
                for i in 0..N_KNOBS {
                    sd.knob[i] += (c.knob[i] - sd.knob[i]) * kp;
                }
            }
            // LFO values
            let grid = self.grid;
            for l in 0..kind.n_lanes() {
                for b in 0..nb {
                    let lfo = set.lfo_of(l, b);
                    let d = self.sl[s].lfo[l][b];
                    let base = if d.done { lfo.end.min(0.99999) } else { (d.phase as f32 + lfo.phase / 360.0).rem_euclid(1.0) };
                    let ph_l = swing_warp(base, lfo.swing, grid);
                    let raw_l = self.lookup(s, l, b, ph_l);
                    let raw_r = if lfo.spread > 0.05 {
                        let ph_r = swing_warp((base + lfo.spread / 360.0).rem_euclid(1.0), lfo.swing, grid);
                        self.lookup(s, l, b, ph_r)
                    } else {
                        raw_l
                    };
                    let d = &mut self.sl[s].lfo[l][b];
                    if d.primed {
                        d.y[0] += (raw_l - d.y[0]) * d.smooth_a;
                        d.y[1] += (raw_r - d.y[1]) * d.smooth_a;
                    } else {
                        d.y = [raw_l, raw_r];
                        d.primed = true;
                    }
                }
            }
            let x_in = sig;
            let out = self.run_slot(s, x_in, sc, stereo, set);
            let f = self.sl[s].fade;
            sig = [x_in[0] + (out[0] - x_in[0]) * f, x_in[1] + (out[1] - x_in[1]) * f];
            if listen_slot == s + 1 {
                listening = true;
            }
            // the viewed item's input / output for the scope and the vectorscope
            if s == self.view {
                self.scope_capture(s, x_in, sig, shared);
            }
        }

        // ---- advance the LFOs (also of items that are off, so they stay in time)
        for k in 0..self.chain_len {
            let s = self.chain[k];
            // SAFETY: see above
            let set: &SlotSet = unsafe { &*(&self.sets[s] as *const SlotSet) };
            let nb = if set.multiband { N_BANDS } else { 1 };
            for l in 0..set.kind.n_lanes() {
                for b in 0..nb {
                    let lfo = set.lfo_of(l, b);
                    let d = &mut self.sl[s].lfo[l][b];
                    if d.done {
                        continue;
                    }
                    d.phase += d.inc;
                    if !lfo.looping && d.phase >= lfo.end as f64 {
                        d.phase = lfo.end as f64;
                        d.done = true;
                    } else if d.phase >= 1.0 {
                        d.phase -= d.phase.floor();
                    }
                }
            }
        }

        self.started = true;
        // ---- structural change waiting: through dry and back
        if self.pending.is_some() {
            self.duck = (self.duck - self.k_duck).max(0.0);
        } else if self.duck_hold > 0 {
            self.duck_hold -= 1;
        } else if self.duck < 1.0 {
            self.duck = (self.duck + self.k_duck).min(1.0);
        }
        if self.duck < 1.0 {
            sig = [input[0] + (sig[0] - input[0]) * self.duck, input[1] + (sig[1] - input[1]) * self.duck];
        }
        // ---- master mix / bypass, safety clipper
        let target = if self.bypass { 0.0 } else { self.master };
        self.wet += (target - self.wet) * self.k_param;
        let mut out = [x[0] + (sig[0] * self.out_s - x[0]) * self.wet, x[1] + (sig[1] * self.out_s - x[1]) * self.wet];
        if listening {
            out = [self.listen, self.listen];
        }
        for o in out.iter_mut() {
            if !o.is_finite() {
                *o = 0.0;
            } else if self.safe_clip && !self.bypass {
                *o = safe(*o);
            }
        }
        self.pk_in = self.pk_in.max(x[0].abs().max(x[1].abs()));
        self.pk_out = self.pk_out.max(out[0].abs().max(out[1].abs()));
        out
    }

    fn scope_capture(&mut self, s: usize, x: [f32; 2], y: [f32; 2], shared: &Shared) {
        // SAFETY: see `process`
        let set: &SlotSet = unsafe { &*(&self.sets[s] as *const SlotSet) };
        let lane = self.view_lane.min(set.kind.n_lanes().max(1) - 1);
        let band = if set.multiband { self.view_band } else { 0 };
        let d = self.sl[s].lfo[lane][band];
        let tool = set.kind == Kind::Scope;
        let vph = if tool {
            self.tool_ph = (self.tool_ph + self.tool_inc).fract();
            Some(self.tool_ph as f32)
        } else if set.kind.n_lanes() > 0 && d.inc < 1.0 / 32.0 {
            let lfo = set.lfo_of(lane, band);
            Some((d.phase as f32 + lfo.phase / 360.0).rem_euclid(1.0))
        } else {
            None
        };
        if let Some(vph) = vph {
            let bin = ((vph * SCOPE_N as f32) as usize).min(SCOPE_N - 1);
            if bin != self.bin {
                let mut b = self.bin;
                let mut guard = 0;
                loop {
                    shared.scope_in[b].store(self.acc_in, Ordering::Relaxed);
                    shared.scope_out[b].store(self.acc_out, Ordering::Relaxed);
                    b = (b + 1) % SCOPE_N;
                    guard += 1;
                    if b == bin || guard >= 8 {
                        break;
                    }
                }
                self.bin = bin;
                self.acc_in = 0.0;
                self.acc_out = 0.0;
            }
        }
        if tool {
            // the Scope tool: left and right at that point of the chain
            self.acc_in = self.acc_in.max(y[0].abs());
            self.acc_out = self.acc_out.max(y[1].abs());
        } else {
            self.acc_in = self.acc_in.max(x[0].abs().max(x[1].abs()));
            self.acc_out = self.acc_out.max(y[0].abs().max(y[1].abs()));
        }
        // the ring for the vectorscope / Scope tool, flushed in small pieces
        self.ring_buf[self.ring_n] = y;
        self.ring_n += 1;
        if self.ring_n == self.ring_buf.len() {
            if let Some(mut r) = shared.ring.try_lock() {
                let (buf, w) = &mut *r;
                for v in self.ring_buf.iter() {
                    buf[*w] = *v;
                    *w = (*w + 1) % RING_N;
                }
            }
            self.ring_n = 0;
        }
    }

    pub fn end_block(&mut self, shared: &Shared) {
        for k in 0..self.chain_len {
            let s = self.chain[k];
            let sd = &self.sl[s];
            for l in 0..N_LANES {
                let lfo = self.sets[s].lfo_of(l, 0);
                let d = sd.lfo[l][0];
                let ph = if d.done { lfo.end } else { d.phase as f32 };
                shared.phase[s][l].store(ph, Ordering::Relaxed);
                for b in 0..N_BANDS {
                    shared.value[s][l][b].store(sd.val[l][b][0], Ordering::Relaxed);
                }
                shared.env[s][l].store(sd.env[l][(self.view_band).min(N_BANDS - 1)].env, Ordering::Relaxed);
            }
            shared.gr[s].store(sd.gr, Ordering::Relaxed);
        }
        shared.in_pk.store(shared.in_pk.load(Ordering::Relaxed).max(self.pk_in), Ordering::Relaxed);
        shared.out_pk.store(shared.out_pk.load(Ordering::Relaxed).max(self.pk_out), Ordering::Relaxed);
        self.pk_in = 0.0;
        self.pk_out = 0.0;
    }

    // --------------------------------------------------------------------------------------
    // One item
    // --------------------------------------------------------------------------------------

    fn run_slot(&mut self, s: usize, x: [f32; 2], sc: [f32; 2], stereo: bool, set: &SlotSet) -> [f32; 2] {
        let kind = set.kind;
        if kind == Kind::Scope {
            return x;
        }
        // noise is made once per item and split like the signal
        let noise = if kind == Kind::Noise { self.make_noise(s, set) } else { [0.0; 2] };
        if !set.multiband {
            let wet = self.band(s, 0, x, x, sc, noise, stereo, set);
            let m = self.sl[s].mix[0];
            return [x[0] + (wet[0] - x[0]) * m, x[1] + (wet[1] - x[1]) * m];
        }
        let sd = &mut self.sl[s];
        let (c1, c2) = (sd.c1, sd.c2);
        let bl = sd.split[0].run(&c1, &c2, set.slope, x[0]);
        let br = sd.split[1].run(&c1, &c2, set.slope, x[1]);
        let (nl, nr) = if kind == Kind::Noise {
            (sd.noise_split[0].run(&c1, &c2, set.slope, noise[0]), sd.noise_split[1].run(&c1, &c2, set.slope, noise[1]))
        } else {
            ([0.0; 3], [0.0; 3])
        };
        let solo = sd.solo;
        let mut out = [0.0f32; 2];
        for b in 0..N_BANDS {
            let xb = [bl[b], br[b]];
            let wet = self.band(s, b, xb, x, sc, [nl[b], nr[b]], stereo, set);
            if solo != 0 && solo & (1 << b) == 0 {
                continue;
            }
            let m = self.sl[s].mix[b];
            out[0] += xb[0] + (wet[0] - xb[0]) * m;
            out[1] += xb[1] + (wet[1] - xb[1]) * m;
        }
        out
    }

    fn make_noise(&mut self, s: usize, set: &SlotSet) -> [f32; 2] {
        let sr = self.sr;
        // pitch from the (first band's) pitch lane
        let pv = if set.kind.n_lanes() > 1 { self.sl[s].lfo[1][0].y[0] + self.sl[s].tile[1] } else { 0.5 };
        let rate = fast_exp2((pv.clamp(0.0, 1.0) - 0.5) * 4.0);
        let sd = &mut self.sl[s];
        let st = if set.n_mono { 0.0 } else { sd.knob[3] };
        let mut n = if let (true, Some(u)) = (set.n_type >= 1000, sd.user.as_ref()) {
            let len = u.data.len().max(2) as f64;
            sd.user_pos = (sd.user_pos + rate as f64 * (u.sr / sr) as f64) % len;
            let i = sd.user_pos.floor() as usize;
            let f = (sd.user_pos - i as f64) as f32;
            let a = u.data[i % u.data.len()];
            let b = u.data[(i + 1) % u.data.len()];
            let v = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
            let mid = 0.5 * (v[0] + v[1]);
            [mid + (v[0] - mid) * st, mid + (v[1] - mid) * st]
        } else {
            let t = (set.n_type as usize).min(noise::N_NOISES - 1);
            let hz = if t == 13 { 60.0 } else { 50.0 };
            sd.hum_ph = (sd.hum_ph + hz * rate / sr) % 1.0;
            let hm = noise::hum(sd.hum_ph);
            let c = sd.gens[0].next(t, &mut self.rng, &self.nco, hm, rate);
            let l = sd.gens[1].next(t, &mut self.rng, &self.nco, hm, rate);
            let r = sd.gens[2].next(t, &mut self.rng, &self.nco, hm, rate);
            let st = if t == 12 || t == 13 { 0.0 } else { st };
            [c + (l - c) * st, c + (r - c) * st]
        };
        for (ch, v) in n.iter_mut().enumerate() {
            let (_, _, h) = sd.nf[ch][0].run(&sd.nc.0, *v);
            let (l, _, _) = sd.nf[ch][1].run(&sd.nc.1, h);
            *v = l * db2lin(sd.knob[0]);
        }
        if set.n_mono {
            n[1] = n[0];
        }
        n
    }

    /// Lane values with the envelope and the offset, then the effect, for one band.
    #[allow(clippy::too_many_arguments)]
    fn band(&mut self, s: usize, b: usize, xb: [f32; 2], xfull: [f32; 2], sc: [f32; 2], noise: [f32; 2], stereo: bool, set: &SlotSet) -> [f32; 2] {
        let kind = set.kind;
        for l in 0..kind.n_lanes() {
            let env = set.env[l][if set.multiband { b } else { 0 }];
            let mut y = self.sl[s].lfo[l][b].y;
            if env.on && kind.has_envelope() {
                let src = match env.input {
                    1 => xfull,
                    2 => sc,
                    _ => xb,
                };
                let m = 0.5 * (src[0] + src[1]);
                let ed = &mut self.sl[s].env[l][b];
                let (det, heard) = if env.input == 0 {
                    (src[0].abs().max(src[1].abs()), m)
                } else {
                    let (ch, cl) = (ed.c_hp, ed.c_lp);
                    let (_, _, h) = ed.hp.run(&ch, m);
                    let (lp, _, _) = ed.lp.run(&cl, h);
                    (lp.abs(), lp)
                };
                // Listen: what the detector of the lane / band on screen hears
                if l == self.view_lane && (b == self.view_band || !set.multiband) {
                    self.listen = heard;
                }
                if det > ed.env {
                    ed.env += (det - ed.env) * ed.a_att;
                    ed.hold = ed.hold_n;
                    ed.peak = ed.env;
                } else if ed.hold > 0 {
                    ed.hold -= 1;
                } else {
                    // adaptive: fast at first, slower in the tail (less pumping)
                    let a = if env.adaptive {
                        if ed.env > ed.peak * 0.5 {
                            ed.a_fast
                        } else {
                            ed.a_slow
                        }
                    } else {
                        ed.a_rel
                    };
                    ed.env += (det - ed.env) * a;
                }
                let db = fast_db(ed.env.max(1e-6));
                let thr = env.thresh.min(-0.5);
                let e = ((db - thr) / -thr).clamp(0.0, 1.0);
                let depth = [1.0, 2.0, 4.0, 8.0][env.depth.min(3) as usize];
                let m = env.amount * depth * e + env.shift;
                for v in y.iter_mut() {
                    *v = if env.multiply { *v * m.clamp(0.0, 1.0) } else { *v + m };
                }
            }
            let tile = self.sl[s].tile[l];
            self.sl[s].val[l][b] = [(y[0] + tile).clamp(0.0, 1.0), (y[1] + tile).clamp(0.0, 1.0)];
        }
        let v0 = self.sl[s].val[0][b];
        let v1 = self.sl[s].val[1][b];
        self.effect(s, b, v0, v1, xb, xfull, sc, noise, stereo, set)
    }

    #[allow(clippy::too_many_arguments)]
    fn effect(&mut self, s: usize, b: usize, v0: [f32; 2], v1: [f32; 2], x: [f32; 2], xfull: [f32; 2], sc: [f32; 2], noise: [f32; 2], stereo: bool, set: &SlotSet) -> [f32; 2] {
        let sr = self.sr;
        let kn = self.sl[s].knob;
        match set.kind {
            Kind::Volume => {
                let t = fast_db2lin(kn[0]);
                [x[0] * v0[0] * t, x[1] * v0[1] * t]
            }
            Kind::Pan => {
                let fx = &mut self.sl[s].fx[b];
                let len = fx.haas.len();
                let w = fx.haas_w;
                fx.haas[w] = x;
                fx.haas_w = (w + 1) % len;
                if !stereo {
                    return x;
                }
                let pv = (2.0 * v0[0] - 1.0).clamp(-1.0, 1.0);
                let k = pv.abs();
                let lvl = if set.p_law == 4 {
                    if pv >= 0.0 {
                        [x[0] + x[1] * k * 0.5, x[1] * (1.0 - k)]
                    } else {
                        [x[0] * (1.0 - k), x[1] + x[0] * k * 0.5]
                    }
                } else {
                    let comp = [0.0, 3.0, 4.5, 6.0][set.p_law.min(3) as usize];
                    let near = db2lin(comp * k);
                    let far = 1.0 - k;
                    if pv >= 0.0 {
                        [x[0] * near, x[1] * far]
                    } else {
                        [x[0] * far, x[1] * near]
                    }
                };
                let haas_amt = kn[0];
                if haas_amt < 1e-4 {
                    return lvl;
                }
                let d = (kn[1] * 0.001 * sr * k).clamp(0.0, (len - 2) as f32);
                let haas = if pv >= 0.0 { [x[0], ring_read(&fx.haas, w, d, 1)] } else { [ring_read(&fx.haas, w, d, 0), x[1]] };
                [lvl[0] + (haas[0] - lvl[0]) * haas_amt, lvl[1] + (haas[1] - lvl[1]) * haas_amt]
            }
            Kind::Width => {
                if !stereo {
                    return x;
                }
                let w = 2.0 * v0[0];
                let g = fast_db2lin(kn[0]);
                let m = 0.5 * (x[0] + x[1]);
                let sd = 0.5 * (x[0] - x[1]) * w;
                [(m + sd) * g, (m - sd) * g]
            }
            Kind::Filter => self.filter(s, b, v0, v1, x, set),
            Kind::Time => {
                let k_xf = self.k_xf;
                let sd = &mut self.sl[s];
                let Some(h) = sd.heavy.as_mut() else { return x };
                if h.time.len() <= b {
                    return x;
                }
                let len = h.time[b].len();
                let w = sd.time_w;
                let buf = &mut h.time[b];
                buf[w] = x;
                let fx = &mut sd.fx[b];
                fx.valid = (fx.valid + 1).min(len);
                let max_d = (fx.valid as f32 - 4.0).max(0.0).min((len - 4) as f32);
                let mut out = [0.0; 2];
                let jump = (0.002 * sr).max(32.0);
                if !fx.tprimed {
                    for ch in 0..2 {
                        fx.td[ch] = ((1.0 - v0[ch]) * sd.time_len).clamp(0.0, max_d);
                    }
                    fx.tprimed = true;
                }
                for ch in 0..2 {
                    let target = ((1.0 - v0[ch]) * sd.time_len).clamp(0.0, max_d);
                    if set.t_instant && (target - fx.td[ch]).abs() > jump {
                        // a jump: the old position fades out while the new one fades in
                        if fx.tx[ch] <= 0.0 {
                            fx.told[ch] = fx.td[ch];
                            fx.tx[ch] = 1.0;
                        }
                    }
                    fx.td[ch] = target;
                    let a = time_read(buf, w, fx.td[ch], ch, x[ch]);
                    out[ch] = if fx.tx[ch] > 0.0 {
                        // the old tap plays on at normal speed (its delay stays the same)
                        let o = time_read(buf, w, fx.told[ch].min(max_d), ch, x[ch]);
                        let t = fx.tx[ch];
                        fx.tx[ch] = (t - k_xf).max(0.0);
                        a + (o - a) * t
                    } else {
                        a
                    };
                }
                let last = if set.multiband { N_BANDS - 1 } else { 0 };
                if b == last {
                    sd.time_w = (w + 1) % len;
                }
                out
            }
            Kind::Noise => {
                let f = (self.follow * 4.0).min(1.0);
                let lvl_db = if set.n_env == 2 { fast_db(self.follow.max(1e-6)) } else { 0.0 };
                let env = match set.n_env {
                    1 => 1.0 + (f - 1.0) * kn[5],
                    2 => {
                        let over = ((lvl_db - kn[4]) / 12.0).clamp(0.0, 1.0);
                        1.0 - kn[5] * over
                    }
                    _ => 1.0,
                };
                let base = if set.n_only { [0.0; 2] } else { x };
                let mut out = [base[0] + noise[0] * v0[0] * env, base[1] + noise[1] * v0[1] * env];
                if !stereo {
                    out[1] = out[0];
                }
                out
            }
            Kind::Crush => {
                let fx = &mut self.sl[s].fx[b];
                let crunch = fast_db2lin(kn[0]);
                let push = kn[1] * 0.5;
                let mut v = [0.0f32; 2];
                for ch in 0..2 {
                    let mut smp = x[ch];
                    if kn[0] > 0.05 {
                        let t = smp * crunch + push;
                        smp = ((t + 1.0).rem_euclid(2.0) - 1.0) - push;
                    }
                    let bits = 1.0 + 15.0 * v0[ch];
                    if bits < 15.95 {
                        let steps = fast_exp2(bits - 1.0);
                        smp += push / steps.max(1.0);
                        if set.c_dither {
                            smp += (self.rng.next() + self.rng.next()) * 0.5 / steps;
                        }
                        smp = (smp * steps).round() / steps - push / steps.max(1.0);
                    }
                    v[ch] = smp;
                }
                // resample: the lower of the two channels' rates
                // top of the shape = the engine rate (no resampling at all)
                let vr = v1[0].min(v1[1]);
                if (fx.rate_k.0 - vr).abs() > 1e-5 {
                    fx.rate_k = (vr, if vr >= 0.9995 { sr } else { 8.82 * (sr / 8.82).powf(vr) });
                }
                let rate = fx.rate_k.1;
                let step = (rate / sr).min(1.0);
                if step < 0.999 {
                    if set.c_prefilter {
                        if (fx.aa_k.0 - rate).abs() > rate * 2e-3 {
                            fx.aa_k = (rate, coef_hz(rate * 0.45, sr));
                        }
                        let a = fx.aa_k.1;
                        for ch in 0..2 {
                            fx.aa[0][ch] += (v[ch] - fx.aa[0][ch]) * a;
                            fx.aa[1][ch] += (fx.aa[0][ch] - fx.aa[1][ch]) * a;
                            v[ch] = fx.aa[1][ch];
                        }
                    }
                    let jit = kn[2] * self.rng.next() * 0.9;
                    fx.count += step;
                    if fx.count >= 1.0 {
                        fx.count = (fx.count - 1.0 - jit * step.min(0.5)).max(0.0);
                        fx.hold = v;
                    }
                    v = fx.hold;
                }
                // what the crush added, without DC (clean settings stay bit-exact)
                let dc_r = self.k_dc;
                for ch in 0..2 {
                    let diff = v[ch] - x[ch];
                    let (x1, y1) = fx.dc[ch];
                    let yv = diff - x1 + dc_r * y1;
                    fx.dc[ch] = (diff, yv);
                    v[ch] = x[ch] + yv;
                }
                // treble: high shelf (100 % = open)
                let a = self.k_2500;
                let g = fast_db2lin(kn[4]);
                let mut out = [0.0; 2];
                for ch in 0..2 {
                    fx.lp[ch] += (v[ch] - fx.lp[ch]) * a;
                    out[ch] = (fx.lp[ch] + (v[ch] - fx.lp[ch]) * kn[3]) * g;
                }
                out
            }
            Kind::Drive => {
                let fx = &mut self.sl[s].fx[b];
                let lvl = x[0].abs().max(x[1].abs());
                let ga = if lvl > fx.grip { 0.05 } else { 0.0005 };
                fx.grip += (lvl - fx.grip) * ga;
                let grip = if kn[3] > 0.0 { fx.grip / (fx.grip + kn[3] * 0.08 + 1e-9) } else { 1.0 };
                let push = kn[2] * 0.6;
                let tone_a = self.k_1200;
                let dc_r = self.k_dc;
                let mut out = x;
                for ch in 0..2 {
                    let c = v0[ch];
                    let lg = (kn[0] + c * 36.0) * (std::f32::consts::LN_10 / 20.0);
                    let g = fast_exp2(lg * std::f32::consts::LOG2_E);
                    let comp = fast_exp2(lg * -0.7 * (1.0 - kn[4]) * std::f32::consts::LOG2_E);
                    let sat = drive_curve(set.d_style, x[ch] * g + push);
                    let (x1, y1) = fx.dc[ch];
                    let yv = sat - x1 + dc_r * y1;
                    fx.dc[ch] = (sat, yv);
                    let sat = yv * comp * grip;
                    fx.lp[ch] += (sat - fx.lp[ch]) * tone_a;
                    let tone = kn[5] * c;
                    let toned = if tone < 0.0 { sat + (fx.lp[ch] - sat) * -tone } else { sat + (sat - fx.lp[ch]) * tone };
                    let blend = (c * 4.0).min(1.0);
                    out[ch] = x[ch] + (toned * fast_db2lin(kn[1]) - x[ch]) * blend;
                }
                out
            }
            Kind::Reverb => {
                let r_type = set.r_type as usize;
                let sd = &mut self.sl[s];
                let fx = &mut sd.fx[b];
                let Some(h) = sd.heavy.as_mut() else { return x };
                let Some(rv) = h.reverbs.get_mut(b) else { return x };
                // clear tails: the volume shape drops to zero at a hard point
                // (a hard point: the level fell from high to zero within ~5 ms)
                let vol = 0.5 * (v1[0] + v1[1]);
                fx.last_vol = vol.max(fx.last_vol * (1.0 - 1.0 / (0.005 * sr)));
                if set.r_clear && vol < 0.001 && fx.last_vol > 0.3 {
                    rv.clear();
                    fx.last_vol = 0.0;
                }
                let send = [x[0] * v0[0], x[1] * v0[1]];
                let wet = if let Some(c) = rv.conv.as_mut() {
                    let y = c.process(send);
                    let m = 0.5 * (y[0] + y[1]);
                    let sdv = 0.5 * (y[0] - y[1]) * kn[3];
                    [m + sdv, m - sdv]
                } else {
                    rv.fdn.process(send, r_type.min(crate::kinds::REVERBS.len() - 1), kn[0], kn[1], kn[2], kn[3], kn[4], set.r_gated)
                };
                [x[0] * kn[5] + wet[0] * v1[0] * kn[6], x[1] * kn[5] + wet[1] * v1[1] * kn[6]]
            }
            Kind::Liquid => self.liquid(s, b, v0, v1, x, set),
            Kind::Compressor => {
                let src = match set.comp_input {
                    1 => xfull,
                    2 => sc,
                    _ => x,
                };
                let det = if set.comp_input == 0 {
                    src[0].abs().max(src[1].abs())
                } else {
                    let m = 0.5 * (src[0] + src[1]);
                    let ed = &self.sl[s].env[0][0];
                    let (ch, cl) = (ed.c_hp, ed.c_lp);
                    let fx = &mut self.sl[s].fx[b];
                    let (_, _, h) = fx.comp_hp.run(&ch, m);
                    let (lp, _, _) = fx.comp_lp.run(&cl, h);
                    lp.abs()
                };
                let fx = &mut self.sl[s].fx[b];
                let (thr, ratio, att, hold, rel, makeup) = (kn[0], kn[1], kn[2], kn[3], kn[4], kn[5]);
                if (fx.comp_k.0 - att).abs() > 1e-3 || (fx.comp_k.1 - rel).abs() > 1e-2 || (fx.comp_k.2 - hold).abs() > 1e-2 {
                    fx.comp_k = (att, rel, hold, [coef_ms(att.max(0.01), sr), coef_ms(rel, sr), coef_ms(rel * 0.25, sr), coef_ms(rel * 1.5, sr)], (hold * 0.001 * sr) as u32);
                }
                let ck = fx.comp_k.3;
                if det > fx.comp {
                    fx.comp += (det - fx.comp) * ck[0];
                    fx.comp_hold = fx.comp_k.4;
                    fx.comp_peak = fx.comp;
                } else if fx.comp_hold > 0 {
                    fx.comp_hold -= 1;
                } else {
                    let a = if set.comp_adaptive {
                        if fx.comp > fx.comp_peak * 0.5 {
                            ck[2]
                        } else {
                            ck[3]
                        }
                    } else {
                        ck[1]
                    };
                    fx.comp += (det - fx.comp) * a;
                }
                let lvl = fast_db(fx.comp.max(1e-6));
                let slope = 1.0 - 1.0 / ratio.max(1.0);
                let gr = (lvl - thr).max(0.0) * slope;
                let mk = makeup + if set.comp_auto { -thr * slope * 0.5 } else { 0.0 };
                let g = fast_db2lin(mk - gr);
                let sd = &mut self.sl[s];
                sd.gr = sd.gr.max(gr);
                [x[0] * g, x[1] * g]
            }
            Kind::Scope => x,
        }
    }

    fn filter(&mut self, s: usize, b: usize, v0: [f32; 2], v1: [f32; 2], x: [f32; 2], set: &SlotSet) -> [f32; 2] {
        let sr = self.sr;
        let kn = self.sl[s].knob;
        let fx = &mut self.sl[s].fx[b];
        let ty = set.f_type;
        let slope = set.f_slope;
        let warm = set.f_warm && ty != 3;
        let dist = if warm { kn[1] } else { 0.0 };
        let outg = fast_db2lin(kn[2]);
        let mut out = [0.0; 2];
        for ch in 0..2 {
            let sp = if ch == 0 { 0.5 } else { -0.5 } * kn[0];
            let fc = (20.6 * fast_exp2(v0[ch] * 10.000_3 + sp / 12.0)).clamp(15.0, 0.47 * sr);
            let res = v1[ch];
            let r1 = (1.0 - res).max(0.0);
            let mut k = 2.0 * r1 * r1.sqrt() + 0.03;
            if warm && set.f_safe {
                k = k.max(0.12);
            }
            let (c, c2) = match fx.fc[ch] {
                Some((cf, ck, c, c2)) if (cf - fc).abs() <= fc * 2e-3 && (ck - k).abs() < 1e-4 => (c, c2),
                _ => {
                    let c = SvfCoef::new(fc, sr, k);
                    let c2 = SvfCoef::new(fc, sr, std::f32::consts::SQRT_2.max(k));
                    fx.fc[ch] = Some((fc, k, c, c2));
                    (c, c2)
                }
            };
            let comp = 1.0 - 0.45 * res;
            let st = &mut fx.svf[ch];
            let mut v = x[ch];
            if warm {
                let drive = 1.0 + dist * 6.0;
                v = (v * drive).tanh() / drive.sqrt();
            }
            let y = match (slope, ty) {
                (0, 0) => st[0].one_pole(&c, v).0,
                (0, 1) => st[0].one_pole(&c, v).1,
                (_, t) => {
                    let one = |svf: &mut Svf, cc: &SvfCoef, i: f32| -> f32 {
                        let (l, bp, h) = svf.run(cc, i);
                        match t {
                            0 => l,
                            1 => h,
                            2 => bp * cc.k,
                            3 => l + h,
                            _ => l - h,
                        }
                    };
                    let a = one(&mut st[0], &c, v);
                    if slope == 2 {
                        let cc = if t >= 2 { c } else { c2 };
                        one(&mut st[1], &cc, a * if t == 4 { comp } else { 1.0 })
                    } else {
                        a
                    }
                }
            };
            if warm {
                st[0].saturate(4.0);
                st[1].saturate(4.0);
            }
            let mut y = if matches!(ty, 0 | 1 | 4) { y * comp } else { y };
            if warm && dist > 0.0 {
                let d = 1.0 + dist * 4.0;
                y = if set.f_hard { (y * d).clamp(-1.0, 1.0) / d.sqrt() } else { (y * d).tanh() / d.sqrt() };
            }
            out[ch] = y * outg;
        }
        out
    }

    fn liquid(&mut self, s: usize, b: usize, v0: [f32; 2], v1: [f32; 2], x: [f32; 2], set: &SlotSet) -> [f32; 2] {
        let sr = self.sr;
        let kn = self.sl[s].knob;
        let fx = &mut self.sl[s].fx[b];
        let sign = if set.l_neg { -1.0 } else { 1.0 };
        match set.l_type {
            1 => {
                // flanger: centre = delay (top = short = high), feedback from the second lane
                let len = fx.flange.len();
                let w = fx.flange_w;
                let mut out = [0.0; 2];
                let mut write = [0.0; 2];
                for ch in 0..2 {
                    let sp = if ch == 0 { 0.5 } else { -0.5 } * kn[0];
                    let ms = 12.0 * fast_exp2(-v0[ch] * 5.584_96 - sp / 12.0);
                    let d = (ms * 0.001 * sr).clamp(1.0, (len - 2) as f32);
                    let dv = ring_read(&fx.flange, (w + len - 1) % len, d, ch);
                    let fb = v1[ch] * 0.95 * sign;
                    write[ch] = x[ch] + fb * dv;
                    out[ch] = 0.5 * (x[ch] + sign * dv);
                }
                fx.flange[w] = [write[0].clamp(-4.0, 4.0), write[1].clamp(-4.0, 4.0)];
                fx.flange_w = (w + 1) % len;
                out
            }
            2 => {
                // phaser: 2-8 all-pass stages spread around the centre
                let n = [2usize, 4, 6, 8][set.l_stages.min(3) as usize];
                let mut out = [0.0; 2];
                for ch in 0..2 {
                    let sp = if ch == 0 { 0.5 } else { -0.5 } * kn[0];
                    let fc = (40.0 * fast_exp2(v0[ch] * 8.228_82 + sp / 12.0)).clamp(20.0, 0.45 * sr);
                    let fb = v1[ch] * 0.9 * sign;
                    let mut a = x[ch] + fx.fb[ch] * fb;
                    let pk = &mut fx.ph_k[ch];
                    if (pk.0 - fc).abs() > fc * 2e-3 || (pk.2 - kn[1]).abs() > 1e-3 || pk.1 != n as f32 {
                        for i in 0..n {
                            let spread = 2f32.powf((i as f32 - (n as f32 - 1.0) * 0.5) * kn[1] * 0.8);
                            pk.3[i] = SvfCoef::new((fc * spread).clamp(20.0, 0.45 * sr), sr, 1.0);
                        }
                        *pk = (fc, n as f32, kn[1], pk.3);
                    }
                    let coefs = pk.3;
                    for (st, c) in fx.svf[ch].iter_mut().take(n).zip(coefs.iter()) {
                        let (_, bp, _) = st.run(c, a);
                        a -= 2.0 * c.k * bp;
                    }
                    fx.fb[ch] = a.clamp(-4.0, 4.0);
                    out[ch] = 0.5 * (x[ch] + sign * a);
                }
                out
            }
            _ => x,
        }
    }
}

#[cfg(test)]
mod tests;
