//! What every chain item is: its modulated parameters (lanes), its knobs (mapped onto the
//! generic automatable knobs of a slot), its switches, default shapes and shape categories.

use crate::wave::{Preset as W, Pt, Wave};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default, Hash)]
pub enum Kind {
    #[default]
    Volume,
    Filter,
    Pan,
    Width,
    Time,
    Noise,
    Crush,
    Drive,
    Reverb,
    Liquid,
    Compressor,
    Scope,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Volume,
        Kind::Filter,
        Kind::Pan,
        Kind::Width,
        Kind::Time,
        Kind::Noise,
        Kind::Crush,
        Kind::Drive,
        Kind::Reverb,
        Kind::Liquid,
        Kind::Compressor,
        Kind::Scope,
    ];
    pub const SHAPERS: [Kind; 10] = [Kind::Volume, Kind::Filter, Kind::Pan, Kind::Width, Kind::Time, Kind::Noise, Kind::Crush, Kind::Drive, Kind::Reverb, Kind::Liquid];

    pub fn index(self) -> usize {
        Kind::ALL.iter().position(|&k| k == self).unwrap_or(0)
    }
    pub fn from_index(i: usize) -> Kind {
        Kind::ALL[i.min(Kind::ALL.len() - 1)]
    }
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
            Kind::Reverb => "Reverb",
            Kind::Liquid => "Liquid",
            Kind::Compressor => "Compressor",
            Kind::Scope => "Scope",
        }
    }
    /// a tool, not a shaper: no shapes, no LFO
    pub fn is_tool(self) -> bool {
        matches!(self, Kind::Compressor | Kind::Scope)
    }
    pub fn about(self) -> &'static str {
        match self {
            Kind::Volume => "The shape is the level: sidechain pumping, gates, tremolo, trimming.",
            Kind::Filter => "Cutoff and resonance, each with its own shape. Clean or warm, 6 / 12 / 24 dB.",
            Kind::Pan => "The shape moves the sound between left and right, by level or Haas delay.",
            Kind::Width => "Top: twice as wide, middle: as is, bottom: mono. Only the side signal moves.",
            Kind::Time => "Scrubs through the recent audio: tape stops, half-time, stutters, scratches.",
            Kind::Noise => "Adds noise; level and pitch each have a shape. Risers, hats, vinyl, air.",
            Kind::Crush => "Bit depth and sample rate, each with its own shape.",
            Kind::Drive => "Distortion amount follows the shape. 12 styles from soft clip to extreme fold.",
            Kind::Reverb => "Reverb send and return, each with a shape: gated, swelling, ducked tails.",
            Kind::Liquid => "Flanger or phaser; centre and feedback each with a shape.",
            Kind::Compressor => "A compressor anywhere in the chain, multiband, with sidechain.",
            Kind::Scope => "An oscilloscope anywhere in the chain: see the signal at that point.",
        }
    }
    /// can have an envelope follower on its lanes
    pub fn has_envelope(self) -> bool {
        !matches!(self, Kind::Time | Kind::Compressor | Kind::Scope)
    }
    pub fn lanes(self) -> &'static [Lane] {
        match self {
            Kind::Volume => &[Lane { name: "Volume", top: "FULL", bottom: "SILENT" }],
            Kind::Filter => &[Lane { name: "Cutoff", top: "OPEN", bottom: "CLOSED" }, Lane { name: "Resonance", top: "MAX", bottom: "NONE" }],
            Kind::Pan => &[Lane { name: "Pan", top: "LEFT", bottom: "RIGHT" }],
            Kind::Width => &[Lane { name: "Width", top: "WIDE", bottom: "MONO" }],
            Kind::Time => &[Lane { name: "Time", top: "NOW", bottom: "LATE" }],
            Kind::Noise => &[Lane { name: "Volume", top: "LOUD", bottom: "OFF" }, Lane { name: "Pitch", top: "+24 st", bottom: "-24 st" }],
            Kind::Crush => &[Lane { name: "Bits", top: "16 BIT", bottom: "1 BIT" }, Lane { name: "Resample", top: "44.1 kHz", bottom: "8.8 Hz" }],
            Kind::Drive => &[Lane { name: "Drive", top: "DRIVEN", bottom: "CLEAN" }],
            Kind::Reverb => &[Lane { name: "Send", top: "FULL", bottom: "NONE" }, Lane { name: "Volume", top: "FULL", bottom: "SILENT" }],
            Kind::Liquid => &[Lane { name: "Centre", top: "HIGH", bottom: "LOW" }, Lane { name: "Feedback", top: "MAX", bottom: "NONE" }],
            Kind::Compressor | Kind::Scope => &[],
        }
    }
    pub fn n_lanes(self) -> usize {
        self.lanes().len()
    }
    /// shape a lane starts with
    pub fn default_wave(self, lane: usize) -> Wave {
        match (self, lane) {
            (Kind::Volume, _) => Wave::preset(W::Pump),
            (Kind::Filter, 0) => Wave::from(&[Pt::new(0.0, 0.25, -0.4), Pt::new(0.5, 1.0, 0.4), Pt::new(1.0, 0.25, 0.0)]),
            (Kind::Filter, _) => Wave::flat(0.3),
            (Kind::Pan, _) => Wave::preset(W::Sine),
            (Kind::Width, _) => Wave::from(&[Pt::new(0.0, 0.5, 0.0), Pt::new(0.5, 1.0, 0.0), Pt::new(1.0, 0.5, 0.0)]),
            (Kind::Time, _) => Wave::preset(W::TapeStop),
            (Kind::Noise, 0) => Wave::from(&[Pt::new(0.0, 0.0, 0.5), Pt::new(1.0, 0.7, 0.0)]),
            (Kind::Noise, _) => Wave::flat(0.5),
            (Kind::Crush, 0) => Wave::steps(&[1.0, 0.5, 1.0, 0.3, 1.0, 0.6, 0.25, 1.0]),
            (Kind::Crush, _) => Wave::flat(0.75),
            (Kind::Drive, _) => Wave::from(&[Pt::new(0.0, 1.0, 0.45), Pt::new(1.0, 0.0, 0.0)]),
            (Kind::Reverb, 0) => Wave::preset(W::Gate8),
            (Kind::Reverb, _) => Wave::flat(1.0),
            (Kind::Liquid, 0) => Wave::preset(W::Sine),
            (Kind::Liquid, _) => Wave::flat(0.6),
            _ => Wave::flat(1.0),
        }
    }
    pub fn default_length(self) -> u8 {
        // index into LENGTHS
        match self {
            Kind::Volume | Kind::Drive => 7,
            Kind::Pan | Kind::Width => 6,
            Kind::Crush => 5,
            _ => 5,
        }
    }
    pub fn knobs(self) -> &'static [KnobDef] {
        use Scale::*;
        match self {
            Kind::Volume => { const K: &[KnobDef] = &[KnobDef::new("Trim", "Level after the shaper.", -24.0, 24.0, 0.0, Lin, Unit::Db, true)]; K },
            Kind::Filter => { const K: &[KnobDef] = &[
                KnobDef::new("Stereo", "Moves the left and right cutoff apart, up to ±60 semitones.", -60.0, 60.0, 0.0, Lin, Unit::St, true),
                KnobDef::new("Dist", "Drive inside the warm filter (Warm flavour).", 0.0, 1.0, 0.0, Lin, Unit::Pct, false),
                KnobDef::new("Output", "Level after the filter.", -12.0, 12.0, 0.0, Lin, Unit::Db, true),
            ]; K },
            Kind::Pan => { const K: &[KnobDef] = &[
                KnobDef::new("Pan / Haas", "0 %: level panning · 100 %: Haas panning (the far side is delayed).", 0.0, 1.0, 0.0, Lin, Unit::Pct, false),
                KnobDef::new("Haas range", "Largest Haas delay.", 0.0, 40.0, 2.0, Pow(2.0), Unit::Ms, false),
            ]; K },
            Kind::Width => { const K: &[KnobDef] = &[KnobDef::new("Gain", "Level compensation for the width changes.", -12.0, 12.0, 0.0, Lin, Unit::Db, true)]; K },
            Kind::Time => { const K: &[KnobDef] = &[]; K },
            Kind::Noise => { const K: &[KnobDef] = &[
                KnobDef::new("Trim", "Noise level, +12 to -48 dB.", -48.0, 12.0, -6.0, Lin, Unit::Db, true),
                KnobDef::new("High-pass", "High-pass on the noise.", 20.0, 20000.0, 120.0, Log, Unit::Hz, false),
                KnobDef::new("Low-pass", "Low-pass on the noise.", 20.0, 20000.0, 20000.0, Log, Unit::Hz, false),
                KnobDef::new("Stereo", "0 %: mono noise · 100 %: independent left / right.", 0.0, 1.0, 1.0, Lin, Unit::Pct, false),
                KnobDef::new("Duck thresh", "Duck: input level from which the noise drops.", -60.0, 0.0, -30.0, Lin, Unit::Db, false),
                KnobDef::new("Env level", "Follow / Duck: how much the input level moves the noise.", 0.0, 1.0, 1.0, Lin, Unit::Pct, false),
            ]; K },
            Kind::Crush => { const K: &[KnobDef] = &[
                KnobDef::new("Crunch", "Wave-wrapping overdrive before the bits.", 0.0, 27.0, 0.0, Lin, Unit::Db, false),
                KnobDef::new("Push", "DC offset before the bit reduction (removed after): keeps low-level detail.", -1.0, 1.0, 0.0, Lin, Unit::Pct, true),
                KnobDef::new("Jitter", "Random variation of the sample rate.", 0.0, 1.0, 0.0, Lin, Unit::Pct, false),
                KnobDef::new("Treble", "High shelf after the crush: 100 % = unfiltered.", 0.0, 1.0, 1.0, Lin, Unit::Pct, false),
                KnobDef::new("Gain", "Level after the crush.", -24.0, 24.0, 0.0, Lin, Unit::Db, true),
            ]; K },
            Kind::Drive => { const K: &[KnobDef] = &[
                KnobDef::new("Input", "Gain into the drive.", -12.0, 36.0, 0.0, Lin, Unit::Db, true),
                KnobDef::new("Output", "Level after the drive.", -36.0, 12.0, 0.0, Lin, Unit::Db, true),
                KnobDef::new("Push", "DC offset into the curve: asymmetry, even harmonics.", -1.0, 1.0, 0.0, Lin, Unit::Pct, true),
                KnobDef::new("Grip", "Makes the quiet parts quieter.", 0.0, 1.0, 0.0, Lin, Unit::Pct, false),
                KnobDef::new("Accent", "Less gain compensation: the driven peaks get louder.", 0.0, 1.0, 0.0, Lin, Unit::Pct, false),
                KnobDef::new("Tone", "Dark (low-pass) to bright, mixed in as the drive rises.", -1.0, 1.0, 0.0, Lin, Unit::Pct, true),
            ]; K },
            Kind::Reverb => { const K: &[KnobDef] = &[
                KnobDef::new("Pre-delay", "Time before the reverb starts.", 0.0, 500.0, 10.0, Pow(2.0), Unit::Ms, false),
                KnobDef::new("Decay", "Length of the tail.", 0.0, 1.0, 0.55, Lin, Unit::Pct, false),
                KnobDef::new("Size", "Size of the space (stretches the reflections).", 0.5, 2.0, 1.0, Lin, Unit::Pct, false),
                KnobDef::new("Width", "Stereo width of the reverb.", 0.0, 2.0, 1.0, Lin, Unit::Pct, false),
                KnobDef::new("Tone", "Dark to bright tail.", 0.0, 1.0, 0.55, Lin, Unit::Pct, false),
                KnobDef::new("Dry", "Level of the dry signal.", 0.0, 1.0, 1.0, Lin, Unit::Pct, false),
                KnobDef::new("Wet", "Level of the reverb.", 0.0, 1.0, 0.4, Lin, Unit::Pct, false),
            ]; K },
            Kind::Liquid => { const K: &[KnobDef] = &[
                KnobDef::new("Stereo", "Moves the left and right centre apart, up to ±60 semitones.", -60.0, 60.0, 12.0, Lin, Unit::St, true),
                KnobDef::new("Spread", "How far the phaser stages spread around the centre.", 0.0, 1.0, 0.5, Lin, Unit::Pct, false),
            ]; K },
            Kind::Compressor => { const K: &[KnobDef] = &[
                KnobDef::new("Threshold", "Level above which it compresses.", -60.0, 0.0, -18.0, Lin, Unit::Db, false),
                KnobDef::new("Ratio", "1:1 (none) to 100:1 (limiting).", 1.0, 100.0, 2.0, Log, Unit::Ratio, false),
                KnobDef::new("Attack", "0.01-200 ms.", 0.01, 200.0, 10.0, Log, Unit::Ms, false),
                KnobDef::new("Hold", "Holds the gain reduction before releasing.", 0.0, 500.0, 0.0, Pow(2.0), Unit::Ms, false),
                KnobDef::new("Release", "50 ms - 2 s.", 50.0, 2000.0, 150.0, Log, Unit::Ms, false),
                KnobDef::new("Makeup", "Makeup gain (on top of Auto).", -12.0, 24.0, 0.0, Lin, Unit::Db, true),
            ]; K },
            Kind::Scope => { const K: &[KnobDef] = &[]; K },
        }
    }
    /// categories of the shape menu: (category, shapes)
    pub fn wave_categories(self) -> Vec<(&'static str, Vec<W>)> {
        let basic = ("Basic", vec![W::Flat, W::Zero, W::SawUp, W::SawDown, W::Triangle, W::Sine, W::Square]);
        let rhythm1 = ("Rhythm 1", vec![W::Gate8, W::Gate16, W::GateTriplet, W::Stutter]);
        let rhythm2 = ("Rhythm 2", vec![W::Ramps16, W::Duck8, W::Bounce, W::Swell]);
        match self {
            Kind::Volume => vec![("Sidechain", vec![W::Pump, W::PumpHard, W::Duck8]), ("Trimming", vec![W::Swell, W::TrimIn, W::TrimOut]), rhythm1, rhythm2, basic],
            Kind::Time => vec![("Pitch & time", vec![W::TapeStop, W::HalfTime, W::Scratch, W::Stutter, W::TapeStart]), basic],
            Kind::Filter | Kind::Liquid => vec![("Sweep", vec![W::SawUp, W::SawDown, W::Swell, W::Sine, W::Triangle]), ("Hit", vec![W::Pump, W::PumpHard, W::Duck8]), rhythm1, rhythm2, basic],
            _ => vec![("Hit", vec![W::Pump, W::PumpHard, W::Duck8, W::Bounce]), ("Sweep", vec![W::SawUp, W::SawDown, W::Swell]), rhythm1, rhythm2, basic],
        }
    }
}

#[derive(Clone, Copy)]
pub struct Lane {
    pub name: &'static str,
    pub top: &'static str,
    pub bottom: &'static str,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Scale {
    Lin,
    Log,
    /// normalized^p
    Pow(f32),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Unit {
    Db,
    Pct,
    Hz,
    Ms,
    St,
    Ratio,
    Deg,
}

#[derive(Clone, Copy)]
pub struct KnobDef {
    pub name: &'static str,
    pub tip: &'static str,
    pub min: f32,
    pub max: f32,
    pub def: f32,
    pub scale: Scale,
    pub unit: Unit,
    pub bipolar: bool,
}

impl KnobDef {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(name: &'static str, tip: &'static str, min: f32, max: f32, def: f32, scale: Scale, unit: Unit, bipolar: bool) -> Self {
        Self { name, tip, min, max, def, scale, unit, bipolar }
    }
    pub fn plain(&self, n: f32) -> f32 {
        let n = n.clamp(0.0, 1.0);
        match self.scale {
            Scale::Lin => self.min + (self.max - self.min) * n,
            Scale::Log => self.min * (self.max / self.min).powf(n),
            Scale::Pow(p) => self.min + (self.max - self.min) * n.powf(p),
        }
    }
    pub fn norm(&self, v: f32) -> f32 {
        let v = v.clamp(self.min.min(self.max), self.max.max(self.min));
        match self.scale {
            Scale::Lin => (v - self.min) / (self.max - self.min),
            Scale::Log => (v / self.min).ln() / (self.max / self.min).ln(),
            Scale::Pow(p) => ((v - self.min) / (self.max - self.min)).max(0.0).powf(1.0 / p),
        }
        .clamp(0.0, 1.0)
    }
    pub fn default_norm(&self) -> f32 {
        self.norm(self.def)
    }
    pub fn fmt(&self, n: f32) -> String {
        fmt_unit(self.plain(n), self.unit)
    }
    pub fn parse(&self, s: &str) -> Option<f32> {
        let t = s.trim().to_lowercase();
        let t = t.trim_end_matches("db").trim_end_matches('%').trim_end_matches("hz").trim_end_matches("ms").trim_end_matches("st").trim_end_matches(":1").trim_end_matches('°').trim();
        let (t, mul) = if let Some(k) = t.strip_suffix('k') { (k.trim(), 1000.0) } else { (t, 1.0) };
        let v: f32 = t.parse().ok()?;
        let v = v * mul;
        Some(self.norm(if self.unit == Unit::Pct { v / 100.0 } else { v }))
    }
}

pub fn fmt_unit(v: f32, u: Unit) -> String {
    match u {
        Unit::Db => format!("{:.1} dB", v + 0.0),
        Unit::Pct => format!("{:.0} %", v * 100.0 + 0.0),
        Unit::Hz => {
            if v >= 1000.0 {
                format!("{:.1}k Hz", v / 1000.0)
            } else {
                format!("{:.0} Hz", v)
            }
        }
        Unit::Ms => {
            if v < 10.0 {
                format!("{v:.2} ms")
            } else if v < 100.0 {
                format!("{v:.1} ms")
            } else {
                format!("{v:.0} ms")
            }
        }
        Unit::St => format!("{:.1} st", v + 0.0),
        Unit::Deg => format!("{:.0}°", v + 0.0),
        Unit::Ratio => {
            if v >= 99.5 {
                "∞:1".into()
            } else {
                format!("{v:.1}:1")
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// Names of the discrete settings
// ------------------------------------------------------------------------------------------

pub const LENGTHS: [&str; 13] = ["32 bars", "16 bars", "8 bars", "4 bars", "2 bars", "1 bar", "1/2", "1/4", "1/8", "1/16", "1/32", "1/64", "1/128"];
pub const FEELS: [&str; 3] = ["Straight", "Dotted", "Triplet"];
pub const MODES: [&str; 3] = ["Beat", "Hz", "Pitch"];
pub const TRIGGERS: [&str; 3] = ["Sync", "MIDI", "Audio"];

/// cycle length in quarter notes
pub fn length_beats(length: u8, feel: u8) -> f64 {
    let base = 4.0 * 2f64.powi(5 - length.min(12) as i32);
    base * match feel {
        1 => 1.5,
        2 => 2.0 / 3.0,
        _ => 1.0,
    }
}

pub const FILTER_TYPES: [&str; 5] = ["Low-pass", "High-pass", "Band-pass", "Notch", "Peak"];
pub const SLOPES: [&str; 3] = ["6 dB", "12 dB", "24 dB"];
pub const DRIVE_STYLES: [&str; 12] = [
    "Soft clip",
    "Smooth clip",
    "Hard clip",
    "Soft square",
    "Hard square",
    "Soft rectify",
    "Hard rectify",
    "Skew fold",
    "Sine fold",
    "Triangle fold",
    "Saw fold",
    "Extreme fold",
];
pub const PAN_LAWS: [&str; 5] = ["Balance 0 dB", "Balance 3 dB", "Balance 4.5 dB", "Balance 6 dB", "Combined"];
pub const TIME_RANGES: [&str; 8] = ["Linked", "1 bar", "2 bars", "4 bars", "8 bars", "2.5 ms", "10 ms", "20 ms"];
pub const LIQUID_TYPES: [&str; 3] = ["Off", "Flanger", "Phaser"];
pub const NOISE_ENVS: [&str; 3] = ["Off", "Follow", "Duck"];
pub const ENV_INPUTS: [&str; 3] = ["Band", "Free", "Sidechain"];
pub const ENV_DEPTHS: [&str; 4] = ["×1", "×2", "×4", "×8"];
pub const SCOPE_MODES: [&str; 3] = ["In + out", "Output", "Off"];
pub const TRIG_ALGOS: [&str; 3] = ["Drums", "General", "Complex"];

/// Built-in noises: (category, name)
pub const NOISES: [(&str, &str); 24] = [
    ("Noise", "White"),
    ("Noise", "Pink"),
    ("Noise", "Brown"),
    ("Noise", "Blue"),
    ("Noise", "Violet"),
    ("Noise", "Grey"),
    ("Organic", "Rain"),
    ("Organic", "Wind"),
    ("Organic", "Ocean"),
    ("Organic", "Fire"),
    ("Analogue", "Tape hiss"),
    ("Analogue", "Vinyl crackle"),
    ("Analogue", "Mains hum 50 Hz"),
    ("Analogue", "Mains hum 60 Hz"),
    ("Analogue", "Radio static"),
    ("Analogue", "Digital"),
    ("Cymbal", "Metal 808"),
    ("Cymbal", "Bright shimmer"),
    ("Cymbal", "Dark ride"),
    ("Percussion", "Shaker"),
    ("Percussion", "Snare wires"),
    ("Percussion", "Clap noise"),
    ("Percussion", "Brush"),
    ("Noise", "Air"),
];

/// Built-in reverbs: (category, name)
pub const REVERBS: [(&str, &str); 14] = [
    ("Hall", "Concert hall"),
    ("Hall", "Bright hall"),
    ("Room", "Small room"),
    ("Room", "Studio room"),
    ("Room", "Chamber"),
    ("Plate", "Realistic plate"),
    ("Plate", "Vocal plate"),
    ("Epic", "Cathedral"),
    ("Epic", "Endless"),
    ("Special", "Shimmer cloud"),
    ("Special", "Modulated"),
    ("Mechanical", "Spring"),
    ("Mechanical", "Metal tank"),
    ("Non-linear", "Gated"),
];
