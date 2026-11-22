//! WaveCube - modular shaper effect (VST3).
//!
//! A chain of up to 12 items, any order, any type, duplicates allowed: Volume, Filter, Pan, Width,
//! Time, Noise, Crush, Drive, Reverb and Liquid shapers, plus Compressor and Scope tools. A shaper
//! has one or two modulated parameters ("lanes"), each with a drawn shape, its own LFO settings
//! and envelope follower, per band in multiband mode.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

pub mod dsp;
pub mod kinds;
pub mod presets;
pub mod wave;
mod editor;

pub use kinds::Kind;
use wave::Wave;

/// chain slots
pub const N_SLOTS: usize = 12;
pub const N_LANES: usize = 2;
pub const N_BANDS: usize = 3;
pub const N_CUSTOM: usize = 9;
/// generic knobs per slot (their meaning depends on the slot's kind)
pub const N_KNOBS: usize = 10;

// ------------------------------------------------------------------------------------------
// Settings that are saved with the project (not host-automatable)
// ------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Lfo {
    /// 0 beat, 1 Hz, 2 pitch
    pub mode: u8,
    /// index into kinds::LENGTHS
    pub length: u8,
    pub feel: u8,
    pub hz: f32,
    /// pitch mode: transpose in semitones (±36)
    pub transpose: f32,
    /// 0 sync, 1 MIDI, 2 audio
    pub trigger: u8,
    /// off: 1-shot, stops at `end`
    pub looping: bool,
    pub end: f32,
    pub phase: f32,
    pub smooth: f32,
    pub swing: f32,
    /// right channel offset, degrees
    pub spread: f32,
}

impl Default for Lfo {
    fn default() -> Self {
        Self { mode: 0, length: 5, feel: 0, hz: 1.0, transpose: 0.0, trigger: 0, looping: true, end: 1.0, phase: 0.0, smooth: 1.5, swing: 0.0, spread: 0.0 }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Env {
    pub on: bool,
    /// -2 .. 2
    pub amount: f32,
    /// 0: ×1, 1: ×2, 2: ×4, 3: ×8
    pub depth: u8,
    pub attack: f32,
    pub hold: f32,
    pub release: f32,
    pub adaptive: bool,
    pub thresh: f32,
    /// -1 .. 1: moves the resting point
    pub shift: f32,
    pub multiply: bool,
    /// 0 band, 1 free (own filters), 2 sidechain
    pub input: u8,
    pub hp: f32,
    pub lp: f32,
}

impl Default for Env {
    fn default() -> Self {
        Self { on: false, amount: 1.0, depth: 0, attack: 5.0, hold: 0.0, release: 150.0, adaptive: false, thresh: -40.0, shift: 0.0, multiply: false, input: 0, hp: 20.0, lp: 20000.0 }
    }
}

/// Everything about a slot that is not a wave and not automatable. Plain data (Copy) so the
/// audio thread can take a snapshot without allocating.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct SlotSet {
    pub kind: Kind,
    pub multiband: bool,
    pub x_lo: f32,
    pub x_hi: f32,
    /// band split slope: 0 6 dB, 1 12 dB, 2 24 dB
    pub slope: u8,
    pub lfo: [[Lfo; N_BANDS]; N_LANES],
    pub link: [bool; N_LANES],
    pub env: [[Env; N_BANDS]; N_LANES],
    // filter
    pub f_type: u8,
    pub f_warm: bool,
    pub f_slope: u8,
    pub f_safe: bool,
    pub f_hard: bool,
    // drive
    pub d_style: u8,
    // noise: < 1000 built-in, >= 1000 user file
    pub n_type: u16,
    pub n_mono: bool,
    pub n_only: bool,
    pub n_env: u8,
    // reverb: < 1000 built-in, >= 1000 user IR
    pub r_type: u16,
    pub r_gated: bool,
    pub r_clear: bool,
    // liquid
    pub l_type: u8,
    pub l_neg: bool,
    pub l_stages: u8,
    // pan / time / crush / compressor / scope
    pub p_law: u8,
    pub t_range: u8,
    pub t_instant: bool,
    pub c_dither: bool,
    pub c_prefilter: bool,
    pub comp_auto: bool,
    pub comp_adaptive: bool,
    pub comp_input: u8,
    pub scope_len: u8,
    pub scope_split: bool,
}

impl Default for SlotSet {
    fn default() -> Self {
        Self::new(Kind::Volume)
    }
}

impl SlotSet {
    pub fn new(kind: Kind) -> Self {
        let mut lfo = Lfo { length: kind.default_length(), ..Lfo::default() };
        if kind == Kind::Time {
            lfo.smooth = 4.0;
        }
        Self {
            kind,
            multiband: false,
            x_lo: 200.0,
            x_hi: 2500.0,
            slope: 2,
            lfo: [[lfo; N_BANDS]; N_LANES],
            link: [true; N_LANES],
            env: [[Env::default(); N_BANDS]; N_LANES],
            f_type: 0,
            f_warm: false,
            f_slope: 1,
            f_safe: true,
            f_hard: false,
            d_style: 0,
            n_type: 0,
            n_mono: false,
            n_only: false,
            n_env: 0,
            r_type: 5,
            r_gated: false,
            r_clear: false,
            l_type: 2,
            l_neg: false,
            l_stages: 1,
            p_law: 1,
            t_range: 0,
            t_instant: true,
            c_dither: false,
            c_prefilter: true,
            comp_auto: true,
            comp_adaptive: false,
            comp_input: 0,
            scope_len: 5,
            scope_split: false,
        }
    }

    /// The LFO settings a band really uses (linked bands follow the mid band, like the host
    /// automation does).
    pub fn lfo_of(&self, lane: usize, band: usize) -> Lfo {
        let l = self.lfo[lane][band];
        if self.link[lane] && self.multiband {
            let m = self.lfo[lane][1];
            Lfo { mode: m.mode, length: m.length, feel: m.feel, hz: m.hz, transpose: m.transpose, trigger: m.trigger, looping: m.looping, ..l }
        } else if !self.multiband {
            self.lfo[lane][0]
        } else {
            l
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct AudioTrig {
    pub sidechain: bool,
    pub hp: f32,
    pub lp: f32,
    /// 0 drums, 1 general, 2 complex
    pub algo: u8,
    /// sensitivity 0..1
    pub detail: f32,
    /// delay of the trigger, 0..4 ms
    pub shift: f32,
}

impl Default for AudioTrig {
    fn default() -> Self {
        Self { sidechain: false, hp: 20.0, lp: 20000.0, algo: 0, detail: 0.5, shift: 0.0 }
    }
}

/// A custom wave slot: a shape and where it came from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Custom {
    pub slot: u8,
    pub lane: u8,
    pub band: u8,
    pub kind: Kind,
    pub wave: Wave,
}

/// Saved with the project / preset.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct State {
    /// the chain: slot indices in processing order
    pub chain: Vec<u8>,
    pub slots: Vec<SlotSet>,
    /// waves[slot][lane][band]
    pub waves: Vec<Vec<Vec<Wave>>>,
    pub custom: Vec<Option<Custom>>,
    pub trig: AudioTrig,
    /// 0 = all
    pub midi_channel: u8,
    pub preset: String,
    pub selected: u8,
    pub lane: u8,
    pub band: u8,
    pub grid: u8,
    pub scope_mode: u8,
    /// user noise / IR file names (index = n_type / r_type - 1000)
    pub user_noises: Vec<String>,
    pub user_irs: Vec<String>,
}

impl Default for State {
    fn default() -> Self {
        let mut st = Self {
            chain: vec![],
            slots: vec![SlotSet::default(); N_SLOTS],
            waves: vec![],
            custom: vec![None; N_CUSTOM],
            trig: AudioTrig::default(),
            midi_channel: 0,
            preset: "Init".into(),
            selected: 0,
            lane: 0,
            band: 0,
            grid: 16,
            scope_mode: 0,
            user_noises: vec![],
            user_irs: vec![],
        };
        st.sanitize();
        st.put(0, Kind::Volume);
        st.chain = vec![0];
        st
    }
}

pub const GRIDS: [u8; 7] = [4, 8, 12, 16, 24, 32, 64];

impl State {
    /// An empty chain.
    pub fn empty() -> Self {
        let mut s = Self::default();
        s.chain.clear();
        s
    }

    /// Sets a slot to a fresh shaper of `kind` (settings and shapes; not the chain).
    pub fn put(&mut self, slot: usize, kind: Kind) {
        self.slots[slot] = SlotSet::new(kind);
        self.waves[slot] = (0..N_LANES).map(|l| vec![kind.default_wave(l); N_BANDS]).collect();
    }

    /// First slot not in the chain.
    pub fn free_slot(&self) -> Option<usize> {
        (0..N_SLOTS).find(|s| !self.chain.contains(&(*s as u8)))
    }

    /// Repairs a state from an older / damaged save.
    pub fn sanitize(&mut self) {
        self.slots.resize(N_SLOTS, SlotSet::default());
        self.waves.resize(N_SLOTS, Vec::new());
        for (s, w) in self.waves.iter_mut().enumerate() {
            let kind = self.slots[s].kind;
            w.resize(N_LANES, Vec::new());
            for (l, lane) in w.iter_mut().enumerate() {
                lane.resize(N_BANDS, kind.default_wave(l));
                for b in lane.iter_mut() {
                    b.sanitize();
                }
            }
        }
        let mut seen = [false; N_SLOTS];
        self.chain.retain(|&i| (i as usize) < N_SLOTS && !std::mem::replace(&mut seen[i as usize], true));
        for s in self.slots.iter_mut() {
            s.x_lo = s.x_lo.clamp(20.0, 20000.0);
            s.x_hi = s.x_hi.clamp(s.x_lo * 1.1, 20000.0);
            s.slope = s.slope.min(2);
            for lane in s.lfo.iter_mut() {
                for l in lane.iter_mut() {
                    l.mode = l.mode.min(2);
                    l.length = l.length.min(12);
                    l.feel = l.feel.min(2);
                    l.trigger = l.trigger.min(2);
                    l.hz = l.hz.clamp(0.02, 5240.0);
                    l.end = l.end.clamp(0.01, 1.0);
                }
            }
        }
        self.custom.resize(N_CUSTOM, None);
        for c in self.custom.iter_mut().flatten() {
            c.slot = c.slot.min(N_SLOTS as u8 - 1);
            c.lane = c.lane.min(N_LANES as u8 - 1);
            c.band = c.band.min(N_BANDS as u8 - 1);
            c.wave.sanitize();
        }
        if !self.chain.contains(&self.selected) {
            self.selected = self.chain.first().copied().unwrap_or(0);
        }
        let k = self.slots[self.selected as usize].kind;
        self.lane = self.lane.min(k.n_lanes().max(1) as u8 - 1);
        self.band = self.band.min(N_BANDS as u8 - 1);
        if !GRIDS.contains(&self.grid) {
            self.grid = 16;
        }
        self.midi_channel = self.midi_channel.min(16);
    }
}

// ------------------------------------------------------------------------------------------
// Parameters (host-automatable)
// ------------------------------------------------------------------------------------------

fn lin(min: f32, max: f32) -> FloatRange {
    FloatRange::Linear { min, max }
}

fn pct_str() -> Arc<dyn Fn(f32) -> String + Send + Sync> {
    Arc::new(|v| format!("{:.0}", (v * 100.0).round() + 0.0))
}

fn parse_pct() -> Arc<dyn Fn(&str) -> Option<f32> + Send + Sync> {
    Arc::new(|s| s.trim().trim_end_matches('%').trim().parse::<f32>().ok().map(|v| v / 100.0))
}

fn percent(name: impl Into<String>, def: f32) -> FloatParam {
    FloatParam::new(name, def, lin(0.0, 1.0)).with_unit(" %").with_value_to_string(pct_str()).with_string_to_value(parse_pct())
}

fn db(name: impl Into<String>, def: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, def, lin(min, max))
        .with_unit(" dB")
        .with_step_size(0.1)
        .with_value_to_string(Arc::new(|v| format!("{:.1}", (v * 10.0).round() / 10.0 + 0.0)))
        .with_string_to_value(Arc::new(|s| s.trim().trim_end_matches("dB").trim().parse().ok()))
}

/// The kind of every slot, shared with the parameter display (a knob shows its value in the
/// units of the shaper that is in the slot).
pub type SlotKinds = Arc<[AtomicU8; N_SLOTS]>;

fn knob_param(slot: usize, k: usize, kinds: &SlotKinds) -> FloatParam {
    let kd = kinds.clone();
    let kp = kinds.clone();
    FloatParam::new(format!("Slot {} Knob {}", slot + 1, k + 1), 0.5, lin(0.0, 1.0))
        .with_value_to_string(Arc::new(move |n| {
            let kind = Kind::from_index(kd[slot].load(Ordering::Relaxed) as usize);
            kind.knobs().get(k).map(|d| d.fmt(n)).unwrap_or_else(|| format!("{:.0} %", n * 100.0))
        }))
        .with_string_to_value(Arc::new(move |s| {
            let kind = Kind::from_index(kp[slot].load(Ordering::Relaxed) as usize);
            kind.knobs().get(k).and_then(|d| d.parse(s))
        }))
}

/// The automatable controls of one chain slot.
#[derive(Params)]
pub struct SlotParams {
    #[id = "on"]
    pub on: BoolParam,
    /// lane offsets ("tiles"): move the whole shape up / down
    #[id = "ta"]
    pub tile_a: FloatParam,
    #[id = "tb"]
    pub tile_b: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "mxl"]
    pub mix_lo: FloatParam,
    #[id = "mxm"]
    pub mix_mid: FloatParam,
    #[id = "mxh"]
    pub mix_hi: FloatParam,
    #[id = "k0"]
    pub k0: FloatParam,
    #[id = "k1"]
    pub k1: FloatParam,
    #[id = "k2"]
    pub k2: FloatParam,
    #[id = "k3"]
    pub k3: FloatParam,
    #[id = "k4"]
    pub k4: FloatParam,
    #[id = "k5"]
    pub k5: FloatParam,
    #[id = "k6"]
    pub k6: FloatParam,
    #[id = "k7"]
    pub k7: FloatParam,
    #[id = "k8"]
    pub k8: FloatParam,
    #[id = "k9"]
    pub k9: FloatParam,
}

impl SlotParams {
    fn new(slot: usize, kinds: &SlotKinds) -> Self {
        let n = slot + 1;
        let tile = |name: String| {
            FloatParam::new(name, 0.0, lin(-1.0, 1.0))
                .with_unit(" %")
                .with_value_to_string(Arc::new(|v| format!("{:+.0}", v * 100.0 + 0.0)))
                .with_string_to_value(Arc::new(|s| s.trim().trim_end_matches('%').trim().parse::<f32>().ok().map(|v| v / 100.0)))
        };
        Self {
            on: BoolParam::new(format!("Slot {n} On"), true),
            tile_a: tile(format!("Slot {n} Offset A")),
            tile_b: tile(format!("Slot {n} Offset B")),
            mix: percent(format!("Slot {n} Mix"), 1.0),
            mix_lo: percent(format!("Slot {n} Mix Low"), 1.0),
            mix_mid: percent(format!("Slot {n} Mix Mid"), 1.0),
            mix_hi: percent(format!("Slot {n} Mix High"), 1.0),
            k0: knob_param(slot, 0, kinds),
            k1: knob_param(slot, 1, kinds),
            k2: knob_param(slot, 2, kinds),
            k3: knob_param(slot, 3, kinds),
            k4: knob_param(slot, 4, kinds),
            k5: knob_param(slot, 5, kinds),
            k6: knob_param(slot, 6, kinds),
            k7: knob_param(slot, 7, kinds),
            k8: knob_param(slot, 8, kinds),
            k9: knob_param(slot, 9, kinds),
        }
    }

    pub fn knob(&self, i: usize) -> &FloatParam {
        match i {
            0 => &self.k0,
            1 => &self.k1,
            2 => &self.k2,
            3 => &self.k3,
            4 => &self.k4,
            5 => &self.k5,
            6 => &self.k6,
            7 => &self.k7,
            8 => &self.k8,
            _ => &self.k9,
        }
    }

    pub fn tile(&self, lane: usize) -> &FloatParam {
        if lane == 0 {
            &self.tile_a
        } else {
            &self.tile_b
        }
    }

    pub fn band_mix(&self, band: usize) -> &FloatParam {
        match band {
            0 => &self.mix_lo,
            1 => &self.mix_mid,
            _ => &self.mix_hi,
        }
    }
}

#[derive(Params)]
pub struct WaveCubeParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    #[persist = "state2"]
    pub state: Arc<RwLock<State>>,

    #[id = "bypass"]
    pub bypass: BoolParam,
    #[id = "ingain"]
    pub in_gain: FloatParam,
    #[id = "outgain"]
    pub out_gain: FloatParam,
    #[id = "gmix"]
    pub mix: FloatParam,
    #[id = "atrig"]
    pub trig_db: FloatParam,
    #[id = "midisw"]
    pub midi_switch: BoolParam,
    #[id = "safe"]
    pub safe_clip: BoolParam,

    #[nested(array, group = "Slot")]
    pub slots: [SlotParams; N_SLOTS],

    /// not a parameter: the kinds the knob displays use
    pub kinds: SlotKinds,
}

impl Default for WaveCubeParams {
    fn default() -> Self {
        let kinds: SlotKinds = Arc::new(std::array::from_fn(|_| AtomicU8::new(0)));
        Self {
            editor_state: EguiState::from_size(editor::BASE_W as u32, editor::BASE_H as u32),
            state: Arc::new(RwLock::new(State::default())),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
            in_gain: db("Input", 0.0, -24.0, 24.0),
            out_gain: db("Output", 0.0, -24.0, 24.0),
            mix: percent("Master Mix", 1.0),
            trig_db: db("Audio Trigger Threshold", -24.0, -60.0, 0.0),
            midi_switch: BoolParam::new("MIDI Switch", false),
            safe_clip: BoolParam::new("Safety Clipper", true),
            slots: std::array::from_fn(|i| SlotParams::new(i, &kinds)),
            kinds,
        }
    }
}

impl WaveCubeParams {
    /// Keeps the knob displays in step with the slot kinds.
    pub fn sync_kinds(&self, st: &State) {
        for (i, s) in st.slots.iter().enumerate() {
            self.kinds[i].store(s.kind.index() as u8, Ordering::Relaxed);
        }
    }
}

// ------------------------------------------------------------------------------------------
// Shared with the GUI
// ------------------------------------------------------------------------------------------

/// Bins of the scope behind the editor (one cycle of the shown lane).
pub const SCOPE_N: usize = 256;
/// Samples of the vectorscope / tool scope ring.
pub const RING_N: usize = 2048;

pub struct Shared {
    /// bumped by the GUI after every edit of the shapes / custom waves
    pub wave_version: AtomicU32,
    /// bumped after every change of the other state (settings, chain)
    pub set_version: AtomicU32,
    /// slot / lane / band the editor shows (the scope follows that LFO)
    pub view: AtomicU32,
    pub view_lane: AtomicU32,
    pub view_band: AtomicU32,
    pub phase: [[AtomicF32; N_LANES]; N_SLOTS],
    pub value: [[[AtomicF32; N_BANDS]; N_LANES]; N_SLOTS],
    pub env: [[AtomicF32; N_LANES]; N_SLOTS],
    pub gr: [AtomicF32; N_SLOTS],
    pub solo: [AtomicU32; N_SLOTS],
    /// envelope "solo sidechain" listen: slot + 1 (0 = off)
    pub env_listen: AtomicU32,
    /// MIDI Switch: (counter << 8) | custom slot, bumped by the audio thread
    pub switched: AtomicU32,
    pub scope_in: Box<[AtomicF32]>,
    pub scope_out: Box<[AtomicF32]>,
    /// output of the viewed slot (L, R): vectorscope and the Scope tool
    pub ring: Mutex<(Vec<[f32; 2]>, usize)>,
    pub in_pk: AtomicF32,
    pub out_pk: AtomicF32,
    pub tempo: AtomicF32,
    /// loaded user noises / IRs (kept alive here so the audio thread never frees them)
    pub noises: Mutex<Vec<Arc<dsp::media::Sample>>>,
    pub irs: Mutex<Vec<Arc<dsp::media::Sample>>>,
    pub media_version: AtomicU32,
    /// big buffers (Time, Reverb) made on the GUI thread for the audio thread, and the old ones
    /// handed back to be freed there
    pub heavy_in: Mutex<Vec<(usize, Box<dsp::Heavy>)>>,
    pub heavy_out: Mutex<Vec<Box<dsp::Heavy>>>,
    pub sr: AtomicF32,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            wave_version: AtomicU32::new(1),
            set_version: AtomicU32::new(1),
            view: AtomicU32::new(0),
            view_lane: AtomicU32::new(0),
            view_band: AtomicU32::new(0),
            phase: std::array::from_fn(|_| std::array::from_fn(|_| AtomicF32::new(0.0))),
            value: std::array::from_fn(|_| std::array::from_fn(|_| std::array::from_fn(|_| AtomicF32::new(1.0)))),
            env: std::array::from_fn(|_| std::array::from_fn(|_| AtomicF32::new(0.0))),
            gr: std::array::from_fn(|_| AtomicF32::new(0.0)),
            solo: std::array::from_fn(|_| AtomicU32::new(0)),
            env_listen: AtomicU32::new(0),
            switched: AtomicU32::new(0),
            scope_in: (0..SCOPE_N).map(|_| AtomicF32::new(0.0)).collect(),
            scope_out: (0..SCOPE_N).map(|_| AtomicF32::new(0.0)).collect(),
            ring: Mutex::new((vec![[0.0; 2]; RING_N], 0)),
            in_pk: AtomicF32::new(0.0),
            out_pk: AtomicF32::new(0.0),
            tempo: AtomicF32::new(120.0),
            noises: Mutex::new(Vec::new()),
            irs: Mutex::new(Vec::new()),
            media_version: AtomicU32::new(0),
            heavy_in: Mutex::new(Vec::with_capacity(64)),
            heavy_out: Mutex::new(Vec::with_capacity(64)),
            sr: AtomicF32::new(48000.0),
        }
    }
}

impl Shared {
    pub fn bump_waves(&self) {
        self.wave_version.fetch_add(1, Ordering::AcqRel);
    }
    pub fn bump_settings(&self) {
        self.set_version.fetch_add(1, Ordering::AcqRel);
    }
}

// ------------------------------------------------------------------------------------------
// Plugin
// ------------------------------------------------------------------------------------------

pub struct WaveCube {
    params: Arc<WaveCubeParams>,
    shared: Arc<Shared>,
    engine: dsp::Engine,
}

impl Default for WaveCube {
    fn default() -> Self {
        Self { params: Arc::new(WaveCubeParams::default()), shared: Arc::new(Shared::default()), engine: dsp::Engine::new(48000.0) }
    }
}

impl Plugin for WaveCube {
    const NAME: &'static str = "WaveCube";
    const VENDOR: &'static str = "WaveCube";
    const URL: &'static str = "https://github.com/xslimakx/WaveCube";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            aux_input_ports: &[new_nonzero_u32(2)],
            names: PortNames { layout: Some("Stereo + sidechain"), main_input: None, main_output: None, aux_inputs: &["Sidechain"], aux_outputs: &[] },
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() },
        AudioIOLayout { main_input_channels: NonZeroU32::new(1), main_output_channels: NonZeroU32::new(1), ..AudioIOLayout::const_default() },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        static HOOK: std::sync::Once = std::sync::Once::new();
        HOOK.call_once(|| {
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                baseview::nylog(&format!("PANIC: {info}\n{}", std::backtrace::Backtrace::force_capture()));
                prev(info);
            }));
        });
        editor::create(self.params.clone(), self.shared.clone())
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _context: &mut impl InitContext<Self>) -> bool {
        // also called after a state was loaded: repair it and make the audio thread re-read it
        {
            let mut st = self.params.state.write();
            st.sanitize();
            self.params.sync_kinds(&st);
            dsp::media::load_user_media(&st, &self.shared);
        }
        self.shared.sr.store(config.sample_rate, Ordering::Relaxed);
        self.engine = dsp::Engine::new(config.sample_rate);
        {
            // big buffers for the current chain, made here (not on the audio thread)
            let st = self.params.state.read();
            self.engine.prepare(&st, &self.shared);
        }
        self.shared.bump_waves();
        self.shared.bump_settings();
        self.shared.media_version.fetch_add(1, Ordering::AcqRel);
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        demo_input(buffer);
        let t = context.transport();
        let transport = dsp::Transport { tempo: t.tempo.unwrap_or(120.0), pos_beats: t.pos_beats(), playing: t.playing };

        // snapshots of the state (never waits: if the GUI holds the lock, next block)
        self.engine.sync(&self.params, &self.shared);

        let mut next_event = context.next_event();
        let ctl = dsp::Controls::from_params(&self.params);
        self.engine.begin_block(&ctl, &transport, &self.shared);
        let midi_ch = self.engine.midi_channel();
        let sc_buf = aux.inputs.first().map(|b| b.as_slice_immutable());
        for (i, mut frame) in buffer.iter_samples().enumerate() {
            let sc = match sc_buf {
                Some(ch) if !ch.is_empty() => {
                    let l = ch[0].get(i).copied().unwrap_or(0.0);
                    let r = ch.get(1).and_then(|c| c.get(i)).copied().unwrap_or(l);
                    [l, r]
                }
                _ => [0.0; 2],
            };
            let mut note: Option<u8> = None;
            while let Some(ev) = next_event {
                if ev.timing() as usize > i {
                    break;
                }
                if let NoteEvent::NoteOn { note: n, velocity, channel, .. } = ev {
                    if velocity > 0.0 && (midi_ch == 0 || channel + 1 == midi_ch) {
                        note = Some(n);
                    }
                }
                next_event = context.next_event();
            }
            let nch = frame.len().min(2);
            let mut x = [0.0f32; 2];
            for (c, s) in frame.iter_mut().enumerate().take(nch) {
                x[c] = *s;
            }
            if nch == 1 {
                x[1] = x[0];
            }
            let y = self.engine.process(x, sc, nch == 2, note, &self.shared);
            for (c, s) in frame.iter_mut().enumerate().take(nch) {
                *s = y[c];
            }
        }
        self.engine.end_block(&self.shared);
        ProcessStatus::Normal
    }
}

impl Vst3Plugin for WaveCube {
    const VST3_CLASS_ID: [u8; 16] = *b"WaveCubeShaperFx";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation, Vst3SubCategory::Filter];
}

nih_export_vst3!(WaveCube);

/// Developer helper for screenshots: with `WAVECUBE_DEMO=1` the standalone build plays a little
/// drum and chord loop at 120 BPM instead of using the input.
fn demo_input(buffer: &mut Buffer) {
    use std::sync::Mutex;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("WAVECUBE_DEMO").is_ok()) {
        return;
    }
    static ST: Mutex<(u64, u32)> = Mutex::new((0, 12345));
    let Ok(mut g) = ST.try_lock() else { return };
    let fs = 48000.0f64;
    let beat = fs * 0.5;
    for mut frame in buffer.iter_samples() {
        let n = g.0;
        g.0 += 1;
        let t = n as f64 / fs;
        let in_beat = (n as f64 % beat) / fs;
        let in_8th = (n as f64 % (beat / 2.0)) / fs;
        let kf = 50.0 + 120.0 * (-in_beat * 30.0).exp();
        let kick = (2.0 * std::f64::consts::PI * kf * in_beat).sin() * (-in_beat * 7.0).exp() * 0.8;
        g.1 ^= g.1 << 13;
        g.1 ^= g.1 >> 17;
        g.1 ^= g.1 << 5;
        let noise = (g.1 as f64 / u32::MAX as f64) * 2.0 - 1.0;
        let off = ((n as f64 / (beat / 2.0)) as u64) % 2 == 1;
        let hat = if off { noise * (-in_8th * 60.0).exp() * 0.18 } else { 0.0 };
        let chord = [220.0, 261.63, 329.63, 392.0];
        let mut pad = 0.0;
        for (k, f) in chord.iter().enumerate() {
            pad += ((2.0 * std::f64::consts::PI * f * t + k as f64).sin() + 0.3 * (4.0 * std::f64::consts::PI * f * t).sin()) * 0.06;
        }
        let l = (kick + hat * 0.8 + pad) as f32;
        let r = (kick + hat * 1.2 + pad * 0.95) as f32;
        for (c, s) in frame.iter_mut().enumerate() {
            *s = if c == 0 { l } else { r };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrip_and_repair() {
        let st = State::default();
        let s = serde_json::to_string(&st).unwrap();
        let mut back: State = serde_json::from_str(&s).unwrap();
        back.sanitize();
        assert_eq!(back, st);
        let mut bad: State = serde_json::from_str(r#"{"chain":[3,3,40,0],"selected":9,"grid":7}"#).unwrap();
        bad.sanitize();
        assert_eq!(bad.chain, vec![3, 0]);
        assert_eq!(bad.slots.len(), N_SLOTS);
        assert!(bad.waves.iter().all(|w| w.len() == N_LANES && w.iter().all(|b| b.len() == N_BANDS)));
        assert_eq!(bad.selected, 3);
        assert_eq!(bad.grid, 16);
    }

    #[test]
    fn duplicates_are_fine() {
        let mut st = State::empty();
        for _ in 0..3 {
            let s = st.free_slot().unwrap();
            st.put(s, Kind::Drive);
            st.chain.push(s as u8);
        }
        st.sanitize();
        assert_eq!(st.chain.len(), 3);
        assert!(st.chain.iter().all(|&s| st.slots[s as usize].kind == Kind::Drive));
    }

    #[test]
    fn knob_params_show_the_slot_kind() {
        let p = WaveCubeParams::default();
        p.kinds[2].store(Kind::Filter.index() as u8, Ordering::Relaxed);
        let s = p.slots[2].k0.normalized_value_to_string(0.75, true);
        assert!(s.contains("st"), "{s}");
        p.kinds[2].store(Kind::Compressor.index() as u8, Ordering::Relaxed);
        let s = p.slots[2].k1.normalized_value_to_string(0.0, true);
        assert!(s.contains(":1"), "{s}");
    }

    #[test]
    fn param_ids_are_unique() {
        let p = WaveCubeParams::default();
        let map = p.param_map();
        let mut ids: Vec<&str> = map.iter().map(|(id, _, _)| id.as_str()).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert!(ids.contains(&"k9_12") && ids.contains(&"ta_1"));
    }

    #[test]
    fn knob_defs_roundtrip() {
        for k in Kind::ALL {
            assert!(k.knobs().len() <= N_KNOBS);
            assert!(k.n_lanes() <= N_LANES);
            for d in k.knobs() {
                for n in [0.0, 0.3, 0.77, 1.0] {
                    assert!((d.norm(d.plain(n)) - n).abs() < 1e-3, "{} {}", d.name, n);
                }
                assert!((d.plain(d.default_norm()) - d.def).abs() < 1e-2 * (d.max - d.min).abs().max(1.0), "{}", d.name);
            }
        }
    }
}
