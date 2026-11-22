//! Presets: the whole plugin (every parameter + shapes + user waves) in one JSON file.
//!
//! Factory presets are built in code, user presets live in
//! `<data dir>/WaveCube/Presets/*.json` (Linux `~/.local/share`, Windows `%APPDATA%`,
//! macOS `~/Library/Application Support`). Favourites are kept in `favorites.json` next to them.

use crate::wave::{Preset as W, Pt, Wave};
use crate::{DriveStyle, Feel, FilterType, Kind, NoiseEnv, NoiseType, Rate, State, TimeRange, Trigger, WaveCubeParams, N_BANDS};
use nih_plug::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

pub const CATEGORIES: [&str; 9] = ["Sidechain", "Rhythmic", "Filter", "Stereo", "Time FX", "Lo-fi", "Drive", "Risers & FX", "Mixing"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PresetData {
    pub name: String,
    pub category: String,
    #[serde(default)]
    pub author: String,
    /// normalized values by parameter id; missing ids load their default
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
    pub state: State,
}

impl PresetData {
    /// Kinds switched on in this preset (for the coloured squares in the browser).
    pub fn kinds_on(&self) -> Vec<usize> {
        (0..Kind::ALL.len()).filter(|i| self.params.get(&format!("on_{}", i + 1)).copied().unwrap_or(if *i == 0 { 1.0 } else { 0.0 }) > 0.5).collect()
    }
}

// ------------------------------------------------------------------------------------------
// Building presets in code
// ------------------------------------------------------------------------------------------

pub struct Builder {
    vals: BTreeMap<String, f32>,
    ptrs: HashMap<String, ParamPtr>,
    state: State,
    // the parameters the pointers point into: must live as long as the builder
    _owner: Box<WaveCubeParams>,
}

impl Builder {
    pub fn new() -> Self {
        let owner = Box::new(WaveCubeParams::default());
        let mut vals = BTreeMap::new();
        let mut ptrs = HashMap::new();
        for (id, ptr, _) in owner.param_map() {
            if id == "bypass" {
                continue;
            }
            // SAFETY: the pointers point into `owner`, which the builder keeps alive
            vals.insert(id.clone(), unsafe { ptr.default_normalized_value() });
            ptrs.insert(id, ptr);
        }
        // presets start with every shaper off
        for k in Kind::ALL {
            vals.insert(format!("on_{}", k.index() + 1), 0.0);
        }
        Self { vals, ptrs, state: State::default(), _owner: owner }
    }

    /// Sets a parameter to a plain value (enums: the index, bools: 0 / 1).
    pub fn g(&mut self, id: &str, plain: f32) -> &mut Self {
        if let Some(p) = self.ptrs.get(id) {
            // SAFETY: see `new`
            let v = unsafe { p.preview_normalized(plain) };
            self.vals.insert(id.to_string(), v);
        } else {
            debug_assert!(false, "unknown parameter {id}");
        }
        self
    }

    /// A shaper's own parameter (`amount`, `rate`, ...).
    pub fn k(&mut self, kind: Kind, field: &str, plain: f32) -> &mut Self {
        self.g(&format!("{field}_{}", kind.index() + 1), plain)
    }

    pub fn on(&mut self, kind: Kind) -> &mut Self {
        self.k(kind, "on", 1.0)
    }

    pub fn rate(&mut self, kind: Kind, r: Rate) -> &mut Self {
        self.k(kind, "rate", r.to_index() as f32)
    }

    pub fn wave(&mut self, kind: Kind, w: Wave) -> &mut Self {
        self.state.waves[kind.index()] = vec![w; N_BANDS];
        self
    }

    pub fn wave_b(&mut self, kind: Kind, band: usize, w: Wave) -> &mut Self {
        self.state.waves[kind.index()][band] = w;
        self
    }

    pub fn select(&mut self, kind: Kind) -> &mut Self {
        self.state.selected = kind.index() as u8;
        self
    }

    pub fn finish(&mut self, name: &str, category: &str) -> PresetData {
        let mut state = self.state.clone();
        state.preset = name.to_string();
        state.sanitize();
        PresetData { name: name.into(), category: category.into(), author: "Factory".into(), params: self.vals.clone(), state }
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

fn inv(mut w: Wave) -> Wave {
    w.invert();
    w
}

fn rep(mut w: Wave, n: usize) -> Wave {
    w.repeat(n);
    w
}

fn random(seed: u32, grid: usize) -> Wave {
    let mut w = Wave::flat(1.0);
    let levels: Vec<f32> = (0..grid)
        .map(|i| {
            let mut r = seed.wrapping_mul(2654435761).wrapping_add(i as u32 * 40503);
            r ^= r >> 13;
            r = r.wrapping_mul(0x5bd1e995);
            r ^= r >> 15;
            (r % 1000) as f32 / 1000.0
        })
        .collect();
    for (i, l) in levels.iter().enumerate() {
        w.paint_step(i as f32 / grid as f32, (i + 1) as f32 / grid as f32, (l * 4.0).round() / 4.0);
    }
    w
}

/// Every factory preset.
pub fn factory() -> Vec<PresetData> {
    use Kind::*;
    let mut out = Vec::new();
    let mut add = |name: &str, cat: &str, f: &dyn Fn(&mut Builder)| {
        let mut b = Builder::new();
        f(&mut b);
        out.push(b.finish(name, cat));
    };
    let ft = |t: FilterType| t.to_index() as f32;

    // ---- Sidechain
    add("Sidechain Pump 1/4", "Sidechain", &|b| {
        b.on(Volume).rate(Volume, Rate::Quarter).wave(Volume, Wave::preset(W::Pump));
    });
    add("Hard Duck 1/4", "Sidechain", &|b| {
        b.on(Volume).rate(Volume, Rate::Quarter).wave(Volume, Wave::preset(W::PumpHard));
    });
    add("Soft Pump 1/2", "Sidechain", &|b| {
        b.on(Volume).rate(Volume, Rate::Half).k(Volume, "amount", 0.7).wave(Volume, Wave::preset(W::Pump));
    });
    add("Low-end Duck", "Sidechain", &|b| {
        b.on(Volume).rate(Volume, Rate::Quarter).k(Volume, "multi", 1.0).k(Volume, "xlo", 160.0);
        b.wave_b(Volume, 0, Wave::preset(W::PumpHard)).wave_b(Volume, 1, Wave::preset(W::Pump)).wave_b(Volume, 2, Wave::flat(1.0));
        b.k(Volume, "amount", 1.0);
    });
    add("Pump + Glue", "Sidechain", &|b| {
        b.on(Volume).rate(Volume, Rate::Quarter).wave(Volume, Wave::preset(W::Pump));
        b.g("vcomp", 1.0).g("vthr", -20.0).g("vratio", 3.0).g("vatt", 15.0).g("vrel", 120.0);
    });

    // ---- Rhythmic
    add("Trance Gate 1/16", "Rhythmic", &|b| {
        b.on(Volume).rate(Volume, Rate::Bar1).k(Volume, "smooth", 3.0).wave(Volume, Wave::preset(W::Gate16));
    });
    add("Triplet Gate", "Rhythmic", &|b| {
        b.on(Volume).rate(Volume, Rate::Bar1).k(Volume, "smooth", 2.5).wave(Volume, Wave::preset(W::GateTriplet));
    });
    add("Chopper 1/8", "Rhythmic", &|b| {
        b.on(Volume).rate(Volume, Rate::Half).k(Volume, "smooth", 2.0).wave(Volume, Wave::preset(W::Gate8));
    });
    add("Bouncing Ball", "Rhythmic", &|b| {
        b.on(Volume).rate(Volume, Rate::Bar1).wave(Volume, Wave::preset(W::Bounce));
    });
    add("Tremolo 1/16T", "Rhythmic", &|b| {
        b.on(Volume).rate(Volume, Rate::Sixteenth).k(Volume, "feel", Feel::Triplet.to_index() as f32).k(Volume, "amount", 0.6).wave(Volume, Wave::preset(W::Sine));
    });

    // ---- Filter
    add("Wobble 1/8", "Filter", &|b| {
        b.on(Filter).select(Filter).rate(Filter, Rate::Eighth).wave(Filter, Wave::preset(W::Sine));
        b.g("ftype", ft(FilterType::Lp24)).g("flo", 120.0).g("fhi", 4000.0).g("fres", 0.6);
    });
    add("Slow Sweep 4 bars", "Filter", &|b| {
        b.on(Filter).select(Filter).rate(Filter, Rate::Bar4).wave(Filter, Wave::from(&[Pt::new(0.0, 0.0, 0.35), Pt::new(1.0, 1.0, 0.0)]));
        b.g("ftype", ft(FilterType::Lp12)).g("flo", 200.0).g("fhi", 16000.0).g("fres", 0.35);
    });
    add("Phaser Drift", "Filter", &|b| {
        b.on(Filter).select(Filter).rate(Filter, Rate::Bar2).wave(Filter, Wave::preset(W::Sine));
        b.g("ftype", ft(FilterType::PhN6)).g("flo", 300.0).g("fhi", 6000.0).g("fres", 0.5);
    });
    add("Envelope Funk", "Filter", &|b| {
        b.on(Filter).select(Filter).wave(Filter, Wave::flat(0.15));
        b.g("ftype", ft(FilterType::Bp12)).g("flo", 200.0).g("fhi", 3500.0).g("fres", 0.55);
        b.k(Filter, "envon", 1.0).k(Filter, "envamt", 1.4).k(Filter, "envatt", 2.0).k(Filter, "envrel", 120.0).k(Filter, "envthr", -36.0);
    });
    add("Stereo Sweep", "Filter", &|b| {
        b.on(Filter).select(Filter).rate(Filter, Rate::Bar1).wave(Filter, Wave::preset(W::Triangle));
        b.g("ftype", ft(FilterType::Lp12)).g("flo", 300.0).g("fhi", 12000.0).g("fres", 0.4).g("fstereo", 24.0);
    });

    // ---- Stereo
    add("Auto-pan 1/4", "Stereo", &|b| {
        b.on(Pan).select(Pan).rate(Pan, Rate::Quarter).k(Pan, "amount", 0.8).wave(Pan, Wave::preset(W::Sine));
    });
    add("Haas Swirl", "Stereo", &|b| {
        b.on(Pan).select(Pan).rate(Pan, Rate::Bar1).wave(Pan, Wave::preset(W::Sine)).g("phaas", 1.0).g("phaasms", 15.0);
    });
    add("Ping-pong 1/8", "Stereo", &|b| {
        b.on(Pan).select(Pan).rate(Pan, Rate::Quarter).k(Pan, "smooth", 4.0).wave(Pan, Wave::preset(W::Square));
    });
    add("Breathing Width", "Stereo", &|b| {
        b.on(Width).select(Width).rate(Width, Rate::Bar2).wave(Width, Wave::preset(W::Sine));
    });
    add("Mono Bass", "Stereo", &|b| {
        b.on(Width).select(Width).k(Width, "multi", 1.0).k(Width, "xlo", 150.0);
        b.wave_b(Width, 0, Wave::flat(0.0)).wave_b(Width, 1, Wave::flat(0.5)).wave_b(Width, 2, Wave::flat(0.5));
    });

    // ---- Time FX
    add("Tape Stop", "Time FX", &|b| {
        b.on(Time).select(Time).rate(Time, Rate::Bar1).wave(Time, Wave::preset(W::TapeStop));
    });
    add("Half-time", "Time FX", &|b| {
        b.on(Time).select(Time).rate(Time, Rate::Half).k(Time, "smooth", 3.0).wave(Time, Wave::preset(W::HalfTime));
    });
    add("Vinyl Scratch", "Time FX", &|b| {
        b.on(Time).select(Time).rate(Time, Rate::Bar1).k(Time, "smooth", 8.0).wave(Time, Wave::preset(W::Scratch));
    });
    add("Stutter Repeats", "Time FX", &|b| {
        b.on(Time).select(Time).rate(Time, Rate::Bar1).k(Time, "smooth", 2.0).wave(Time, Wave::preset(W::Stutter));
    });

    // ---- Lo-fi
    add("Bit Steps", "Lo-fi", &|b| {
        b.on(Crush).select(Crush).rate(Crush, Rate::Bar1).wave(Crush, random(7, 8)).g("cbits", 4.0).g("crate", 4000.0);
    });
    add("Downsample Sweep", "Lo-fi", &|b| {
        b.on(Crush).select(Crush).rate(Crush, Rate::Bar2).wave(Crush, Wave::preset(W::SawUp)).g("cbits", 16.0).g("crate", 900.0).g("cjitter", 0.3);
    });
    add("Vinyl Night", "Lo-fi", &|b| {
        b.on(Noise).on(Filter).on(Crush).select(Noise);
        b.wave(Noise, Wave::flat(1.0)).k(Noise, "amount", 0.25).g("ntype", NoiseType::Crackle.to_index() as f32).g("nhp", 300.0).g("nlp", 9000.0);
        b.wave(Filter, Wave::flat(1.0)).g("ftype", ft(FilterType::Lp12)).g("fhi", 7000.0).g("fres", 0.1);
        b.wave(Crush, Wave::flat(1.0)).k(Crush, "amount", 0.4).g("cbits", 12.0).g("crate", 16000.0);
    });

    // ---- Drive
    add("Drive Pump", "Drive", &|b| {
        b.on(Drive).select(Drive).rate(Drive, Rate::Quarter).wave(Drive, inv(Wave::preset(W::Pump))).g("dstyle", DriveStyle::SoftClip.to_index() as f32);
    });
    add("Fold Sweep", "Drive", &|b| {
        b.on(Drive).select(Drive).rate(Drive, Rate::Bar1).wave(Drive, Wave::preset(W::Triangle)).k(Drive, "amount", 0.7).g("dstyle", DriveStyle::TriFold.to_index() as f32).g("dtone", -0.3);
    });
    add("Rectified Hits", "Drive", &|b| {
        b.on(Drive).select(Drive).rate(Drive, Rate::Eighth).wave(Drive, inv(Wave::preset(W::PumpHard))).k(Drive, "amount", 0.5).g("dstyle", DriveStyle::SoftRectify.to_index() as f32).g("dpush", 0.3);
    });

    // ---- Risers & FX
    add("Noise Riser 4 bars", "Risers & FX", &|b| {
        b.on(Noise).on(Filter).select(Noise).rate(Noise, Rate::Bar4).rate(Filter, Rate::Bar4);
        b.wave(Noise, Wave::from(&[Pt::new(0.0, 0.0, 0.6), Pt::new(1.0, 1.0, 0.0)])).k(Noise, "amount", 0.5).g("ntype", NoiseType::White.to_index() as f32);
        b.wave(Filter, Wave::from(&[Pt::new(0.0, 0.0, 0.4), Pt::new(1.0, 1.0, 0.0)])).g("ftype", ft(FilterType::Lp12)).g("flo", 300.0).g("fres", 0.4);
    });
    add("Air Hats 1/16", "Risers & FX", &|b| {
        b.on(Noise).select(Noise).rate(Noise, Rate::Bar1).wave(Noise, Wave::preset(W::Ramps16)).k(Noise, "amount", 0.3);
        b.g("ntype", NoiseType::Air.to_index() as f32).g("nhp", 6000.0);
    });
    add("Build-up 8 bars", "Risers & FX", &|b| {
        b.on(Filter).on(Noise).on(Width).select(Filter);
        for k in [Filter, Noise, Width] {
            b.rate(k, Rate::Bar8);
        }
        b.wave(Filter, Wave::preset(W::SawUp)).g("ftype", ft(FilterType::Hp12)).g("flo", 30.0).g("fhi", 1200.0).g("fres", 0.45);
        b.wave(Noise, Wave::preset(W::SawUp)).k(Noise, "amount", 0.35);
        b.wave(Width, Wave::from(&[Pt::new(0.0, 0.5, 0.0), Pt::new(1.0, 0.1, 0.0)]));
    });
    add("Glitch Stutter", "Risers & FX", &|b| {
        b.on(Time).on(Volume).select(Time).rate(Time, Rate::Half).rate(Volume, Rate::Half);
        b.wave(Time, rep(Wave::preset(W::Stutter), 2)).wave(Volume, Wave::preset(W::Gate16));
    });

    // ---- Mixing
    add("Ducked Air", "Mixing", &|b| {
        b.on(Noise).select(Noise).wave(Noise, Wave::flat(1.0)).k(Noise, "amount", 0.12);
        b.g("ntype", NoiseType::Tape.to_index() as f32).g("nenv", NoiseEnv::Duck.to_index() as f32).g("nenvamt", 1.0);
    });
    add("Gentle Glue", "Mixing", &|b| {
        b.on(Volume).wave(Volume, Wave::flat(1.0)).g("vcomp", 1.0).g("vthr", -18.0).g("vratio", 2.0).g("vatt", 20.0).g("vrel", 200.0);
    });
    add("Hit-triggered Pump", "Mixing", &|b| {
        b.on(Volume).rate(Volume, Rate::Quarter).k(Volume, "trig", Trigger::Audio.to_index() as f32).wave(Volume, Wave::preset(W::Pump));
    });
    add("Long Tape Slowdown", "Mixing", &|b| {
        b.on(Time).select(Time).rate(Time, Rate::Bar2).k(Time, "trig", Trigger::MidiOnce.to_index() as f32).g("trange", TimeRange::Bar2.to_index() as f32);
        b.wave(Time, Wave::from(&[Pt::new(0.0, 1.0, -0.4), Pt::new(1.0, 0.0, 0.0)]));
    });
    out
}

/// The quick presets of one shaper: (name, preset). Applying one only touches that shaper.
pub fn quick(kind: Kind) -> Vec<(&'static str, PresetData)> {
    use Kind::*;
    let mut out = Vec::new();
    let mut add = |name: &'static str, f: &dyn Fn(&mut Builder)| {
        let mut b = Builder::new();
        b.on(kind);
        f(&mut b);
        out.push((name, b.finish(name, "")));
    };
    add("Init", &|b| {
        b.wave(kind, kind.default_wave());
    });
    match kind {
        Volume => {
            add("Sidechain 1/4", &|b| {
                b.wave(Volume, Wave::preset(W::Pump));
            });
            add("Trance gate", &|b| {
                b.rate(Volume, Rate::Bar1).k(Volume, "smooth", 3.0).wave(Volume, Wave::preset(W::Gate16));
            });
            add("Tremolo 1/8", &|b| {
                b.rate(Volume, Rate::Eighth).k(Volume, "amount", 0.5).wave(Volume, Wave::preset(W::Sine));
            });
        }
        Filter => {
            add("Wobble 1/8", &|b| {
                b.rate(Filter, Rate::Eighth).wave(Filter, Wave::preset(W::Sine)).g("ftype", FilterType::Lp24.to_index() as f32).g("flo", 120.0).g("fhi", 4000.0).g("fres", 0.6);
            });
            add("Slow sweep", &|b| {
                b.rate(Filter, Rate::Bar4).wave(Filter, Wave::preset(W::SawUp)).g("ftype", FilterType::Lp12.to_index() as f32).g("flo", 200.0).g("fhi", 16000.0);
            });
            add("Envelope funk", &|b| {
                b.wave(Filter, Wave::flat(0.15)).g("ftype", FilterType::Bp12.to_index() as f32).g("fres", 0.55).g("fhi", 3500.0);
                b.k(Filter, "envon", 1.0).k(Filter, "envamt", 1.4).k(Filter, "envatt", 2.0).k(Filter, "envrel", 120.0).k(Filter, "envthr", -36.0);
            });
        }
        Pan => {
            add("Auto-pan 1/4", &|b| {
                b.rate(Pan, Rate::Quarter).k(Pan, "amount", 0.8).wave(Pan, Wave::preset(W::Sine));
            });
            add("Haas swirl", &|b| {
                b.rate(Pan, Rate::Bar1).wave(Pan, Wave::preset(W::Sine)).g("phaas", 1.0).g("phaasms", 15.0);
            });
            add("Ping-pong 1/8", &|b| {
                b.rate(Pan, Rate::Quarter).k(Pan, "smooth", 4.0).wave(Pan, Wave::preset(W::Square));
            });
        }
        Width => {
            add("Mono bass", &|b| {
                b.k(Width, "multi", 1.0).k(Width, "xlo", 150.0);
                b.wave_b(Width, 0, Wave::flat(0.0)).wave_b(Width, 1, Wave::flat(0.5)).wave_b(Width, 2, Wave::flat(0.5));
            });
            add("Breathing", &|b| {
                b.rate(Width, Rate::Bar2).wave(Width, Wave::preset(W::Sine));
            });
            add("Wide off-beats", &|b| {
                b.rate(Width, Rate::Quarter).wave(Width, Wave::from(&[Pt::new(0.0, 0.5, -0.5), Pt::new(0.5, 1.0, 0.5), Pt::new(1.0, 0.5, 0.0)]));
            });
        }
        Time => {
            add("Tape stop", &|b| {
                b.rate(Time, Rate::Bar1).wave(Time, Wave::preset(W::TapeStop));
            });
            add("Half-time", &|b| {
                b.rate(Time, Rate::Half).k(Time, "smooth", 3.0).wave(Time, Wave::preset(W::HalfTime));
            });
            add("Vinyl scratch", &|b| {
                b.rate(Time, Rate::Bar1).k(Time, "smooth", 8.0).wave(Time, Wave::preset(W::Scratch));
            });
        }
        Noise => {
            add("Riser 4 bars", &|b| {
                b.rate(Noise, Rate::Bar4).k(Noise, "amount", 0.5).wave(Noise, Wave::from(&[Pt::new(0.0, 0.0, 0.6), Pt::new(1.0, 1.0, 0.0)]));
            });
            add("Vinyl crackle", &|b| {
                b.wave(Noise, Wave::flat(1.0)).k(Noise, "amount", 0.3).g("ntype", NoiseType::Crackle.to_index() as f32).g("nhp", 300.0).g("nlp", 9000.0);
            });
            add("Air hats", &|b| {
                b.rate(Noise, Rate::Bar1).wave(Noise, Wave::preset(W::Ramps16)).k(Noise, "amount", 0.3).g("ntype", NoiseType::Air.to_index() as f32).g("nhp", 6000.0);
            });
        }
        Crush => {
            add("Bit steps", &|b| {
                b.rate(Crush, Rate::Bar1).wave(Crush, random(7, 8)).g("cbits", 4.0).g("crate", 4000.0);
            });
            add("Downsample sweep", &|b| {
                b.rate(Crush, Rate::Bar2).wave(Crush, Wave::preset(W::SawUp)).g("cbits", 16.0).g("crate", 900.0);
            });
            add("Lo-fi always", &|b| {
                b.wave(Crush, Wave::flat(1.0)).k(Crush, "amount", 0.5).g("cbits", 10.0).g("crate", 12000.0);
            });
        }
        Drive => {
            add("Drive pump", &|b| {
                b.rate(Drive, Rate::Quarter).wave(Drive, inv(Wave::preset(W::Pump)));
            });
            add("Warm", &|b| {
                b.wave(Drive, Wave::flat(0.35)).g("dtone", -0.2);
            });
            add("Fold sweep", &|b| {
                b.rate(Drive, Rate::Bar1).wave(Drive, Wave::preset(W::Triangle)).k(Drive, "amount", 0.7).g("dstyle", DriveStyle::TriFold.to_index() as f32);
            });
        }
    }
    out
}

/// Does this parameter belong to that shaper (its own controls + its kind's controls)?
pub fn belongs_to(id: &str, kind: Kind) -> bool {
    let suffix = format!("_{}", kind.index() + 1);
    if id.ends_with(&suffix) {
        return true;
    }
    let letter = match kind {
        Kind::Volume => 'v',
        Kind::Filter => 'f',
        Kind::Pan => 'p',
        Kind::Width => 'w',
        Kind::Time => 't',
        Kind::Noise => 'n',
        Kind::Crush => 'c',
        Kind::Drive => 'd',
    };
    !id.contains('_') && id.starts_with(letter)
}

// ------------------------------------------------------------------------------------------
// Capture / apply
// ------------------------------------------------------------------------------------------

pub fn capture(p: &WaveCubeParams, name: &str, category: &str) -> PresetData {
    let mut params = BTreeMap::new();
    for (id, ptr, _) in p.param_map() {
        if id != "bypass" {
            // SAFETY: the pointer points into `p`
            params.insert(id, unsafe { ptr.unmodulated_normalized_value() });
        }
    }
    let mut state = p.state.read().clone();
    state.preset = name.to_string();
    PresetData { name: name.into(), category: category.into(), author: "User".into(), params, state }
}

/// Loads a preset. With `only` just that shaper's controls and shapes are touched.
pub fn apply(setter: &ParamSetter, p: &WaveCubeParams, data: &PresetData, only: Option<Kind>) {
    for (id, ptr, _) in p.param_map() {
        if id == "bypass" {
            continue;
        }
        if let Some(k) = only {
            if !belongs_to(&id, k) {
                continue;
            }
        }
        // SAFETY: the pointer points into `p`, which outlives this call
        unsafe {
            let v = data.params.get(&id).copied().unwrap_or_else(|| ptr.default_normalized_value()).clamp(0.0, 1.0);
            if (ptr.unmodulated_normalized_value() - v).abs() > 1e-7 {
                setter.raw_context.raw_begin_set_parameter(ptr);
                setter.raw_context.raw_set_parameter_normalized(ptr, v);
                setter.raw_context.raw_end_set_parameter(ptr);
            }
        }
    }
    let mut st = p.state.write();
    match only {
        Some(k) => {
            st.waves[k.index()] = data.state.waves[k.index()].clone();
            st.selected = k.index() as u8;
        }
        None => {
            let grid = st.grid;
            *st = data.state.clone();
            st.preset = data.name.clone();
            st.grid = grid;
        }
    }
    st.sanitize();
}

// ------------------------------------------------------------------------------------------
// Files
// ------------------------------------------------------------------------------------------

pub fn dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("WaveCube").join("Presets"))
}

fn file_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    let s = s.trim();
    format!("{}.json", if s.is_empty() { "Preset" } else { s })
}

pub fn list_user() -> Vec<PresetData> {
    let Some(d) = dir() else { return vec![] };
    let Ok(rd) = std::fs::read_dir(&d) else { return vec![] };
    let mut out: Vec<PresetData> = rd
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| serde_json::from_str::<PresetData>(&s).ok())
        .map(|mut p| {
            p.author = "User".into();
            p
        })
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

pub fn save_user(data: &PresetData) -> Result<PathBuf, String> {
    let d = dir().ok_or("no data folder")?;
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let path = d.join(file_name(&data.name));
    let json = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn delete_user(name: &str) -> bool {
    dir().map(|d| std::fs::remove_file(d.join(file_name(name))).is_ok()).unwrap_or(false)
}

fn fav_path() -> Option<PathBuf> {
    dir().map(|d| d.join("favorites.json"))
}

/// Favourites: "F:<name>" for factory, "U:<name>" for user presets.
pub fn load_favorites() -> Vec<String> {
    fav_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_favorites(f: &[String]) {
    if let (Some(d), Some(p)) = (dir(), fav_path()) {
        let _ = std::fs::create_dir_all(d);
        let _ = std::fs::write(p, serde_json::to_string(f).unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_presets_are_valid() {
        let all = factory();
        assert!(all.len() >= 30);
        let mut names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len(), "duplicate names");
        for p in &all {
            assert!(CATEGORIES.contains(&p.category.as_str()), "{}", p.category);
            assert!(!p.kinds_on().is_empty(), "{} has no shaper on", p.name);
            assert!(p.params.values().all(|v| (0.0..=1.0).contains(v)));
            let s = serde_json::to_string(p).unwrap();
            let back: PresetData = serde_json::from_str(&s).unwrap();
            assert_eq!(&back, p);
        }
        for k in Kind::ALL {
            let q = quick(k);
            assert_eq!(q.len(), 4);
            assert!(q.iter().all(|(_, p)| p.params[&format!("on_{}", k.index() + 1)] > 0.5));
        }
    }

    #[test]
    fn plain_values_land() {
        let mut b = Builder::new();
        b.g("flo", 120.0).rate(Kind::Filter, Rate::Eighth);
        let d = b.finish("t", "Filter");
        let p = WaveCubeParams::default();
        let n = d.params["flo"];
        assert!((p.f_lo.preview_plain(n) - 120.0).abs() < 0.5);
        assert_eq!(p.sh[1].rate.preview_plain(d.params["rate_2"]), Rate::Eighth);
    }

    #[test]
    fn ownership_of_parameters() {
        assert!(belongs_to("amount_2", Kind::Filter));
        assert!(belongs_to("fres", Kind::Filter));
        assert!(!belongs_to("phase_2", Kind::Pan));
        assert!(belongs_to("phaas", Kind::Pan));
        assert!(!belongs_to("gmix", Kind::Volume));
        assert!(!belongs_to("amount_1", Kind::Filter));
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("My / Preset: 1"), "My _ Preset_ 1.json");
        assert_eq!(file_name("  "), "Preset.json");
    }
}
