//! Presets.
//!
//! * A preset is the whole plugin: every parameter (normalized, by id) plus the [`State`]
//!   (chain, slot settings, shapes, custom waves).
//! * An item preset is one chain item: its settings, shapes and knobs. Every kind has a few
//!   factory ones; user item presets can be saved too.
//!
//! User files live in `<data dir>/WaveCube/Presets/*.json` and `Presets/Items/<Kind>/*.json`
//! (Linux `~/.local/share`, Windows `%APPDATA%`, macOS `~/Library/Application Support`).

use crate::kinds::LENGTHS;
use crate::wave::{Preset as W, Pt, Wave};
use crate::{Env, Kind, Lfo, SlotSet, State, WaveCubeParams, N_BANDS, N_KNOBS, N_LANES, N_SLOTS};
use nih_plug::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const CATEGORIES: [&str; 8] = ["Sidechain", "Rhythmic", "Motion", "Color", "Pitch & Time", "Space", "Mixing", "Sound Design"];

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
    /// The kinds in the chain, in order (for the coloured squares and the filter in the browser).
    pub fn kinds(&self) -> Vec<Kind> {
        self.state.chain.iter().map(|&s| self.state.slots[s as usize].kind).collect()
    }
}

/// One chain item.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ItemPreset {
    pub name: String,
    pub kind: Kind,
    pub set: SlotSet,
    /// [lane][band]
    pub waves: Vec<Vec<Wave>>,
    /// normalized knob values
    pub knobs: Vec<f32>,
    pub tiles: [f32; N_LANES],
    pub mix: f32,
    pub band_mix: [f32; N_BANDS],
    #[serde(default)]
    pub user: bool,
}

/// Parameter ids of a slot's controls.
pub fn slot_id(field: &str, slot: usize) -> String {
    format!("{field}_{}", slot + 1)
}

// ------------------------------------------------------------------------------------------
// Building presets in code
// ------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct Builder {
    pub st: State,
    knobs: [[f32; N_KNOBS]; N_SLOTS],
    on: [bool; N_SLOTS],
    tiles: [[f32; N_LANES]; N_SLOTS],
    mix: [f32; N_SLOTS],
    bmix: [[f32; N_BANDS]; N_SLOTS],
    global: BTreeMap<String, f32>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Self { st: State::empty(), knobs: [[0.5; N_KNOBS]; N_SLOTS], on: [true; N_SLOTS], tiles: [[0.0; N_LANES]; N_SLOTS], mix: [1.0; N_SLOTS], bmix: [[1.0; N_BANDS]; N_SLOTS], global: BTreeMap::new() }
    }

    /// Adds an item at the end of the chain; returns its slot.
    pub fn add(&mut self, kind: Kind) -> usize {
        let s = self.st.free_slot().expect("chain full");
        self.st.put(s, kind);
        self.st.chain.push(s as u8);
        for (i, d) in kind.knobs().iter().enumerate() {
            self.knobs[s][i] = d.default_norm();
        }
        s
    }

    /// A knob of the slot by its name, in the kind's units.
    pub fn knob(&mut self, s: usize, name: &str, plain: f32) -> &mut Self {
        let kind = self.st.slots[s].kind;
        match kind.knobs().iter().position(|d| d.name == name) {
            Some(i) => self.knobs[s][i] = kind.knobs()[i].norm(plain),
            None => debug_assert!(false, "{} has no knob {name}", kind.name()),
        }
        self
    }

    pub fn wave(&mut self, s: usize, lane: usize, w: Wave) -> &mut Self {
        self.st.waves[s][lane] = vec![w; N_BANDS];
        self
    }

    pub fn wave_b(&mut self, s: usize, lane: usize, band: usize, w: Wave) -> &mut Self {
        self.st.waves[s][lane][band] = w;
        self
    }

    /// The LFO of a lane, every band.
    pub fn lfo(&mut self, s: usize, lane: usize, f: impl Fn(&mut Lfo)) -> &mut Self {
        for l in self.st.slots[s].lfo[lane].iter_mut() {
            f(l);
        }
        self
    }

    /// Length by name ("1/4", "1 bar", "2 bars", ...), every lane.
    pub fn len(&mut self, s: usize, name: &str) -> &mut Self {
        let i = LENGTHS.iter().position(|&l| l == name).unwrap_or(5) as u8;
        for lane in 0..N_LANES {
            self.lfo(s, lane, |l| l.length = i);
        }
        self
    }

    pub fn env(&mut self, s: usize, lane: usize, f: impl Fn(&mut Env)) -> &mut Self {
        for e in self.st.slots[s].env[lane].iter_mut() {
            f(e);
        }
        self
    }

    pub fn set(&mut self, s: usize, f: impl FnOnce(&mut SlotSet)) -> &mut Self {
        f(&mut self.st.slots[s]);
        self
    }

    pub fn tile(&mut self, s: usize, lane: usize, v: f32) -> &mut Self {
        self.tiles[s][lane] = v;
        self
    }

    pub fn mix(&mut self, s: usize, v: f32) -> &mut Self {
        self.mix[s] = v;
        self
    }

    pub fn band_mix(&mut self, s: usize, b: usize, v: f32) -> &mut Self {
        self.bmix[s][b] = v;
        self
    }

    pub fn off(&mut self, s: usize) -> &mut Self {
        self.on[s] = false;
        self
    }

    /// A global parameter, normalized.
    pub fn global(&mut self, id: &str, norm: f32) -> &mut Self {
        self.global.insert(id.into(), norm);
        self
    }

    pub fn finish(&self, name: &str, category: &str) -> PresetData {
        let mut st = self.st.clone();
        st.preset = name.into();
        st.selected = st.chain.first().copied().unwrap_or(0);
        st.lane = 0;
        st.band = 0;
        st.sanitize();
        let mut params = self.global.clone();
        for s in 0..N_SLOTS {
            params.insert(slot_id("on", s), if self.on[s] { 1.0 } else { 0.0 });
            params.insert(slot_id("ta", s), (self.tiles[s][0] + 1.0) * 0.5);
            params.insert(slot_id("tb", s), (self.tiles[s][1] + 1.0) * 0.5);
            params.insert(slot_id("mix", s), self.mix[s]);
            for (b, f) in ["mxl", "mxm", "mxh"].iter().enumerate() {
                params.insert(slot_id(f, s), self.bmix[s][b]);
            }
            for k in 0..N_KNOBS {
                params.insert(slot_id(&format!("k{k}"), s), self.knobs[s][k]);
            }
        }
        PresetData { name: name.into(), category: category.into(), author: "Factory".into(), params, state: st }
    }

    /// The first item as an item preset.
    pub fn item(&self, name: &str) -> ItemPreset {
        let s = self.st.chain[0] as usize;
        let set = self.st.slots[s];
        ItemPreset {
            name: name.into(),
            kind: set.kind,
            set,
            waves: self.st.waves[s].clone(),
            knobs: self.knobs[s].to_vec(),
            tiles: self.tiles[s],
            mix: self.mix[s],
            band_mix: self.bmix[s],
            user: false,
        }
    }
}

fn wv(p: W) -> Wave {
    Wave::preset(p)
}

fn inv(mut w: Wave) -> Wave {
    w.invert();
    w
}

fn rep(mut w: Wave, n: usize) -> Wave {
    w.repeat(n);
    w
}

fn rev(mut w: Wave) -> Wave {
    w.reverse();
    w
}

fn ramp(a: f32, b: f32, c: f32) -> Wave {
    Wave::from(&[Pt::new(0.0, a, c), Pt::new(1.0, b, 0.0)])
}

fn hash(seed: u32, i: u32) -> f32 {
    let mut r = seed.wrapping_mul(2654435761).wrapping_add(i.wrapping_mul(40503));
    r ^= r >> 13;
    r = r.wrapping_mul(0x5bd1e995);
    r ^= r >> 15;
    (r % 1000) as f32 / 1000.0
}

fn random_steps(seed: u32, n: usize, lo: f32) -> Wave {
    let lv: Vec<f32> = (0..n).map(|i| lo + (1.0 - lo) * ((hash(seed, i as u32) * 4.0).round() / 4.0)).collect();
    Wave::steps(&lv)
}

fn noise_idx(name: &str) -> u16 {
    crate::kinds::NOISES.iter().position(|(_, n)| *n == name).unwrap_or(0) as u16
}

fn reverb_idx(name: &str) -> u16 {
    crate::kinds::REVERBS.iter().position(|(_, n)| *n == name).unwrap_or(5) as u16
}

fn drive_idx(name: &str) -> u8 {
    crate::kinds::DRIVE_STYLES.iter().position(|n| *n == name).unwrap_or(0) as u8
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

    // ---------------- Sidechain
    add("Classic Pump 1/4", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").wave(v, 0, wv(W::Pump));
    });
    add("Hard Duck 1/4", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").wave(v, 0, wv(W::PumpHard));
    });
    add("Soft Pump 1/2", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/2").wave(v, 0, wv(W::Pump)).tile(v, 0, 0.3);
    });
    add("Low-end Duck", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").set(v, |s| {
            s.multiband = true;
            s.x_lo = 150.0;
            s.x_hi = 2500.0;
        });
        b.wave_b(v, 0, 0, wv(W::PumpHard)).wave_b(v, 0, 1, wv(W::Pump)).wave_b(v, 0, 2, Wave::flat(1.0)).band_mix(v, 1, 0.5);
    });
    add("Pump into Glue", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").wave(v, 0, wv(W::Pump));
        let c = b.add(Compressor);
        b.knob(c, "Threshold", -20.0).knob(c, "Ratio", 3.0).knob(c, "Attack", 15.0).knob(c, "Release", 120.0);
    });
    add("Filter Duck", "Sidechain", &|b| {
        let f = b.add(Filter);
        b.len(f, "1/4").wave(f, 0, wv(W::Pump)).set(f, |s| {
            s.f_type = 0;
            s.f_slope = 2;
        });
        b.wave(f, 1, Wave::flat(0.2));
    });
    add("Kick Follower (audio)", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").wave(v, 0, wv(W::PumpHard)).lfo(v, 0, |l| {
            l.trigger = 2;
            l.looping = false;
            l.end = 0.6;
        });
    });
    add("MIDI Duck (play notes)", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").wave(v, 0, wv(W::PumpHard)).lfo(v, 0, |l| {
            l.trigger = 1;
            l.looping = false;
        });
    });
    add("Duck + Width Bloom", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").wave(v, 0, wv(W::Pump));
        let w = b.add(Width);
        b.len(w, "1/4").wave(w, 0, ramp(0.5, 1.0, -0.4));
    });

    // ---------------- Rhythmic
    add("Trance Gate 1/16", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1 bar").wave(v, 0, wv(W::Gate16)).lfo(v, 0, |l| l.smooth = 3.0);
    });
    add("Triplet Gate", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1 bar").wave(v, 0, wv(W::GateTriplet)).lfo(v, 0, |l| l.smooth = 2.5);
    });
    add("Swing Gate", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1 bar").wave(v, 0, wv(W::Gate16)).lfo(v, 0, |l| {
            l.smooth = 2.5;
            l.swing = 0.6;
        });
    });
    add("Chop & Pan", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/2").wave(v, 0, wv(W::Gate8));
        let p = b.add(Pan);
        b.len(p, "1/4").wave(p, 0, wv(W::Square)).lfo(p, 0, |l| l.smooth = 4.0).tile(p, 0, 0.0);
    });
    add("Bouncing Ball", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1 bar").wave(v, 0, wv(W::Bounce));
    });
    add("Stereo Tremolo", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/8").wave(v, 0, wv(W::Sine)).tile(v, 0, 0.2).lfo(v, 0, |l| l.spread = 180.0);
    });
    add("Random Steps", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1 bar").wave(v, 0, random_steps(7, 16, 0.0)).lfo(v, 0, |l| l.smooth = 2.0);
    });
    add("Gated Reverb Chops", "Rhythmic", &|b| {
        let r = b.add(Reverb);
        b.len(r, "1 bar").wave(r, 0, wv(W::Gate8)).wave(r, 1, wv(W::Gate16)).set(r, |s| s.r_type = reverb_idx("Studio room"));
        b.knob(r, "Wet", 0.6);
    });
    add("Double Gate (two Volumes)", "Rhythmic", &|b| {
        let a = b.add(Volume);
        b.len(a, "1/4").wave(a, 0, wv(W::Pump));
        let c = b.add(Volume);
        b.len(c, "1 bar").wave(c, 0, wv(W::GateTriplet)).lfo(c, 0, |l| l.smooth = 3.0).tile(c, 0, 0.25);
    });

    // ---------------- Motion
    add("Wobble 1/8", "Motion", &|b| {
        let f = b.add(Filter);
        b.len(f, "1/8").wave(f, 0, wv(W::Sine)).wave(f, 1, Wave::flat(0.55)).set(f, |s| s.f_slope = 2);
    });
    add("Slow Sweep 4 bars", "Motion", &|b| {
        let f = b.add(Filter);
        b.len(f, "4 bars").wave(f, 0, ramp(0.0, 1.0, 0.35)).wave(f, 1, Wave::flat(0.35));
    });
    add("Riser Sweep 8 bars", "Motion", &|b| {
        let f = b.add(Filter);
        b.len(f, "8 bars").set(f, |s| s.f_type = 1).wave(f, 0, ramp(0.0, 0.9, 0.3)).wave(f, 1, ramp(0.2, 0.6, 0.0));
        let n = b.add(Noise);
        b.len(n, "8 bars").wave(n, 0, ramp(0.0, 1.0, 0.5)).wave(n, 1, ramp(0.2, 1.0, 0.0)).set(n, |s| s.n_type = noise_idx("White"));
        let w = b.add(Width);
        b.len(w, "8 bars").wave(w, 0, ramp(0.5, 1.0, 0.0));
    });
    add("Auto-pan 1/4", "Motion", &|b| {
        let p = b.add(Pan);
        b.len(p, "1/4").wave(p, 0, wv(W::Sine));
    });
    add("Haas Swirl", "Motion", &|b| {
        let p = b.add(Pan);
        b.len(p, "1 bar").wave(p, 0, wv(W::Sine)).knob(p, "Pan / Haas", 1.0).knob(p, "Haas range", 15.0);
    });
    add("Breathing Width", "Motion", &|b| {
        let w = b.add(Width);
        b.len(w, "2 bars").wave(w, 0, wv(W::Sine));
    });
    add("Envelope Funk", "Motion", &|b| {
        let f = b.add(Filter);
        b.wave(f, 0, Wave::flat(0.3)).wave(f, 1, Wave::flat(0.55)).set(f, |s| s.f_type = 2);
        b.env(f, 0, |e| {
            e.on = true;
            e.amount = 0.45;
            e.attack = 2.0;
            e.release = 120.0;
            e.thresh = -36.0;
        });
    });
    add("Jet Flanger", "Motion", &|b| {
        let l = b.add(Liquid);
        b.len(l, "2 bars").set(l, |s| s.l_type = 1).wave(l, 0, wv(W::Triangle)).wave(l, 1, Wave::flat(0.75));
    });
    add("Phaser Drift", "Motion", &|b| {
        let l = b.add(Liquid);
        b.len(l, "1 bar").set(l, |s| {
            s.l_type = 2;
            s.l_stages = 3;
        });
        b.wave(l, 0, wv(W::Sine)).wave(l, 1, Wave::flat(0.6));
    });
    add("Stereo Sweep", "Motion", &|b| {
        let f = b.add(Filter);
        b.len(f, "1 bar").wave(f, 0, wv(W::Triangle)).wave(f, 1, Wave::flat(0.4)).knob(f, "Stereo", 24.0);
    });
    add("Band Movers", "Motion", &|b| {
        let p = b.add(Pan);
        b.set(p, |s| {
            s.multiband = true;
            s.link = [false, false];
        });
        b.wave_b(p, 0, 0, Wave::flat(0.5)).wave_b(p, 0, 1, wv(W::Sine)).wave_b(p, 0, 2, inv(wv(W::Sine)));
        b.lfo(p, 0, |l| l.length = 6);
    });

    // ---------------- Color
    add("Pumping Drive", "Color", &|b| {
        let d = b.add(Drive);
        b.len(d, "1/4").wave(d, 0, inv(wv(W::Pump))).knob(d, "Input", 12.0).set(d, |s| s.d_style = drive_idx("Smooth clip"));
    });
    add("Two Drives (soft into fold)", "Color", &|b| {
        let a = b.add(Drive);
        b.len(a, "1/4").wave(a, 0, Wave::flat(0.6)).knob(a, "Input", 8.0).set(a, |s| s.d_style = drive_idx("Soft clip"));
        let c = b.add(Drive);
        b.len(c, "1 bar").wave(c, 0, wv(W::Triangle)).knob(c, "Input", 4.0).set(c, |s| s.d_style = drive_idx("Sine fold")).mix(c, 0.5);
    });
    add("Bass Grit (multiband)", "Color", &|b| {
        let d = b.add(Drive);
        b.set(d, |s| {
            s.multiband = true;
            s.x_lo = 120.0;
            s.d_style = drive_idx("Hard clip");
        });
        b.wave_b(d, 0, 0, Wave::flat(0.0)).wave_b(d, 0, 1, Wave::flat(0.7)).wave_b(d, 0, 2, Wave::flat(0.35)).knob(d, "Input", 10.0);
    });
    add("Bitcrush Steps", "Color", &|b| {
        let c = b.add(Crush);
        b.len(c, "1 bar").wave(c, 0, random_steps(3, 8, 0.3)).wave(c, 1, Wave::flat(0.85));
    });
    add("Lo-fi Radio", "Color", &|b| {
        let c = b.add(Crush);
        b.wave(c, 0, Wave::flat(0.65)).wave(c, 1, Wave::flat(0.55)).knob(c, "Treble", 0.4);
        let f = b.add(Filter);
        b.set(f, |s| {
            s.f_type = 2;
            s.f_warm = true;
        });
        b.wave(f, 0, Wave::flat(0.55)).wave(f, 1, Wave::flat(0.35));
        let n = b.add(Noise);
        b.wave(n, 0, Wave::flat(0.3)).set(n, |s| s.n_type = noise_idx("Radio static")).knob(n, "Trim", -24.0);
    });
    add("Vinyl Warmth", "Color", &|b| {
        let n = b.add(Noise);
        b.wave(n, 0, Wave::flat(0.45)).set(n, |s| s.n_type = noise_idx("Vinyl crackle")).knob(n, "Trim", -20.0);
        let d = b.add(Drive);
        b.wave(d, 0, Wave::flat(0.3)).set(d, |s| s.d_style = drive_idx("Soft clip")).knob(d, "Tone", -0.4);
    });
    add("Warm Filter Squelch", "Color", &|b| {
        let f = b.add(Filter);
        b.len(f, "1/8").set(f, |s| {
            s.f_warm = true;
            s.f_slope = 2;
        });
        b.wave(f, 0, wv(W::Pump)).wave(f, 1, Wave::flat(0.75)).knob(f, "Dist", 0.6);
    });
    add("Crunch Accents", "Color", &|b| {
        let d = b.add(Drive);
        b.len(d, "1 bar").wave(d, 0, wv(W::Gate8)).set(d, |s| s.d_style = drive_idx("Hard square")).knob(d, "Accent", 0.6).mix(d, 0.7);
    });

    // ---------------- Pitch & Time
    add("Tape Stop", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.len(t, "1 bar").wave(t, 0, wv(W::TapeStop));
    });
    add("Tape Start", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.len(t, "1 bar").wave(t, 0, wv(W::TapeStart));
    });
    add("Half-time Feel", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.len(t, "1 bar").wave(t, 0, wv(W::HalfTime));
    });
    add("Stutter Roll", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.len(t, "1 bar").wave(t, 0, wv(W::Stutter)).lfo(t, 0, |l| l.smooth = 0.5);
    });
    add("Scratch", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.len(t, "1/2").wave(t, 0, wv(W::Scratch)).set(t, |s| s.t_instant = false);
    });
    add("Vibrato (10 ms)", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.set(t, |s| s.t_range = 6).lfo(t, 0, |l| {
            l.mode = 1;
            l.hz = 5.5;
        });
        b.wave(t, 0, Wave::from(&[Pt::new(0.0, 0.8, -0.42), Pt::new(0.5, 1.0, 0.42), Pt::new(1.0, 0.8, 0.0)]));
    });
    add("Chorus Wobble", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.set(t, |s| s.t_range = 7).lfo(t, 0, |l| {
            l.mode = 1;
            l.hz = 0.7;
            l.spread = 120.0;
        });
        b.wave(t, 0, wv(W::Sine)).mix(t, 0.5);
    });
    add("Pitched Comb (MIDI)", "Pitch & Time", &|b| {
        let l = b.add(Liquid);
        b.set(l, |s| s.l_type = 1).lfo(l, 0, |f| f.mode = 2);
        b.wave(l, 0, Wave::flat(0.6)).wave(l, 1, Wave::flat(0.85));
    });

    // ---------------- Space
    add("Swelling Tails", "Space", &|b| {
        let r = b.add(Reverb);
        b.len(r, "1 bar").wave(r, 0, wv(W::Swell)).wave(r, 1, Wave::flat(1.0)).set(r, |s| s.r_type = reverb_idx("Concert hall"));
        b.knob(r, "Wet", 0.5);
    });
    add("Ducked Reverb", "Space", &|b| {
        let r = b.add(Reverb);
        b.len(r, "1/4").wave(r, 0, Wave::flat(1.0)).wave(r, 1, wv(W::Pump)).set(r, |s| s.r_type = reverb_idx("Vocal plate"));
        b.knob(r, "Wet", 0.55);
    });
    add("Shimmer Bloom", "Space", &|b| {
        let r = b.add(Reverb);
        b.len(r, "2 bars").wave(r, 0, ramp(0.0, 1.0, 0.4)).wave(r, 1, Wave::flat(1.0)).set(r, |s| s.r_type = reverb_idx("Shimmer cloud"));
        b.knob(r, "Wet", 0.6).knob(r, "Decay", 0.8);
    });
    add("Cathedral Throw", "Space", &|b| {
        let r = b.add(Reverb);
        b.len(r, "1 bar").wave(r, 0, Wave::steps(&[0.0, 0.0, 0.0, 1.0])).set(r, |s| s.r_type = reverb_idx("Cathedral"));
        b.knob(r, "Wet", 0.7);
    });
    add("Gated 80s", "Space", &|b| {
        let r = b.add(Reverb);
        b.set(r, |s| {
            s.r_type = reverb_idx("Gated");
            s.r_gated = true;
        });
        b.wave(r, 0, Wave::flat(1.0)).wave(r, 1, Wave::flat(1.0)).knob(r, "Wet", 0.45);
    });
    add("Space Pan", "Space", &|b| {
        let p = b.add(Pan);
        b.len(p, "1/2").wave(p, 0, wv(W::Sine));
        let r = b.add(Reverb);
        b.len(r, "1/4").wave(r, 0, wv(W::Gate8)).set(r, |s| s.r_type = reverb_idx("Small room")).knob(r, "Wet", 0.35);
    });

    // ---------------- Mixing
    add("Mono Bass", "Mixing", &|b| {
        let w = b.add(Width);
        b.set(w, |s| {
            s.multiband = true;
            s.x_lo = 150.0;
        });
        b.wave_b(w, 0, 0, Wave::flat(0.0)).wave_b(w, 0, 1, Wave::flat(0.5)).wave_b(w, 0, 2, Wave::flat(0.5));
    });
    add("Wide Highs", "Mixing", &|b| {
        let w = b.add(Width);
        b.set(w, |s| {
            s.multiband = true;
            s.x_hi = 4000.0;
        });
        b.wave_b(w, 0, 0, Wave::flat(0.35)).wave_b(w, 0, 1, Wave::flat(0.5)).wave_b(w, 0, 2, Wave::flat(0.8));
    });
    add("De-mud Envelope", "Mixing", &|b| {
        let v = b.add(Volume);
        b.set(v, |s| {
            s.multiband = true;
            s.x_lo = 180.0;
            s.x_hi = 600.0;
        });
        b.wave_b(v, 0, 0, Wave::flat(1.0)).wave_b(v, 0, 1, Wave::flat(0.75)).wave_b(v, 0, 2, Wave::flat(1.0));
    });
    add("Multiband Glue", "Mixing", &|b| {
        let c = b.add(Compressor);
        b.set(c, |s| {
            s.multiband = true;
            s.comp_adaptive = true;
        });
        b.knob(c, "Threshold", -24.0).knob(c, "Ratio", 2.5).knob(c, "Attack", 20.0).knob(c, "Release", 200.0);
        b.add(Scope);
    });
    add("Fade In / Fade Out", "Mixing", &|b| {
        let v = b.add(Volume);
        b.len(v, "4 bars").wave(v, 0, wv(W::TrimIn)).lfo(v, 0, |l| l.looping = false);
    });
    add("Safe Limit", "Mixing", &|b| {
        let c = b.add(Compressor);
        b.knob(c, "Threshold", -6.0).knob(c, "Ratio", 100.0).knob(c, "Attack", 0.5).knob(c, "Release", 80.0);
    });

    // ---------------- Sound Design
    add("Hats from Noise", "Sound Design", &|b| {
        let n = b.add(Noise);
        b.len(n, "1/8").wave(n, 0, rep(wv(W::PumpHard), 1)).set(n, |s| {
            s.n_type = noise_idx("Metal 808");
            s.n_only = false;
        });
        b.wave(n, 0, inv(wv(W::Pump))).knob(n, "High-pass", 6000.0).knob(n, "Trim", -12.0);
    });
    add("Riser Noise 4 bars", "Sound Design", &|b| {
        let n = b.add(Noise);
        b.len(n, "4 bars").wave(n, 0, ramp(0.0, 1.0, 0.45)).wave(n, 1, ramp(0.1, 1.0, 0.0)).set(n, |s| s.n_type = noise_idx("White"));
        b.knob(n, "High-pass", 400.0);
    });
    add("Rain Pad", "Sound Design", &|b| {
        let n = b.add(Noise);
        b.wave(n, 0, Wave::flat(0.6)).set(n, |s| {
            s.n_type = noise_idx("Rain");
            s.n_env = 1;
        });
        b.knob(n, "Trim", -14.0);
        let r = b.add(Reverb);
        b.wave(r, 0, Wave::flat(0.7)).set(r, |s| s.r_type = reverb_idx("Endless")).knob(r, "Wet", 0.5);
    });
    add("Glitch Machine", "Sound Design", &|b| {
        let t = b.add(Time);
        b.len(t, "1 bar").wave(t, 0, random_steps(11, 8, 0.4)).lfo(t, 0, |l| l.smooth = 0.3);
        let c = b.add(Crush);
        b.len(c, "1/2").wave(c, 0, random_steps(5, 8, 0.2)).wave(c, 1, random_steps(9, 8, 0.4));
        let p = b.add(Pan);
        b.len(p, "1/4").wave(p, 0, random_steps(13, 8, 0.0));
    });
    add("Robot Voice", "Sound Design", &|b| {
        let l = b.add(Liquid);
        b.set(l, |s| s.l_type = 1).wave(l, 0, Wave::flat(0.45)).wave(l, 1, Wave::flat(0.9));
        let c = b.add(Crush);
        b.wave(c, 0, Wave::flat(0.55)).wave(c, 1, Wave::flat(0.7));
    });
    add("Underwater", "Sound Design", &|b| {
        let f = b.add(Filter);
        b.len(f, "2 bars").set(f, |s| s.f_warm = true).wave(f, 0, rev(ramp(0.15, 0.35, 0.0))).wave(f, 1, Wave::flat(0.5));
        let l = b.add(Liquid);
        b.len(l, "1 bar").wave(l, 0, wv(W::Sine)).wave(l, 1, Wave::flat(0.5));
    });
    add("Everything Chain", "Sound Design", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").wave(v, 0, wv(W::Pump));
        let f = b.add(Filter);
        b.len(f, "2 bars").wave(f, 0, wv(W::Sine)).wave(f, 1, Wave::flat(0.4));
        let d = b.add(Drive);
        b.len(d, "1 bar").wave(d, 0, wv(W::Triangle));
        let p = b.add(Pan);
        b.len(p, "1/2").wave(p, 0, wv(W::Sine));
        let r = b.add(Reverb);
        b.len(r, "1/4").wave(r, 0, wv(W::Gate8)).knob(r, "Wet", 0.3);
        b.add(Scope);
    });

    // ================= more of everything
    // ---- Sidechain
    add("Pump 1/8", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/8").wave(v, 0, wv(W::Pump));
    });
    add("Pump Dotted 1/4", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").lfo(v, 0, |l| l.feel = 1).wave(v, 0, wv(W::Pump));
    });
    add("Offbeat Pump", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").lfo(v, 0, |l| l.phase = 180.0).wave(v, 0, wv(W::Pump));
    });
    add("Triplet Duck", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").lfo(v, 0, |l| l.feel = 2).wave(v, 0, wv(W::PumpHard));
    });
    add("Real Sidechain Duck (SC input)", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.wave(v, 0, Wave::flat(1.0)).env(v, 0, |e| {
            e.on = true;
            e.input = 2;
            e.amount = -1.6;
            e.depth = 1;
            e.attack = 1.0;
            e.release = 180.0;
            e.adaptive = true;
            e.thresh = -36.0;
        });
    });
    add("Vocal Pocket (mid duck)", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").set(v, |s| {
            s.multiband = true;
            s.x_lo = 300.0;
            s.x_hi = 4000.0;
        });
        b.wave_b(v, 0, 0, Wave::flat(1.0)).wave_b(v, 0, 1, wv(W::Pump)).wave_b(v, 0, 2, Wave::flat(1.0)).tile(v, 0, 0.35);
    });
    add("Pump + Opening Filter", "Sidechain", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").wave(v, 0, wv(W::Pump));
        let f = b.add(Filter);
        b.len(f, "1/4").wave(f, 0, ramp(0.35, 1.0, -0.5)).wave(f, 1, Wave::flat(0.3));
    });
    add("Ducked Wash", "Sidechain", &|b| {
        let r = b.add(Reverb);
        b.len(r, "1/4").wave(r, 0, Wave::flat(1.0)).wave(r, 1, wv(W::PumpHard)).set(r, |s| s.r_type = reverb_idx("Concert hall")).knob(r, "Wet", 0.7).knob(r, "Decay", 0.75);
    });

    // ---- Rhythmic
    add("Dotted Gate", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1 bar").wave(v, 0, Wave::steps(&[1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0])).lfo(v, 0, |l| l.smooth = 2.5);
    });
    add("Accent Pattern", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/2").wave(v, 0, Wave::steps(&[1.0, 0.4, 0.65, 0.4, 0.9, 0.4, 0.7, 0.45])).lfo(v, 0, |l| l.smooth = 4.0);
    });
    add("Swing Chopper", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/2").wave(v, 0, rep(wv(W::Gate8), 2)).lfo(v, 0, |l| {
            l.swing = 0.75;
            l.smooth = 2.0;
        });
    });
    add("Ping-pong 1/16", "Rhythmic", &|b| {
        let p = b.add(Pan);
        b.len(p, "1/8").wave(p, 0, wv(W::Square)).lfo(p, 0, |l| l.smooth = 3.0);
    });
    add("Filter Gate", "Rhythmic", &|b| {
        let f = b.add(Filter);
        b.len(f, "1 bar").wave(f, 0, Wave::steps(&[0.9, 0.3, 0.75, 0.3, 0.9, 0.9, 0.3, 0.6, 0.9, 0.3, 0.75, 0.3, 0.9, 0.3, 0.6, 0.3])).wave(f, 1, Wave::flat(0.55)).lfo(f, 0, |l| l.smooth = 3.0);
    });
    add("Crush Rhythm", "Rhythmic", &|b| {
        let c = b.add(Crush);
        b.len(c, "1 bar").wave(c, 0, inv(wv(W::Gate8))).wave(c, 1, Wave::steps(&[1.0, 0.5, 1.0, 0.35, 1.0, 0.6, 1.0, 0.4]));
        b.mix(c, 0.8);
    });
    add("Polyrhythm 3 over 4", "Rhythmic", &|b| {
        let a = b.add(Volume);
        b.len(a, "1/4").wave(a, 0, wv(W::Pump));
        let c = b.add(Volume);
        b.len(c, "1/4").lfo(c, 0, |l| l.feel = 2).wave(c, 0, wv(W::Sine)).tile(c, 0, 0.4);
    });
    add("Triplet Stereo Gate", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1 bar").wave(v, 0, wv(W::GateTriplet)).lfo(v, 0, |l| {
            l.spread = 90.0;
            l.smooth = 2.5;
        });
    });
    add("Broken Beat Gate", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "2 bars").wave(v, 0, random_steps(21, 32, 0.0)).lfo(v, 0, |l| l.smooth = 2.0);
    });
    add("Gate + Width Burst", "Rhythmic", &|b| {
        let v = b.add(Volume);
        b.len(v, "1 bar").wave(v, 0, wv(W::Gate16)).lfo(v, 0, |l| l.smooth = 3.0);
        let w = b.add(Width);
        b.len(w, "1 bar").wave(w, 0, Wave::steps(&[0.5, 0.5, 0.5, 1.0, 0.5, 0.5, 1.0, 1.0]));
    });

    // ---- Motion
    add("Auto-wah (envelope)", "Motion", &|b| {
        let f = b.add(Filter);
        b.set(f, |s| {
            s.f_type = 2;
            s.f_slope = 1;
        });
        b.wave(f, 0, Wave::flat(0.3)).wave(f, 1, Wave::flat(0.7)).env(f, 0, |e| {
            e.on = true;
            e.input = 1;
            e.hp = 150.0;
            e.amount = 0.45;
            e.attack = 3.0;
            e.release = 90.0;
            e.thresh = -40.0;
        });
    });
    add("Slow Phaser 4 bars", "Motion", &|b| {
        let l = b.add(Liquid);
        b.len(l, "4 bars").set(l, |s| {
            s.l_type = 2;
            s.l_stages = 2;
        });
        b.wave(l, 0, wv(W::Triangle)).wave(l, 1, Wave::flat(0.5));
    });
    add("Flanger Rise", "Motion", &|b| {
        let l = b.add(Liquid);
        b.len(l, "2 bars").set(l, |s| s.l_type = 1).wave(l, 0, ramp(0.0, 1.0, 0.3)).wave(l, 1, Wave::flat(0.85));
    });
    add("Resonant Saw Sweep", "Motion", &|b| {
        let f = b.add(Filter);
        b.len(f, "1/2").set(f, |s| s.f_slope = 2).wave(f, 0, rev(ramp(0.2, 0.95, 0.4))).wave(f, 1, Wave::flat(0.82));
    });
    add("MIDI Filter Pluck", "Motion", &|b| {
        let f = b.add(Filter);
        b.len(f, "1/4").set(f, |s| s.f_warm = true).wave(f, 0, rev(ramp(0.15, 1.0, -0.6))).wave(f, 1, Wave::flat(0.6)).lfo(f, 0, |l| {
            l.trigger = 1;
            l.looping = false;
        });
    });
    add("Rotary Speaker", "Motion", &|b| {
        let p = b.add(Pan);
        b.wave(p, 0, Wave::from(&[Pt::new(0.0, 0.25, -0.42), Pt::new(0.25, 0.5, 0.42), Pt::new(0.5, 0.75, -0.42), Pt::new(0.75, 0.5, 0.42), Pt::new(1.0, 0.25, 0.0)])).lfo(p, 0, |l| {
            l.mode = 1;
            l.hz = 6.2;
        });
        let v = b.add(Volume);
        b.wave(v, 0, wv(W::Sine)).tile(v, 0, 0.75).lfo(v, 0, |l| {
            l.mode = 1;
            l.hz = 6.2;
            l.phase = 90.0;
        });
        let t = b.add(Time);
        b.set(t, |s| s.t_range = 5).wave(t, 0, wv(W::Sine)).lfo(t, 0, |l| {
            l.mode = 1;
            l.hz = 6.2;
        });
    });
    add("Width Pulse", "Motion", &|b| {
        let w = b.add(Width);
        b.len(w, "1/4").wave(w, 0, Wave::from(&[Pt::new(0.0, 1.0, 0.5), Pt::new(0.6, 0.3, 0.0), Pt::new(1.0, 0.3, 0.0)]));
    });
    add("Talking Filter", "Motion", &|b| {
        let f = b.add(Filter);
        b.len(f, "1 bar").set(f, |s| s.f_type = 2).wave(f, 0, random_steps(4, 8, 0.25)).wave(f, 1, Wave::flat(0.75)).lfo(f, 0, |l| l.smooth = 25.0);
    });
    add("Stereo Phaser Wide", "Motion", &|b| {
        let l = b.add(Liquid);
        b.len(l, "2 bars").set(l, |s| {
            s.l_type = 2;
            s.l_stages = 3;
        });
        b.wave(l, 0, wv(W::Sine)).wave(l, 1, Wave::flat(0.7)).knob(l, "Stereo", 36.0).lfo(l, 0, |x| x.spread = 120.0);
    });
    add("Haas Ping", "Motion", &|b| {
        let p = b.add(Pan);
        b.len(p, "1/4").wave(p, 0, wv(W::Square)).knob(p, "Pan / Haas", 1.0).knob(p, "Haas range", 12.0).lfo(p, 0, |l| l.smooth = 5.0);
    });

    // ---- Color
    add("Tape Saturation", "Color", &|b| {
        let d = b.add(Drive);
        b.wave(d, 0, Wave::flat(0.22)).set(d, |s| s.d_style = drive_idx("Smooth clip")).knob(d, "Tone", -0.35).knob(d, "Push", 0.15);
    });
    add("Fuzz Box", "Color", &|b| {
        let d = b.add(Drive);
        b.wave(d, 0, Wave::flat(0.65)).set(d, |s| s.d_style = drive_idx("Hard square")).knob(d, "Output", -10.0).knob(d, "Tone", 0.3);
    });
    add("Rectifier Pump", "Color", &|b| {
        let d = b.add(Drive);
        b.len(d, "1/4").wave(d, 0, inv(wv(W::Pump))).set(d, |s| s.d_style = drive_idx("Soft rectify")).mix(d, 0.7);
    });
    add("8-bit Console", "Color", &|b| {
        let c = b.add(Crush);
        b.wave(c, 0, Wave::flat(0.47)).wave(c, 1, Wave::flat(0.62)).knob(c, "Treble", 0.7);
    });
    add("Telephone", "Color", &|b| {
        let f = b.add(Filter);
        b.set(f, |s| {
            s.f_type = 2;
            s.f_slope = 2;
        });
        b.wave(f, 0, Wave::flat(0.66)).wave(f, 1, Wave::flat(0.25));
        let d = b.add(Drive);
        b.wave(d, 0, Wave::flat(0.3)).set(d, |s| s.d_style = drive_idx("Hard clip"));
        let w = b.add(Width);
        b.wave(w, 0, Wave::flat(0.0));
    });
    add("Warm Bus", "Color", &|b| {
        let d = b.add(Drive);
        b.wave(d, 0, Wave::flat(0.12)).set(d, |s| s.d_style = drive_idx("Smooth clip"));
        let c = b.add(Compressor);
        b.knob(c, "Threshold", -18.0).knob(c, "Ratio", 1.8).knob(c, "Attack", 25.0).knob(c, "Release", 250.0);
    });
    add("Parallel Crush", "Color", &|b| {
        let c = b.add(Crush);
        b.wave(c, 0, Wave::flat(0.3)).wave(c, 1, Wave::flat(0.5)).knob(c, "Crunch", 9.0).mix(c, 0.35);
    });
    add("Low-end Saturator", "Color", &|b| {
        let d = b.add(Drive);
        b.set(d, |s| {
            s.multiband = true;
            s.x_lo = 150.0;
            s.d_style = drive_idx("Soft clip");
        });
        b.wave_b(d, 0, 0, Wave::flat(0.45)).wave_b(d, 0, 1, Wave::flat(0.0)).wave_b(d, 0, 2, Wave::flat(0.0)).knob(d, "Input", 6.0);
    });
    add("High Exciter", "Color", &|b| {
        let d = b.add(Drive);
        b.set(d, |s| {
            s.multiband = true;
            s.x_hi = 5000.0;
            s.d_style = drive_idx("Soft rectify");
        });
        b.wave_b(d, 0, 0, Wave::flat(0.0)).wave_b(d, 0, 1, Wave::flat(0.0)).wave_b(d, 0, 2, Wave::flat(0.35)).band_mix(d, 2, 0.5);
    });
    add("Dirty Wobble", "Color", &|b| {
        let f = b.add(Filter);
        b.len(f, "1/8").set(f, |s| {
            s.f_warm = true;
            s.f_slope = 2;
        });
        b.wave(f, 0, wv(W::Sine)).wave(f, 1, Wave::flat(0.7)).knob(f, "Dist", 0.8);
        let d = b.add(Drive);
        b.wave(d, 0, Wave::flat(0.35)).set(d, |s| s.d_style = drive_idx("Skew fold")).mix(d, 0.5);
    });

    // ---- Pitch & Time
    add("Tape Stop 1/2", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.len(t, "1/2").wave(t, 0, wv(W::TapeStop));
    });
    let repeat = |n: usize| {
        let mut pts = Vec::new();
        for k in 0..n {
            pts.push(Pt::new(k as f32 / n as f32, 1.0, 0.0));
            pts.push(Pt::new((k + 1) as f32 / n as f32, 1.0 - 1.0 / n as f32, 0.0));
        }
        Wave::from(&pts)
    };
    add("Beat Repeat 1/8", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.len(t, "1 bar").wave(t, 0, repeat(8));
    });
    add("Beat Repeat 1/16", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.len(t, "1/2").wave(t, 0, repeat(8));
    });
    add("Repeat on the 4", "Pitch & Time", &|b| {
        let t = b.add(Time);
        let pts = [Pt::new(0.0, 1.0, 0.0), Pt::new(0.75, 1.0, 0.0), Pt::new(0.75, 1.0, 0.0), Pt::new(0.8125, 0.9375, 0.0), Pt::new(0.8125, 1.0, 0.0), Pt::new(0.875, 0.9375, 0.0), Pt::new(0.875, 1.0, 0.0), Pt::new(0.9375, 0.9375, 0.0), Pt::new(0.9375, 1.0, 0.0), Pt::new(1.0, 0.9375, 0.0)];
        b.len(t, "1 bar").wave(t, 0, Wave::from(&pts));
    });
    add("Vinyl Wow", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.set(t, |s| {
            s.t_range = 7;
            s.t_instant = false;
        });
        b.wave(t, 0, wv(W::Sine)).lfo(t, 0, |l| {
            l.mode = 1;
            l.hz = 0.55;
        });
        let n = b.add(Noise);
        b.wave(n, 0, Wave::flat(0.4)).set(n, |s| s.n_type = noise_idx("Vinyl crackle")).knob(n, "Trim", -22.0);
        let f = b.add(Filter);
        b.wave(f, 0, Wave::flat(0.82)).wave(f, 1, Wave::flat(0.2));
    });
    add("Pitch Drop (MIDI)", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.len(t, "1/2").wave(t, 0, wv(W::TapeStop)).set(t, |s| s.t_instant = false).lfo(t, 0, |l| {
            l.trigger = 1;
            l.looping = false;
        });
    });
    add("Doubler", "Pitch & Time", &|b| {
        let t = b.add(Time);
        b.set(t, |s| {
            s.t_range = 7;
            s.t_instant = false;
        });
        b.wave(t, 0, Wave::flat(0.4)).lfo(t, 0, |l| {
            l.mode = 1;
            l.hz = 0.3;
            l.spread = 90.0;
        });
        b.mix(t, 0.5);
    });
    add("Random Repeats", "Pitch & Time", &|b| {
        let t = b.add(Time);
        let lv: Vec<f32> = (0..16).map(|i| if hash(31, i) > 0.6 { 0.9375 } else { 1.0 }).collect();
        b.len(t, "1 bar").wave(t, 0, Wave::steps(&lv));
    });

    // ---- Space
    add("Plate on 2 and 4", "Space", &|b| {
        let r = b.add(Reverb);
        b.len(r, "1 bar").wave(r, 0, Wave::steps(&[0.0, 1.0, 0.0, 1.0])).set(r, |s| s.r_type = reverb_idx("Realistic plate")).knob(r, "Wet", 0.55);
    });
    add("Spring Slap", "Space", &|b| {
        let r = b.add(Reverb);
        b.wave(r, 0, Wave::flat(0.8)).wave(r, 1, Wave::flat(1.0)).set(r, |s| s.r_type = reverb_idx("Spring")).knob(r, "Wet", 0.4);
    });
    add("Metal Tank Chops", "Space", &|b| {
        let r = b.add(Reverb);
        b.len(r, "1 bar").wave(r, 0, wv(W::Gate16)).set(r, |s| s.r_type = reverb_idx("Metal tank")).knob(r, "Wet", 0.5);
    });
    add("Modulated Pad Verb", "Space", &|b| {
        let r = b.add(Reverb);
        b.wave(r, 0, Wave::flat(1.0)).wave(r, 1, Wave::flat(1.0)).set(r, |s| s.r_type = reverb_idx("Modulated")).knob(r, "Wet", 0.6).knob(r, "Decay", 0.8).knob(r, "Width", 1.6);
    });
    add("Endless Wash", "Space", &|b| {
        let r = b.add(Reverb);
        b.wave(r, 0, Wave::flat(0.7)).wave(r, 1, Wave::flat(1.0)).set(r, |s| s.r_type = reverb_idx("Endless")).knob(r, "Wet", 0.55).knob(r, "Dry", 0.7);
    });
    add("Small Room Width", "Space", &|b| {
        let r = b.add(Reverb);
        b.wave(r, 0, Wave::flat(0.6)).set(r, |s| s.r_type = reverb_idx("Small room")).knob(r, "Wet", 0.3);
        let w = b.add(Width);
        b.wave(w, 0, Wave::flat(0.68));
    });
    add("Reverse Swell", "Space", &|b| {
        let r = b.add(Reverb);
        b.len(r, "1/2").wave(r, 0, ramp(0.0, 1.0, 0.5)).wave(r, 1, ramp(0.0, 1.0, 0.6)).set(r, |s| s.r_type = reverb_idx("Bright hall")).knob(r, "Dry", 0.0).knob(r, "Wet", 0.9);
        b.mix(r, 0.6);
    });
    add("Sidechained Chamber", "Space", &|b| {
        let r = b.add(Reverb);
        b.wave(r, 0, Wave::flat(1.0)).wave(r, 1, Wave::flat(1.0)).set(r, |s| s.r_type = reverb_idx("Chamber")).knob(r, "Wet", 0.6);
        b.env(r, 1, |e| {
            e.on = true;
            e.input = 0;
            e.amount = -1.4;
            e.release = 250.0;
            e.adaptive = true;
        });
    });

    // ---- Mixing
    add("Transient Softener", "Mixing", &|b| {
        let v = b.add(Volume);
        b.wave(v, 0, Wave::flat(1.0)).env(v, 0, |e| {
            e.on = true;
            e.amount = -0.5;
            e.attack = 0.3;
            e.release = 40.0;
            e.thresh = -24.0;
        });
    });
    add("Expander", "Mixing", &|b| {
        let v = b.add(Volume);
        b.wave(v, 0, Wave::flat(1.0)).env(v, 0, |e| {
            e.on = true;
            e.multiply = true;
            e.amount = 1.0;
            e.shift = 0.15;
            e.release = 120.0;
            e.thresh = -45.0;
        });
    });
    add("Stereo Imager", "Mixing", &|b| {
        let w = b.add(Width);
        b.set(w, |s| {
            s.multiband = true;
            s.x_lo = 180.0;
            s.x_hi = 3500.0;
        });
        b.wave_b(w, 0, 0, Wave::flat(0.15)).wave_b(w, 0, 1, Wave::flat(0.55)).wave_b(w, 0, 2, Wave::flat(0.75));
    });
    add("Mono Maker", "Mixing", &|b| {
        let w = b.add(Width);
        b.wave(w, 0, Wave::flat(0.0));
    });
    add("Gentle Glue", "Mixing", &|b| {
        let c = b.add(Compressor);
        b.knob(c, "Threshold", -16.0).knob(c, "Ratio", 1.5).knob(c, "Attack", 30.0).knob(c, "Release", 300.0);
    });
    add("Vocal Ducker (SC)", "Mixing", &|b| {
        let c = b.add(Compressor);
        b.set(c, |s| {
            s.comp_input = 2;
            s.comp_auto = false;
            s.multiband = true;
        });
        b.knob(c, "Threshold", -30.0).knob(c, "Ratio", 3.0).knob(c, "Attack", 5.0).knob(c, "Release", 200.0).band_mix(c, 0, 0.2).band_mix(c, 2, 0.4);
    });
    add("Drive then Limit", "Mixing", &|b| {
        let d = b.add(Drive);
        b.wave(d, 0, Wave::flat(0.3));
        let c = b.add(Compressor);
        b.knob(c, "Threshold", -3.0).knob(c, "Ratio", 100.0).knob(c, "Attack", 0.2).knob(c, "Release", 60.0);
    });
    add("Kick Space (low duck)", "Mixing", &|b| {
        let v = b.add(Volume);
        b.len(v, "1/4").set(v, |s| {
            s.multiband = true;
            s.x_lo = 120.0;
        });
        b.wave_b(v, 0, 0, wv(W::PumpHard)).wave_b(v, 0, 1, Wave::flat(1.0)).wave_b(v, 0, 2, Wave::flat(1.0));
    });

    // ---- Sound Design
    add("Wind Swell", "Sound Design", &|b| {
        let n = b.add(Noise);
        b.len(n, "4 bars").wave(n, 0, wv(W::Swell)).wave(n, 1, ramp(0.3, 0.7, 0.0)).set(n, |s| s.n_type = noise_idx("Wind")).knob(n, "Trim", -10.0);
    });
    add("Ocean Bed", "Sound Design", &|b| {
        let n = b.add(Noise);
        b.len(n, "4 bars").wave(n, 0, wv(W::Sine)).set(n, |s| s.n_type = noise_idx("Ocean")).knob(n, "Trim", -14.0).tile(n, 0, 0.2);
    });
    add("Fire Crackle", "Sound Design", &|b| {
        let n = b.add(Noise);
        b.wave(n, 0, Wave::flat(0.6)).set(n, |s| s.n_type = noise_idx("Fire")).knob(n, "Trim", -16.0);
    });
    add("Snare Wires", "Sound Design", &|b| {
        let n = b.add(Noise);
        b.wave(n, 0, Wave::flat(0.9)).set(n, |s| {
            s.n_type = noise_idx("Snare wires");
            s.n_env = 1;
        });
        b.knob(n, "Trim", -8.0).knob(n, "High-pass", 1500.0);
    });
    add("Shaker Groove", "Sound Design", &|b| {
        let n = b.add(Noise);
        b.len(n, "1/2").set(n, |s| s.n_type = noise_idx("Shaker")).wave(n, 0, rep(inv(wv(W::Pump)), 4)).lfo(n, 0, |l| l.swing = 0.5).knob(n, "Trim", -12.0);
    });
    add("Radio Tuning", "Sound Design", &|b| {
        let n = b.add(Noise);
        b.len(n, "2 bars").set(n, |s| s.n_type = noise_idx("Radio static")).wave(n, 0, wv(W::Triangle)).knob(n, "Trim", -14.0);
        let f = b.add(Filter);
        b.len(f, "2 bars").set(f, |s| s.f_type = 2).wave(f, 0, random_steps(8, 8, 0.4)).wave(f, 1, Wave::flat(0.6)).lfo(f, 0, |l| l.smooth = 40.0);
        let c = b.add(Crush);
        b.wave(c, 0, Wave::flat(0.6)).wave(c, 1, Wave::flat(0.6));
    });
    add("Alien Comb (MIDI)", "Sound Design", &|b| {
        let l = b.add(Liquid);
        b.set(l, |s| s.l_type = 1).lfo(l, 0, |f| {
            f.mode = 2;
            f.transpose = 12.0;
        });
        b.wave(l, 0, wv(W::Sine)).wave(l, 1, Wave::flat(0.9));
    });
    add("Bit Riser", "Sound Design", &|b| {
        let c = b.add(Crush);
        b.len(c, "4 bars").wave(c, 0, ramp(0.2, 1.0, 0.0)).wave(c, 1, ramp(0.25, 1.0, 0.3));
        let f = b.add(Filter);
        b.len(f, "4 bars").set(f, |s| s.f_type = 1).wave(f, 0, ramp(0.0, 0.6, 0.0)).wave(f, 1, Wave::flat(0.5));
    });
    add("Broken Machine", "Sound Design", &|b| {
        let t = b.add(Time);
        b.len(t, "1 bar").wave(t, 0, random_steps(41, 16, 0.6));
        let d = b.add(Drive);
        b.len(d, "1/2").wave(d, 0, random_steps(43, 8, 0.0)).set(d, |s| s.d_style = drive_idx("Saw fold"));
        let p = b.add(Pan);
        b.len(p, "1/4").wave(p, 0, random_steps(47, 8, 0.0));
        let c = b.add(Crush);
        b.len(c, "2 bars").wave(c, 0, random_steps(53, 8, 0.4)).wave(c, 1, random_steps(59, 8, 0.5)).mix(c, 0.6);
    });
    add("Ghost Choir", "Sound Design", &|b| {
        let r = b.add(Reverb);
        b.wave(r, 0, Wave::flat(1.0)).set(r, |s| s.r_type = reverb_idx("Shimmer cloud")).knob(r, "Dry", 0.2).knob(r, "Wet", 0.9).knob(r, "Decay", 0.85);
        let l = b.add(Liquid);
        b.len(l, "4 bars").wave(l, 0, wv(W::Sine)).wave(l, 1, Wave::flat(0.5));
        let f = b.add(Filter);
        b.wave(f, 0, Wave::flat(0.75)).wave(f, 1, Wave::flat(0.2));
    });
    out
}

/// Factory item presets of one kind (shown in the item's menu).
pub fn items(kind: Kind) -> Vec<ItemPreset> {
    use Kind::*;
    let mut out = Vec::new();
    let mut add = |name: &str, f: &dyn Fn(&mut Builder, usize)| {
        let mut b = Builder::new();
        let s = b.add(kind);
        f(&mut b, s);
        out.push(b.item(name));
    };
    add("Default", &|_, _| {});
    match kind {
        Volume => {
            add("Pump 1/4", &|b, s| {
                b.len(s, "1/4").wave(s, 0, wv(W::Pump));
            });
            add("Hard duck 1/4", &|b, s| {
                b.len(s, "1/4").wave(s, 0, wv(W::PumpHard));
            });
            add("Gate 1/16", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, wv(W::Gate16)).lfo(s, 0, |l| l.smooth = 3.0);
            });
            add("Tremolo 1/8", &|b, s| {
                b.len(s, "1/8").wave(s, 0, wv(W::Sine)).tile(s, 0, 0.3);
            });
            add("Audio-triggered duck", &|b, s| {
                b.len(s, "1/4").wave(s, 0, wv(W::PumpHard)).lfo(s, 0, |l| {
                    l.trigger = 2;
                    l.looping = false;
                });
            });
        }
        Filter => {
            add("Wobble 1/8", &|b, s| {
                b.len(s, "1/8").wave(s, 0, wv(W::Sine)).wave(s, 1, Wave::flat(0.55));
            });
            add("Sweep up 4 bars", &|b, s| {
                b.len(s, "4 bars").wave(s, 0, ramp(0.0, 1.0, 0.35));
            });
            add("High-pass rise", &|b, s| {
                b.len(s, "2 bars").set(s, |x| x.f_type = 1).wave(s, 0, ramp(0.0, 0.8, 0.3));
            });
            add("Envelope band-pass", &|b, s| {
                b.set(s, |x| x.f_type = 2).wave(s, 0, Wave::flat(0.3)).env(s, 0, |e| {
                    e.on = true;
                    e.amount = 0.45;
                });
            });
        }
        Pan => {
            add("Auto-pan 1/4", &|b, s| {
                b.len(s, "1/4").wave(s, 0, wv(W::Sine));
            });
            add("Ping-pong 1/8", &|b, s| {
                b.len(s, "1/4").wave(s, 0, wv(W::Square)).lfo(s, 0, |l| l.smooth = 4.0);
            });
            add("Haas swirl", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, wv(W::Sine)).knob(s, "Pan / Haas", 1.0).knob(s, "Haas range", 15.0);
            });
            add("Random positions", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, random_steps(47, 8, 0.0)).lfo(s, 0, |l| l.smooth = 8.0);
            });
            add("Slow drift 4 bars", &|b, s| {
                b.len(s, "4 bars").wave(s, 0, wv(W::Sine)).tile(s, 0, 0.0);
            });
        }
        Width => {
            add("Breathing", &|b, s| {
                b.len(s, "2 bars").wave(s, 0, wv(W::Sine));
            });
            add("Mono bass", &|b, s| {
                b.set(s, |x| {
                    x.multiband = true;
                    x.x_lo = 150.0;
                });
                b.wave_b(s, 0, 0, Wave::flat(0.0)).wave_b(s, 0, 1, Wave::flat(0.5)).wave_b(s, 0, 2, Wave::flat(0.5));
            });
            add("Open up on the beat", &|b, s| {
                b.len(s, "1/4").wave(s, 0, inv(wv(W::Pump)));
            });
            add("Wide highs", &|b, s| {
                b.set(s, |x| {
                    x.multiband = true;
                    x.x_hi = 4000.0;
                });
                b.wave_b(s, 0, 0, Wave::flat(0.35)).wave_b(s, 0, 1, Wave::flat(0.5)).wave_b(s, 0, 2, Wave::flat(0.8));
            });
            add("Mono", &|b, s| {
                b.wave(s, 0, Wave::flat(0.0));
            });
        }
        Time => {
            add("Tape stop", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, wv(W::TapeStop));
            });
            add("Half-time", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, wv(W::HalfTime));
            });
            add("Stutter", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, wv(W::Stutter));
            });
            add("Vibrato", &|b, s| {
                b.set(s, |x| x.t_range = 6).lfo(s, 0, |l| {
                    l.mode = 1;
                    l.hz = 5.5;
                });
                b.wave(s, 0, wv(W::Sine));
            });
        }
        Noise => {
            add("Riser", &|b, s| {
                b.len(s, "4 bars").wave(s, 0, ramp(0.0, 1.0, 0.45)).wave(s, 1, ramp(0.1, 1.0, 0.0));
            });
            add("Vinyl bed", &|b, s| {
                b.set(s, |x| x.n_type = noise_idx("Vinyl crackle")).wave(s, 0, Wave::flat(0.5)).knob(s, "Trim", -18.0);
            });
            add("Offbeat hats", &|b, s| {
                b.len(s, "1/4").set(s, |x| x.n_type = noise_idx("Metal 808")).wave(s, 0, Wave::steps(&[0.0, 0.0, 1.0, 0.0])).knob(s, "High-pass", 6000.0);
            });
            add("Follow the input", &|b, s| {
                b.set(s, |x| {
                    x.n_type = noise_idx("Pink");
                    x.n_env = 1;
                });
                b.wave(s, 0, Wave::flat(0.7));
            });
        }
        Crush => {
            add("Steps", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, random_steps(3, 8, 0.3));
            });
            add("Lo-fi 8 bit", &|b, s| {
                b.wave(s, 0, Wave::flat(0.47)).wave(s, 1, Wave::flat(0.6)).knob(s, "Treble", 0.5);
            });
            add("Downsample sweep", &|b, s| {
                b.len(s, "2 bars").wave(s, 0, Wave::flat(1.0)).wave(s, 1, ramp(1.0, 0.3, -0.3));
            });
            add("Crunch wrap", &|b, s| {
                b.wave(s, 0, Wave::flat(0.85)).wave(s, 1, Wave::flat(1.0)).knob(s, "Crunch", 12.0).mix(s, 0.6);
            });
            add("Gated crush", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, inv(wv(W::Gate8))).wave(s, 1, inv(wv(W::Gate8)));
            });
        }
        Drive => {
            add("Pumping drive", &|b, s| {
                b.len(s, "1/4").wave(s, 0, inv(wv(W::Pump))).knob(s, "Input", 12.0);
            });
            add("Constant warmth", &|b, s| {
                b.wave(s, 0, Wave::flat(0.4)).set(s, |x| x.d_style = drive_idx("Soft clip"));
            });
            add("Fold sweep", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, wv(W::Triangle)).set(s, |x| x.d_style = drive_idx("Triangle fold"));
            });
            add("Accented gate", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, wv(W::Gate8)).knob(s, "Accent", 0.6);
            });
        }
        Reverb => {
            add("Swell", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, wv(W::Swell)).wave(s, 1, Wave::flat(1.0));
            });
            add("Ducked plate", &|b, s| {
                b.len(s, "1/4").wave(s, 0, Wave::flat(1.0)).wave(s, 1, wv(W::Pump)).set(s, |x| x.r_type = reverb_idx("Vocal plate"));
            });
            add("Throw on 4", &|b, s| {
                b.len(s, "1 bar").wave(s, 0, Wave::steps(&[0.0, 0.0, 0.0, 1.0]));
            });
            add("Shimmer", &|b, s| {
                b.set(s, |x| x.r_type = reverb_idx("Shimmer cloud")).wave(s, 0, Wave::flat(0.8)).wave(s, 1, Wave::flat(1.0));
            });
        }
        Liquid => {
            add("Flanger", &|b, s| {
                b.len(s, "2 bars").set(s, |x| x.l_type = 1).wave(s, 0, wv(W::Triangle)).wave(s, 1, Wave::flat(0.75));
            });
            add("Phaser 8 stages", &|b, s| {
                b.len(s, "1 bar").set(s, |x| {
                    x.l_type = 2;
                    x.l_stages = 3;
                });
                b.wave(s, 0, wv(W::Sine));
            });
            add("Slow flanger", &|b, s| {
                b.len(s, "4 bars").set(s, |x| x.l_type = 1).wave(s, 0, wv(W::Sine)).wave(s, 1, Wave::flat(0.6));
            });
            add("Negative phaser", &|b, s| {
                b.len(s, "1 bar").set(s, |x| {
                    x.l_type = 2;
                    x.l_neg = true;
                });
                b.wave(s, 0, wv(W::Triangle)).wave(s, 1, Wave::flat(0.8));
            });
            add("Pitched comb (MIDI)", &|b, s| {
                b.set(s, |x| x.l_type = 1).lfo(s, 0, |l| l.mode = 2).wave(s, 0, Wave::flat(0.6)).wave(s, 1, Wave::flat(0.85));
            });
        }
        Compressor => {
            add("Glue", &|b, s| {
                b.knob(s, "Threshold", -20.0).knob(s, "Ratio", 2.0).knob(s, "Attack", 20.0).knob(s, "Release", 200.0);
            });
            add("Punch", &|b, s| {
                b.knob(s, "Threshold", -24.0).knob(s, "Ratio", 4.0).knob(s, "Attack", 30.0).knob(s, "Release", 90.0);
            });
            add("Limiter", &|b, s| {
                b.knob(s, "Threshold", -6.0).knob(s, "Ratio", 100.0).knob(s, "Attack", 0.5).knob(s, "Release", 80.0);
            });
            add("Sidechain ducker", &|b, s| {
                b.set(s, |x| x.comp_input = 2).knob(s, "Threshold", -30.0).knob(s, "Ratio", 8.0).knob(s, "Attack", 1.0).knob(s, "Release", 150.0);
            });
        }
        Scope => {
            add("Long window", &|b, s| {
                b.set(s, |x| x.scope_len = 3);
            });
            add("Short window", &|b, s| {
                b.set(s, |x| x.scope_len = 7);
            });
        }
    }
    out
}

// ------------------------------------------------------------------------------------------
// Capture / apply
// ------------------------------------------------------------------------------------------

fn set_norm(setter: &ParamSetter, ptr: ParamPtr, v: f32) {
    // SAFETY: the pointers come from the plugin's own parameters, which outlive the editor
    unsafe {
        let v = v.clamp(0.0, 1.0);
        if (ptr.unmodulated_normalized_value() - v).abs() > 1e-7 {
            setter.raw_context.raw_begin_set_parameter(ptr);
            setter.raw_context.raw_set_parameter_normalized(ptr, v);
            setter.raw_context.raw_end_set_parameter(ptr);
        }
    }
}

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

/// Loads a whole preset.
pub fn apply(setter: &ParamSetter, p: &WaveCubeParams, data: &PresetData) {
    {
        let mut st = p.state.write();
        let grid = st.grid;
        let (noises, irs) = (st.user_noises.clone(), st.user_irs.clone());
        *st = data.state.clone();
        st.preset = data.name.clone();
        st.grid = grid;
        // user media of the preset that this machine does not have are dropped by sanitize
        if st.user_noises.is_empty() {
            st.user_noises = noises;
        }
        if st.user_irs.is_empty() {
            st.user_irs = irs;
        }
        st.sanitize();
        p.sync_kinds(&st);
    }
    for (id, ptr, _) in p.param_map() {
        if id == "bypass" {
            continue;
        }
        // SAFETY: see set_norm
        let v = data.params.get(&id).copied().unwrap_or_else(|| unsafe { ptr.default_normalized_value() });
        set_norm(setter, ptr, v);
    }
}

fn slot_ptrs(p: &WaveCubeParams, s: usize) -> BTreeMap<String, ParamPtr> {
    let suffix = format!("_{}", s + 1);
    p.param_map().into_iter().filter(|(id, _, _)| id.ends_with(&suffix) && !id.starts_with("on_")).map(|(id, ptr, _)| (id.trim_end_matches(&suffix).to_string(), ptr)).collect()
}

/// Captures one slot as an item preset.
pub fn capture_item(p: &WaveCubeParams, s: usize, name: &str) -> ItemPreset {
    let st = p.state.read();
    let ptrs = slot_ptrs(p, s);
    // SAFETY: the pointers point into `p`
    let get = |id: &str| ptrs.get(id).map(|ptr| unsafe { ptr.unmodulated_normalized_value() }).unwrap_or(0.5);
    ItemPreset {
        name: name.into(),
        kind: st.slots[s].kind,
        set: st.slots[s],
        waves: st.waves[s].clone(),
        knobs: (0..N_KNOBS).map(|k| get(&format!("k{k}"))).collect(),
        tiles: [get("ta") * 2.0 - 1.0, get("tb") * 2.0 - 1.0],
        mix: get("mix"),
        band_mix: [get("mxl"), get("mxm"), get("mxh")],
        user: true,
    }
}

/// Puts an item preset into slot `s` (the slot takes the preset's kind).
pub fn apply_item(setter: &ParamSetter, p: &WaveCubeParams, s: usize, item: &ItemPreset) {
    {
        let mut st = p.state.write();
        st.slots[s] = SlotSet { kind: item.kind, ..item.set };
        st.waves[s] = item.waves.clone();
        st.sanitize();
        p.sync_kinds(&st);
    }
    let ptrs = slot_ptrs(p, s);
    let put = |id: &str, v: f32| {
        if let Some(&ptr) = ptrs.get(id) {
            set_norm(setter, ptr, v);
        }
    };
    for k in 0..N_KNOBS {
        put(&format!("k{k}"), item.knobs.get(k).copied().unwrap_or(0.5));
    }
    put("ta", (item.tiles[0] + 1.0) * 0.5);
    put("tb", (item.tiles[1] + 1.0) * 0.5);
    put("mix", item.mix);
    put("mxl", item.band_mix[0]);
    put("mxm", item.band_mix[1]);
    put("mxh", item.band_mix[2]);
}

/// A fresh item of `kind` in slot `s`: default settings, shapes and knobs.
pub fn reset_item(setter: &ParamSetter, p: &WaveCubeParams, s: usize, kind: Kind) {
    let mut b = Builder::new();
    b.add(kind);
    apply_item(setter, p, s, &b.item("Default"));
}

/// Copies slot `from` into slot `to` (duplicate an item).
pub fn copy_item(setter: &ParamSetter, p: &WaveCubeParams, from: usize, to: usize) {
    let it = capture_item(p, from, "");
    apply_item(setter, p, to, &it);
}

// ------------------------------------------------------------------------------------------
// Files
// ------------------------------------------------------------------------------------------

pub fn dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("WaveCube").join("Presets"))
}

fn item_dir(kind: Kind) -> Option<PathBuf> {
    dir().map(|d| d.join("Items").join(kind.name()))
}

pub fn file_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    let s = s.trim();
    format!("{}.json", if s.is_empty() { "Preset" } else { s })
}

fn read_dir_json<T: for<'de> Deserialize<'de>>(d: Option<PathBuf>) -> Vec<T> {
    let Some(d) = d else { return vec![] };
    let Ok(rd) = std::fs::read_dir(&d) else { return vec![] };
    rd.flatten()
        .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
        .filter(|e| e.metadata().map(|m| m.len() < 8 << 20).unwrap_or(false))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| serde_json::from_str::<T>(&s).ok())
        .collect()
}

pub fn list_user() -> Vec<PresetData> {
    let mut out: Vec<PresetData> = read_dir_json(dir());
    for p in out.iter_mut() {
        p.author = "User".into();
        p.state.sanitize();
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

fn write_json<T: Serialize>(d: PathBuf, name: &str, data: &T) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let path = d.join(file_name(name));
    let json = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn save_user(data: &PresetData) -> Result<PathBuf, String> {
    write_json(dir().ok_or("no data folder")?, &data.name, data)
}

pub fn delete_user(name: &str) -> bool {
    dir().map(|d| std::fs::remove_file(d.join(file_name(name))).is_ok()).unwrap_or(false)
}

pub fn list_user_items(kind: Kind) -> Vec<ItemPreset> {
    let mut out: Vec<ItemPreset> = read_dir_json(item_dir(kind));
    out.retain(|i| i.kind == kind);
    for i in out.iter_mut() {
        i.user = true;
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

pub fn save_user_item(item: &ItemPreset) -> Result<PathBuf, String> {
    write_json(item_dir(item.kind).ok_or("no data folder")?, &item.name, item)
}

pub fn delete_user_item(kind: Kind, name: &str) -> bool {
    item_dir(kind).map(|d| std::fs::remove_file(d.join(file_name(name))).is_ok()).unwrap_or(false)
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
        assert!(all.len() >= 130, "{}", all.len());
        let mut names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len(), "duplicate names");
        let p = WaveCubeParams::default();
        let ids: Vec<String> = p.param_map().into_iter().map(|(id, _, _)| id).collect();
        for d in &all {
            assert!(CATEGORIES.contains(&d.category.as_str()), "{}", d.category);
            assert!(!d.state.chain.is_empty(), "{} is empty", d.name);
            assert!(d.params.values().all(|v| (0.0..=1.0).contains(v)), "{}", d.name);
            assert!(d.params.keys().all(|k| ids.contains(k)), "{}: unknown id", d.name);
            let s = serde_json::to_string(d).unwrap();
            let back: PresetData = serde_json::from_str(&s).unwrap();
            assert_eq!(&back, d);
        }
        // the categories are all used and a few presets use the same kind twice
        for c in CATEGORIES {
            assert!(all.iter().any(|d| d.category == c), "{c}");
        }
        assert!(all.iter().any(|d| d.kinds().iter().filter(|&&k| k == Kind::Drive).count() == 2));
    }

    #[test]
    fn every_kind_has_item_presets() {
        for k in Kind::ALL {
            let it = items(k);
            assert!(it.len() >= if k == Kind::Scope { 3 } else { 5 }, "{}", k.name());
            for i in &it {
                assert_eq!(i.kind, k);
                assert_eq!(i.set.kind, k);
                assert_eq!(i.knobs.len(), N_KNOBS);
                let s = serde_json::to_string(i).unwrap();
                let back: ItemPreset = serde_json::from_str(&s).unwrap();
                assert_eq!(&back, i);
            }
        }
    }

    #[test]
    fn plain_values_land() {
        let mut b = Builder::new();
        let s = b.add(Kind::Compressor);
        b.knob(s, "Threshold", -20.0);
        let d = b.finish("t", "Mixing");
        let n = d.params[&slot_id("k0", s)];
        assert!((Kind::Compressor.knobs()[0].plain(n) + 20.0).abs() < 0.05);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("My / Preset: 1"), "My _ Preset_ 1.json");
        assert_eq!(file_name("  "), "Preset.json");
    }
}
