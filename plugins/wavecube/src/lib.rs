//! WaveCube - multi-shaper LFO effect (VST3).
//!
//! Eight shapers in a chain you can reorder: Volume, Filter, Pan, Width, Time, Noise, Crush and
//! Drive. Every shaper is driven by its own drawn shape (points with curves and steps) and, for
//! most of them, an envelope follower; Volume has a compressor. Shapes run in time with the
//! song, free in Hz, at the pitch of MIDI notes, and can be restarted by MIDI notes or by hits
//! of the audio. Every shaper can split the signal into three bands with settings of their own.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub mod dsp;
pub mod presets;
pub mod wave;
mod editor;

use wave::{Preset, Pt, Wave};

pub const N_SHAPERS: usize = 8;
pub const N_BANDS: usize = 3;
pub const N_SLOTS: usize = 9;

// ------------------------------------------------------------------------------------------
// Shaper kinds
// ------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Volume,
    Filter,
    Pan,
    Width,
    Time,
    Noise,
    Crush,
    Drive,
}

impl Kind {
    pub const ALL: [Kind; N_SHAPERS] = [Kind::Volume, Kind::Filter, Kind::Pan, Kind::Width, Kind::Time, Kind::Noise, Kind::Crush, Kind::Drive];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Volume => "Volume",
            Kind::Filter => "Filter",
            Kind::Pan => "Pan",
            Kind::Width => "Width",
            Kind::Time => "Time",
            Kind::Noise => "Noise",
            Kind::Crush => "Crush",
            Kind::Drive => "Drive",
        }
    }

    pub fn index(self) -> usize {
        Kind::ALL.iter().position(|&k| k == self).unwrap_or(0)
    }

    /// What the top and the bottom of the editor mean.
    pub fn axis(self) -> (&'static str, &'static str) {
        match self {
            Kind::Volume => ("FULL", "SILENT"),
            Kind::Filter => ("OPEN", "CLOSED"),
            Kind::Pan => ("LEFT", "RIGHT"),
            Kind::Width => ("WIDE", "MONO"),
            Kind::Time => ("NOW", "LATE"),
            Kind::Noise => ("LOUD", "OFF"),
            Kind::Crush => ("CRUSHED", "CLEAN"),
            Kind::Drive => ("DRIVEN", "CLEAN"),
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Kind::Volume => "The shape is the level: sidechain pumping, gates, tremolo. Plus a compressor.",
            Kind::Filter => "The shape moves the cutoff between Low and High.",
            Kind::Pan => "The shape moves the sound between left (top) and right (bottom).",
            Kind::Width => "Top: twice as wide, middle: as is, bottom: mono.",
            Kind::Time => "Lower = later. Slopes bend the pitch: tape stops, half-time, scratches.",
            Kind::Noise => "Adds noise shaped by the line: risers, hats, air, vinyl.",
            Kind::Crush => "The shape blends in the crush chain: crunch, bits, resample.",
            Kind::Drive => "Saturation follows the shape: from clean to driven.",
        }
    }

    pub fn has_envelope(self) -> bool {
        !matches!(self, Kind::Volume | Kind::Time)
    }

    pub fn default_wave(self) -> Wave {
        match self {
            Kind::Volume => Wave::preset(Preset::Pump),
            Kind::Filter => Wave::from(&[Pt::new(0.0, 0.25, -0.4), Pt::new(0.5, 1.0, 0.4), Pt::new(1.0, 0.25, 0.0)]),
            Kind::Pan => Wave::preset(Preset::Sine),
            Kind::Width => Wave::from(&[Pt::new(0.0, 0.5, 0.0), Pt::new(0.5, 1.0, 0.0), Pt::new(1.0, 0.5, 0.0)]),
            Kind::Time => Wave::preset(Preset::TapeStop),
            Kind::Noise => Wave::from(&[Pt::new(0.0, 0.0, 0.5), Pt::new(1.0, 1.0, 0.0)]),
            Kind::Crush => Wave::preset(Preset::Square),
            Kind::Drive => Wave::from(&[Pt::new(0.0, 1.0, 0.45), Pt::new(1.0, 0.0, 0.0)]),
        }
    }

    pub fn default_rate(self) -> Rate {
        match self {
            Kind::Volume => Rate::Quarter,
            Kind::Filter | Kind::Noise => Rate::Bar1,
            Kind::Pan | Kind::Width => Rate::Half,
            Kind::Time => Rate::Bar1,
            Kind::Crush => Rate::Eighth,
            Kind::Drive => Rate::Quarter,
        }
    }
}

// ------------------------------------------------------------------------------------------
// Enums shown in the host / GUI
// ------------------------------------------------------------------------------------------

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum Mode {
    /// tempo and song position
    #[name = "Beat"]
    Beat,
    /// free running, in Hz
    #[name = "Hz"]
    Hz,
    /// speed = frequency of the last MIDI note
    #[name = "Pitch"]
    Pitch,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum Trigger {
    #[name = "Free"]
    Off,
    #[name = "MIDI"]
    Midi,
    #[name = "MIDI 1-shot"]
    MidiOnce,
    #[name = "Audio"]
    Audio,
    #[name = "Audio 1-shot"]
    AudioOnce,
}

impl Trigger {
    pub fn midi(self) -> bool {
        matches!(self, Trigger::Midi | Trigger::MidiOnce)
    }
    pub fn audio(self) -> bool {
        matches!(self, Trigger::Audio | Trigger::AudioOnce)
    }
    pub fn once(self) -> bool {
        matches!(self, Trigger::MidiOnce | Trigger::AudioOnce)
    }
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum Rate {
    #[name = "32 bars"]
    Bar32,
    #[name = "16 bars"]
    Bar16,
    #[name = "8 bars"]
    Bar8,
    #[name = "4 bars"]
    Bar4,
    #[name = "2 bars"]
    Bar2,
    #[name = "1 bar"]
    Bar1,
    #[name = "1/2"]
    Half,
    #[name = "1/4"]
    Quarter,
    #[name = "1/8"]
    Eighth,
    #[name = "1/16"]
    Sixteenth,
    #[name = "1/32"]
    ThirtySecond,
    #[name = "1/64"]
    SixtyFourth,
    #[name = "1/128"]
    OneTwentyEighth,
}

impl Rate {
    /// length of one cycle in quarter notes
    pub fn beats(self) -> f64 {
        4.0 * 2f64.powi(5 - self.to_index() as i32)
    }
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum Feel {
    #[name = "Straight"]
    Straight,
    #[name = "Dotted"]
    Dotted,
    #[name = "Triplet"]
    Triplet,
}

impl Feel {
    pub fn factor(self) -> f64 {
        match self {
            Feel::Straight => 1.0,
            Feel::Dotted => 1.5,
            Feel::Triplet => 2.0 / 3.0,
        }
    }
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum FilterType {
    #[name = "LP 6"]
    Lp6,
    #[name = "HP 6"]
    Hp6,
    #[name = "LP 12"]
    Lp12,
    #[name = "HP 12"]
    Hp12,
    #[name = "BP 12"]
    Bp12,
    #[name = "Notch 12"]
    Notch12,
    #[name = "Peak 12"]
    Peak12,
    #[name = "LP 24"]
    Lp24,
    #[name = "HP 24"]
    Hp24,
    #[name = "BP 24"]
    Bp24,
    #[name = "Notch 24"]
    Notch24,
    #[name = "Peak 24"]
    Peak24,
    #[name = "Phaser 2 notches"]
    PhN2,
    #[name = "Phaser 4 notches"]
    PhN4,
    #[name = "Phaser 6 notches"]
    PhN6,
    #[name = "Phaser 8 notches"]
    PhN8,
    #[name = "Phaser 2 peaks"]
    PhP2,
    #[name = "Phaser 4 peaks"]
    PhP4,
    #[name = "Phaser 6 peaks"]
    PhP6,
    #[name = "Phaser 8 peaks"]
    PhP8,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum DriveStyle {
    #[name = "Soft clip"]
    SoftClip,
    #[name = "Hard clip"]
    HardClip,
    #[name = "Soft square"]
    SoftSquare,
    #[name = "Hard square"]
    HardSquare,
    #[name = "Soft rectify"]
    SoftRectify,
    #[name = "Hard rectify"]
    HardRectify,
    #[name = "Skew fold"]
    SkewFold,
    #[name = "Sine fold"]
    SineFold,
    #[name = "Triangle fold"]
    TriFold,
    #[name = "Extreme fold"]
    ExtremeFold,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum NoiseType {
    #[name = "White"]
    White,
    #[name = "Pink"]
    Pink,
    #[name = "Brown"]
    Brown,
    #[name = "Vinyl crackle"]
    Crackle,
    #[name = "Digital"]
    Digital,
    #[name = "Mains hum"]
    Hum,
    #[name = "Air"]
    Air,
    #[name = "Tape hiss"]
    Tape,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum NoiseEnv {
    #[name = "Off"]
    Off,
    #[name = "Follow"]
    Follow,
    #[name = "Duck"]
    Duck,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum PanMode {
    #[name = "Balance 0 dB"]
    Bal0,
    #[name = "Balance 3 dB"]
    Bal3,
    #[name = "Balance 4.5 dB"]
    Bal45,
    #[name = "Balance 6 dB"]
    Bal6,
    #[name = "Combined"]
    Combined,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum TimeRange {
    #[name = "Cycle"]
    Cycle,
    #[name = "1 bar"]
    Bar1,
    #[name = "2 bars"]
    Bar2,
    #[name = "4 bars"]
    Bar4,
    #[name = "8 bars"]
    Bar8,
    #[name = "2.5 ms"]
    Ms2,
    #[name = "10 ms"]
    Ms10,
    #[name = "20 ms"]
    Ms20,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum EnvMode {
    #[name = "Add"]
    Add,
    #[name = "Multiply"]
    Multiply,
}

// ------------------------------------------------------------------------------------------
// Parameters
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

fn hz_str() -> Arc<dyn Fn(f32) -> String + Send + Sync> {
    Arc::new(|v| if v >= 1000.0 { format!("{:.1}k", v / 1000.0) } else if v < 10.0 { format!("{:.2}", v) } else { format!("{:.0}", v) })
}

fn parse_hz() -> Arc<dyn Fn(&str) -> Option<f32> + Send + Sync> {
    Arc::new(|s| {
        let s = s.trim().to_lowercase();
        let s = s.trim_end_matches("hz").trim();
        if let Some(k) = s.strip_suffix('k') {
            k.trim().parse::<f32>().ok().map(|v| v * 1000.0)
        } else {
            s.parse().ok()
        }
    })
}

fn one_dec() -> Arc<dyn Fn(f32) -> String + Send + Sync> {
    Arc::new(|v| format!("{:.1}", (v * 10.0).round() / 10.0 + 0.0))
}

fn parse_num(unit: &'static str) -> Arc<dyn Fn(&str) -> Option<f32> + Send + Sync> {
    Arc::new(move |s| s.trim().trim_end_matches(unit).trim().parse().ok())
}

fn percent(name: impl Into<String>, def: f32) -> FloatParam {
    FloatParam::new(name, def, lin(0.0, 1.0)).with_unit(" %").with_value_to_string(pct_str()).with_string_to_value(parse_pct())
}

fn bipercent(name: impl Into<String>, def: f32, max: f32) -> FloatParam {
    FloatParam::new(name, def, lin(-max, max)).with_unit(" %").with_value_to_string(pct_str()).with_string_to_value(parse_pct())
}

fn freq(name: impl Into<String>, def: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, def, FloatRange::Skewed { min, max, factor: FloatRange::skew_factor(-2.2) })
        .with_unit(" Hz")
        .with_value_to_string(hz_str())
        .with_string_to_value(parse_hz())
}

fn ms(name: impl Into<String>, def: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, def, FloatRange::Skewed { min, max, factor: FloatRange::skew_factor(-1.8) })
        .with_unit(" ms")
        .with_value_to_string(Arc::new(|v| if v < 10.0 { format!("{v:.2}") } else if v < 100.0 { format!("{v:.1}") } else { format!("{v:.0}") }))
        .with_string_to_value(parse_num("ms"))
}

fn db(name: impl Into<String>, def: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, def, lin(min, max))
        .with_unit(" dB")
        .with_step_size(0.1)
        .with_value_to_string(one_dec())
        .with_string_to_value(parse_num("dB"))
}

/// The controls every shaper has.
#[derive(Params)]
pub struct ShaperParams {
    #[id = "on"]
    pub on: BoolParam,
    #[id = "mode"]
    pub mode: EnumParam<Mode>,
    #[id = "trig"]
    pub trigger: EnumParam<Trigger>,
    #[id = "rate"]
    pub rate: EnumParam<Rate>,
    #[id = "feel"]
    pub feel: EnumParam<Feel>,
    #[id = "hz"]
    pub hz: FloatParam,
    #[id = "phase"]
    pub phase: FloatParam,
    #[id = "amount"]
    pub amount: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "smooth"]
    pub smooth: FloatParam,
    #[id = "multi"]
    pub multiband: BoolParam,
    #[id = "xlo"]
    pub x_lo: FloatParam,
    #[id = "xhi"]
    pub x_hi: FloatParam,
    // envelope follower
    #[id = "envon"]
    pub env_on: BoolParam,
    #[id = "envamt"]
    pub env_amt: FloatParam,
    #[id = "envatt"]
    pub env_att: FloatParam,
    #[id = "envhold"]
    pub env_hold: FloatParam,
    #[id = "envrel"]
    pub env_rel: FloatParam,
    #[id = "envthr"]
    pub env_thr: FloatParam,
    #[id = "envmode"]
    pub env_mode: EnumParam<EnvMode>,
    #[id = "envsc"]
    pub env_sc: BoolParam,
    // groove / stereo
    #[id = "swing"]
    pub swing: FloatParam,
    #[id = "spread"]
    pub spread: FloatParam,
}

impl ShaperParams {
    fn new(kind: Kind) -> Self {
        let n = kind.name();
        Self {
            on: BoolParam::new(format!("{n} On"), kind == Kind::Volume),
            mode: EnumParam::new(format!("{n} LFO Mode"), Mode::Beat),
            trigger: EnumParam::new(format!("{n} Trigger"), Trigger::Off),
            rate: EnumParam::new(format!("{n} Length"), kind.default_rate()),
            feel: EnumParam::new(format!("{n} Feel"), Feel::Straight),
            hz: FloatParam::new(format!("{n} Speed"), 1.0, FloatRange::Skewed { min: 0.02, max: 5000.0, factor: FloatRange::skew_factor(-2.8) })
                .with_unit(" Hz")
                .with_value_to_string(hz_str())
                .with_string_to_value(parse_hz()),
            phase: FloatParam::new(format!("{n} Phase"), 0.0, lin(0.0, 360.0))
                .with_unit("°")
                .with_step_size(1.0)
                .with_value_to_string(Arc::new(|v| format!("{:.0}", v)))
                .with_string_to_value(parse_num("°")),
            amount: percent(format!("{n} Amount"), 1.0),
            mix: percent(format!("{n} Mix"), 1.0),
            smooth: ms(format!("{n} Smooth"), if kind == Kind::Time { 4.0 } else { 1.5 }, 0.0, 100.0),
            multiband: BoolParam::new(format!("{n} Multiband"), false),
            x_lo: freq(format!("{n} Low / Mid"), 200.0, 20.0, 20000.0),
            x_hi: freq(format!("{n} Mid / High"), 2500.0, 20.0, 20000.0),
            env_on: BoolParam::new(format!("{n} Envelope On"), false),
            env_amt: bipercent(format!("{n} Envelope Amount"), 1.0, 2.0),
            env_att: ms(format!("{n} Envelope Attack"), 5.0, 0.1, 500.0),
            env_hold: ms(format!("{n} Envelope Hold"), 0.0, 0.0, 500.0),
            env_rel: ms(format!("{n} Envelope Release"), 150.0, 5.0, 2000.0),
            env_thr: db(format!("{n} Envelope Threshold"), -40.0, -60.0, 0.0),
            env_mode: EnumParam::new(format!("{n} Envelope Mode"), EnvMode::Add),
            env_sc: BoolParam::new(format!("{n} Envelope Sidechain"), false),
            swing: percent(format!("{n} Swing"), 0.0),
            spread: FloatParam::new(format!("{n} Stereo Phase"), 0.0, lin(0.0, 180.0))
                .with_unit("°")
                .with_step_size(1.0)
                .with_value_to_string(Arc::new(|v| format!("{:.0}", v)))
                .with_string_to_value(parse_num("°")),
        }
    }
}

/// A user wave slot: a shape and where it came from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Slot {
    pub shaper: u8,
    pub band: u8,
    pub wave: Wave,
}

/// Shapes, chain order, user waves and what the editor shows: saved with the project / preset.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct State {
    /// waves[shaper][band]; band 0 is also the full-band shape
    pub waves: Vec<Vec<Wave>>,
    /// chain order: indices into Kind::ALL
    pub order: Vec<u8>,
    /// shapers in the chain (the others are removed: not shown, not processed)
    #[serde(default = "all_present")]
    pub present: Vec<bool>,
    #[serde(default)]
    pub slots: Vec<Option<Slot>>,
    #[serde(default)]
    pub preset: String,
    pub selected: u8,
    pub band: u8,
    pub grid: u8,
}

fn all_present() -> Vec<bool> {
    vec![true; N_SHAPERS]
}

impl Default for State {
    fn default() -> Self {
        Self {
            waves: Kind::ALL.iter().map(|k| vec![k.default_wave(); N_BANDS]).collect(),
            order: (0..N_SHAPERS as u8).collect(),
            present: (0..N_SHAPERS).map(|i| i == 0).collect(),
            slots: vec![None; N_SLOTS],
            preset: "Init".into(),
            selected: 0,
            band: 0,
            grid: 16,
        }
    }
}

pub const GRIDS: [u8; 7] = [4, 8, 12, 16, 24, 32, 64];

impl State {
    /// Repairs a state from an older / damaged save.
    pub fn sanitize(&mut self) {
        let d = State::default();
        self.waves.resize(N_SHAPERS, Vec::new());
        for (s, w) in self.waves.iter_mut().enumerate() {
            w.resize(N_BANDS, d.waves[s][0].clone());
            for b in w.iter_mut() {
                b.sanitize();
            }
        }
        let mut seen = [false; N_SHAPERS];
        self.order.retain(|&i| (i as usize) < N_SHAPERS && !std::mem::replace(&mut seen[i as usize], true));
        for i in 0..N_SHAPERS {
            if !seen[i] {
                self.order.push(i as u8);
            }
        }
        self.present.resize(N_SHAPERS, true);
        self.slots.resize(N_SLOTS, None);
        for s in self.slots.iter_mut().flatten() {
            s.shaper = s.shaper.min(N_SHAPERS as u8 - 1);
            s.band = s.band.min(N_BANDS as u8 - 1);
            s.wave.sanitize();
        }
        self.selected = self.selected.min(N_SHAPERS as u8 - 1);
        if !self.present[self.selected as usize] {
            if let Some(&first) = self.order.iter().find(|&&i| self.present[i as usize]) {
                self.selected = first;
            }
        }
        self.band = self.band.min(N_BANDS as u8 - 1);
        if !GRIDS.contains(&self.grid) {
            self.grid = 16;
        }
    }
}

#[derive(Params)]
pub struct WaveCubeParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    #[persist = "shapes"]
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

    #[nested(array, group = "Shaper")]
    pub sh: [ShaperParams; N_SHAPERS],

    // ---- Volume
    #[id = "vtrim"]
    pub v_trim: FloatParam,
    #[id = "vcomp"]
    pub v_comp: BoolParam,
    #[id = "vthr"]
    pub v_thr: FloatParam,
    #[id = "vratio"]
    pub v_ratio: FloatParam,
    #[id = "vatt"]
    pub v_att: FloatParam,
    #[id = "vhold"]
    pub v_hold: FloatParam,
    #[id = "vrel"]
    pub v_rel: FloatParam,
    #[id = "vmakeup"]
    pub v_makeup: FloatParam,
    #[id = "vauto"]
    pub v_auto: BoolParam,
    #[id = "vsc"]
    pub v_sc: BoolParam,
    // ---- Filter
    #[id = "ftype"]
    pub f_type: EnumParam<FilterType>,
    #[id = "flo"]
    pub f_lo: FloatParam,
    #[id = "fhi"]
    pub f_hi: FloatParam,
    #[id = "fres"]
    pub f_res: FloatParam,
    #[id = "fstereo"]
    pub f_stereo: FloatParam,
    // ---- Pan
    #[id = "pmode"]
    pub p_mode: EnumParam<PanMode>,
    #[id = "phaas"]
    pub p_haas: FloatParam,
    #[id = "phaasms"]
    pub p_haas_ms: FloatParam,
    // ---- Width
    #[id = "wgain"]
    pub w_gain: FloatParam,
    // ---- Time
    #[id = "trange"]
    pub t_range: EnumParam<TimeRange>,
    // ---- Noise
    #[id = "ntype"]
    pub n_type: EnumParam<NoiseType>,
    #[id = "nhp"]
    pub n_hp: FloatParam,
    #[id = "nlp"]
    pub n_lp: FloatParam,
    #[id = "nstereo"]
    pub n_stereo: FloatParam,
    #[id = "nonly"]
    pub n_only: BoolParam,
    #[id = "nenv"]
    pub n_env: EnumParam<NoiseEnv>,
    #[id = "nenvamt"]
    pub n_env_amt: FloatParam,
    // ---- Crush
    #[id = "ccrunch"]
    pub c_crunch: FloatParam,
    #[id = "cpush"]
    pub c_push: FloatParam,
    #[id = "cbits"]
    pub c_bits: FloatParam,
    #[id = "cdither"]
    pub c_dither: BoolParam,
    #[id = "crate"]
    pub c_rate: FloatParam,
    #[id = "cjitter"]
    pub c_jitter: FloatParam,
    #[id = "caa"]
    pub c_aa: BoolParam,
    #[id = "clp"]
    pub c_lp: FloatParam,
    #[id = "cgain"]
    pub c_gain: FloatParam,
    // ---- Drive
    #[id = "dstyle"]
    pub d_style: EnumParam<DriveStyle>,
    #[id = "din"]
    pub d_in: FloatParam,
    #[id = "dout"]
    pub d_out: FloatParam,
    #[id = "dpush"]
    pub d_push: FloatParam,
    #[id = "dgrip"]
    pub d_grip: FloatParam,
    #[id = "daccent"]
    pub d_accent: FloatParam,
    #[id = "dtone"]
    pub d_tone: FloatParam,
}

impl Default for WaveCubeParams {
    fn default() -> Self {
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
            sh: Kind::ALL.map(ShaperParams::new),

            v_trim: db("Volume Trim", 0.0, -24.0, 24.0),
            v_comp: BoolParam::new("Compressor On", false),
            v_thr: db("Compressor Threshold", -18.0, -60.0, 0.0),
            v_ratio: FloatParam::new("Compressor Ratio", 2.0, FloatRange::Skewed { min: 1.0, max: 100.0, factor: FloatRange::skew_factor(-2.0) })
                .with_value_to_string(Arc::new(|v| if v >= 99.5 { "∞:1".into() } else { format!("{v:.1}:1") }))
                .with_string_to_value(Arc::new(|s| s.trim().trim_end_matches(":1").trim().parse().ok())),
            v_att: ms("Compressor Attack", 10.0, 0.01, 200.0),
            v_hold: ms("Compressor Hold", 0.0, 0.0, 500.0),
            v_rel: ms("Compressor Release", 150.0, 50.0, 2000.0),
            v_makeup: db("Compressor Makeup", 0.0, -12.0, 24.0),
            v_auto: BoolParam::new("Compressor Auto Makeup", true),
            v_sc: BoolParam::new("Compressor Sidechain", false),

            f_type: EnumParam::new("Filter Type", FilterType::Lp12),
            f_lo: freq("Filter Low", 250.0, 20.0, 21000.0),
            f_hi: freq("Filter High", 18000.0, 20.0, 21000.0),
            f_res: percent("Filter Resonance", 0.25),
            f_stereo: FloatParam::new("Filter Stereo", 0.0, lin(-60.0, 60.0))
                .with_unit(" st")
                .with_step_size(0.1)
                .with_value_to_string(one_dec())
                .with_string_to_value(parse_num("st")),

            p_mode: EnumParam::new("Pan Mode", PanMode::Bal3),
            p_haas: percent("Pan / Haas", 0.0),
            p_haas_ms: ms("Haas Range", 2.0, 0.0, 40.0),

            w_gain: db("Width Gain", 0.0, -12.0, 12.0),

            t_range: EnumParam::new("Time Range", TimeRange::Cycle),

            n_type: EnumParam::new("Noise Type", NoiseType::White),
            n_hp: freq("Noise High-pass", 120.0, 20.0, 20000.0),
            n_lp: freq("Noise Low-pass", 16000.0, 20.0, 20000.0),
            n_stereo: percent("Noise Stereo", 1.0),
            n_only: BoolParam::new("Noise Only", false),
            n_env: EnumParam::new("Noise Envelope", NoiseEnv::Off),
            n_env_amt: percent("Noise Envelope Amount", 1.0),

            c_crunch: db("Crush Crunch", 0.0, 0.0, 27.0),
            c_push: bipercent("Crush Push", 0.0, 1.0),
            c_bits: FloatParam::new("Crush Bits", 6.0, lin(1.0, 16.0))
                .with_step_size(0.1)
                .with_value_to_string(Arc::new(|v| if v > 15.95 { "Max".into() } else { format!("{v:.1}") }))
                .with_string_to_value(parse_num("bit")),
            c_dither: BoolParam::new("Crush Dither", false),
            c_rate: freq("Crush Resample", 6000.0, 8.82, 44100.0),
            c_jitter: percent("Crush Jitter", 0.0),
            c_aa: BoolParam::new("Crush Anti-alias", true),
            c_lp: freq("Crush Low-pass", 20000.0, 200.0, 20000.0),
            c_gain: db("Crush Gain", 0.0, -24.0, 24.0),

            d_style: EnumParam::new("Drive Style", DriveStyle::SoftClip),
            d_in: db("Drive Input", 0.0, -12.0, 36.0),
            d_out: db("Drive Output", 0.0, -36.0, 12.0),
            d_push: bipercent("Drive Push", 0.0, 1.0),
            d_grip: percent("Drive Grip", 0.0),
            d_accent: percent("Drive Accent", 0.0),
            d_tone: bipercent("Drive Tone", 0.0, 1.0),
        }
    }
}

// ------------------------------------------------------------------------------------------
// Shared with the GUI
// ------------------------------------------------------------------------------------------

/// Bins of the scope behind the editor (one cycle of the shown shaper).
pub const SCOPE_N: usize = 256;

pub struct Shared {
    /// bumped by the GUI after every edit of the shapes / order
    pub version: AtomicU32,
    /// shaper the editor shows (the scope follows its cycle)
    pub view: AtomicU32,
    pub phase: [AtomicF32; N_SHAPERS],
    /// current (smoothed, with the envelope) value of every band of every shaper
    pub value: [[AtomicF32; N_BANDS]; N_SHAPERS],
    /// envelope follower (0..1) / compressor gain reduction (dB) per shaper
    pub env: [AtomicF32; N_SHAPERS],
    pub gr: AtomicF32,
    /// solo mask of the bands, per shaper
    pub solo: [AtomicU32; N_SHAPERS],
    /// MIDI Switch: (counter << 8) | slot, bumped by the audio thread
    pub switched: AtomicU32,
    pub scope_in: Box<[AtomicF32]>,
    pub scope_out: Box<[AtomicF32]>,
    pub in_pk: AtomicF32,
    pub out_pk: AtomicF32,
    pub tempo: AtomicF32,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            version: AtomicU32::new(1),
            view: AtomicU32::new(0),
            phase: std::array::from_fn(|_| AtomicF32::new(0.0)),
            value: std::array::from_fn(|_| std::array::from_fn(|_| AtomicF32::new(1.0))),
            env: std::array::from_fn(|_| AtomicF32::new(0.0)),
            gr: AtomicF32::new(0.0),
            solo: std::array::from_fn(|_| AtomicU32::new(0)),
            switched: AtomicU32::new(0),
            scope_in: (0..SCOPE_N).map(|_| AtomicF32::new(0.0)).collect(),
            scope_out: (0..SCOPE_N).map(|_| AtomicF32::new(0.0)).collect(),
            in_pk: AtomicF32::new(0.0),
            out_pk: AtomicF32::new(0.0),
            tempo: AtomicF32::new(120.0),
        }
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
        self.params.state.write().sanitize();
        self.engine = dsp::Engine::new(config.sample_rate);
        self.shared.version.fetch_add(1, Ordering::AcqRel);
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        demo_input(buffer);
        let t = context.transport();
        let transport = dsp::Transport { tempo: t.tempo.unwrap_or(120.0), pos_beats: t.pos_beats(), playing: t.playing };
        let ep = dsp::Settings::from_params(&self.params);

        // re-read the shapes after an edit (never waits: if the GUI holds the lock, next block)
        let v = self.shared.version.load(Ordering::Acquire);
        if v != self.engine.baked_version {
            if let Some(st) = self.params.state.try_read() {
                self.engine.bake(&st);
                self.engine.baked_version = v;
            }
        }

        let mut next_event = context.next_event();
        self.engine.begin_block(&ep, &transport, &self.shared);
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
                if let NoteEvent::NoteOn { note: n, velocity, .. } = ev {
                    if velocity > 0.0 {
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
            let y = self.engine.process(x, sc, nch == 2, note, &ep, &self.shared);
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
        let mut bad = State { waves: vec![], order: vec![3, 3, 9, 0], present: vec![], slots: vec![], preset: String::new(), selected: 40, band: 9, grid: 7 };
        bad.sanitize();
        assert_eq!(bad.waves.len(), N_SHAPERS);
        assert!(bad.waves.iter().all(|w| w.len() == N_BANDS));
        assert_eq!(bad.slots.len(), N_SLOTS);
        let mut o = bad.order.clone();
        o.sort();
        assert_eq!(o, (0..N_SHAPERS as u8).collect::<Vec<_>>());
        assert_eq!(bad.order[0], 3);
        assert_eq!(bad.grid, 16);
    }

    #[test]
    fn rates_double() {
        assert_eq!(Rate::Bar1.beats(), 4.0);
        assert_eq!(Rate::Quarter.beats(), 1.0);
        assert_eq!(Rate::Bar32.beats(), 128.0);
        assert_eq!(Rate::OneTwentyEighth.beats(), 1.0 / 32.0);
    }

    #[test]
    fn param_ids_are_unique_and_stable() {
        let p = WaveCubeParams::default();
        let map = p.param_map();
        let mut ids: Vec<&str> = map.iter().map(|(id, _, _)| id.as_str()).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert!(ids.contains(&"amount_1") && ids.contains(&"envthr_8"));
    }

    #[test]
    fn typed_values_parse() {
        let p = WaveCubeParams::default();
        let n = p.f_lo.string_to_normalized_value("1.5k").unwrap();
        assert!((p.f_lo.preview_plain(n) - 1500.0).abs() < 1.0);
        let n = p.sh[0].amount.string_to_normalized_value("50%").unwrap();
        assert!((p.sh[0].amount.preview_plain(n) - 0.5).abs() < 1e-4);
        let n = p.v_ratio.string_to_normalized_value("4:1").unwrap();
        assert!((p.v_ratio.preview_plain(n) - 4.0).abs() < 0.01);
    }
}
