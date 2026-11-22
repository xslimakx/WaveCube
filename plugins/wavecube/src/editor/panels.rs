//! The panels under the editor (LFO, envelope, the item's own settings, bands, output) and the
//! views of the tools (Compressor, Scope).

use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, epaint::PathStroke, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use std::sync::atomic::Ordering;

use super::widgets::*;
use super::{commit, Cx, Gui, BAND_NAMES, BASE_W};
use crate::dsp::blocks::{drive_curve, filter_mag_db};
use crate::kinds::{self, KnobDef, Scale::*, Unit, LENGTHS};
use crate::{Env, Kind, Lfo, SlotSet, N_BANDS, SCOPE_N};

const Y1: f32 = 490.0;
const H1: f32 = 150.0;
const Y2: f32 = 646.0;
const H2: f32 = 162.0;

// knob maps of the saved (not automatable) settings
const K_PHASE: KnobDef = KnobDef::new("Phase", "Moves the shape in time (the start of the cycle).", 0.0, 360.0, 0.0, Lin, Unit::Deg, false);
const K_SMOOTH: KnobDef = KnobDef::new("Smooth", "Rounds off jumps in the shape (clicks, hard gates).", 0.0, 50.0, 1.5, Pow(2.0), Unit::Ms, false);
const K_SWING: KnobDef = KnobDef::new("Swing", "Delays every second grid step: shuffle.", 0.0, 1.0, 0.0, Lin, Unit::Pct, false);
const K_SPREAD: KnobDef = KnobDef::new("Stereo", "The right channel runs ahead of the left by this much of a cycle.", -180.0, 180.0, 0.0, Lin, Unit::Deg, true);
const K_HZ: KnobDef = KnobDef::new("Rate", "Cycles per second (up to audio rate).", 0.02, 5000.0, 1.0, Log, Unit::Hz, false);
const K_TRANS: KnobDef = KnobDef::new("Transpose", "Pitch mode: the cycle follows the MIDI note, moved by this.", -36.0, 36.0, 0.0, Lin, Unit::St, true);

const K_AMT: KnobDef = KnobDef::new("Amount", "How far the input level moves the lane (negative: the other way).", -2.0, 2.0, 1.0, Lin, Unit::Pct, true);
const K_THR: KnobDef = KnobDef::new("Threshold", "Input level where the envelope starts.", -60.0, 0.0, -40.0, Lin, Unit::Db, false);
const K_ATT: KnobDef = KnobDef::new("Attack", "How fast it rises.", 0.1, 500.0, 5.0, Log, Unit::Ms, false);
const K_HOLD: KnobDef = KnobDef::new("Hold", "How long it stays up before it falls.", 0.0, 500.0, 0.0, Pow(2.0), Unit::Ms, false);
const K_REL: KnobDef = KnobDef::new("Release", "How fast it falls.", 5.0, 2000.0, 150.0, Log, Unit::Ms, false);
const K_SHIFT: KnobDef = KnobDef::new("Shift", "Moves the resting point up or down.", -1.0, 1.0, 0.0, Lin, Unit::Pct, true);
const K_EHP: KnobDef = KnobDef::new("HP", "High-pass on the detector (Free / Sidechain input).", 20.0, 20000.0, 20.0, Log, Unit::Hz, false);
const K_ELP: KnobDef = KnobDef::new("LP", "Low-pass on the detector (Free / Sidechain input).", 20.0, 20000.0, 20000.0, Log, Unit::Hz, false);

const K_XLO: KnobDef = KnobDef::new("Low / mid", "Split between the low and the mid band.", 20.0, 20000.0, 200.0, Log, Unit::Hz, false);
const K_XHI: KnobDef = KnobDef::new("Mid / high", "Split between the mid and the high band.", 20.0, 20000.0, 2500.0, Log, Unit::Hz, false);

/// Changes the LFO of the lane (the bands it applies to).
fn edit_lfo(cx: &Cx, gui: &mut Gui, key: &str, linked: bool, f: impl Fn(&mut Lfo)) {
    gui.checkpoint(cx, key, cx.now);
    let bands = cx.bands(linked);
    let (s, l) = (cx.sel, cx.lane);
    commit(cx.p, cx.shared, |st| {
        for b in bands {
            f(&mut st.slots[s].lfo[l][b]);
        }
    });
}

fn edit_env(cx: &Cx, gui: &mut Gui, key: &str, f: impl Fn(&mut Env)) {
    gui.checkpoint(cx, key, cx.now);
    let bands = cx.bands(false);
    let (s, l) = (cx.sel, cx.lane);
    commit(cx.p, cx.shared, |st| {
        for b in bands {
            f(&mut st.slots[s].env[l][b]);
        }
    });
}

fn edit_set(cx: &Cx, gui: &mut Gui, key: &str, f: impl FnOnce(&mut SlotSet)) {
    gui.checkpoint(cx, key, cx.now);
    let s = cx.sel;
    commit(cx.p, cx.shared, |st| f(&mut st.slots[s]));
}

pub fn bottom(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    if cx.kind == Kind::Scope {
        item_panel(ui, cx, gui, cx.r(14.0, Y1, BASE_W - 28.0, H1));
        tool_side(ui, cx, gui, cx.r(14.0, Y2, 946.0, H2));
        output_panel(ui, cx, gui, cx.r(966.0, Y2, 200.0, H2));
        return;
    } else if cx.kind.is_tool() {
        item_panel(ui, cx, gui, cx.r(14.0, Y1, BASE_W - 28.0, H1));
        tool_side(ui, cx, gui, cx.r(14.0, Y2, 640.0, H2));
    } else {
        lfo_panel(ui, cx, gui, cx.r(14.0, Y1, 520.0, H1));
        env_panel(ui, cx, gui, cx.r(540.0, Y1, 626.0, H1));
        item_panel(ui, cx, gui, cx.r(14.0, Y2, 640.0, H2));
    }
    bands_panel(ui, cx, gui, cx.r(660.0, Y2, 300.0, H2));
    output_panel(ui, cx, gui, cx.r(966.0, Y2, 200.0, H2));
}

// ------------------------------------------------------------------------------------------
// LFO
// ------------------------------------------------------------------------------------------

fn lfo_panel(ui: &mut Ui, cx: &Cx, gui: &mut Gui, r: Rect) {
    let col = cx.col;
    let ln = cx.kind.lanes()[cx.lane].name;
    panel(ui, r, &format!("LFO · {ln}{}", if cx.multi { format!(" · {} band", BAND_NAMES[cx.band].to_lowercase()) } else { String::new() }), col);
    let set = cx.st.slots[cx.sel];
    let lfo = set.lfo_of(cx.lane, cx.band);
    let raw = set.lfo[cx.lane][if cx.multi { cx.band } else { 0 }];
    let x0 = r.left();
    let y0 = r.top();
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(Pos2::new(x0 + x, y0 + y), Vec2::new(w, h));
    let modal = cx.modal;
    // row 1: mode, length, quick lengths, feel
    if let Some(i) = segmented(ui, at(12.0, 28.0, 150.0, 22.0), &kinds::MODES, lfo.mode as usize, col, "Beat: synced to the tempo · Hz: free running · Pitch: the cycle follows the MIDI note (audio-rate effects).") {
        if !modal {
            edit_lfo(cx, gui, "", true, |l| l.mode = i as u8);
        }
    }
    let enabled_beat = lfo.mode == 0;
    if enabled_beat {
        let names: Vec<String> = LENGTHS.iter().map(|s| s.to_string()).collect();
        if let Some(i) = combo(ui, "lfolen", at(170.0, 28.0, 86.0, 22.0), &names, lfo.length as usize, LENGTHS[lfo.length as usize], "Length of one cycle of the shape.") {
            edit_lfo(cx, gui, "", true, |l| l.length = i as u8);
        }
        for (k, (name, idx)) in [("¼", 7u8), ("½", 6), ("1", 5), ("2", 4)].iter().enumerate() {
            if btn(ui, at(262.0 + k as f32 * 26.0, 28.0, 24.0, 22.0), name, lfo.length == *idx, col, LENGTHS[*idx as usize]).clicked() && !modal {
                let v = *idx;
                edit_lfo(cx, gui, "", true, |l| l.length = v);
            }
        }
        if let Some(i) = combo_s(ui, "lfofeel", at(370.0, 28.0, 92.0, 22.0), &kinds::FEELS, lfo.feel as usize, "", "Straight, dotted (× 1.5) or triplet (× 2/3).") {
            edit_lfo(cx, gui, "", true, |l| l.feel = i as u8);
        }
        // ÷2 / ×2
        if btn(ui, at(466.0, 28.0, 21.0, 22.0), "÷", false, col, "Half as fast").clicked() && !modal && lfo.length > 0 {
            edit_lfo(cx, gui, "", true, |l| l.length = l.length.saturating_sub(1));
        }
        if btn(ui, at(489.0, 28.0, 21.0, 22.0), "×", false, col, "Twice as fast").clicked() && !modal {
            edit_lfo(cx, gui, "", true, |l| l.length = (l.length + 1).min(12));
        }
    } else {
        let txt = if lfo.mode == 1 { "free running, set the rate below" } else { "the cycle follows the MIDI note" };
        label(ui, Pos2::new(x0 + 172.0, y0 + 39.0), txt, DIM, 10.5);
    }
    // row 2: trigger, loop, link
    if let Some(i) = segmented(ui, at(12.0, 56.0, 150.0, 22.0), &kinds::TRIGGERS, lfo.trigger as usize, col, "Sync: follows the song position · MIDI: restarts on every note · Audio: restarts on every hit of the input (or the sidechain; see ⚙ settings).") {
        if !modal {
            edit_lfo(cx, gui, "", true, |l| l.trigger = i as u8);
        }
    }
    if btn(ui, at(170.0, 56.0, 70.0, 22.0), if lfo.looping { "Loop" } else { "1-shot" }, !lfo.looping, GOLD, "Loop, or play the shape once per trigger and stop at the END marker (drag it in the editor).").clicked() && !modal {
        let v = !lfo.looping;
        edit_lfo(cx, gui, "", true, |l| l.looping = v);
    }
    if cx.multi {
        let link = set.link[cx.lane];
        if btn(ui, at(246.0, 56.0, 92.0, 22.0), if link { "Bands linked" } else { "Bands free" }, link, col, "Linked: the three bands share mode, length, feel and trigger (each keeps its shape, phase, smooth, swing).\nFree: every band has its own LFO.").clicked() && !modal {
            let (s, l) = (cx.sel, cx.lane);
            gui.checkpoint(cx, "", cx.now);
            commit(cx.p, cx.shared, |st| {
                st.slots[s].link[l] = !link;
                if !link {
                    // linking: every band takes the mid band's timing
                    let m = st.slots[s].lfo[l][1];
                    for b in [0, 2] {
                        let d = &mut st.slots[s].lfo[l][b];
                        d.mode = m.mode;
                        d.length = m.length;
                        d.feel = m.feel;
                        d.hz = m.hz;
                        d.transpose = m.transpose;
                        d.trigger = m.trigger;
                        d.looping = m.looping;
                    }
                }
            });
        }
    }
    // the length of a cycle in time
    let cyc = match lfo.mode {
        1 => format!("1 cycle = {:.3} s", 1.0 / lfo.hz.max(1e-3)),
        2 => "1 cycle = 1 period of the note".into(),
        _ => {
            let s = kinds::length_beats(lfo.length, lfo.feel) * 60.0 / cx.tempo.max(1.0) as f64;
            if s < 1.0 {
                format!("1 cycle = {:.0} ms at {:.0} BPM", s * 1000.0, cx.tempo)
            } else {
                format!("1 cycle = {:.2} s at {:.0} BPM", s, cx.tempo)
            }
        }
    };
    label(ui, Pos2::new(x0 + 348.0, y0 + 67.0), &cyc, FAINT, 9.5);
    // knobs
    let ky = y0 + 100.0;
    let mut kx = x0 + 40.0;
    let step = 74.0;
    let id = |n: &str| format!("lfo{n}{}{}{}", cx.sel, cx.lane, cx.band);
    if let Some(v) = sknob(ui, &id("ph"), &K_PHASE, raw.phase, Pos2::new(kx, ky), 15.0, col, !modal) {
        edit_lfo(cx, gui, "lfoph", false, |l| l.phase = v);
    }
    kx += step;
    // Time with "cut" jumps cross-fades instead of smoothing
    if !(cx.kind == Kind::Time && set.t_instant) {
        if let Some(v) = sknob(ui, &id("sm"), &K_SMOOTH, raw.smooth, Pos2::new(kx, ky), 15.0, col, !modal) {
            edit_lfo(cx, gui, "lfosm", false, |l| l.smooth = v);
        }
        kx += step;
    }
    if let Some(v) = sknob(ui, &id("sw"), &K_SWING, raw.swing, Pos2::new(kx, ky), 15.0, col, !modal) {
        edit_lfo(cx, gui, "lfosw", false, |l| l.swing = v);
    }
    kx += step;
    if let Some(v) = sknob(ui, &id("sp"), &K_SPREAD, raw.spread, Pos2::new(kx, ky), 15.0, col, !modal) {
        edit_lfo(cx, gui, "lfosp", false, |l| l.spread = v);
    }
    kx += step;
    match lfo.mode {
        1 => {
            if let Some(v) = sknob(ui, &id("hz"), &K_HZ, lfo.hz, Pos2::new(kx, ky), 15.0, col, !modal) {
                edit_lfo(cx, gui, "lfohz", true, |l| l.hz = v);
            }
        }
        2 => {
            if let Some(v) = sknob(ui, &id("tr"), &K_TRANS, lfo.transpose, Pos2::new(kx, ky), 15.0, col, !modal) {
                let v = v.round();
                edit_lfo(cx, gui, "lfotr", true, |l| l.transpose = v);
            }
        }
        _ => kx -= step,
    }
    kx += step;
    // copy the LFO to the other lane
    if cx.kind.n_lanes() > 1 && btn(ui, Rect::from_center_size(Pos2::new(kx + 6.0, ky - 4.0), Vec2::new(70.0, 22.0)), "→ other lane", false, col, "Give the other lane the same LFO settings.").clicked() && !modal {
        let (s, l) = (cx.sel, cx.lane);
        gui.checkpoint(cx, "", cx.now);
        commit(cx.p, cx.shared, |st| {
            let src = st.slots[s].lfo[l];
            st.slots[s].lfo[1 - l] = src;
        });
        gui.say("LFO copied to the other lane", cx.now);
    }
}

// ------------------------------------------------------------------------------------------
// Envelope follower
// ------------------------------------------------------------------------------------------

fn env_panel(ui: &mut Ui, cx: &Cx, gui: &mut Gui, r: Rect) {
    let col = cx.col;
    panel(ui, r, "ENVELOPE FOLLOWER", col);
    let x0 = r.left();
    let y0 = r.top();
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(Pos2::new(x0 + x, y0 + y), Vec2::new(w, h));
    if !cx.kind.has_envelope() {
        label(ui, Pos2::new(x0 + 16.0, y0 + 70.0), &format!("{} has no envelope follower: its shape is the time position.", cx.kind.name()), DIM, 11.5);
        return;
    }
    let modal = cx.modal;
    let env = cx.st.slots[cx.sel].env[cx.lane][if cx.multi { cx.band } else { 0 }];
    if btn(ui, at(150.0, 8.0, 46.0, 18.0), if env.on { "ON" } else { "OFF" }, env.on, col, "The level of the input moves the lane on top of its shape.").clicked() && !modal {
        let v = !env.on;
        edit_env(cx, gui, "", |e| e.on = v);
    }
    let en = env.on && !modal;
    if let Some(i) = segmented(ui, at(12.0, 32.0, 180.0, 22.0), &kinds::ENV_INPUTS, env.input as usize, col, "What it listens to: this band · the full input with its own filters · the sidechain input.") {
        if en {
            edit_env(cx, gui, "", |e| e.input = i as u8);
        }
    }
    if let Some(i) = segmented(ui, at(200.0, 32.0, 136.0, 22.0), &kinds::ENV_DEPTHS, env.depth as usize, col, "Depth: makes small level changes count more.") {
        if en {
            edit_env(cx, gui, "", |e| e.depth = i as u8);
        }
    }
    if btn(ui, at(344.0, 32.0, 72.0, 22.0), "Adaptive", env.adaptive, col, "Fast release at first, slower in the tail: less pumping.").clicked() && en {
        let v = !env.adaptive;
        edit_env(cx, gui, "", |e| e.adaptive = v);
    }
    if btn(ui, at(420.0, 32.0, 72.0, 22.0), "Multiply", env.multiply, col, "Off: the envelope is added to the shape · On: it scales the shape.").clicked() && en {
        let v = !env.multiply;
        edit_env(cx, gui, "", |e| e.multiply = v);
    }
    let listen = cx.shared.env_listen.load(Ordering::Relaxed) as usize == cx.sel + 1;
    if btn(ui, at(496.0, 32.0, 64.0, 22.0), "Listen", listen, HOT, "Hear what the detector hears (its filters and input).").clicked() && !modal {
        cx.shared.env_listen.store(if listen { 0 } else { cx.sel as u32 + 1 }, Ordering::Relaxed);
    }
    let ky = y0 + 102.0;
    let step = 64.0;
    let mut kx = x0 + 36.0;
    let id = |n: &str| format!("env{n}{}{}{}", cx.sel, cx.lane, cx.band);
    let filt = env.input != 0;
    macro_rules! k {
        ($key:expr, $def:expr, $val:expr, $en:expr, $set:expr) => {
            if let Some(v) = sknob(ui, &id($key), &$def, $val, Pos2::new(kx, ky), 14.0, col, $en) {
                edit_env(cx, gui, concat!("env", $key), |e| $set(e, v));
            }
            kx += step;
        };
    }
    k!("amt", K_AMT, env.amount, en, |e: &mut Env, v| e.amount = v);
    k!("thr", K_THR, env.thresh, en, |e: &mut Env, v| e.thresh = v);
    k!("att", K_ATT, env.attack, en, |e: &mut Env, v| e.attack = v);
    k!("hold", K_HOLD, env.hold, en, |e: &mut Env, v| e.hold = v);
    k!("rel", K_REL, env.release, en, |e: &mut Env, v| e.release = v);
    k!("shift", K_SHIFT, env.shift, en, |e: &mut Env, v| e.shift = v);
    k!("hp", K_EHP, env.hp, en && filt, |e: &mut Env, v| e.hp = v);
    k!("lp", K_ELP, env.lp, en && filt, |e: &mut Env, v| e.lp = v);
    let _ = kx;
    // level history
    let hr = at(r.width() - 66.0, 30.0, 52.0, 100.0);
    ui.painter().rect_filled(hr, CornerRadius::same(4), EDIT_BG);
    let n = gui.env_hist.len();
    let pts: Vec<Pos2> = gui.env_hist.iter().enumerate().map(|(i, v)| Pos2::new(hr.left() + hr.width() * i as f32 / (n - 1) as f32, hr.bottom() - v.clamp(0.0, 1.0) * hr.height())).collect();
    ui.painter().add(Shape::line(pts, PathStroke::new(1.2f32, if env.on { OK } else { FAINT })));
    ui.painter().text(Pos2::new(hr.center().x, hr.bottom() + 8.0), Align2::CENTER_CENTER, "level", pf(9.0), DIM);
}

// ------------------------------------------------------------------------------------------
// The item's own settings
// ------------------------------------------------------------------------------------------

/// The automatable knobs of the slot, in a row from `x`.
fn slot_knobs(ui: &mut Ui, cx: &Cx, from: usize, to: usize, x: f32, y: f32, step: f32) {
    let defs = cx.kind.knobs();
    for (j, i) in (from..to.min(defs.len())).enumerate() {
        let d = &defs[i];
        let p = cx.p.slots[cx.sel].knob(i);
        if cx.modal {
            let _ = knob_core(ui, egui::Id::new(("ko", cx.sel, i)), Pos2::new(x + j as f32 * step, y), 15.0, p.unmodulated_normalized_value(), 0.0, d.name, &p.to_string(), &|_| None, d.tip, d.bipolar, cx.col, false);
        } else {
            pknob(ui, cx.setter, p, d.name, Pos2::new(x + j as f32 * step, y), 15.0, d.tip, d.bipolar, cx.col);
        }
    }
}

fn item_panel(ui: &mut Ui, cx: &Cx, gui: &mut Gui, r: Rect) {
    let col = cx.col;
    let set = cx.st.slots[cx.sel];
    panel(ui, r, &format!("{} SETTINGS", cx.kind.name().to_uppercase()), col);
    let x0 = r.left();
    let y0 = r.top();
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(Pos2::new(x0 + x, y0 + y), Vec2::new(w, h));
    let modal = cx.modal;
    let ky = y0 + r.height() - 52.0;
    let kx = x0 + 40.0;
    let step = 72.0;
    let n = cx.kind.knobs().len();
    let sid = |n: &str| format!("{n}{}", cx.sel);
    match cx.kind {
        Kind::Volume => {
            slot_knobs(ui, cx, 0, n, kx, ky, step);
            label(ui, Pos2::new(x0 + 16.0, y0 + 44.0), "Gates, pumping, tremolo, fades. Use the envelope follower for an expander / ducker.", DIM, 10.5);
        }
        Kind::Filter => {
            if let Some(i) = combo_s(ui, &sid("ftype"), at(12.0, 30.0, 120.0, 22.0), &kinds::FILTER_TYPES, set.f_type as usize, "", "Filter type") {
                edit_set(cx, gui, "", |s| s.f_type = i as u8);
            }
            if let Some(i) = segmented(ui, at(138.0, 30.0, 150.0, 22.0), &kinds::SLOPES, set.f_slope as usize, col, "Slope") {
                if !modal {
                    edit_set(cx, gui, "", |s| s.f_slope = i as u8);
                }
            }
            if btn(ui, at(294.0, 30.0, 56.0, 22.0), "Warm", set.f_warm, col, "Clean (off) or warm: saturating, analogue-like filter. Dist drives it.").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.f_warm = !s.f_warm);
            }
            if set.f_warm {
                if btn(ui, at(12.0, 58.0, 76.0, 20.0), "Safe res", set.f_safe, col, "Keeps the resonance from self-oscillating.").clicked() && !modal {
                    edit_set(cx, gui, "", |s| s.f_safe = !s.f_safe);
                }
                if btn(ui, at(92.0, 58.0, 76.0, 20.0), "Hard dist", set.f_hard, col, "Hard clipping inside the filter instead of soft.").clicked() && !modal {
                    edit_set(cx, gui, "", |s| s.f_hard = !s.f_hard);
                }
            }
            slot_knobs(ui, cx, 0, n, kx, ky, step);
            // response at the playhead
            let gr = at(r.width() - 290.0, 26.0, 276.0, r.height() - 36.0);
            filter_display(ui, cx, gr, &set);
        }
        Kind::Pan => {
            if let Some(i) = combo_s(ui, &sid("plaw"), at(12.0, 30.0, 150.0, 22.0), &kinds::PAN_LAWS, set.p_law as usize, "", "Pan law: how much the near side is raised (0 / 3 / 4.5 / 6 dB), or Combined: the far side is folded into the near side.") {
                edit_set(cx, gui, "", |s| s.p_law = i as u8);
            }
            slot_knobs(ui, cx, 0, n, kx, ky, step);
            vectorscope(ui, cx, at(r.width() - 150.0, 26.0, 136.0, r.height() - 36.0));
        }
        Kind::Width => {
            slot_knobs(ui, cx, 0, n, kx, ky, step);
            label(ui, Pos2::new(x0 + 16.0, y0 + 44.0), "Top: twice as wide · middle: as is · bottom: mono. Multiband: mono bass, wide highs.", DIM, 10.5);
            vectorscope(ui, cx, at(r.width() - 150.0, 26.0, 136.0, r.height() - 36.0));
        }
        Kind::Time => {
            if let Some(i) = combo_s(ui, &sid("trange"), at(12.0, 30.0, 140.0, 22.0), &kinds::TIME_RANGES, set.t_range as usize, "Range", "How far back the bottom of the shape reaches: one cycle (Linked), whole bars, or a few ms (vibrato, chorus).") {
                edit_set(cx, gui, "", |s| s.t_range = i as u8);
            }
            if let Some(i) = segmented(ui, at(158.0, 30.0, 180.0, 22.0), &["Jumps: cut", "Jumps: glide"], if set.t_instant { 0 } else { 1 }, col, "Cut: a jump in the shape cross-fades to the new time position (clean stutters, repeats).\nGlide: the time position slides there like a turntable, as fast as the LFO's Smooth (scratches).") {
                if !modal {
                    edit_set(cx, gui, "", |s| s.t_instant = i == 0);
                }
            }
            label(ui, Pos2::new(x0 + 16.0, y0 + 78.0), "Top: now · lower: later in the past. A falling line slows down (tape stop), a flat line holds, steps stutter.", DIM, 10.5);
            label(ui, Pos2::new(x0 + 16.0, y0 + 96.0), "Ranges longer than 16 s (8 bars below 120 BPM) reach back 16 s at most.", FAINT, 10.0);
        }
        Kind::Noise => {
            let mut names: Vec<String> = Vec::new();
            let mut map: Vec<Option<u16>> = Vec::new();
            let mut last = "";
            for (i, (c, nm)) in kinds::NOISES.iter().enumerate() {
                if *c != last {
                    names.push(format!("──{c}"));
                    map.push(None);
                    last = c;
                }
                names.push(nm.to_string());
                map.push(Some(i as u16));
            }
            if !gui.noise_files.is_empty() {
                names.push("──Your noises".into());
                map.push(None);
                for f in &gui.noise_files {
                    names.push(f.clone());
                    map.push(Some(u16::MAX));
                }
            }
            let cur_name = if set.n_type >= 1000 { cx.st.user_noises.get((set.n_type - 1000) as usize).cloned().unwrap_or("?".into()) } else { kinds::NOISES.get(set.n_type as usize).map(|n| n.1.to_string()).unwrap_or_default() };
            let cur_i = names.iter().position(|n| *n == cur_name).unwrap_or(0);
            if let Some(i) = combo(ui, &sid("ntype"), at(12.0, 30.0, 170.0, 22.0), &names, cur_i, &cur_name, "The noise. Your own WAV files go into the Noises folder (⚙ settings).") {
                match map.get(i).copied().flatten() {
                    Some(u16::MAX) => {
                        let name = names[i].clone();
                        gui.checkpoint(cx, "", cx.now);
                        let s = cx.sel;
                        commit(cx.p, cx.shared, |st| {
                            let k = crate::dsp::media::user_index(&mut st.user_noises, &name);
                            st.slots[s].n_type = 1000 + k as u16;
                        });
                        crate::dsp::media::load_user_media(&cx.p.state.read(), cx.shared);
                    }
                    Some(t) => edit_set(cx, gui, "", |s| s.n_type = t),
                    None => {}
                }
            }
            if let Some(i) = combo_s(ui, &sid("nenv"), at(188.0, 30.0, 110.0, 22.0), &kinds::NOISE_ENVS, set.n_env as usize, "Env", "Follow: the noise follows the input level · Duck: the noise drops while the input is loud.") {
                edit_set(cx, gui, "", |s| s.n_env = i as u8);
            }
            if btn(ui, at(304.0, 30.0, 52.0, 22.0), "Mono", set.n_mono, col, "Mono noise.").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.n_mono = !s.n_mono);
            }
            if btn(ui, at(360.0, 30.0, 78.0, 22.0), "Noise only", set.n_only, col, "Mute the input: only the noise comes out of this item.").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.n_only = !s.n_only);
            }
            slot_knobs(ui, cx, 0, n, kx, ky, step);
        }
        Kind::Crush => {
            if btn(ui, at(12.0, 30.0, 64.0, 22.0), "Dither", set.c_dither, col, "Noise before the bit reduction: smoother, hissy low levels.").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.c_dither = !s.c_dither);
            }
            if btn(ui, at(80.0, 30.0, 82.0, 22.0), "Anti-alias", set.c_prefilter, col, "Filters the input before the resampling: less fizzy aliasing.").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.c_prefilter = !s.c_prefilter);
            }
            slot_knobs(ui, cx, 0, n, kx, ky, step);
        }
        Kind::Drive => {
            if let Some(i) = combo_s(ui, &sid("dstyle"), at(12.0, 30.0, 140.0, 22.0), &kinds::DRIVE_STYLES, set.d_style as usize, "", "The distortion curve.") {
                edit_set(cx, gui, "", |s| s.d_style = i as u8);
            }
            slot_knobs(ui, cx, 0, n, kx, ky, step);
            drive_display(ui, cx, at(r.width() - 150.0, 26.0, 136.0, r.height() - 36.0), &set);
        }
        Kind::Reverb => {
            let mut names: Vec<String> = Vec::new();
            let mut map: Vec<Option<u16>> = Vec::new();
            let mut last = "";
            for (i, (c, nm)) in kinds::REVERBS.iter().enumerate() {
                if *c != last {
                    names.push(format!("──{c}"));
                    map.push(None);
                    last = c;
                }
                names.push(nm.to_string());
                map.push(Some(i as u16));
            }
            if !gui.ir_files.is_empty() {
                names.push("──Your impulse responses".into());
                map.push(None);
                for f in &gui.ir_files {
                    names.push(f.clone());
                    map.push(Some(u16::MAX));
                }
            }
            let cur_name = if set.r_type >= 1000 { cx.st.user_irs.get((set.r_type - 1000) as usize).cloned().unwrap_or("?".into()) } else { kinds::REVERBS.get(set.r_type as usize).map(|n| n.1.to_string()).unwrap_or_default() };
            let cur_i = names.iter().position(|n| *n == cur_name).unwrap_or(0);
            if let Some(i) = combo(ui, &sid("rtype"), at(12.0, 30.0, 170.0, 22.0), &names, cur_i, &cur_name, "The space. Your own impulse responses (WAV) go into the IRs folder (⚙ settings).") {
                match map.get(i).copied().flatten() {
                    Some(u16::MAX) => {
                        let name = names[i].clone();
                        gui.checkpoint(cx, "", cx.now);
                        let s = cx.sel;
                        commit(cx.p, cx.shared, |st| {
                            let k = crate::dsp::media::user_index(&mut st.user_irs, &name);
                            st.slots[s].r_type = 1000 + k as u16;
                        });
                        crate::dsp::media::load_user_media(&cx.p.state.read(), cx.shared);
                    }
                    Some(t) => edit_set(cx, gui, "", |s| s.r_type = t),
                    None => {}
                }
            }
            if btn(ui, at(188.0, 30.0, 60.0, 22.0), "Gated", set.r_gated, col, "Cuts the tail when the input stops (80s gated reverb).").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.r_gated = !s.r_gated);
            }
            if btn(ui, at(252.0, 30.0, 80.0, 22.0), "Clear tails", set.r_clear, col, "When the Volume shape drops to zero, the reverb is emptied at once (hard cuts).").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.r_clear = !s.r_clear);
            }
            slot_knobs(ui, cx, 0, n, kx, ky, step);
        }
        Kind::Liquid => {
            if let Some(i) = segmented(ui, at(12.0, 30.0, 200.0, 22.0), &kinds::LIQUID_TYPES, set.l_type as usize, col, "Flanger (short delay with feedback) or phaser (all-pass stages).") {
                if !modal {
                    edit_set(cx, gui, "", |s| s.l_type = i as u8);
                }
            }
            if btn(ui, at(218.0, 30.0, 76.0, 22.0), "Negative", set.l_neg, col, "Negative feedback: hollow instead of ringing.").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.l_neg = !s.l_neg);
            }
            if set.l_type == 2 {
                let st_names = ["2 stages", "4 stages", "6 stages", "8 stages"];
                if let Some(i) = combo_s(ui, &sid("lst"), at(300.0, 30.0, 100.0, 22.0), &st_names, set.l_stages as usize, "", "Phaser stages: more = more notches.") {
                    edit_set(cx, gui, "", |s| s.l_stages = i as u8);
                }
            }
            slot_knobs(ui, cx, 0, n, kx, ky, step);
        }
        Kind::Compressor => {
            if let Some(i) = segmented(ui, at(12.0, 30.0, 200.0, 22.0), &["Band", "Free", "Sidechain"], set.comp_input as usize, col, "What it listens to: its own signal · the full input with the filters · the sidechain input.") {
                if !modal {
                    edit_set(cx, gui, "", |s| s.comp_input = i as u8);
                }
            }
            if btn(ui, at(218.0, 30.0, 86.0, 22.0), "Auto gain", set.comp_auto, col, "Makes up the level it takes away.").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.comp_auto = !s.comp_auto);
            }
            if btn(ui, at(308.0, 30.0, 76.0, 22.0), "Adaptive", set.comp_adaptive, col, "Fast release at first, slower in the tail.").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.comp_adaptive = !s.comp_adaptive);
            }
            slot_knobs(ui, cx, 0, n, kx, ky, step);
            // gain reduction now
            let g = cx.shared.gr[cx.sel].load(Ordering::Relaxed);
            let mr = at(r.width() - 40.0, 30.0, 12.0, r.height() - 52.0);
            vmeter(ui, mr, g / 24.0, HOT, "GR", true);
            ui.painter().text(Pos2::new(mr.left() - 6.0, mr.top() + 6.0), Align2::RIGHT_CENTER, format!("{g:.1} dB"), pf(10.0), TEXT);
        }
        Kind::Scope => {
            if let Some(i) = combo_s(ui, &sid("slen"), at(12.0, 30.0, 110.0, 22.0), &LENGTHS[2..10], (set.scope_len as usize).clamp(2, 9) - 2, "Window", "How much time the scope shows (synced to the tempo).") {
                edit_set(cx, gui, "", |s| s.scope_len = i as u8 + 2);
            }
            if btn(ui, at(128.0, 30.0, 100.0, 22.0), "Split L / R", set.scope_split, col, "Left above, right below the middle line.").clicked() && !modal {
                edit_set(cx, gui, "", |s| s.scope_split = !s.scope_split);
            }
            label(ui, Pos2::new(x0 + 16.0, y0 + 74.0), "Shows the signal at this point of the chain. Move the Scope around the chain to see each step.", DIM, 10.5);
        }
    }
}

/// Compressor sidechain filters / Scope info.
fn tool_side(ui: &mut Ui, cx: &Cx, gui: &mut Gui, r: Rect) {
    let col = cx.col;
    match cx.kind {
        Kind::Compressor => {
            panel(ui, r, "DETECTOR FILTERS (Free / Sidechain)", col);
            let env = cx.st.slots[cx.sel].env[0][0];
            let en = cx.st.slots[cx.sel].comp_input != 0 && !cx.modal;
            let ky = r.top() + 84.0;
            let s = cx.sel;
            if let Some(v) = sknob(ui, &format!("chp{s}"), &K_EHP, env.hp, Pos2::new(r.left() + 46.0, ky), 16.0, col, en) {
                gui.checkpoint(cx, "chp", cx.now);
                commit(cx.p, cx.shared, |st| st.slots[s].env[0][0].hp = v);
            }
            if let Some(v) = sknob(ui, &format!("clp{s}"), &K_ELP, env.lp, Pos2::new(r.left() + 120.0, ky), 16.0, col, en) {
                gui.checkpoint(cx, "clp", cx.now);
                commit(cx.p, cx.shared, |st| st.slots[s].env[0][0].lp = v);
            }
            label(ui, Pos2::new(r.left() + 180.0, r.top() + 60.0), "With Free or Sidechain the detector listens through these filters,", DIM, 10.5);
            label(ui, Pos2::new(r.left() + 180.0, r.top() + 76.0), "e.g. high-pass the sidechain to duck on the kick only.", DIM, 10.5);
            label(ui, Pos2::new(r.left() + 180.0, r.top() + 102.0), "Multiband: one compressor per band (bands panel).", FAINT, 10.0);
        }
        _ => {
            panel(ui, r, "SCOPE", col);
            label(ui, Pos2::new(r.left() + 16.0, r.top() + 50.0), "A Scope has no shape of its own: it passes the sound through unchanged.", DIM, 10.5);
            label(ui, Pos2::new(r.left() + 16.0, r.top() + 68.0), "Put one before and one after an item to compare.", DIM, 10.5);
        }
    }
}

fn filter_display(ui: &mut Ui, cx: &Cx, gr: Rect, set: &SlotSet) {
    let p = ui.painter();
    p.rect_filled(gr, CornerRadius::same(5), EDIT_BG);
    let b = cx.band;
    let v0 = cx.shared.value[cx.sel][0][b].load(Ordering::Relaxed);
    let v1 = cx.shared.value[cx.sel][1][b].load(Ordering::Relaxed);
    let fc = 20.6 * (21100.0f32 / 20.6).powf(v0);
    let mut k = 2.0 * (1.0 - v1).powf(1.5) + 0.03;
    if set.f_warm && set.f_safe {
        k = k.max(0.12);
    }
    let fx = |f: f32| gr.left() + gr.width() * ((f / 20.0).ln() / (1000.0f32).ln());
    for f in [100.0f32, 1000.0, 10000.0] {
        p.vline(fx(f), gr.y_range(), Stroke::new(1.0f32, LINE));
        p.text(Pos2::new(fx(f) + 3.0, gr.bottom() - 7.0), Align2::LEFT_CENTER, if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") }, pf(8.5), FAINT);
    }
    let y0db = gr.top() + gr.height() * 0.35;
    p.hline(gr.x_range(), y0db, Stroke::new(1.0f32, LINE));
    let n = 120;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let f = 20.0 * 1000f32.powf(i as f32 / n as f32);
            let db = filter_mag_db(set.f_type, set.f_slope, fc, k, f).clamp(-48.0, 24.0);
            Pos2::new(gr.left() + gr.width() * i as f32 / n as f32, (y0db - db / 48.0 * gr.height() * 0.65).clamp(gr.top() + 2.0, gr.bottom() - 2.0))
        })
        .collect();
    p.add(fill_under(&pts, gr.bottom(), cx.col, 70, gr.height()));
    p.add(Shape::line(pts, PathStroke::new(1.6f32, cx.col)));
    let fct = if fc >= 1000.0 { format!("{:.1} kHz", fc / 1000.0) } else { format!("{fc:.0} Hz") };
    p.text(gr.right_top() + Vec2::new(-6.0, 9.0), Align2::RIGHT_CENTER, format!("{fct} · res {:.0} %", v1 * 100.0), pf(9.5), DIM);
}

fn drive_display(ui: &mut Ui, cx: &Cx, gr: Rect, set: &SlotSet) {
    let p = ui.painter();
    p.rect_filled(gr, CornerRadius::same(5), EDIT_BG);
    p.hline(gr.x_range(), gr.center().y, Stroke::new(1.0f32, LINE));
    p.vline(gr.center().x, gr.y_range(), Stroke::new(1.0f32, LINE));
    let v = cx.shared.value[cx.sel][0][cx.band].load(Ordering::Relaxed);
    let kn_in = cx.kind.knobs()[0].plain(cx.p.slots[cx.sel].knob(0).unmodulated_normalized_value());
    let push = cx.kind.knobs()[2].plain(cx.p.slots[cx.sel].knob(2).unmodulated_normalized_value()) * 0.6;
    let g = 10f32.powf((kn_in + v * 36.0) / 20.0);
    let n = 400;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let x = i as f32 / n as f32 * 2.0 - 1.0;
            let y = drive_curve(set.d_style, x * g + push) - drive_curve(set.d_style, push);
            let blend = (v * 4.0).min(1.0);
            let y = x + (y - x) * blend;
            Pos2::new(gr.center().x + x * gr.width() * 0.45, gr.center().y - y.clamp(-1.1, 1.1) * gr.height() * 0.42)
        })
        .collect();
    p.add(Shape::line(pts, PathStroke::new(1.6f32, cx.col)));
    p.text(gr.right_top() + Vec2::new(-6.0, 9.0), Align2::RIGHT_CENTER, format!("drive {:.0} %", v * 100.0), pf(9.5), DIM);
}

fn vectorscope(ui: &mut Ui, cx: &Cx, gr: Rect) {
    let p = ui.painter();
    p.rect_filled(gr, CornerRadius::same(5), EDIT_BG);
    let c = gr.center();
    let rad = gr.width().min(gr.height()) * 0.45;
    p.line_segment([c - Vec2::new(rad * 0.7, rad * 0.7), c + Vec2::new(rad * 0.7, rad * 0.7)], Stroke::new(1.0f32, LINE));
    p.line_segment([c - Vec2::new(-rad * 0.7, rad * 0.7), c + Vec2::new(-rad * 0.7, rad * 0.7)], Stroke::new(1.0f32, LINE));
    p.vline(c.x, gr.y_range(), Stroke::new(1.0f32, LINE));
    let ring = cx.shared.ring.lock();
    let (buf, w) = (&ring.0, ring.1);
    let n = buf.len();
    let col = cx.col.gamma_multiply(0.7);
    for k in (0..n).step_by(2) {
        let s = buf[(w + k) % n];
        // mid up, side across
        let m = (s[0] + s[1]) * 0.5;
        let sd = (s[0] - s[1]) * 0.5;
        let pt = Pos2::new(c.x + (sd * rad * 1.4).clamp(-rad, rad), c.y - (m * rad * 1.4).clamp(-rad, rad));
        p.circle_filled(pt, 0.9, col);
    }
    p.text(gr.left_top() + Vec2::new(6.0, 9.0), Align2::LEFT_CENTER, "L", pf(8.5), FAINT);
    p.text(gr.right_top() + Vec2::new(-6.0, 9.0), Align2::RIGHT_CENTER, "R", pf(8.5), FAINT);
}

// ------------------------------------------------------------------------------------------
// Bands
// ------------------------------------------------------------------------------------------

fn bands_panel(ui: &mut Ui, cx: &Cx, gui: &mut Gui, r: Rect) {
    let col = cx.col;
    let set = cx.st.slots[cx.sel];
    panel(ui, r, "BANDS", col);
    let x0 = r.left();
    let y0 = r.top();
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(Pos2::new(x0 + x, y0 + y), Vec2::new(w, h));
    let modal = cx.modal;
    if btn(ui, at(70.0, 7.0, 84.0, 19.0), "Multiband", set.multiband, col, "Split into low / mid / high, each band with its own shape, LFO, envelope and mix.").clicked() && !modal {
        edit_set(cx, gui, "", |s| s.multiband = !s.multiband);
        if set.multiband {
            commit(cx.p, cx.shared, |st| st.band = 0);
        }
    }
    if !set.multiband {
        label(ui, Pos2::new(x0 + 16.0, y0 + 60.0), "One band (off).", DIM, 11.0);
        label(ui, Pos2::new(x0 + 16.0, y0 + 80.0), "Switch on to shape the lows, mids and", FAINT, 10.0);
        label(ui, Pos2::new(x0 + 16.0, y0 + 96.0), "highs on their own (mono bass, kick duck...).", FAINT, 10.0);
        return;
    }
    if let Some(i) = segmented(ui, at(160.0, 7.0, 128.0, 19.0), &kinds::SLOPES, set.slope as usize, col, "Slope of the crossovers (the bands always add up flat).") {
        if !modal {
            edit_set(cx, gui, "", |s| s.slope = i as u8);
        }
    }
    // the split diagram: drag the lines
    let dr = at(12.0, 32.0, r.width() - 24.0, 48.0);
    let painter = ui.painter().clone();
    painter.rect_filled(dr, CornerRadius::same(4), EDIT_BG);
    let fx = |f: f32| dr.left() + dr.width() * ((f / 20.0).ln() / 1000f32.ln()).clamp(0.0, 1.0);
    let ffrom = |x: f32| 20.0 * 1000f32.powf(((x - dr.left()) / dr.width()).clamp(0.0, 1.0));
    let xs = [dr.left(), fx(set.x_lo), fx(set.x_hi), dr.right()];
    let solo = cx.shared.solo[cx.sel].load(Ordering::Relaxed);
    for b in 0..N_BANDS {
        let br = Rect::from_x_y_ranges(xs[b]..=xs[b + 1], dr.y_range());
        let is = b == cx.band;
        painter.rect_filled(br.shrink(1.0), CornerRadius::same(3), if is { col.gamma_multiply(0.22) } else { Color32::TRANSPARENT });
        painter.text(Pos2::new(br.center().x, br.top() + 12.0), Align2::CENTER_CENTER, BAND_NAMES[b], pf(9.5), if is { TEXT } else { DIM });
        // solo
        let sr = Rect::from_center_size(Pos2::new(br.center().x, br.bottom() - 12.0), Vec2::new(22.0, 16.0));
        let on = solo & (1 << b) != 0;
        if br.width() > 26.0 && btn_f(ui, sr, "S", on, GOLD, "Solo this band (listen to it alone). Shift: add to the solo.", 9.5).clicked() && !modal {
            let add = ui.input(|i| i.modifiers.shift);
            let ns = if on { solo & !(1 << b) } else if add { solo | (1 << b) } else { 1 << b };
            cx.shared.solo[cx.sel].store(ns, Ordering::Relaxed);
        }
    }
    for (k, &f) in [set.x_lo, set.x_hi].iter().enumerate() {
        let x = fx(f);
        let lr = Rect::from_center_size(Pos2::new(x, dr.center().y), Vec2::new(10.0, dr.height()));
        let resp = ui.interact(lr, egui::Id::new(("split", cx.sel, k)), Sense::drag()).on_hover_text("Drag: move the split · double-click: default");
        let hot = resp.hovered() || resp.dragged();
        painter.vline(x, dr.y_range(), Stroke::new(if hot { 2.5f32 } else { 1.5 }, if hot { TEXT } else { col }));
        let ft = if f >= 1000.0 { format!("{:.1}k", f / 1000.0) } else { format!("{f:.0}") };
        painter.text(Pos2::new(x, dr.bottom() + 8.0), Align2::CENTER_CENTER, ft, pf(9.0), DIM);
        if !modal {
            if resp.dragged() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let v = ffrom(pos.x);
                    gui.checkpoint(cx, "split", cx.now);
                    edit_set(cx, gui, "split", |s| {
                        if k == 0 {
                            s.x_lo = v.min(s.x_hi / 1.1);
                        } else {
                            s.x_hi = v.max(s.x_lo * 1.1);
                        }
                    });
                }
            }
            if resp.double_clicked() {
                edit_set(cx, gui, "", |s| {
                    if k == 0 {
                        s.x_lo = 200.0;
                    } else {
                        s.x_hi = 2500.0;
                    }
                });
            }
        }
    }
    let _ = (K_XLO, K_XHI);
    // band mixes
    let ky = y0 + r.height() - 40.0;
    for b in 0..N_BANDS {
        let p = cx.p.slots[cx.sel].band_mix(b);
        let c = Pos2::new(x0 + 54.0 + b as f32 * 96.0, ky);
        let name = ["Low mix", "Mid mix", "High mix"][b];
        if modal {
            let _ = knob_core(ui, egui::Id::new(("bm", b)), c, 13.0, p.unmodulated_normalized_value(), 1.0, name, &p.to_string(), &|_| None, "", false, col, false);
        } else {
            pknob(ui, cx.setter, p, name, c, 13.0, "How much of the effect this band gets.", false, col);
        }
    }
}

// ------------------------------------------------------------------------------------------
// Output
// ------------------------------------------------------------------------------------------

fn output_panel(ui: &mut Ui, cx: &Cx, _gui: &mut Gui, r: Rect) {
    let col = cx.col;
    panel(ui, r, "OUTPUT", col);
    let p = cx.p;
    let c = |x: f32, y: f32| Pos2::new(r.left() + x, r.top() + y);
    let knobs: [(&FloatParam, &str, &str, bool, Pos2); 4] = [
        (&p.slots[cx.sel].mix, "Item mix", "Dry / wet of this item.", false, c(52.0, 52.0)),
        (&p.mix, "Mix", "Dry / wet of the whole plugin.", false, c(148.0, 52.0)),
        (&p.in_gain, "Input", "Level into the chain.", true, c(52.0, 112.0)),
        (&p.out_gain, "Output", "Level out of the plugin.", true, c(148.0, 112.0)),
    ];
    for (prm, name, tip, bip, pos) in knobs {
        let kcol = if name == "Item mix" { col } else { TEXT };
        if cx.modal {
            let _ = knob_core(ui, egui::Id::new(("out", name)), pos, 14.0, prm.unmodulated_normalized_value(), 1.0, name, &prm.to_string(), &|_| None, tip, bip, kcol, false);
        } else {
            pknob(ui, cx.setter, prm, name, pos, 14.0, tip, bip, kcol);
        }
    }
}

// ------------------------------------------------------------------------------------------
// Tool views (in place of the shape editor)
// ------------------------------------------------------------------------------------------

pub fn tool_view(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    let er = cx.r(14.0, 160.0, BASE_W - 28.0, 324.0);
    let painter = ui.painter().clone();
    painter.rect_filled(er, CornerRadius::same(8), EDIT_BG);
    painter.rect_stroke(er, CornerRadius::same(8), Stroke::new(1.0f32, LINE), StrokeKind::Inside);
    let col = cx.col;
    let set = cx.st.slots[cx.sel];
    match cx.kind {
        Kind::Compressor => {
            // gain reduction over time (left) and the curve (right)
            let hr = Rect::from_min_max(er.min + Vec2::new(14.0, 26.0), Pos2::new(er.right() - 330.0, er.bottom() - 16.0));
            painter.text(er.min + Vec2::new(14.0, 13.0), Align2::LEFT_CENTER, "GAIN REDUCTION", pf(9.5), DIM);
            for db in [3.0f32, 6.0, 12.0, 18.0, 24.0] {
                let y = hr.top() + hr.height() * db / 24.0;
                painter.hline(hr.x_range(), y, Stroke::new(1.0f32, Color32::from_rgb(0x1c, 0x20, 0x27)));
                painter.text(Pos2::new(hr.right() + 4.0, y), Align2::LEFT_CENTER, format!("-{db:.0}"), pf(8.5), FAINT);
            }
            let n = gui.gr_hist.len();
            let pts: Vec<Pos2> = gui.gr_hist.iter().enumerate().map(|(i, g)| Pos2::new(hr.left() + hr.width() * i as f32 / (n - 1) as f32, hr.top() + hr.height() * (g / 24.0).clamp(0.0, 1.0))).collect();
            let mut top: Vec<Pos2> = vec![Pos2::new(hr.left(), hr.top())];
            top.extend(pts.iter().copied());
            top.push(Pos2::new(hr.right(), hr.top()));
            painter.add(Shape::line(pts, PathStroke::new(1.8f32, HOT)));
            // curve
            let cr = Rect::from_min_size(Pos2::new(er.right() - 300.0, er.top() + 26.0), Vec2::splat(er.height() - 42.0));
            painter.text(Pos2::new(cr.left(), er.top() + 13.0), Align2::LEFT_CENTER, "CURVE (in → out, dB)", pf(9.5), DIM);
            painter.rect_filled(cr, CornerRadius::same(4), PANEL);
            let defs = cx.kind.knobs();
            let thr = defs[0].plain(cx.p.slots[cx.sel].knob(0).unmodulated_normalized_value());
            let ratio = defs[1].plain(cx.p.slots[cx.sel].knob(1).unmodulated_normalized_value());
            let mk = defs[5].plain(cx.p.slots[cx.sel].knob(5).unmodulated_normalized_value());
            let slope = 1.0 - 1.0 / ratio.max(1.0);
            let auto = if set.comp_auto { -thr * slope * 0.5 } else { 0.0 };
            let map = |db: f32| (db + 60.0) / 60.0;
            let pt = |i: f32, o: f32| Pos2::new(cr.left() + cr.width() * map(i), cr.bottom() - cr.height() * map(o).clamp(0.0, 1.0));
            for db in [-48.0f32, -36.0, -24.0, -12.0] {
                painter.vline(cr.left() + cr.width() * map(db), cr.y_range(), Stroke::new(1.0f32, LINE));
                painter.hline(cr.x_range(), cr.bottom() - cr.height() * map(db), Stroke::new(1.0f32, LINE));
            }
            painter.line_segment([pt(-60.0, -60.0), pt(0.0, 0.0)], Stroke::new(1.0f32, FAINT));
            let curve: Vec<Pos2> = (0..=60).map(|k| {
                let i = -60.0 + k as f32;
                let o = i - (i - thr).max(0.0) * slope + mk + auto;
                pt(i, o)
            }).collect();
            painter.add(Shape::line(curve, PathStroke::new(2.0f32, col)));
            painter.circle_stroke(pt(thr, thr + mk + auto), 4.0, Stroke::new(1.5f32, TEXT));
            let g = cx.shared.gr[cx.sel].load(Ordering::Relaxed);
            painter.text(Pos2::new(hr.right() - 4.0, hr.bottom() - 10.0), Align2::RIGHT_CENTER, format!("{g:.1} dB  ·  threshold {thr:.1} dB  ·  {ratio:.1}:1"), pf(11.0), TEXT);
        }
        _ => {
            // Scope: the signal at this point, beat-synced (L above, R below when split)
            let inner = er.shrink2(Vec2::new(12.0, 18.0));
            let bw = inner.width() / SCOPE_N as f32;
            let mid = inner.center().y;
            painter.hline(inner.x_range(), mid, Stroke::new(1.0f32, LINE));
            let beats = kinds::length_beats(set.scope_len, 0);
            let divs = (beats.round() as usize).clamp(1, 32);
            for k in 1..divs {
                painter.vline(inner.left() + inner.width() * k as f32 / divs as f32, inner.y_range(), Stroke::new(1.0f32, Color32::from_rgb(0x1c, 0x20, 0x27)));
            }
            let hh = inner.height() * 0.5;
            for i in 0..SCOPE_N {
                let x0 = inner.left() + i as f32 * bw;
                let l = gui.scope_in[i].min(1.0).sqrt() * hh;
                let r = gui.scope_out[i].min(1.0).sqrt() * hh;
                if set.scope_split {
                    painter.rect_filled(Rect::from_min_max(Pos2::new(x0, mid - l), Pos2::new(x0 + bw - 0.5, mid)), CornerRadius::ZERO, col.gamma_multiply(0.8));
                    painter.rect_filled(Rect::from_min_max(Pos2::new(x0, mid), Pos2::new(x0 + bw - 0.5, mid + r)), CornerRadius::ZERO, col.gamma_multiply(0.5));
                } else {
                    let m = l.max(r);
                    painter.rect_filled(Rect::from_min_max(Pos2::new(x0, mid - m), Pos2::new(x0 + bw - 0.5, mid + m)), CornerRadius::ZERO, col.gamma_multiply(0.7));
                }
            }
            painter.text(er.min + Vec2::new(14.0, 11.0), Align2::LEFT_CENTER, format!("SCOPE · {} · at position {} of the chain", LENGTHS[set.scope_len as usize], cx.st.chain.iter().position(|&c| c as usize == cx.sel).map(|p| p + 1).unwrap_or(0)), pf(9.5), DIM);
            if set.scope_split {
                painter.text(Pos2::new(inner.left() + 4.0, inner.top() + 6.0), Align2::LEFT_CENTER, "L", pf(9.0), FAINT);
                painter.text(Pos2::new(inner.left() + 4.0, inner.bottom() - 6.0), Align2::LEFT_CENTER, "R", pf(9.0), FAINT);
            }
            if btn(ui, Rect::from_min_size(er.right_top() + Vec2::new(-70.0, 4.0), Vec2::new(60.0, 18.0)), "Freeze", gui.freeze, GOLD, "Freeze the picture.").clicked() {
                gui.freeze = !gui.freeze;
            }
        }
    }
}
