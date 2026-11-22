//! The audio side: one LFO per shaper reading its baked shape (with swing and a left / right
//! phase offset), envelope followers (own signal or sidechain), an optional three-band split
//! (Linkwitz-Riley 24 dB/oct, flat sum) and the eight effects.
//!
//! Click safety: shapers fade in / out (8 ms) when switched, added or removed; Amount, Mix and
//! the global gains are smoothed; delay lines never read audio older than what was written
//! since the shaper was switched on.

use crate::wave::TABLE;
use crate::{
    DriveStyle, EnvMode, FilterType, Kind, Mode, NoiseEnv, NoiseType, PanMode, Shared, State, TimeRange, Trigger, WaveCubeParams, N_BANDS,
    N_SHAPERS, N_SLOTS, SCOPE_N,
};
use std::f32::consts::{LN_10, PI, SQRT_2};
use std::sync::atomic::Ordering;

/// longest Time delay (seconds)
const TIME_MAX_S: f32 = 8.0;
/// longest Haas delay (seconds)
const HAAS_MAX_S: f32 = 0.045;

pub struct Transport {
    pub tempo: f64,
    pub pos_beats: Option<f64>,
    pub playing: bool,
}

#[derive(Clone, Copy)]
pub struct ShaperSet {
    pub on: bool,
    pub mode: Mode,
    pub trigger: Trigger,
    /// cycle length in quarter notes (Beat mode)
    pub beats: f64,
    pub hz: f64,
    pub offset: f32,
    pub amount: f32,
    pub mix: f32,
    pub smooth_ms: f32,
    pub multiband: bool,
    pub x_lo: f32,
    pub x_hi: f32,
    pub env_on: bool,
    pub env_amt: f32,
    pub env_att: f32,
    pub env_hold: f32,
    pub env_rel: f32,
    pub env_thr: f32,
    pub env_mult: bool,
    pub env_sc: bool,
    pub swing: f32,
    /// right channel phase offset (0..0.5 of a cycle)
    pub spread: f32,
}

impl Default for ShaperSet {
    fn default() -> Self {
        Self {
            on: false,
            mode: Mode::Beat,
            trigger: Trigger::Off,
            beats: 1.0,
            hz: 1.0,
            offset: 0.0,
            amount: 1.0,
            mix: 1.0,
            smooth_ms: 1.5,
            multiband: false,
            x_lo: 200.0,
            x_hi: 2500.0,
            env_on: false,
            env_amt: 1.0,
            env_att: 5.0,
            env_hold: 0.0,
            env_rel: 150.0,
            env_thr: -40.0,
            env_mult: false,
            env_sc: false,
            swing: 0.0,
            spread: 0.0,
        }
    }
}

/// Plain copy of the parameters for one block.
#[derive(Clone, Copy)]
pub struct Settings {
    pub bypass: bool,
    pub in_gain: f32,
    pub out_gain: f32,
    pub mix: f32,
    pub trig: f32,
    pub midi_switch: bool,
    pub safe_clip: bool,
    pub sh: [ShaperSet; N_SHAPERS],
    // volume
    pub v_trim: f32,
    pub v_comp: bool,
    pub v_thr: f32,
    pub v_ratio: f32,
    pub v_att: f32,
    pub v_hold: f32,
    pub v_rel: f32,
    pub v_makeup: f32,
    pub v_auto: bool,
    pub v_sc: bool,
    // filter
    pub f_type: FilterType,
    pub f_lo: f32,
    pub f_hi: f32,
    pub f_res: f32,
    pub f_stereo: f32,
    // pan / width / time
    pub p_mode: PanMode,
    pub p_haas: f32,
    pub p_haas_ms: f32,
    pub w_gain: f32,
    pub t_range: TimeRange,
    // noise
    pub n_type: NoiseType,
    pub n_hp: f32,
    pub n_lp: f32,
    pub n_stereo: f32,
    pub n_only: bool,
    pub n_env: NoiseEnv,
    pub n_env_amt: f32,
    // crush
    pub c_crunch: f32,
    pub c_push: f32,
    pub c_bits: f32,
    pub c_dither: bool,
    pub c_rate: f32,
    pub c_jitter: f32,
    pub c_aa: bool,
    pub c_lp: f32,
    pub c_gain: f32,
    // drive
    pub d_style: DriveStyle,
    pub d_in: f32,
    pub d_out: f32,
    pub d_push: f32,
    pub d_grip: f32,
    pub d_accent: f32,
    pub d_tone: f32,
}

#[inline]
fn db2lin(db: f32) -> f32 {
    (db * (LN_10 / 20.0)).exp()
}

/// one-pole smoothing coefficient for a time constant in ms
#[inline]
fn coef_ms(ms: f32, sr: f32) -> f32 {
    if ms < 0.005 {
        1.0
    } else {
        1.0 - (-1.0 / (ms * 0.001 * sr)).exp()
    }
}

/// one-pole low-pass coefficient for a cutoff in Hz
#[inline]
fn coef_hz(hz: f32, sr: f32) -> f32 {
    1.0 - (-2.0 * PI * hz.min(sr * 0.45) / sr).exp()
}

impl Settings {
    pub fn from_params(p: &WaveCubeParams) -> Self {
        let mut sh = [ShaperSet::default(); N_SHAPERS];
        for (s, q) in sh.iter_mut().zip(p.sh.iter()) {
            let lo = q.x_lo.value();
            let hi = q.x_hi.value().max(lo * 1.25);
            *s = ShaperSet {
                on: q.on.value(),
                mode: q.mode.value(),
                trigger: q.trigger.value(),
                beats: q.rate.value().beats() * q.feel.value().factor(),
                hz: q.hz.value() as f64,
                offset: q.phase.value() / 360.0,
                amount: q.amount.value(),
                mix: q.mix.value(),
                smooth_ms: q.smooth.value(),
                multiband: q.multiband.value(),
                x_lo: lo,
                x_hi: hi,
                env_on: q.env_on.value(),
                env_amt: q.env_amt.value(),
                env_att: q.env_att.value(),
                env_hold: q.env_hold.value(),
                env_rel: q.env_rel.value(),
                env_thr: q.env_thr.value(),
                env_mult: q.env_mode.value() == EnvMode::Multiply,
                env_sc: q.env_sc.value(),
                swing: q.swing.value(),
                spread: q.spread.value() / 360.0,
            };
        }
        Self {
            bypass: p.bypass.value(),
            in_gain: db2lin(p.in_gain.value()),
            out_gain: db2lin(p.out_gain.value()),
            mix: p.mix.value(),
            trig: db2lin(p.trig_db.value()),
            midi_switch: p.midi_switch.value(),
            safe_clip: p.safe_clip.value(),
            sh,
            v_trim: db2lin(p.v_trim.value()),
            v_comp: p.v_comp.value(),
            v_thr: p.v_thr.value(),
            v_ratio: p.v_ratio.value(),
            v_att: p.v_att.value(),
            v_hold: p.v_hold.value(),
            v_rel: p.v_rel.value(),
            v_makeup: p.v_makeup.value(),
            v_auto: p.v_auto.value(),
            v_sc: p.v_sc.value(),
            f_type: p.f_type.value(),
            f_lo: p.f_lo.value(),
            f_hi: p.f_hi.value(),
            f_res: p.f_res.value(),
            f_stereo: p.f_stereo.value(),
            p_mode: p.p_mode.value(),
            p_haas: p.p_haas.value(),
            p_haas_ms: p.p_haas_ms.value(),
            w_gain: db2lin(p.w_gain.value()),
            t_range: p.t_range.value(),
            n_type: p.n_type.value(),
            n_hp: p.n_hp.value(),
            n_lp: p.n_lp.value(),
            n_stereo: p.n_stereo.value(),
            n_only: p.n_only.value(),
            n_env: p.n_env.value(),
            n_env_amt: p.n_env_amt.value(),
            c_crunch: p.c_crunch.value(),
            c_push: p.c_push.value(),
            c_bits: p.c_bits.value(),
            c_dither: p.c_dither.value(),
            c_rate: p.c_rate.value(),
            c_jitter: p.c_jitter.value(),
            c_aa: p.c_aa.value(),
            c_lp: p.c_lp.value(),
            c_gain: db2lin(p.c_gain.value()),
            d_style: p.d_style.value(),
            d_in: p.d_in.value(),
            d_out: db2lin(p.d_out.value()),
            d_push: p.d_push.value(),
            d_grip: p.d_grip.value(),
            d_accent: p.d_accent.value(),
            d_tone: p.d_tone.value(),
        }
    }
}

// ------------------------------------------------------------------------------------------
// Building blocks
// ------------------------------------------------------------------------------------------

/// Trapezoidal state-variable filter (Cytomic / Simper).
#[derive(Clone, Copy, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
}

#[derive(Clone, Copy)]
pub struct SvfCoef {
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl SvfCoef {
    pub fn new(f: f32, sr: f32, k: f32) -> Self {
        let g = (PI * (f / sr).clamp(1e-5, 0.49)).tan();
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self { g, k, a1, a2, a3: g * a2 }
    }
}

impl Svf {
    /// (low, band, high)
    #[inline]
    pub fn run(&mut self, c: &SvfCoef, v0: f32) -> (f32, f32, f32) {
        let v3 = v0 - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, v0 - c.k * v1 - v2)
    }

    /// one-pole (6 dB) low-pass using the same g; returns (low, high)
    #[inline]
    pub fn one_pole(&mut self, c: &SvfCoef, x: f32) -> (f32, f32) {
        let v = (x - self.ic1) * c.g / (1.0 + c.g);
        let lp = v + self.ic1;
        self.ic1 = lp + v;
        (lp, x - lp)
    }
}

/// Linkwitz-Riley 4th order low / high split: LP and HP sum to an all-pass (flat level).
#[derive(Clone, Copy, Default)]
struct Lr4 {
    a: Svf,
    lp: Svf,
    hp: Svf,
}

impl Lr4 {
    #[inline]
    fn run(&mut self, c: &SvfCoef, x: f32) -> (f32, f32) {
        let (l, _, h) = self.a.run(c, x);
        let (ll, _, _) = self.lp.run(c, l);
        let (_, _, hh) = self.hp.run(c, h);
        (ll, hh)
    }
}

/// Three bands; the low band also goes through the all-pass of the upper split so all three
/// stay in phase and sum flat.
#[derive(Clone, Copy, Default)]
struct Split3 {
    x1: Lr4,
    x2: Lr4,
    ap: Lr4,
}

impl Split3 {
    #[inline]
    fn run(&mut self, c1: &SvfCoef, c2: &SvfCoef, x: f32) -> [f32; 3] {
        let (lo, rest) = self.x1.run(c1, x);
        let (mid, hi) = self.x2.run(c2, rest);
        let (a, b) = self.ap.run(c2, lo);
        [a + b, mid, hi]
    }
}

pub struct Rng(pub u32);

impl Rng {
    /// -1 .. 1
    #[inline]
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
    /// 0 .. 1
    #[inline]
    pub fn uni(&mut self) -> f32 {
        0.5 * (self.next() + 1.0)
    }
}

#[inline]
fn fold(v: f32) -> f32 {
    1.0 - ((v + 1.0).rem_euclid(4.0) - 2.0).abs()
}

#[inline]
fn drive_curve(style: DriveStyle, v: f32) -> f32 {
    match style {
        DriveStyle::SoftClip => v.tanh(),
        DriveStyle::HardClip => v.clamp(-1.0, 1.0),
        DriveStyle::SoftSquare => (v * 4.0).tanh(),
        DriveStyle::HardSquare => (v * 40.0).clamp(-1.0, 1.0),
        DriveStyle::SoftRectify => (v.abs() * 1.5).tanh() * 1.2 - 0.3,
        DriveStyle::HardRectify => v.abs().min(1.0) * 1.4 - 0.4,
        DriveStyle::SkewFold => fold(v + 0.4 * v * v.abs()),
        DriveStyle::SineFold => (v * PI * 0.5).sin(),
        DriveStyle::TriFold => fold(v),
        DriveStyle::ExtremeFold => fold(v * 2.5 + (v * 5.0).sin() * 0.6),
    }
}

/// Soft safety clipper: transparent below 0.9, never above 1.0.
#[inline]
fn safe(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.9 {
        x
    } else {
        (0.9 + 0.1 * ((a - 0.9) * 10.0).tanh()).copysign(x)
    }
}

/// Swing: the second step of every pair of grid steps starts later.
#[inline]
fn swing_warp(ph: f32, swing: f32, grid: f32) -> f32 {
    if swing < 1e-4 {
        return ph;
    }
    let pair = 2.0 / grid;
    let base = (ph / pair).floor() * pair;
    let t = (ph - base) / pair;
    let mid = 0.5 + 0.25 * swing;
    let t2 = if t < mid { 0.5 * t / mid } else { 0.5 + 0.5 * (t - mid) / (1.0 - mid) };
    (base + t2 * pair).min(0.999_999)
}

/// Per-block coefficients for the effects (no exp / tan per sample).
#[derive(Clone, Copy)]
struct Coefs {
    comp_a: f32,
    comp_r: f32,
    comp_hold: u32,
    crush_aa: f32,
    crush_lp: f32,
    crush_steps: f32,
    crush_rate: f32,
    tone_a: f32,
    dc_r: f32,
    air_a: f32,
    tape_a: f32,
    tape_a2: f32,
    fstereo: [f32; 2],
}

impl Coefs {
    fn new(p: &Settings, sr: f32) -> Self {
        Self {
            comp_a: coef_ms(p.v_att.max(0.01), sr),
            comp_r: coef_ms(p.v_rel, sr),
            comp_hold: (p.v_hold * 0.001 * sr) as u32,
            crush_aa: coef_hz(p.c_rate * 0.45, sr),
            crush_lp: coef_hz(p.c_lp, sr),
            crush_steps: 2f32.powf(p.c_bits - 1.0),
            crush_rate: (p.c_rate / sr).min(1.0),
            tone_a: coef_hz(1200.0, sr),
            dc_r: 1.0 - 2.0 * PI * 12.0 / sr,
            air_a: coef_hz(5000.0, sr),
            tape_a: coef_hz(9000.0, sr),
            tape_a2: coef_hz(400.0, sr),
            fstereo: [2f32.powf(p.f_stereo / 24.0), 2f32.powf(-p.f_stereo / 24.0)],
        }
    }
}

/// One noise source (three of them make the stereo noise).
#[derive(Default, Clone, Copy)]
struct NoiseGen {
    b: [f32; 7],
    brown: f32,
    hold: f32,
    cnt: f32,
    rate: f32,
    crack: f32,
    lp: f32,
    lp2: f32,
}

impl NoiseGen {
    #[inline]
    fn pink(&mut self, w: f32) -> f32 {
        let b = &mut self.b;
        b[0] = 0.99886 * b[0] + w * 0.0555179;
        b[1] = 0.99332 * b[1] + w * 0.0750759;
        b[2] = 0.96900 * b[2] + w * 0.1538520;
        b[3] = 0.86650 * b[3] + w * 0.3104856;
        b[4] = 0.55000 * b[4] + w * 0.5329522;
        b[5] = -0.7616 * b[5] - w * 0.0168980;
        let out = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
        b[6] = w * 0.115926;
        out * 0.11
    }

    #[inline]
    fn next(&mut self, t: NoiseType, rng: &mut Rng, sr: f32, hum: f32, c: &Coefs) -> f32 {
        let w = rng.next();
        match t {
            NoiseType::White => w * 0.5,
            NoiseType::Pink => self.pink(w) * 1.2,
            NoiseType::Brown => {
                self.brown = (self.brown + w * 0.02) / 1.02;
                self.brown * 3.0
            }
            NoiseType::Crackle => {
                if rng.uni() < 30.0 / sr {
                    self.crack = rng.next() * 0.9;
                }
                self.crack *= 0.55;
                let hiss = self.pink(w) * 0.04;
                self.crack + hiss
            }
            NoiseType::Digital => {
                self.cnt += self.rate.max(200.0) / sr;
                if self.cnt >= 1.0 {
                    self.cnt -= 1.0;
                    self.hold = w * 0.5;
                    self.rate = 600.0 + rng.uni() * 7000.0;
                }
                self.hold
            }
            NoiseType::Hum => hum + w * 0.004,
            NoiseType::Air => {
                self.lp += (w - self.lp) * c.air_a;
                (w - self.lp) * 0.7
            }
            NoiseType::Tape => {
                let p = self.pink(w);
                self.lp2 += (p - self.lp2) * c.tape_a;
                self.lp += (self.lp2 - self.lp) * c.tape_a2;
                (self.lp2 - self.lp) * 1.6
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// One shaper
// ------------------------------------------------------------------------------------------

#[derive(Default)]
struct BandState {
    /// filter stages per channel (up to 8 for the phasers)
    svf: [[Svf; 8]; 2],
    /// cached filter coefficients per channel: (freq, k, main, second stage)
    fc: [Option<(f32, f32, SvfCoef, SvfCoef)>; 2],
    hold: [f32; 2],
    count: f32,
    lp: [f32; 2],
    aa: [[f32; 2]; 2],
    dc: [(f32, f32); 2],
    grip: f32,
    delay: Vec<[f32; 2]>,
    /// samples written into `delay` since the shaper was switched on
    valid: usize,
    haas: Vec<[f32; 2]>,
    haas_w: usize,
    env: f32,
    env_hold: u32,
    comp: f32,
    comp_hold: u32,
}

struct ShaperDsp {
    kind: Kind,
    phase: f64,
    inc: f64,
    /// one cycle in samples
    cycle: f32,
    /// Time range in samples
    time_len: f32,
    /// LFO value per band and channel (smoothed), and with the envelope applied
    y: [[f32; 2]; N_BANDS],
    y_eff: [[f32; 2]; N_BANDS],
    smooth_a: f32,
    split: [Split3; 2],
    noise_split: [Split3; 2],
    c1: SvfCoef,
    c2: SvfCoef,
    band: [BandState; N_BANDS],
    write: usize,
    was_multi: bool,
    primed: bool,
    primed_bands: usize,
    /// 1-shot finished: hold the end of the shape
    done: bool,
    env_a: f32,
    env_r: f32,
    env_h: u32,
    solo: u32,
    /// on / off fade (0..1) and whether the shaper is running
    fade: f32,
    active: bool,
    amt_s: f32,
    mix_s: f32,
}

impl ShaperDsp {
    fn new(kind: Kind, sr: f32) -> Self {
        let mut band: [BandState; N_BANDS] = Default::default();
        for b in band.iter_mut() {
            if kind == Kind::Time {
                b.delay = vec![[0.0; 2]; (TIME_MAX_S * sr) as usize + 8];
            }
            if kind == Kind::Pan {
                b.haas = vec![[0.0; 2]; (HAAS_MAX_S * sr) as usize + 8];
            }
        }
        Self {
            kind,
            phase: 0.0,
            inc: 0.0,
            cycle: sr,
            time_len: sr,
            y: [[1.0; 2]; N_BANDS],
            y_eff: [[1.0; 2]; N_BANDS],
            smooth_a: 1.0,
            split: Default::default(),
            noise_split: Default::default(),
            c1: SvfCoef::new(200.0, sr, SQRT_2),
            c2: SvfCoef::new(2500.0, sr, SQRT_2),
            band,
            write: 0,
            was_multi: false,
            primed: false,
            primed_bands: 0,
            done: false,
            env_a: 1.0,
            env_r: 1.0,
            env_h: 0,
            solo: 0,
            fade: 0.0,
            active: false,
            amt_s: 1.0,
            mix_s: 1.0,
        }
    }

    /// Fresh state when the shaper starts running (nothing old comes back).
    fn activate(&mut self) {
        self.primed = false;
        self.split = Default::default();
        self.noise_split = Default::default();
        for b in self.band.iter_mut() {
            b.svf = Default::default();
            b.fc = [None; 2];
            b.hold = [0.0; 2];
            b.count = 0.0;
            b.lp = [0.0; 2];
            b.aa = [[0.0; 2]; 2];
            b.dc = [(0.0, 0.0); 2];
            b.grip = 0.0;
            b.env = 0.0;
            b.comp = 0.0;
            b.valid = 0;
            for d in b.haas.iter_mut() {
                *d = [0.0; 2];
            }
        }
    }

    fn reset(&mut self) {
        self.activate();
        self.done = false;
        self.active = false;
        self.fade = 0.0;
    }
}

// ------------------------------------------------------------------------------------------
// Engine
// ------------------------------------------------------------------------------------------

pub struct Engine {
    sr: f32,
    tempo: f32,
    pub baked_version: u32,
    tables: Box<[[[f32; TABLE]; N_BANDS]; N_SHAPERS]>,
    slot_tables: Box<[[f32; TABLE]; N_SLOTS]>,
    slot_dest: [Option<(usize, usize)>; N_SLOTS],
    switch_count: u32,
    order: [usize; N_SHAPERS],
    present: [bool; N_SHAPERS],
    grid: f32,
    sh: Vec<ShaperDsp>,
    rng: Rng,
    co: Coefs,
    // fixed coefficients
    k_fast_rel: f32,
    k_slow_att: f32,
    k_slow_rel: f32,
    k_fol_att: f32,
    k_fol_rel: f32,
    k_fade: f32,
    k_param: f32,
    // audio trigger / follow
    env_fast: f32,
    env_slow: f32,
    hold: u32,
    follow: f32,
    note_hz: f64,
    // noise
    gens: [NoiseGen; 3],
    hum_ph: f32,
    noise_f: [[Svf; 2]; 2],
    noise_c: (SvfCoef, SvfCoef),
    // smoothed globals
    wet: f32,
    in_s: f32,
    out_s: f32,
    gr: f32,
    // scope
    bin: usize,
    acc_in: f32,
    acc_out: f32,
    pk_in: f32,
    pk_out: f32,
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
        let d = Settings::from_params(&WaveCubeParams::default());
        Self {
            sr,
            tempo: 120.0,
            baked_version: 0,
            tables: boxed([[1.0; TABLE]; N_BANDS]),
            slot_tables: boxed([1.0; TABLE]),
            slot_dest: [None; N_SLOTS],
            switch_count: 0,
            order: std::array::from_fn(|i| i),
            present: [true; N_SHAPERS],
            grid: 16.0,
            sh: Kind::ALL.iter().map(|&k| ShaperDsp::new(k, sr)).collect(),
            rng: Rng(0x9e3779b9),
            co: Coefs::new(&d, sr),
            k_fast_rel: coef_ms(12.0, sr),
            k_slow_att: coef_ms(30.0, sr),
            k_slow_rel: coef_ms(250.0, sr),
            k_fol_att: coef_ms(2.0, sr),
            k_fol_rel: coef_ms(80.0, sr),
            k_fade: 1.0 / (0.008 * sr),
            k_param: coef_ms(15.0, sr),
            env_fast: 0.0,
            env_slow: 0.0,
            hold: 0,
            follow: 0.0,
            note_hz: 440.0,
            gens: [NoiseGen::default(); 3],
            hum_ph: 0.0,
            noise_f: Default::default(),
            noise_c: (SvfCoef::new(120.0, sr, SQRT_2), SvfCoef::new(16000.0, sr, SQRT_2)),
            wet: 1.0,
            in_s: 1.0,
            out_s: 1.0,
            gr: 0.0,
            bin: 0,
            acc_in: 0.0,
            acc_out: 0.0,
            pk_in: 0.0,
            pk_out: 0.0,
        }
    }

    pub fn reset(&mut self) {
        for s in self.sh.iter_mut() {
            s.reset();
        }
        self.env_fast = 0.0;
        self.env_slow = 0.0;
        self.hold = 0;
        self.noise_f = Default::default();
    }

    pub fn bake(&mut self, st: &State) {
        for s in 0..N_SHAPERS {
            for b in 0..N_BANDS {
                if let Some(w) = st.waves.get(s).and_then(|w| w.get(b)) {
                    w.bake(&mut self.tables[s][b]);
                }
            }
            self.present[s] = st.present.get(s).copied().unwrap_or(true);
        }
        self.grid = st.grid.max(1) as f32;
        for i in 0..N_SLOTS {
            self.slot_dest[i] = None;
            if let Some(Some(slot)) = st.slots.get(i) {
                slot.wave.bake(&mut self.slot_tables[i]);
                self.slot_dest[i] = Some(((slot.shaper as usize).min(N_SHAPERS - 1), (slot.band as usize).min(N_BANDS - 1)));
            }
        }
        let mut seen = [false; N_SHAPERS];
        let mut k = 0;
        for &i in st.order.iter() {
            let i = i as usize;
            if i < N_SHAPERS && !seen[i] && k < N_SHAPERS {
                seen[i] = true;
                self.order[k] = i;
                k += 1;
            }
        }
        for (i, s) in seen.iter().enumerate() {
            if !s && k < N_SHAPERS {
                self.order[k] = i;
                k += 1;
            }
        }
    }

    fn set_inc(&mut self, si: usize, q: &ShaperSet) {
        let sr = self.sr as f64;
        let s = &mut self.sh[si];
        match q.mode {
            Mode::Hz => {
                s.inc = q.hz / sr;
                s.cycle = (sr / q.hz.max(1e-3)) as f32;
            }
            Mode::Pitch => {
                s.inc = self.note_hz / sr;
                s.cycle = (sr / self.note_hz.max(1e-3)) as f32;
            }
            Mode::Beat => {
                let beats = q.beats.max(1e-3);
                s.inc = self.tempo as f64 / 60.0 / sr / beats;
                s.cycle = (beats * 60.0 / self.tempo as f64 * sr) as f32;
            }
        }
    }

    pub fn begin_block(&mut self, p: &Settings, t: &Transport, shared: &Shared) {
        let tempo = t.tempo.clamp(20.0, 999.0);
        self.tempo = tempo as f32;
        shared.tempo.store(self.tempo, Ordering::Relaxed);
        let sr = self.sr;
        self.co = Coefs::new(p, sr);
        for si in 0..N_SHAPERS {
            let q = p.sh[si];
            self.set_inc(si, &q);
            let s = &mut self.sh[si];
            if q.mode == Mode::Beat && q.trigger == Trigger::Off && t.playing {
                if let Some(pos) = t.pos_beats {
                    s.phase = (pos / q.beats.max(1e-3)).rem_euclid(1.0);
                }
            }
            if q.trigger == Trigger::Off {
                s.done = false;
            }
            s.smooth_a = coef_ms(q.smooth_ms, sr);
            s.env_a = coef_ms(q.env_att, sr);
            s.env_r = coef_ms(q.env_rel, sr);
            s.env_h = (q.env_hold * 0.001 * sr) as u32;
            let bar = 4.0 * 60.0 / self.tempo * sr;
            s.time_len = match p.t_range {
                TimeRange::Cycle => s.cycle,
                TimeRange::Bar1 => bar,
                TimeRange::Bar2 => 2.0 * bar,
                TimeRange::Bar4 => 4.0 * bar,
                TimeRange::Bar8 => 8.0 * bar,
                TimeRange::Ms2 => 0.0025 * sr,
                TimeRange::Ms10 => 0.010 * sr,
                TimeRange::Ms20 => 0.020 * sr,
            };
            s.solo = if q.multiband { shared.solo[si].load(Ordering::Relaxed) & 7 } else { 0 };
            if q.multiband {
                s.c1 = SvfCoef::new(q.x_lo, sr, SQRT_2);
                s.c2 = SvfCoef::new(q.x_hi, sr, SQRT_2);
            }
            if q.multiband != s.was_multi {
                // the bands change: no old audio, no filter memory from the other layout
                s.split = Default::default();
                s.noise_split = Default::default();
                for b in s.band.iter_mut() {
                    b.valid = 0;
                    b.svf = Default::default();
                }
            }
            s.was_multi = q.multiband;
        }
        self.noise_c = (SvfCoef::new(p.n_hp, sr, SQRT_2), SvfCoef::new(p.n_lp.max(p.n_hp * 1.05), sr, SQRT_2));
        self.gr = 0.0;
    }

    #[inline]
    fn lookup(&self, s: usize, b: usize, ph: f32) -> f32 {
        let t = &self.tables[s][b];
        let pos = ph * TABLE as f32;
        let i = (pos as usize).min(TABLE - 1);
        let f = pos - i as f32;
        let a = t[i];
        // the last entry is not blended into the first: a jump at the wrap point stays a jump
        if i + 1 >= TABLE {
            a
        } else {
            a + (t[i + 1] - a) * f
        }
    }

    /// One stereo frame. `sc`: sidechain input; `note`: a MIDI note started at this sample.
    pub fn process(&mut self, x: [f32; 2], sc: [f32; 2], stereo: bool, note: Option<u8>, p: &Settings, shared: &Shared) -> [f32; 2] {
        let sr = self.sr;
        self.in_s += (p.in_gain - self.in_s) * self.k_param;
        self.out_s += (p.out_gain - self.out_s) * self.k_param;
        let input = [x[0] * self.in_s, x[1] * self.in_s];

        // ---- audio trigger: a fast envelope jumping over a slow one
        let lv = input[0].abs().max(input[1].abs());
        let af = if lv > self.env_fast { 0.3 } else { self.k_fast_rel };
        self.env_fast += (lv - self.env_fast) * af;
        let as_ = if lv > self.env_slow { self.k_slow_att } else { self.k_slow_rel };
        self.env_slow += (lv - self.env_slow) * as_;
        let mut audio_trig = false;
        if self.hold > 0 {
            self.hold -= 1;
        } else if self.env_fast > p.trig && self.env_fast > 2.0 * self.env_slow + 1e-4 {
            audio_trig = true;
            self.hold = (0.06 * sr) as u32;
        }
        let fa = if lv > self.follow { self.k_fol_att } else { self.k_fol_rel };
        self.follow += (lv - self.follow) * fa;

        // ---- MIDI: switch user waves (C# .. A) or trigger (any note; only C with MIDI Switch)
        let mut midi_trig = false;
        if let Some(n) = note {
            let pc = (n % 12) as usize;
            if p.midi_switch && (1..=N_SLOTS).contains(&pc) {
                let slot = pc - 1;
                if let Some((s, b)) = self.slot_dest[slot] {
                    self.tables[s][b] = self.slot_tables[slot];
                    self.switch_count = self.switch_count.wrapping_add(1);
                    shared.switched.store((self.switch_count << 8) | slot as u32, Ordering::Release);
                }
            } else if !p.midi_switch || pc == 0 {
                midi_trig = true;
                self.note_hz = 440.0 * 2f64.powf((n as f64 - 69.0) / 12.0);
                for si in 0..N_SHAPERS {
                    if p.sh[si].mode == Mode::Pitch {
                        let q = p.sh[si];
                        self.set_inc(si, &q);
                    }
                }
            }
        }
        for (s, q) in self.sh.iter_mut().zip(p.sh.iter()) {
            if (q.trigger.midi() && midi_trig) || (q.trigger.audio() && audio_trig) {
                s.phase = 0.0;
                s.done = false;
            }
        }

        // ---- noise source (once per frame, split like the signal)
        let ni = Kind::Noise.index();
        let noise = if self.sh[ni].active || (p.sh[ni].on && self.present[ni]) {
            self.hum_ph = (self.hum_ph + 50.0 / sr) % 1.0;
            let h = 2.0 * PI * self.hum_ph;
            let hum = (h.sin() + 0.5 * (2.0 * h).sin() + 0.3 * (3.0 * h).sin() + 0.15 * (5.0 * h).sin()) * 0.3;
            let co = self.co;
            let c = self.gens[0].next(p.n_type, &mut self.rng, sr, hum, &co);
            let l = self.gens[1].next(p.n_type, &mut self.rng, sr, hum, &co);
            let r = self.gens[2].next(p.n_type, &mut self.rng, sr, hum, &co);
            let st = if p.n_type == NoiseType::Hum { 0.0 } else { p.n_stereo };
            let mut n = [c + (l - c) * st, c + (r - c) * st];
            for (ch, v) in n.iter_mut().enumerate() {
                let (_, _, h) = self.noise_f[ch][0].run(&self.noise_c.0, *v);
                let (l, _, _) = self.noise_f[ch][1].run(&self.noise_c.1, h);
                *v = l;
            }
            n
        } else {
            [0.0; 2]
        };

        // ---- the chain
        let mut sig = input;
        for k in 0..N_SHAPERS {
            let si = self.order[k];
            let q = p.sh[si];
            let target = if q.on && self.present[si] { 1.0 } else { 0.0 };
            {
                let s = &mut self.sh[si];
                if target > 0.5 && !s.active {
                    s.activate();
                    s.active = true;
                    s.fade = 0.0;
                }
                if !s.active {
                    continue;
                }
                // linear 8 ms ramp: exactly 1 (fully on) or 0 (off) when it ends
                if target > s.fade {
                    s.fade = (s.fade + self.k_fade).min(1.0);
                } else if target < s.fade {
                    s.fade = (s.fade - self.k_fade).max(0.0);
                }
                if target < 0.5 && s.fade <= 0.0 {
                    s.fade = 0.0;
                    s.active = false;
                    continue;
                }
            }
            let grid = self.grid;
            let base = if self.sh[si].done { 0.99999 } else { (self.sh[si].phase as f32 + q.offset).rem_euclid(1.0) };
            let ph = [swing_warp(base, q.swing, grid), swing_warp((base + q.spread).rem_euclid(1.0), q.swing, grid)];
            let nb = if q.multiband { N_BANDS } else { 1 };
            let primed = self.sh[si].primed && self.sh[si].primed_bands >= nb;
            for b in 0..nb {
                let raw_l = self.lookup(si, b, ph[0]);
                let raw_r = if q.spread > 1e-5 { self.lookup(si, b, ph[1]) } else { raw_l };
                let s = &mut self.sh[si];
                if primed {
                    s.y[b][0] += (raw_l - s.y[b][0]) * s.smooth_a;
                    s.y[b][1] += (raw_r - s.y[b][1]) * s.smooth_a;
                } else {
                    s.y[b] = [raw_l, raw_r];
                }
            }
            {
                let s = &mut self.sh[si];
                if !s.primed {
                    s.amt_s = q.amount;
                    s.mix_s = q.mix;
                }
                s.amt_s += (q.amount - s.amt_s) * self.k_param;
                s.mix_s += (q.mix - s.mix_s) * self.k_param;
                s.primed = true;
                s.primed_bands = nb;
            }
            let mut qs = q;
            qs.amount = self.sh[si].amt_s;
            qs.mix = self.sh[si].mix_s;
            let out = self.run_shaper(si, sig, sc, stereo, &qs, p, noise);
            let f = self.sh[si].fade;
            sig = [sig[0] + (out[0] - sig[0]) * f, sig[1] + (out[1] - sig[1]) * f];
        }

        // ---- advance every LFO (also those that are off, so they stay in time)
        for (s, q) in self.sh.iter_mut().zip(p.sh.iter()) {
            if s.done {
                continue;
            }
            s.phase += s.inc;
            if s.phase >= 1.0 {
                if q.trigger.once() {
                    s.phase = 0.99999;
                    s.done = true;
                } else {
                    s.phase -= s.phase.floor();
                }
            }
        }

        // ---- master mix / bypass (short fade, no clicks), safety clipper
        let target = if p.bypass { 0.0 } else { p.mix };
        self.wet += (target - self.wet) * self.k_param;
        let mut out = [x[0] + (sig[0] * self.out_s - x[0]) * self.wet, x[1] + (sig[1] * self.out_s - x[1]) * self.wet];
        for o in out.iter_mut() {
            if !o.is_finite() {
                *o = 0.0;
            } else if p.safe_clip && !p.bypass {
                *o = safe(*o);
            }
        }

        // ---- scope of the shaper shown in the editor (skipped for audio-rate LFOs)
        let view = (shared.view.load(Ordering::Relaxed) as usize).min(N_SHAPERS - 1);
        if self.sh[view].inc < 1.0 / 32.0 {
            let vph = (self.sh[view].phase as f32 + p.sh[view].offset).rem_euclid(1.0);
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
        let li = x[0].abs().max(x[1].abs());
        let lo = out[0].abs().max(out[1].abs());
        self.acc_in = self.acc_in.max(li);
        self.acc_out = self.acc_out.max(lo);
        self.pk_in = self.pk_in.max(li);
        self.pk_out = self.pk_out.max(lo);
        out
    }

    pub fn end_block(&mut self, shared: &Shared) {
        for (i, s) in self.sh.iter().enumerate() {
            shared.phase[i].store(if s.done { 0.99999 } else { s.phase as f32 }, Ordering::Relaxed);
            for b in 0..N_BANDS {
                shared.value[i][b].store(s.y_eff[b][0], Ordering::Relaxed);
            }
            shared.env[i].store(s.band[0].env, Ordering::Relaxed);
        }
        shared.gr.store(self.gr, Ordering::Relaxed);
        shared.in_pk.store(shared.in_pk.load(Ordering::Relaxed).max(self.pk_in), Ordering::Relaxed);
        shared.out_pk.store(shared.out_pk.load(Ordering::Relaxed).max(self.pk_out), Ordering::Relaxed);
        self.pk_in = 0.0;
        self.pk_out = 0.0;
    }

    #[allow(clippy::too_many_arguments)]
    fn run_shaper(&mut self, si: usize, x: [f32; 2], sc: [f32; 2], stereo: bool, q: &ShaperSet, p: &Settings, noise: [f32; 2]) -> [f32; 2] {
        let kind = self.sh[si].kind;
        if !q.multiband {
            let wet = self.band_fx(si, 0, x, sc, noise, stereo, q, p);
            return [x[0] + (wet[0] - x[0]) * q.mix, x[1] + (wet[1] - x[1]) * q.mix];
        }
        let s = &mut self.sh[si];
        let (c1, c2) = (s.c1, s.c2);
        let bl = s.split[0].run(&c1, &c2, x[0]);
        let br = s.split[1].run(&c1, &c2, x[1]);
        let (nl, nr) = if kind == Kind::Noise {
            (s.noise_split[0].run(&c1, &c2, noise[0]), s.noise_split[1].run(&c1, &c2, noise[1]))
        } else {
            ([0.0; 3], [0.0; 3])
        };
        let solo = s.solo;
        let mut out = [0.0f32; 2];
        for b in 0..N_BANDS {
            let xb = [bl[b], br[b]];
            let wet = self.band_fx(si, b, xb, sc, [nl[b], nr[b]], stereo, q, p);
            if solo != 0 && solo & (1 << b) == 0 {
                continue;
            }
            out[0] += xb[0] + (wet[0] - xb[0]) * q.mix;
            out[1] += xb[1] + (wet[1] - xb[1]) * q.mix;
        }
        out
    }

    /// Envelope follower on the band (or the sidechain), applied to the LFO value; then the effect.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn band_fx(&mut self, si: usize, b: usize, x: [f32; 2], sc: [f32; 2], noise: [f32; 2], stereo: bool, q: &ShaperSet, p: &Settings) -> [f32; 2] {
        let s = &mut self.sh[si];
        let mut y = s.y[b];
        if q.env_on && s.kind.has_envelope() {
            let bs = &mut s.band[b];
            let src = if q.env_sc { sc } else { x };
            let lvl = src[0].abs().max(src[1].abs());
            if lvl > bs.env {
                bs.env += (lvl - bs.env) * s.env_a;
                bs.env_hold = s.env_h;
            } else if bs.env_hold > 0 {
                bs.env_hold -= 1;
            } else {
                bs.env += (lvl - bs.env) * s.env_r;
            }
            let db = 20.0 * bs.env.max(1e-6).log10();
            let thr = q.env_thr.min(-0.5);
            let e = ((db - thr) / -thr).clamp(0.0, 1.0);
            for v in y.iter_mut() {
                *v = if q.env_mult {
                    if q.env_amt >= 0.0 {
                        *v * (e * q.env_amt).clamp(0.0, 1.0)
                    } else {
                        *v * (1.0 - (e * -q.env_amt).clamp(0.0, 1.0))
                    }
                } else {
                    (*v + q.env_amt * e).clamp(0.0, 1.0)
                };
            }
        }
        s.y_eff[b] = y;
        self.effect(si, b, y, x, sc, noise, stereo, q, p)
    }

    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn effect(&mut self, si: usize, b: usize, y: [f32; 2], x: [f32; 2], sc: [f32; 2], noise: [f32; 2], stereo: bool, q: &ShaperSet, p: &Settings) -> [f32; 2] {
        let amt = q.amount;
        let sr = self.sr;
        let co = self.co;
        match self.sh[si].kind {
            Kind::Volume => {
                let g = [(1.0 - amt * (1.0 - y[0])) * p.v_trim, (1.0 - amt * (1.0 - y[1])) * p.v_trim];
                let mut cg = 1.0;
                if p.v_comp {
                    let bs = &mut self.sh[si].band[b];
                    let src = if p.v_sc { sc } else { x };
                    let det = src[0].abs().max(src[1].abs());
                    if det > bs.comp {
                        bs.comp += (det - bs.comp) * co.comp_a;
                        bs.comp_hold = co.comp_hold;
                    } else if bs.comp_hold > 0 {
                        bs.comp_hold -= 1;
                    } else {
                        bs.comp += (det - bs.comp) * co.comp_r;
                    }
                    let lvl = 20.0 * bs.comp.max(1e-6).log10();
                    let slope = 1.0 - 1.0 / p.v_ratio.max(1.0);
                    let gr = (lvl - p.v_thr).max(0.0) * slope;
                    let makeup = p.v_makeup + if p.v_auto { -p.v_thr * slope * 0.5 } else { 0.0 };
                    cg = db2lin(makeup - gr);
                    self.gr = self.gr.max(gr);
                }
                [x[0] * g[0] * cg, x[1] * g[1] * cg]
            }
            Kind::Pan => {
                let bs = &mut self.sh[si].band[b];
                // the Haas line always runs, so switching Haas on never plays stale audio
                let len = bs.haas.len();
                let w = bs.haas_w;
                bs.haas[w] = x;
                bs.haas_w = (w + 1) % len;
                if !stereo {
                    return x;
                }
                // +1 = left
                let pv = (amt * (2.0 * y[0] - 1.0)).clamp(-1.0, 1.0);
                let k = pv.abs();
                let lvl = match p.p_mode {
                    PanMode::Combined => {
                        if pv >= 0.0 {
                            [x[0] + x[1] * k * 0.5, x[1] * (1.0 - k)]
                        } else {
                            [x[0] * (1.0 - k), x[1] + x[0] * k * 0.5]
                        }
                    }
                    m => {
                        let comp = match m {
                            PanMode::Bal0 => 0.0,
                            PanMode::Bal3 => 3.0,
                            PanMode::Bal45 => 4.5,
                            _ => 6.0,
                        };
                        let near = db2lin(comp * k);
                        let far = 1.0 - k;
                        if pv >= 0.0 {
                            [x[0] * near, x[1] * far]
                        } else {
                            [x[0] * far, x[1] * near]
                        }
                    }
                };
                if p.p_haas < 1e-4 {
                    return lvl;
                }
                let d = (p.p_haas_ms * 0.001 * sr * k).clamp(0.0, (len - 2) as f32);
                let pos = w as f32 - d;
                let i = pos.floor();
                let f = pos - i;
                let i0 = (i as isize).rem_euclid(len as isize) as usize;
                let i1 = if d < 1.0 { w } else { (i0 + 1) % len };
                let rd = |ch: usize| bs.haas[i0][ch] + (bs.haas[i1][ch] - bs.haas[i0][ch]) * f;
                let haas = if pv >= 0.0 { [x[0], rd(1)] } else { [rd(0), x[1]] };
                [lvl[0] + (haas[0] - lvl[0]) * p.p_haas, lvl[1] + (haas[1] - lvl[1]) * p.p_haas]
            }
            Kind::Width => {
                if !stereo {
                    return x;
                }
                let w = (1.0 + amt * (2.0 * y[0] - 1.0)).max(0.0);
                let m = 0.5 * (x[0] + x[1]);
                let s = 0.5 * (x[0] - x[1]) * w;
                [(m + s) * p.w_gain, (m - s) * p.w_gain]
            }
            Kind::Filter => self.filter(si, b, y, x, q, p),
            Kind::Time => {
                let s = &mut self.sh[si];
                let len = s.band[b].delay.len();
                let w = s.write;
                let bs = &mut s.band[b];
                bs.delay[w] = x;
                bs.valid = (bs.valid + 1).min(len);
                // never older than what was written since the shaper started
                let max_d = (bs.valid as f32 - 4.0).max(0.0);
                let mut out = [0.0; 2];
                for ch in 0..2 {
                    let d = ((1.0 - y[ch]) * amt * s.time_len).clamp(0.0, max_d.min((len - 4) as f32));
                    out[ch] = if d < 1e-3 {
                        x[ch]
                    } else {
                        let buf = &bs.delay;
                        let pos = w as f32 - d;
                        let n = len as isize;
                        let i = pos.floor();
                        let t = pos - i;
                        let i = i as isize;
                        let at = |k: isize| buf[(i + k).rem_euclid(n) as usize][ch];
                        if d < 2.0 {
                            // too close to the write head for 4-point interpolation
                            at(0) + (at(1) - at(0)) * t
                        } else {
                            let (xm1, x0, x1, x2) = (at(-1), at(0), at(1), at(2));
                            let c1 = 0.5 * (x1 - xm1);
                            let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
                            let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
                            ((c3 * t + c2) * t + c1) * t + x0
                        }
                    };
                }
                let last = if q.multiband { N_BANDS - 1 } else { 0 };
                if b == last {
                    s.write = (w + 1) % len;
                }
                out
            }
            Kind::Noise => {
                let f = (self.follow * 4.0).min(1.0);
                let env = match p.n_env {
                    NoiseEnv::Off => 1.0,
                    NoiseEnv::Follow => 1.0 + (f - 1.0) * p.n_env_amt,
                    NoiseEnv::Duck => 1.0 - p.n_env_amt * f,
                };
                let base = if p.n_only { [0.0; 2] } else { x };
                let mut out = [base[0] + noise[0] * amt * y[0] * env, base[1] + noise[1] * amt * y[1] * env];
                if !stereo {
                    out[1] = out[0];
                }
                out
            }
            Kind::Crush => {
                let c = [(amt * y[0]).clamp(0.0, 1.0), (amt * y[1]).clamp(0.0, 1.0)];
                if c[0] < 1e-4 && c[1] < 1e-4 {
                    return x;
                }
                let crunch = db2lin(p.c_crunch);
                let steps = co.crush_steps;
                let bits_on = p.c_bits < 15.95;
                let mut v = [0.0f32; 2];
                for ch in 0..2 {
                    let mut s = x[ch];
                    if p.c_crunch > 0.05 {
                        let push = p.c_push * 0.5;
                        let t = s * crunch + push;
                        s = ((t + 1.0).rem_euclid(2.0) - 1.0) - push;
                    }
                    if bits_on {
                        if p.c_dither {
                            s += (self.rng.next() + self.rng.next()) * 0.5 / steps;
                        }
                        s = (s * steps).round() / steps;
                    }
                    v[ch] = s;
                }
                let jit = p.c_jitter * self.rng.next() * 0.9;
                let bs = &mut self.sh[si].band[b];
                if p.c_aa {
                    for ch in 0..2 {
                        bs.aa[0][ch] += (v[ch] - bs.aa[0][ch]) * co.crush_aa;
                        bs.aa[1][ch] += (bs.aa[0][ch] - bs.aa[1][ch]) * co.crush_aa;
                        v[ch] = bs.aa[1][ch];
                    }
                }
                bs.count += co.crush_rate;
                if bs.count >= 1.0 || co.crush_rate >= 1.0 {
                    bs.count = (bs.count - 1.0 - jit * co.crush_rate.min(0.5)).max(0.0);
                    bs.hold = v;
                }
                let mut out = [0.0; 2];
                for ch in 0..2 {
                    bs.lp[ch] += (bs.hold[ch] - bs.lp[ch]) * co.crush_lp;
                    let wet = bs.lp[ch] * p.c_gain;
                    out[ch] = x[ch] + (wet - x[ch]) * c[ch];
                }
                out
            }
            Kind::Drive => {
                let bs = &mut self.sh[si].band[b];
                // grip: quiet parts get quieter
                let lvl = x[0].abs().max(x[1].abs());
                let ga = if lvl > bs.grip { 0.05 } else { 0.0005 };
                bs.grip += (lvl - bs.grip) * ga;
                let grip = if p.d_grip > 0.0 { bs.grip / (bs.grip + p.d_grip * 0.08 + 1e-9) } else { 1.0 };
                let push = p.d_push * 0.6;
                let mut out = x;
                for ch in 0..2 {
                    let c = (amt * y[ch]).clamp(0.0, 1.0);
                    // the filters keep running at zero drive, so nothing jumps when it comes back
                    let lg = (p.d_in + c * 36.0) * (LN_10 / 20.0);
                    let g = lg.exp();
                    let comp = (lg * -0.7 * (1.0 - p.d_accent)).exp();
                    let sat = drive_curve(p.d_style, x[ch] * g + push);
                    let (x1, y1) = bs.dc[ch];
                    let yv = sat - x1 + co.dc_r * y1;
                    bs.dc[ch] = (sat, yv);
                    let sat = yv * comp * grip;
                    bs.lp[ch] += (sat - bs.lp[ch]) * co.tone_a;
                    let tone = p.d_tone * c;
                    let toned = if tone < 0.0 { sat + (bs.lp[ch] - sat) * -tone } else { sat + (sat - bs.lp[ch]) * tone };
                    let blend = (c * 4.0).min(1.0);
                    out[ch] = x[ch] + (toned * p.d_out - x[ch]) * blend;
                }
                out
            }
        }
    }

    fn filter(&mut self, si: usize, b: usize, y: [f32; 2], x: [f32; 2], q: &ShaperSet, p: &Settings) -> [f32; 2] {
        let sr = self.sr;
        let lo = p.f_lo.min(p.f_hi);
        let hi = p.f_hi.max(lo);
        let ratio = hi / lo;
        let res = p.f_res;
        let ty = p.f_type;
        use FilterType::*;
        let phaser = matches!(ty, PhN2 | PhN4 | PhN6 | PhN8 | PhP2 | PhP4 | PhP6 | PhP8);
        let k = if phaser { 2.0 * (1.0 - 0.85 * res) } else { 2.0 * (1.0 - res).powf(1.5) + 0.06 };
        let comp = 1.0 - 0.45 * res;
        let fst = self.co.fstereo;
        let bs = &mut self.sh[si].band[b];
        let mut out = [0.0; 2];
        for ch in 0..2 {
            let ye = 1.0 - q.amount * (1.0 - y[ch]);
            let fc = (lo * ratio.powf(ye) * fst[ch]).clamp(15.0, 0.47 * sr);
            // new coefficients only when the cutoff moved by more than 0.2 % (inaudible steps)
            let (c, c2) = match bs.fc[ch] {
                Some((cf, ck, c, c2)) if (cf - fc).abs() <= fc * 2e-3 && (ck - k).abs() < 1e-6 => (c, c2),
                _ => {
                    let c = SvfCoef::new(fc, sr, k);
                    let c2 = SvfCoef::new(fc, sr, SQRT_2.max(k));
                    bs.fc[ch] = Some((fc, k, c, c2));
                    (c, c2)
                }
            };
            let st = &mut bs.svf[ch];
            let v = x[ch];
            out[ch] = match ty {
                Lp6 => st[0].one_pole(&c, v).0,
                Hp6 => st[0].one_pole(&c, v).1,
                Lp12 => st[0].run(&c, v).0 * comp,
                Hp12 => st[0].run(&c, v).2 * comp,
                Bp12 => st[0].run(&c, v).1 * c.k,
                Notch12 => {
                    let (l, _, h) = st[0].run(&c, v);
                    l + h
                }
                Peak12 => {
                    let (l, _, h) = st[0].run(&c, v);
                    (l - h) * comp
                }
                Lp24 => {
                    let (l, _, _) = st[0].run(&c, v);
                    st[1].run(&c2, l).0 * comp
                }
                Hp24 => {
                    let (_, _, h) = st[0].run(&c, v);
                    st[1].run(&c2, h).2 * comp
                }
                Bp24 => {
                    let (_, bp, _) = st[0].run(&c, v);
                    st[1].run(&c, bp * c.k).1 * c.k
                }
                Notch24 => {
                    let (l, _, h) = st[0].run(&c, v);
                    let (l2, _, h2) = st[1].run(&c, l + h);
                    l2 + h2
                }
                Peak24 => {
                    let (l, _, h) = st[0].run(&c, v);
                    let (l2, _, h2) = st[1].run(&c, (l - h) * comp);
                    (l2 - h2) * comp
                }
                _ => {
                    let (n, peaks) = match ty {
                        PhN2 => (2, false),
                        PhN4 => (4, false),
                        PhN6 => (6, false),
                        PhN8 => (8, false),
                        PhP2 => (2, true),
                        PhP4 => (4, true),
                        PhP6 => (6, true),
                        _ => (8, true),
                    };
                    // n second-order all-passes: n notches (x + ap) or n peaks (x - ap)
                    let mut a = v;
                    for s in st.iter_mut().take(n) {
                        let (_, bp, _) = s.run(&c, a);
                        a -= 2.0 * c.k * bp;
                    }
                    if peaks {
                        0.5 * (v - a)
                    } else {
                        0.5 * (v + a)
                    }
                }
            };
        }
        out
    }
}

#[cfg(test)]
#[path = "dsp_tests.rs"]
mod tests;
