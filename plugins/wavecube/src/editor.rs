//! WaveCube GUI: preset bar on top, the chain of shapers as coloured tabs, the shape editor with
//! the scope of the audio behind it, user waves, and the shaper / envelope / LFO / multiband /
//! output panels below. A preset browser and a save dialog open over the editor.

use nih_plug::prelude::*;
use nih_plug_egui::{
    create_egui_editor,
    egui::{self, epaint::PathStroke, Align2, Color32, CornerRadius, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2},
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::presets::{self, PresetData, CATEGORIES};
use crate::wave::{bend, Pen, Preset, Wave};
use crate::{
    DriveStyle, EnvMode, FilterType, Kind, Mode, NoiseEnv, NoiseType, PanMode, Shared, Slot, TimeRange, Trigger, WaveCubeParams, GRIDS, N_BANDS,
    N_SHAPERS, N_SLOTS, SCOPE_N,
};

// palette
const BG: Color32 = Color32::from_rgb(0x10, 0x12, 0x16);
const PANEL: Color32 = Color32::from_rgb(0x17, 0x1a, 0x20);
const PANEL2: Color32 = Color32::from_rgb(0x21, 0x25, 0x2d);
const EDIT_BG: Color32 = Color32::from_rgb(0x0c, 0x0e, 0x12);
const LINE: Color32 = Color32::from_rgb(0x2b, 0x30, 0x39);
const TEXT: Color32 = Color32::from_rgb(0xe8, 0xea, 0xf0);
const DIM: Color32 = Color32::from_rgb(0x8a, 0x91, 0xa0);
const FAINT: Color32 = Color32::from_rgb(0x4b, 0x51, 0x5d);
const HOT: Color32 = Color32::from_rgb(0xf2, 0x5c, 0x6a);
const GOLD: Color32 = Color32::from_rgb(0xf5, 0xc5, 0x42);

pub const BASE_W: f32 = 1100.0;
pub const BASE_H: f32 = 760.0;

const BAND_NAMES: [&str; N_BANDS] = ["LOW", "MID", "HIGH"];
const SLOT_NOTES: [&str; N_SLOTS] = ["C#", "D", "D#", "E", "F", "F#", "G", "G#", "A"];

/// The colour of each shaper.
pub fn kc(k: Kind) -> Color32 {
    match k {
        Kind::Volume => Color32::from_rgb(0x5a, 0xb8, 0xf0),
        Kind::Filter => Color32::from_rgb(0xf5, 0xb1, 0x4c),
        Kind::Pan => Color32::from_rgb(0x8b, 0xd4, 0x5a),
        Kind::Width => Color32::from_rgb(0xb0, 0x8c, 0xf7),
        Kind::Time => Color32::from_rgb(0xf2, 0x70, 0x9c),
        Kind::Noise => Color32::from_rgb(0x4f, 0xd1, 0xc5),
        Kind::Crush => Color32::from_rgb(0xf2, 0x69, 0x5c),
        Kind::Drive => Color32::from_rgb(0xf0, 0x8c, 0x3a),
    }
}

fn pf(size: f32) -> FontId {
    FontId::proportional(size)
}

#[derive(Clone, Copy)]
enum Drag {
    Point(usize),
    Curve(usize, f32, f32),
    Paint,
}

struct Library {
    factory: Vec<PresetData>,
    user: Vec<PresetData>,
    favs: Vec<String>,
}

impl Library {
    fn load() -> Self {
        Self { factory: presets::factory(), user: presets::list_user(), favs: presets::load_favorites() }
    }
    fn key(user: bool, name: &str) -> String {
        format!("{}:{name}", if user { "U" } else { "F" })
    }
    fn all(&self) -> Vec<(bool, &PresetData)> {
        self.factory.iter().map(|p| (false, p)).chain(self.user.iter().map(|p| (true, p))).collect()
    }
}

struct Gui {
    in_lv: f32,
    out_lv: f32,
    scope_in: [f32; SCOPE_N],
    scope_out: [f32; SCOPE_N],
    drag: Option<Drag>,
    undo: Vec<(usize, usize, Wave)>,
    redo: Vec<(usize, usize, Wave)>,
    clip: Option<Wave>,
    pen: Option<Pen>,
    snap: bool,
    freeze: bool,
    lib: Option<Library>,
    browser: bool,
    save_open: bool,
    save_name: String,
    save_cat: usize,
    search: String,
    filter: usize,
    status: Option<(String, f64)>,
    last_switch: u32,
    seed: u32,
}

fn set_norm<P: Param>(setter: &ParamSetter, p: &P, v: f32) {
    setter.begin_set_parameter(p);
    setter.set_parameter_normalized(p, v.clamp(0.0, 1.0));
    setter.end_set_parameter(p);
}
fn set_b(setter: &ParamSetter, p: &BoolParam, v: bool) {
    setter.begin_set_parameter(p);
    setter.set_parameter(p, v);
    setter.end_set_parameter(p);
}
fn set_e<E: Enum + PartialEq>(setter: &ParamSetter, p: &EnumParam<E>, v: E) {
    setter.begin_set_parameter(p);
    setter.set_parameter(p, v);
    setter.end_set_parameter(p);
}

fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("dejavu".into(), Arc::new(FontData::from_static(include_bytes!("fonts/DejaVuSans.ttf"))));
    for fam in [FontFamily::Monospace, FontFamily::Proportional] {
        if let Some(list) = fonts.families.get_mut(&fam) {
            list.push("dejavu".into());
        }
    }
    ctx.set_fonts(fonts);
}

/// Toggle / action button. `col` = colour when on.
fn btn(ui: &mut Ui, rect: Rect, text: &str, on: bool, col: Color32, tip: &str) -> egui::Response {
    let mut resp = ui.allocate_rect(rect, Sense::click());
    if !tip.is_empty() {
        resp = resp.on_hover_text(tip);
    }
    let p = ui.painter();
    let hov = resp.hovered();
    let fill = if on { col } else if hov { PANEL2 } else { PANEL };
    p.rect_filled(rect, CornerRadius::same(5), fill);
    p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0f32, if on { col } else if hov { FAINT } else { LINE }), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, text, pf(11.5), if on { BG } else { TEXT });
    resp
}

fn panel(ui: &Ui, rect: Rect, title: &str, col: Color32) {
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(8), PANEL);
    p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0f32, LINE), StrokeKind::Inside);
    if !title.is_empty() {
        p.circle_filled(Pos2::new(rect.left() + 14.0, rect.top() + 15.0), 3.0, col);
        p.text(Pos2::new(rect.left() + 24.0, rect.top() + 15.0), Align2::LEFT_CENTER, title, pf(11.5), DIM);
    }
}

/// A combo box over a list of names; returns the picked index.
fn combo(ui: &mut Ui, id: &str, rect: Rect, names: &[&str], idx: usize, shown: &str, tip: &str) -> Option<usize> {
    let mut pick = None;
    ui.allocate_new_ui(UiBuilder::new().max_rect(rect), |ui| {
        let r = egui::ComboBox::from_id_salt(id)
            .width(rect.width() - 22.0)
            .height(460.0)
            .selected_text(egui::RichText::new(shown).font(pf(12.0)))
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    if n.starts_with("──") {
                        ui.label(egui::RichText::new(n.trim_start_matches('─').trim()).font(pf(10.5)).color(DIM));
                        continue;
                    }
                    if ui.selectable_label(i == idx, egui::RichText::new(*n).font(pf(12.5))).clicked() {
                        pick = Some(i);
                    }
                }
            });
        if !tip.is_empty() {
            r.response.on_hover_text(tip);
        }
    });
    pick
}

fn enum_combo<E: Enum + PartialEq + Copy + 'static>(ui: &mut Ui, setter: &ParamSetter, p: &EnumParam<E>, id: &str, rect: Rect, label: &str, tip: &str) {
    let cur = p.value();
    let names = E::variants();
    let shown = if label.is_empty() { names[cur.to_index()].to_string() } else { format!("{label}  {}", names[cur.to_index()]) };
    if let Some(i) = combo(ui, id, rect, names, cur.to_index(), &shown, tip) {
        set_e(setter, p, E::from_index(i));
    }
}

/// Knob: drag (shift = fine), wheel, double-click = default, click the value to type.
#[allow(clippy::too_many_arguments)]
fn knob<P: Param>(ui: &mut Ui, setter: &ParamSetter, p: &P, label: &str, id: &str, c: Pos2, r: f32, tip: &str, bipolar: bool, col: Color32, vw: f32) {
    let hit = Rect::from_center_size(c, Vec2::splat(2.0 * r + 8.0));
    let resp = ui.allocate_rect(hit, Sense::click_and_drag());
    let resp = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    let norm = p.unmodulated_normalized_value();
    let raw_id = egui::Id::new(("knob_raw", id));
    if resp.drag_started() {
        setter.begin_set_parameter(p);
        ui.data_mut(|d| d.insert_temp(raw_id, norm));
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let d = -resp.drag_delta().y / if fine { 1200.0 } else { 220.0 };
        let raw = ui.data(|dd| dd.get_temp::<f32>(raw_id)).unwrap_or(norm) + d;
        ui.data_mut(|dd| dd.insert_temp(raw_id, raw));
        setter.set_parameter_normalized(p, raw.clamp(0.0, 1.0));
    }
    if resp.drag_stopped() {
        setter.end_set_parameter(p);
    }
    if resp.double_clicked() {
        set_norm(setter, p, p.default_normalized_value());
    }
    if resp.hovered() {
        let sc = ui.input(|i| i.smooth_scroll_delta.y);
        if sc != 0.0 {
            set_norm(setter, p, norm + sc / 1500.0);
        }
    }
    let norm = p.unmodulated_normalized_value();
    let painter = ui.painter();
    let a0 = std::f32::consts::PI * 0.75;
    let sweep = std::f32::consts::PI * 1.5;
    let ring = r - 2.0;
    let pt = |rad: f32, t: f32| -> Pos2 {
        let a = a0 + t * sweep;
        Pos2::new(c.x + rad * a.cos(), c.y + rad * a.sin())
    };
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let n = (((to - from).abs() / sweep) * 120.0).ceil().max(2.0) as usize;
        (0..=n).map(|i| pt(ring, from + (to - from) * i as f32 / n as f32)).collect()
    };
    painter.add(Shape::line(arc(0.0, 1.0), PathStroke::new(3.5f32, Color32::from_rgb(0x2a, 0x2f, 0x38))));
    let from = if bipolar { 0.5 } else { 0.0 };
    if (norm - from).abs() > 0.002 {
        painter.add(Shape::line(arc(from, norm), PathStroke::new(3.5f32, col)));
    }
    let body = r * 0.68;
    painter.circle_filled(c, body, Color32::from_rgb(0x26, 0x2b, 0x33));
    painter.circle_stroke(c, body, Stroke::new(1.0f32, Color32::from_rgb(0x3a, 0x40, 0x4b)));
    painter.circle_filled(pt(body * 0.62, norm), 2.4, TEXT);

    let val = p.normalized_value_to_string(norm, true);
    let vr = Rect::from_center_size(Pos2::new(c.x, c.y + r + 11.0), Vec2::new(vw, 16.0));
    let edit_id = egui::Id::new(("knob_edit", id));
    let editing: Option<String> = ui.data(|d| d.get_temp::<String>(edit_id));
    ui.painter().text(Pos2::new(c.x, vr.bottom() + 9.0), Align2::CENTER_CENTER, label, pf(10.5), DIM);
    if let Some(mut buf) = editing {
        let te = egui::TextEdit::singleline(&mut buf).font(pf(11.0)).horizontal_align(egui::Align::Center).desired_width(vr.width()).margin(egui::Margin::symmetric(2, 1));
        let r_te = ui.put(vr, te);
        if !r_te.has_focus() && !r_te.lost_focus() {
            r_te.request_focus();
        }
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if esc {
            ui.data_mut(|d| d.remove::<String>(edit_id));
        } else if enter || r_te.lost_focus() {
            let txt = buf.trim().replace(',', ".");
            if let Some(nv) = p.string_to_normalized_value(&txt) {
                set_norm(setter, p, nv);
            }
            ui.data_mut(|d| d.remove::<String>(edit_id));
        } else {
            ui.data_mut(|d| d.insert_temp(edit_id, buf));
        }
    } else {
        let vresp = ui.allocate_rect(vr, Sense::click()).on_hover_text("Click to type a value");
        let gw = ui.fonts(|f| f.layout_no_wrap(val.clone(), pf(11.0), TEXT).size().x);
        let size = if gw > vr.width() - 4.0 { (11.0 * (vr.width() - 4.0) / gw).max(7.5) } else { 11.0 };
        ui.painter().text(vr.center(), Align2::CENTER_CENTER, &val, pf(size), if vresp.hovered() { col } else { TEXT });
        if vresp.clicked() {
            let first = val.trim().split(' ').next().unwrap_or("").to_string();
            let same = p.string_to_normalized_value(&first).map(|m| p.normalized_value_to_string(m, true)).as_deref() == Some(val.as_str());
            let plain = if same { first } else { val.trim().to_string() };
            ui.data_mut(|d| d.insert_temp(edit_id, plain));
        }
    }
}

/// Horizontal peak meter, -36 .. 0 dBFS.
fn hmeter(ui: &Ui, rect: Rect, lin: f32, label: &str) {
    let p = ui.painter();
    p.text(Pos2::new(rect.left() - 6.0, rect.center().y), Align2::RIGHT_CENTER, label, pf(9.5), DIM);
    p.rect_filled(rect, CornerRadius::same(2), EDIT_BG);
    let db = 20.0 * lin.max(1e-6).log10();
    let t = ((db + 36.0) / 36.0).clamp(0.0, 1.0);
    let col = if db > -0.3 { HOT } else { Color32::from_rgb(0x7f, 0xd6, 0xa0) };
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.left() + rect.width() * t, rect.bottom())), CornerRadius::same(2), col);
}

/// Vertical 0..1 meter (envelope, gain reduction).
fn vmeter(ui: &Ui, rect: Rect, t: f32, col: Color32, label: &str, from_top: bool) {
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(2), EDIT_BG);
    let h = rect.height() * t.clamp(0.0, 1.0);
    let r = if from_top { Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + h)) } else { Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - h), rect.max) };
    p.rect_filled(r, CornerRadius::same(2), col);
    p.text(Pos2::new(rect.center().x, rect.bottom() + 9.0), Align2::CENTER_CENTER, label, pf(9.5), DIM);
}

/// The logo: an isometric cube, three faces in three shaper colours.
fn logo(ui: &Ui, c: Pos2, s: f32) {
    let p = ui.painter();
    let h = s * 0.5;
    let w = s * 0.866;
    let top = Pos2::new(c.x, c.y - s);
    let ul = Pos2::new(c.x - w, c.y - h);
    let ur = Pos2::new(c.x + w, c.y - h);
    let ll = Pos2::new(c.x - w, c.y + h);
    let lr = Pos2::new(c.x + w, c.y + h);
    let bot = Pos2::new(c.x, c.y + s);
    p.add(Shape::convex_polygon(vec![top, ur, c, ul], kc(Kind::Volume), Stroke::NONE));
    p.add(Shape::convex_polygon(vec![ul, c, bot, ll], kc(Kind::Width), Stroke::NONE));
    p.add(Shape::convex_polygon(vec![c, ur, lr, bot], kc(Kind::Time), Stroke::NONE));
    // a little wave across the front faces
    let pts: Vec<Pos2> = (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let x = ll.x + (lr.x - ll.x) * t;
            let base = if t < 0.5 { c.y + s * 0.15 + (t / 0.5) * h } else { c.y + s * 0.15 + ((1.0 - t) / 0.5) * h };
            Pos2::new(x, base - 0.25 * s * (t * std::f32::consts::TAU * 1.5).sin())
        })
        .collect();
    p.add(Shape::line(pts, PathStroke::new(1.6f32, BG)));
}

/// The shape as screen points; segments are sampled on their own so jumps stay vertical.
fn wave_path(w: &Wave, inner: Rect, res: f32) -> Vec<Pos2> {
    let mut out = Vec::new();
    let px = |x: f32, y: f32| Pos2::new(inner.left() + x * inner.width(), inner.bottom() - y * inner.height());
    for s in 0..w.pts.len() - 1 {
        let (a, b) = (w.pts[s], w.pts[s + 1]);
        out.push(px(a.x, a.y));
        if b.x - a.x <= 1e-7 {
            continue;
        }
        let n = if a.c.abs() < 1e-4 { 1 } else { (((b.x - a.x) * res).ceil() as usize).max(1) };
        for k in 1..=n {
            let t = k as f32 / n as f32;
            out.push(px(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * bend(t, a.c)));
        }
    }
    out
}

/// Gradient fill under a path down to `bottom`: `top` alpha at the line, nothing at the bottom.
fn fill_under(path: &[Pos2], bottom: f32, col: Color32, top_a: u8, height: f32) -> Mesh {
    let mut m = Mesh::default();
    let ca = |y: f32| {
        let t = ((bottom - y) / height.max(1.0)).clamp(0.0, 1.0);
        let a = (top_a as f32 * (0.25 + 0.75 * t)) as u8;
        Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), a)
    };
    let cb = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), (top_a as f32 * 0.08) as u8);
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (b.x - a.x).abs() < 0.01 {
            continue;
        }
        let i = m.vertices.len() as u32;
        m.colored_vertex(a, ca(a.y));
        m.colored_vertex(b, ca(b.y));
        m.colored_vertex(Pos2::new(b.x, bottom), cb);
        m.colored_vertex(Pos2::new(a.x, bottom), cb);
        m.add_triangle(i, i + 1, i + 2);
        m.add_triangle(i, i + 2, i + 3);
    }
    m
}

fn bump(shared: &Shared) {
    shared.version.fetch_add(1, Ordering::AcqRel);
}

fn edit_wave(params: &WaveCubeParams, shared: &Shared, s: usize, b: usize, f: impl FnOnce(&mut Wave)) {
    let mut st = params.state.write();
    f(&mut st.waves[s][b]);
    st.waves[s][b].sanitize();
    drop(st);
    bump(shared);
}

fn push_undo(gui: &mut Gui, s: usize, b: usize, w: &Wave) {
    if gui.undo.last().map(|u| u.0 == s && u.1 == b && &u.2 == w).unwrap_or(false) {
        return;
    }
    gui.undo.push((s, b, w.clone()));
    gui.redo.clear();
    if gui.undo.len() > 100 {
        gui.undo.remove(0);
    }
}

fn cycle_text(p: &crate::ShaperParams, tempo: f32) -> String {
    match p.mode.value() {
        Mode::Hz => format!("1 cycle = {:.3} s", 1.0 / p.hz.value().max(1e-3)),
        Mode::Pitch => "speed follows the MIDI note".into(),
        Mode::Beat => {
            let beats = p.rate.value().beats() * p.feel.value().factor();
            let s = beats * 60.0 / tempo.max(1.0) as f64;
            if s < 1.0 {
                format!("1 cycle = {:.0} ms", s * 1000.0)
            } else {
                format!("1 cycle = {:.2} s", s)
            }
        }
    }
}

type KnobRow<'a> = Vec<(&'a FloatParam, &'static str, &'static str, &'static str, bool)>;

/// Kind-specific knobs (after Amount): (param, label, id, tooltip, bipolar)
fn kind_knobs(p: &WaveCubeParams, k: Kind) -> KnobRow<'_> {
    match k {
        Kind::Volume => vec![(&p.v_trim, "Trim", "vtrim", "Level after the shaper, ±24 dB.", true)],
        Kind::Filter => vec![
            (&p.f_lo, "Low", "flo", "Cutoff at the bottom of the shape.", false),
            (&p.f_hi, "High", "fhi", "Cutoff at the top of the shape.", false),
            (&p.f_res, "Resonance", "fres", "Resonance (phasers: notch / peak sharpness).", false),
            (&p.f_stereo, "Stereo", "fst", "Moves the left and right cutoff apart, up to 60 semitones.", true),
        ],
        Kind::Pan => vec![
            (&p.p_haas, "Pan / Haas", "phaas", "0 %: level panning · 100 %: Haas panning (the far side is delayed).", false),
            (&p.p_haas_ms, "Haas", "phaasms", "Largest Haas delay, 0-40 ms.", false),
        ],
        Kind::Width => vec![(&p.w_gain, "Gain", "wgain", "Level compensation for the width changes.", true)],
        Kind::Time => vec![],
        Kind::Noise => vec![
            (&p.n_hp, "High-pass", "nhp", "High-pass on the noise.", false),
            (&p.n_lp, "Low-pass", "nlp", "Low-pass on the noise.", false),
            (&p.n_stereo, "Stereo", "nst", "0 %: mono noise · 100 %: independent left / right.", false),
            (&p.n_env_amt, "Env amt", "nea", "How much Follow / Duck acts.", false),
        ],
        Kind::Crush => vec![
            (&p.c_crunch, "Crunch", "ccr", "Wave-wrapping overdrive before the bit reduction.", false),
            (&p.c_push, "Push", "cpu", "DC offset into the crunch (removed after): asymmetric wrap.", true),
            (&p.c_bits, "Bits", "cbi", "16 down to 1 bit; Max = off.", false),
            (&p.c_rate, "Resample", "cra", "Sample rate the signal is held at.", false),
            (&p.c_jitter, "Jitter", "cji", "Random timing of the resampling.", false),
            (&p.c_lp, "Low-pass", "clp", "Output low-pass.", false),
            (&p.c_gain, "Gain", "cga", "Output level of the crush chain.", true),
        ],
        Kind::Drive => vec![
            (&p.d_in, "Input", "din", "Gain into the drive, -12..+36 dB.", true),
            (&p.d_out, "Output", "dout", "Output level, +12..-36 dB.", true),
            (&p.d_push, "Push", "dpu", "DC offset into the curve: asymmetry, even harmonics.", true),
            (&p.d_grip, "Grip", "dgr", "Makes the quiet parts quieter.", false),
            (&p.d_accent, "Accent", "dac", "Less gain compensation: the driven peaks of the shape get louder.", false),
            (&p.d_tone, "Tone", "dto", "Dark (low-pass) to bright, mixed in as the drive rises.", true),
        ],
    }
}

fn amount_tip(k: Kind) -> &'static str {
    match k {
        Kind::Volume => "How deep the volume follows the shape (100 %: the bottom is silence).",
        Kind::Filter => "How far the cutoff follows the shape (0 %: always at High).",
        Kind::Pan => "How far the sound moves (100 %: fully left / right).",
        Kind::Width => "How much the width follows the shape (100 %: mono to twice as wide).",
        Kind::Time => "Range of the delay: 100 % = the whole Range at the bottom of the shape.",
        Kind::Noise => "Noise level at the top of the shape.",
        Kind::Crush => "FX mix of the crush chain at the top of the shape.",
        Kind::Drive => "Drive amount at the top of the shape (up to +36 dB, gain compensated).",
    }
}

pub fn create(params: Arc<WaveCubeParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    let editor_state = params.editor_state.clone();
    let size_state = editor_state.clone();
    create_egui_editor(
        editor_state,
        Gui {
            in_lv: 0.0,
            out_lv: 0.0,
            scope_in: [0.0; SCOPE_N],
            scope_out: [0.0; SCOPE_N],
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            clip: None,
            pen: None,
            snap: true,
            freeze: false,
            lib: None,
            browser: false,
            save_open: false,
            save_name: String::new(),
            save_cat: 0,
            search: String::new(),
            filter: 0,
            status: None,
            last_switch: 0,
            seed: 1,
        },
        |ctx, _| {
            let mut v = egui::Visuals::dark();
            v.panel_fill = BG;
            v.window_fill = PANEL2;
            v.extreme_bg_color = EDIT_BG;
            v.override_text_color = Some(TEXT);
            v.window_stroke = Stroke::new(1.0f32, LINE);
            v.selection.bg_fill = Color32::from_rgb(0x34, 0x4a, 0x60);
            v.selection.stroke = Stroke::new(1.0f32, TEXT);
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                w.corner_radius = CornerRadius::same(5);
                w.bg_stroke = Stroke::new(1.0f32, LINE);
            }
            v.widgets.inactive.weak_bg_fill = PANEL;
            v.widgets.inactive.bg_fill = PANEL;
            v.widgets.hovered.weak_bg_fill = PANEL2;
            v.widgets.hovered.bg_fill = PANEL2;
            v.widgets.active.weak_bg_fill = PANEL2;
            v.widgets.open.weak_bg_fill = PANEL2;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0f32, FAINT);
            v.popup_shadow = egui::Shadow::NONE;
            v.window_shadow = egui::Shadow::NONE;
            ctx.set_visuals(v);
            install_fonts(ctx);
        },
        move |ctx, setter, gui| {
            ctx.request_repaint_after(Duration::from_millis(16));
            let p = &*params;
            let now = ctx.input(|i| i.time);
            if gui.lib.is_none() {
                gui.lib = Some(Library::load());
            }

            // developer helper for screenshots:
            // WAVECUBE_DEMO_SET="preset=<factory index>,sel=1,band=0,pen=1,browser=1,save=1,size=1320"
            static DEMO: std::sync::Once = std::sync::Once::new();
            DEMO.call_once(|| {
                for kv in std::env::var("WAVECUBE_DEMO_SET").unwrap_or_default().split(',') {
                    let Some((k, v)) = kv.split_once('=') else { continue };
                    let Ok(v) = v.trim().parse::<f32>() else { continue };
                    match k.trim() {
                        "preset" => {
                            if let Some(d) = gui.lib.as_ref().and_then(|l| l.factory.get(v as usize)).cloned() {
                                presets::apply(setter, p, &d, None);
                            }
                        }
                        "sel" => p.state.write().selected = v as u8,
                        "band" => p.state.write().band = v as u8,
                        "pen" => gui.pen = if v > 0.5 { Some(Pen::ALL[(v as usize - 1).min(5)]) } else { None },
                        "browser" => gui.browser = v > 0.5,
                        "save" => {
                            gui.save_open = v > 0.5;
                            gui.save_name = "My pump".into();
                        }
                        "size" => size_state.set_requested_size((v as u32, (v * BASE_H / BASE_W) as u32)),
                        _ => {}
                    }
                }
                p.state.write().sanitize();
                bump(&shared);
            });

            // MIDI Switch from the audio thread: show the loaded user wave in the editor
            let sw = shared.switched.load(Ordering::Acquire);
            if sw != gui.last_switch {
                gui.last_switch = sw;
                let slot = (sw & 0xff) as usize;
                let mut st = p.state.write();
                if let Some(Some(s)) = st.slots.get(slot).cloned() {
                    st.waves[s.shaper as usize][s.band as usize] = s.wave;
                }
                drop(st);
                bump(&shared);
            }

            let st = p.state.read().clone();
            let sel = (st.selected as usize).min(N_SHAPERS - 1);
            let kind = Kind::ALL[sel];
            let col = kc(kind);
            let sp = &p.sh[sel];
            let multi = sp.multiband.value();
            let band = if multi { (st.band as usize).min(N_BANDS - 1) } else { 0 };
            shared.view.store(sel as u32, Ordering::Relaxed);

            let fall = |cur: f32, new: f32| if new > cur { new } else { cur * 0.86 };
            gui.in_lv = fall(gui.in_lv, shared.in_pk.swap(0.0, Ordering::Relaxed));
            gui.out_lv = fall(gui.out_lv, shared.out_pk.swap(0.0, Ordering::Relaxed));
            if !gui.freeze {
                for i in 0..SCOPE_N {
                    let a = shared.scope_in[i].load(Ordering::Relaxed);
                    let b = shared.scope_out[i].load(Ordering::Relaxed);
                    gui.scope_in[i] += (a - gui.scope_in[i]) * 0.5;
                    gui.scope_out[i] += (b - gui.scope_out[i]) * 0.5;
                }
            }
            let tempo = shared.tempo.load(Ordering::Relaxed);
            let (undo_key, redo_key) = ctx.input(|i| {
                let z = i.modifiers.command && i.key_pressed(egui::Key::Z);
                (z && !i.modifiers.shift, (z && i.modifiers.shift) || (i.modifiers.command && i.key_pressed(egui::Key::Y)))
            });

            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG)).show(ctx, |ui| {
                let full = ui.max_rect();
                let o = full.min;
                let rect = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(Pos2::new(o.x + x, o.y + y), Vec2::new(w, h));
                let at = |x: f32, y: f32| Pos2::new(o.x + x, o.y + y);
                let painter = ui.painter().clone();
                let set_state = |f: &dyn Fn(&mut crate::State)| {
                    let mut s = p.state.write();
                    f(&mut s);
                    drop(s);
                    bump(&shared);
                };
                let modal = gui.browser || gui.save_open;

                // ================= header
                logo(ui, at(30.0, 29.0), 14.0);
                painter.text(at(52.0, 29.0), Align2::LEFT_CENTER, "WaveCube", FontId::proportional(21.0), TEXT);

                // preset bar
                let lib = gui.lib.as_ref().unwrap();
                let filtered: Vec<(bool, PresetData)> = {
                    let q = gui.search.to_lowercase();
                    lib.all()
                        .into_iter()
                        .filter(|(u, d)| {
                            let cat_ok = match gui.filter {
                                0 => true,
                                1 => lib.favs.contains(&Library::key(*u, &d.name)),
                                2 => !*u,
                                3 => *u,
                                i => d.category == CATEGORIES[(i - 4).min(CATEGORIES.len() - 1)],
                            };
                            cat_ok && (q.is_empty() || d.name.to_lowercase().contains(&q))
                        })
                        .map(|(u, d)| (u, d.clone()))
                        .collect()
                };
                let cur_idx = filtered.iter().position(|(_, d)| d.name == st.preset);
                let cat = lib.all().into_iter().find(|(_, d)| d.name == st.preset).map(|(_, d)| d.category.clone()).unwrap_or_default();
                let cur_is_user = lib.user.iter().any(|d| d.name == st.preset);
                let cur_key = Library::key(cur_is_user, &st.preset);
                let cur_fav = lib.favs.contains(&cur_key);
                let px0 = 262.0;
                let mut load: Option<PresetData> = None;
                if btn(ui, rect(px0, 16.0, 28.0, 26.0), "◀", false, col, "Previous preset").clicked() && !filtered.is_empty() {
                    let i = cur_idx.map(|i| (i + filtered.len() - 1) % filtered.len()).unwrap_or(0);
                    load = Some(filtered[i].1.clone());
                }
                let nr = rect(px0 + 32.0, 16.0, 300.0, 26.0);
                let nresp = ui.allocate_rect(nr, Sense::click()).on_hover_text("Open the preset browser");
                painter.rect_filled(nr, CornerRadius::same(5), if nresp.hovered() || gui.browser { PANEL2 } else { EDIT_BG });
                painter.rect_stroke(nr, CornerRadius::same(5), Stroke::new(1.0f32, if gui.browser { col } else { LINE }), StrokeKind::Inside);
                if !cat.is_empty() {
                    painter.text(nr.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, cat.to_uppercase(), pf(9.5), DIM);
                }
                painter.text(nr.center() + Vec2::new(if cat.is_empty() { 0.0 } else { 24.0 }, 0.0), Align2::CENTER_CENTER, &st.preset, pf(13.0), TEXT);
                painter.text(nr.right_center() - Vec2::new(10.0, 0.0), Align2::RIGHT_CENTER, if gui.browser { "▴" } else { "▾" }, pf(11.0), DIM);
                if nresp.clicked() {
                    gui.browser = !gui.browser;
                    gui.save_open = false;
                    if gui.browser {
                        if let Some(l) = gui.lib.as_mut() {
                            l.user = presets::list_user();
                        }
                    }
                }
                if btn(ui, rect(px0 + 336.0, 16.0, 28.0, 26.0), "▶", false, col, "Next preset").clicked() && !filtered.is_empty() {
                    let i = cur_idx.map(|i| (i + 1) % filtered.len()).unwrap_or(0);
                    load = Some(filtered[i].1.clone());
                }
                if btn(ui, rect(px0 + 370.0, 16.0, 56.0, 26.0), "SAVE", gui.save_open, col, "Save the whole plugin as a preset").clicked() {
                    gui.save_open = !gui.save_open;
                    gui.browser = false;
                    if gui.save_name.is_empty() || gui.save_name == "Init" {
                        gui.save_name = if st.preset == "Init" { String::new() } else { st.preset.clone() };
                    }
                }
                {
                    let (key, fav) = (cur_key.clone(), cur_fav);
                    if btn(ui, rect(px0 + 430.0, 16.0, 30.0, 26.0), if fav { "★" } else { "☆" }, false, col, "Favourite").clicked() {
                        let l = gui.lib.as_mut().unwrap();
                        if fav {
                            l.favs.retain(|f| f != &key);
                        } else {
                            l.favs.push(key);
                        }
                        presets::save_favorites(&l.favs);
                    }
                }
                if let Some(d) = load.take() {
                    presets::apply(setter, p, &d, None);
                    gui.undo.clear();
                    gui.redo.clear();
                    bump(&shared);
                }
                // meters + bypass
                let byp = p.bypass.value();
                if btn(ui, rect(BASE_W - 14.0 - 80.0, 16.0, 80.0, 26.0), "BYPASS", byp, HOT, "Bypass the plugin.").clicked() {
                    set_b(setter, &p.bypass, !byp);
                }
                hmeter(ui, rect(BASE_W - 14.0 - 80.0 - 14.0 - 110.0, 18.0, 110.0, 7.0), gui.in_lv, "IN");
                hmeter(ui, rect(BASE_W - 14.0 - 80.0 - 14.0 - 110.0, 33.0, 110.0, 7.0), gui.out_lv, "OUT");
                painter.text(at(BASE_W - 14.0 - 80.0 - 14.0 - 110.0 - 34.0, 29.0), Align2::RIGHT_CENTER, format!("{:.1} BPM", tempo), pf(11.5), DIM);

                // ================= chain tabs
                let ty = 58.0;
                let th = 52.0;
                let gap = 6.0;
                let tw = (BASE_W - 28.0 - gap * (N_SHAPERS as f32 - 1.0)) / N_SHAPERS as f32;
                for (pos, &si) in st.order.iter().enumerate() {
                    let si = si as usize;
                    let k = Kind::ALL[si];
                    let kcol = kc(k);
                    let r = rect(14.0 + pos as f32 * (tw + gap), ty, tw, th);
                    let on = p.sh[si].on.value();
                    let is_sel = si == sel;
                    let dot = Rect::from_center_size(Pos2::new(r.right() - 14.0, r.top() + 15.0), Vec2::splat(16.0));
                    let dresp = ui.allocate_rect(dot, Sense::click()).on_hover_text(if on { "Switch off" } else { "Switch on" });
                    let resp = ui.interact(r, egui::Id::new(("tab", si)), Sense::click()).on_hover_text(format!("{}: {}", k.name(), k.about()));
                    if !modal {
                        if dresp.clicked() {
                            set_b(setter, &p.sh[si].on, !on);
                        } else if resp.clicked() {
                            set_state(&|s| s.selected = si as u8);
                        }
                    }
                    let fill = if is_sel { PANEL2 } else if resp.hovered() { Color32::from_rgb(0x1c, 0x20, 0x27) } else { PANEL };
                    painter.rect_filled(r, CornerRadius::same(7), fill);
                    painter.rect_stroke(r, CornerRadius::same(7), Stroke::new(if is_sel { 1.5f32 } else { 1.0f32 }, if is_sel { kcol } else { LINE }), StrokeKind::Inside);
                    if on {
                        painter.rect_filled(Rect::from_min_size(r.min + Vec2::new(8.0, 0.0), Vec2::new(r.width() - 16.0, 2.5)), CornerRadius::same(1), kcol);
                    }
                    painter.text(Pos2::new(r.left() + 10.0, r.top() + 15.0), Align2::LEFT_CENTER, k.name(), pf(12.5), if on { TEXT } else { DIM });
                    if on {
                        painter.circle_filled(dot.center(), 5.0, kcol);
                    } else {
                        painter.circle_stroke(dot.center(), 5.0, Stroke::new(1.3f32, if dresp.hovered() { kcol } else { FAINT }));
                    }
                    let thumb = Rect::from_min_max(Pos2::new(r.left() + 9.0, r.top() + 27.0), Pos2::new(r.right() - 9.0, r.bottom() - 6.0));
                    let path = wave_path(&st.waves[si][0], thumb, 80.0);
                    let tc = if on { kcol } else { FAINT };
                    painter.add(fill_under(&path, thumb.bottom(), tc, if on { 90 } else { 30 }, thumb.height()));
                    painter.add(Shape::line(path, PathStroke::new(1.3f32, tc)));
                    if on {
                        let ph = (shared.phase[si].load(Ordering::Relaxed) + p.sh[si].phase.value() / 360.0).rem_euclid(1.0);
                        painter.vline(thumb.left() + ph * thumb.width(), thumb.y_range(), Stroke::new(1.0f32, TEXT.gamma_multiply(0.6)));
                    }
                }

                // ================= toolbar 1
                let yb = 118.0;
                let hb = 24.0;
                let cur_wave = st.waves[sel][band].clone();
                let mut x = 14.0;
                {
                    let mut names: Vec<&str> = Vec::new();
                    names.extend(Preset::ALL.iter().map(|q| q.name()));
                    if let Some(i) = combo(ui, "preset_w", rect(x, yb, 108.0, hb), &names, usize::MAX, "Shapes", "Load a ready-made shape into this shaper / band.") {
                        push_undo(gui, sel, band, &cur_wave);
                        edit_wave(p, &shared, sel, band, |w| *w = Wave::preset(Preset::ALL[i]));
                    }
                    x += 114.0;
                    let mut pens = vec!["Pointer"];
                    pens.extend(Pen::ALL.iter().map(|q| q.name()));
                    let pi = gui.pen.map(|q| Pen::ALL.iter().position(|&z| z == q).unwrap() + 1).unwrap_or(0);
                    let shown = format!("✎ {}", pens[pi]);
                    if let Some(i) = combo(ui, "pen", rect(x, yb, 118.0, hb), &pens, pi, &shown, "Pointer: move points and curves.\nPens: click or drag to draw the shape into the grid cells (height = where you click).") {
                        gui.pen = if i == 0 { None } else { Some(Pen::ALL[i - 1]) };
                    }
                    x += 128.0;
                }
                let tool = |ui: &mut Ui, x: &mut f32, w: f32, label: &str, tip: &str| -> (bool, bool) {
                    let r = btn(ui, rect(*x, yb, w, hb), label, false, col, tip);
                    *x += w + 5.0;
                    let shift = ui.input(|i| i.modifiers.shift);
                    (r.clicked() && !modal, shift)
                };
                let grid = st.grid as f32;
                if tool(ui, &mut x, 60.0, "Invert", "Flip vertically.").0 {
                    push_undo(gui, sel, band, &cur_wave);
                    edit_wave(p, &shared, sel, band, |w| w.invert());
                }
                if tool(ui, &mut x, 68.0, "Reverse", "Flip horizontally.").0 {
                    push_undo(gui, sel, band, &cur_wave);
                    edit_wave(p, &shared, sel, band, |w| w.reverse());
                }
                if tool(ui, &mut x, 34.0, "×2", "Double time.").0 {
                    push_undo(gui, sel, band, &cur_wave);
                    edit_wave(p, &shared, sel, band, |w| w.repeat(2));
                }
                if tool(ui, &mut x, 34.0, "×3", "Triple time.").0 {
                    push_undo(gui, sel, band, &cur_wave);
                    edit_wave(p, &shared, sel, band, |w| w.repeat(3));
                }
                let (c, shift) = tool(ui, &mut x, 74.0, "Random", "A random shape on the grid.\nShift: keep the points where they are, only new heights and bends.");
                if c {
                    push_undo(gui, sel, band, &cur_wave);
                    gui.seed = gui.seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    let seed = gui.seed;
                    edit_wave(p, &shared, sel, band, |w| w.randomize(grid as usize, seed, shift));
                }
                let (c, shift) = tool(ui, &mut x, 30.0, "◀", "Move the shape one grid step earlier (Shift: 1/16).");
                if c {
                    push_undo(gui, sel, band, &cur_wave);
                    let d = if shift { 1.0 / 16.0 } else { 1.0 / grid };
                    edit_wave(p, &shared, sel, band, |w| w.shift(-d));
                }
                let (c, shift) = tool(ui, &mut x, 30.0, "▶", "Move the shape one grid step later (Shift: 1/16).");
                if c {
                    push_undo(gui, sel, band, &cur_wave);
                    let d = if shift { 1.0 / 16.0 } else { 1.0 / grid };
                    edit_wave(p, &shared, sel, band, |w| w.shift(d));
                }
                if tool(ui, &mut x, 50.0, "Clear", "Flat line at the top.").0 {
                    push_undo(gui, sel, band, &cur_wave);
                    edit_wave(p, &shared, sel, band, |w| *w = Wave::flat(1.0));
                }
                // right: snap + grid
                let gnames = ["1/4", "1/8", "1/12 (triplets)", "1/16", "1/24 (triplets)", "1/32", "1/64"];
                let gi = GRIDS.iter().position(|&g| g == st.grid).unwrap_or(3);
                if let Some(i) = combo(ui, "grid", rect(BASE_W - 14.0 - 108.0, yb, 108.0, hb), &gnames, gi, &format!("Grid {}", gnames[gi].split(' ').next().unwrap()), "Editor grid (one cycle).") {
                    let g = GRIDS[i];
                    set_state(&|s| s.grid = g);
                }
                if btn(ui, rect(BASE_W - 14.0 - 108.0 - 6.0 - 56.0, yb, 56.0, hb), "Snap", gui.snap, col, "Snap to the grid (hold Shift to toggle while dragging).").clicked() {
                    gui.snap = !gui.snap;
                }

                // ================= the editor
                let er = rect(14.0, 148.0, BASE_W - 28.0, 236.0);
                painter.rect_filled(er, CornerRadius::same(8), EDIT_BG);
                let inner = er.shrink2(Vec2::new(10.0, 12.0));
                let px = |x: f32, y: f32| Pos2::new(inner.left() + x * inner.width(), inner.bottom() - y * inner.height());
                // scope: input dark, output light (magnitude)
                let bw = inner.width() / SCOPE_N as f32;
                let sc = |a: f32| (a.min(1.0).sqrt() * inner.height()).max(0.0);
                for i in 0..SCOPE_N {
                    let x0 = inner.left() + i as f32 * bw;
                    let hi = sc(gui.scope_in[i]);
                    let ho = sc(gui.scope_out[i]);
                    if hi > 0.5 {
                        painter.rect_filled(Rect::from_min_max(Pos2::new(x0, inner.bottom() - hi), Pos2::new(x0 + bw - 1.0, inner.bottom())), CornerRadius::ZERO, Color32::from_rgb(0x1b, 0x1f, 0x26));
                    }
                    if ho > 0.5 {
                        painter.rect_filled(Rect::from_min_max(Pos2::new(x0, inner.bottom() - ho), Pos2::new(x0 + bw - 1.0, inner.bottom())), CornerRadius::ZERO, Color32::from_rgb(0x30, 0x36, 0x41));
                    }
                }
                let g = st.grid as usize;
                let beat_div = if g % 3 == 0 { 3 } else { 4 };
                for i in 1..g {
                    let xx = inner.left() + inner.width() * i as f32 / g as f32;
                    let major = i % beat_div == 0;
                    painter.vline(xx, er.y_range(), Stroke::new(1.0f32, if major { Color32::from_rgb(0x28, 0x2d, 0x36) } else { Color32::from_rgb(0x18, 0x1b, 0x21) }));
                }
                for i in 0..=8 {
                    let yy = inner.bottom() - inner.height() * i as f32 / 8.0;
                    painter.hline(er.x_range(), yy, Stroke::new(1.0f32, if i % 4 == 0 { Color32::from_rgb(0x24, 0x28, 0x30) } else { Color32::from_rgb(0x15, 0x18, 0x1d) }));
                }
                let (top, bottom) = kind.axis();
                painter.text(Pos2::new(er.left() + 8.0, inner.top() + 8.0), Align2::LEFT_CENTER, top, pf(9.5), DIM);
                painter.text(Pos2::new(er.left() + 8.0, inner.bottom() - 8.0), Align2::LEFT_CENTER, bottom, pf(9.5), DIM);
                let title = if multi { format!("{} · {} band", kind.name(), BAND_NAMES[band].to_lowercase()) } else { kind.name().to_string() };
                painter.text(Pos2::new(er.right() - 10.0, inner.top() + 8.0), Align2::RIGHT_CENTER, title, pf(12.0), col);

                // interaction
                let resp = ui.interact(er, egui::Id::new("wave_editor"), Sense::click_and_drag());
                let hover = if modal { None } else { resp.hover_pos() };
                let shift = ui.input(|i| i.modifiers.shift);
                let snap_on = gui.snap != shift;
                let to_xy = |pos: Pos2| -> (f32, f32) { (((pos.x - inner.left()) / inner.width()).clamp(0.0, 1.0), ((inner.bottom() - pos.y) / inner.height()).clamp(0.0, 1.0)) };
                let snap_x = |x: f32, snap: bool| if snap { (x * grid).round() / grid } else { x };
                let snap_y = |y: f32, snap: bool| if snap { (y * 16.0).round() / 16.0 } else { y };
                let pts = &cur_wave.pts;
                let near_pt = |pos: Pos2| -> Option<usize> {
                    let mut best = None;
                    let mut bd = 9.0f32;
                    for (i, q) in pts.iter().enumerate() {
                        let d = px(q.x, q.y).distance(pos);
                        if d < bd {
                            bd = d;
                            best = Some(i);
                        }
                    }
                    best
                };
                let handle_pos = |i: usize| -> Option<Pos2> {
                    let (a, b) = (pts[i], pts[i + 1]);
                    if b.x - a.x < 0.01 || (b.y - a.y).abs() < 0.01 {
                        return None;
                    }
                    Some(px(0.5 * (a.x + b.x), a.y + (b.y - a.y) * bend(0.5, a.c)))
                };
                let near_handle = |pos: Pos2| -> Option<usize> { (0..pts.len() - 1).find(|&i| handle_pos(i).map(|h| h.distance(pos) < 8.0).unwrap_or(false)) };
                let pointer = gui.pen.is_none();
                let hov_pt = if !pointer { None } else { hover.and_then(near_pt) };
                let hov_h = if !pointer || hov_pt.is_some() { None } else { hover.and_then(near_handle) };
                let paint = |gui: &mut Gui, pos: Pos2| {
                    let (x, y) = to_xy(pos);
                    let cell = (x * grid).floor().min(grid - 1.0);
                    let y = snap_y(y, snap_on);
                    let pen = gui.pen.unwrap_or(Pen::Step);
                    edit_wave(p, &shared, sel, band, |w| w.paint(cell / grid, (cell + 1.0) / grid, y, pen));
                };

                if !modal {
                    if resp.drag_started() {
                        let start = resp.interact_pointer_pos().unwrap_or_default();
                        push_undo(gui, sel, band, &cur_wave);
                        gui.drag = if !pointer {
                            Some(Drag::Paint)
                        } else if let Some(i) = near_pt(start) {
                            Some(Drag::Point(i))
                        } else if let Some(i) = near_handle(start) {
                            Some(Drag::Curve(i, pts[i].c, start.y))
                        } else {
                            let (x, y) = to_xy(start);
                            let mut w = cur_wave.clone();
                            let idx = w.insert(snap_x(x, snap_on), snap_y(y, snap_on));
                            if idx.is_some() {
                                edit_wave(p, &shared, sel, band, |dst| *dst = w);
                            }
                            idx.map(Drag::Point)
                        };
                    }
                    if resp.dragged() {
                        if let (Some(d), Some(pos)) = (gui.drag, resp.interact_pointer_pos()) {
                            match d {
                                Drag::Point(i) => {
                                    let (x, y) = to_xy(pos);
                                    edit_wave(p, &shared, sel, band, |w| w.move_pt(i, snap_x(x, snap_on), snap_y(y, snap_on)));
                                }
                                Drag::Curve(i, c0, y0) => {
                                    if i + 1 < pts.len() {
                                        let dir = if pts[i + 1].y >= pts[i].y { 1.0 } else { -1.0 };
                                        let c = (c0 + (pos.y - y0) / 110.0 * dir).clamp(-1.0, 1.0);
                                        edit_wave(p, &shared, sel, band, |w| {
                                            if i < w.pts.len() {
                                                w.pts[i].c = c
                                            }
                                        });
                                    }
                                }
                                Drag::Paint => paint(gui, pos),
                            }
                        }
                    }
                    if resp.drag_stopped() {
                        gui.drag = None;
                    }
                    if resp.clicked() && !pointer {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            push_undo(gui, sel, band, &cur_wave);
                            paint(gui, pos);
                        }
                    }
                    if resp.double_clicked() && pointer {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            push_undo(gui, sel, band, &cur_wave);
                            if let Some(i) = near_pt(pos) {
                                edit_wave(p, &shared, sel, band, |w| w.remove(i));
                            } else if let Some(i) = near_handle(pos) {
                                edit_wave(p, &shared, sel, band, |w| w.pts[i].c = 0.0);
                            } else {
                                let (x, y) = to_xy(pos);
                                let (x, y) = (snap_x(x, snap_on), snap_y(y, snap_on));
                                edit_wave(p, &shared, sel, band, |w| {
                                    w.insert(x, y);
                                });
                            }
                        }
                    }
                    if resp.secondary_clicked() {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            if let Some(i) = near_pt(pos) {
                                push_undo(gui, sel, band, &cur_wave);
                                edit_wave(p, &shared, sel, band, |w| w.remove(i));
                            } else if let Some(i) = near_handle(pos) {
                                push_undo(gui, sel, band, &cur_wave);
                                edit_wave(p, &shared, sel, band, |w| w.pts[i].c = 0.0);
                            }
                        }
                    }
                }
                let _ = resp.on_hover_text(if !pointer {
                    "Pen: click or drag to draw into the grid cells; the height is where you click."
                } else {
                    "Drag points · drag the small rings to bend a segment\nDrag on empty space or double-click: new point\nDouble-click / right-click a point: delete\nShift: toggle snapping · Ctrl+Z / Ctrl+Shift+Z: undo / redo"
                });

                let w_now = p.state.read().waves[sel][band].clone();
                let on = sp.on.value();
                if multi {
                    for b in 0..N_BANDS {
                        if b != band {
                            painter.add(Shape::line(wave_path(&st.waves[sel][b], inner, 400.0), PathStroke::new(1.0f32, col.gamma_multiply(0.3))));
                        }
                    }
                }
                let path = wave_path(&w_now, inner, 700.0);
                let wc = if on { col } else { DIM };
                painter.add(fill_under(&path, inner.bottom(), wc, if on { 110 } else { 35 }, inner.height()));
                painter.add(Shape::line(path, PathStroke::new(2.2f32, wc)));
                if pointer {
                    for i in 0..w_now.pts.len().saturating_sub(1) {
                        let (a, b) = (w_now.pts[i], w_now.pts[i + 1]);
                        if b.x - a.x < 0.01 || (b.y - a.y).abs() < 0.01 {
                            continue;
                        }
                        let h = px(0.5 * (a.x + b.x), a.y + (b.y - a.y) * bend(0.5, a.c));
                        let hot = hov_h == Some(i) || matches!(gui.drag, Some(Drag::Curve(j, ..)) if j == i);
                        painter.circle_stroke(h, if hot { 5.0 } else { 3.5 }, Stroke::new(1.4f32, if hot { TEXT } else { wc.gamma_multiply(0.8) }));
                    }
                    for (i, q) in w_now.pts.iter().enumerate() {
                        let c = px(q.x, q.y);
                        let hot = hov_pt == Some(i) || matches!(gui.drag, Some(Drag::Point(j)) if j == i);
                        painter.circle_filled(c, if hot { 6.5 } else { 5.0 }, if hot { TEXT } else { EDIT_BG });
                        painter.circle_stroke(c, if hot { 6.5 } else { 5.0 }, Stroke::new(2.0f32, if hot { TEXT } else { wc }));
                    }
                }
                // help line: where the pointer is
                if let Some(h) = hover {
                    let (x, y) = to_xy(h);
                    let (x, y) = (snap_x(x, snap_on), snap_y(y, snap_on));
                    let steps = (x * grid).round() as i32;
                    let txt = if (x * grid - steps as f32).abs() < 1e-3 { format!("{}/{}  ·  {:.0} %", steps, st.grid, y * 100.0) } else { format!("{:.3}  ·  {:.0} %", x, y * 100.0) };
                    painter.text(Pos2::new(er.right() - 10.0, inner.bottom() - 8.0), Align2::RIGHT_CENTER, txt, pf(10.0), DIM);
                    if !pointer {
                        let cell = (x * grid).floor().min(grid - 1.0);
                        let r = Rect::from_x_y_ranges(inner.left() + inner.width() * cell / grid..=inner.left() + inner.width() * (cell + 1.0) / grid, er.y_range());
                        painter.rect_filled(r, CornerRadius::ZERO, col.gamma_multiply(0.08));
                    }
                } else {
                    painter.text(Pos2::new(er.right() - 10.0, inner.bottom() - 8.0), Align2::RIGHT_CENTER, kind.about(), pf(10.0), FAINT);
                }
                if on {
                    let ph = (shared.phase[sel].load(Ordering::Relaxed) + sp.phase.value() / 360.0).rem_euclid(1.0);
                    let yv = shared.value[sel][band].load(Ordering::Relaxed);
                    let c = px(ph, yv);
                    painter.vline(c.x, er.y_range(), Stroke::new(1.0f32, TEXT.gamma_multiply(0.35)));
                    painter.circle_filled(c, 6.0, TEXT);
                    painter.circle_filled(c, 3.5, col);
                } else {
                    painter.text(er.center(), Align2::CENTER_CENTER, format!("{} is off · click the dot on its tab", kind.name()), pf(13.0), DIM);
                }
                if gui.freeze {
                    painter.text(Pos2::new(er.left() + 80.0, inner.top() + 8.0), Align2::LEFT_CENTER, "SCOPE FROZEN", pf(9.5), GOLD);
                }
                painter.rect_stroke(er, CornerRadius::same(8), Stroke::new(1.0f32, LINE), StrokeKind::Inside);

                // ================= toolbar 2: clipboard, undo, user waves, MIDI switch, chain
                let y2 = 390.0;
                let mut x = 14.0;
                let tool2 = |ui: &mut Ui, x: &mut f32, w: f32, label: &str, on: bool, tip: &str| -> bool {
                    let r = btn(ui, rect(*x, y2, w, hb), label, on, col, tip);
                    *x += w + 5.0;
                    r.clicked() && !modal
                };
                if tool2(ui, &mut x, 52.0, "Copy", false, "Copy this shape (works across shapers and bands).") {
                    gui.clip = Some(cur_wave.clone());
                    gui.status = Some(("Shape copied".into(), now));
                }
                if tool2(ui, &mut x, 54.0, "Paste", false, "Paste the copied shape here.") {
                    if let Some(c) = gui.clip.clone() {
                        push_undo(gui, sel, band, &cur_wave);
                        edit_wave(p, &shared, sel, band, |w| *w = c);
                    }
                }
                if tool2(ui, &mut x, 52.0, "Undo", false, "Undo (Ctrl+Z).") || (undo_key && !modal) {
                    if let Some((s, b, w)) = gui.undo.pop() {
                        let cur = p.state.read().waves[s][b].clone();
                        gui.redo.push((s, b, cur));
                        edit_wave(p, &shared, s, b, |dst| *dst = w);
                        set_state(&|st| st.selected = s as u8);
                    }
                }
                if tool2(ui, &mut x, 52.0, "Redo", false, "Redo (Ctrl+Shift+Z).") || (redo_key && !modal) {
                    if let Some((s, b, w)) = gui.redo.pop() {
                        let cur = p.state.read().waves[s][b].clone();
                        gui.undo.push((s, b, cur));
                        edit_wave(p, &shared, s, b, |dst| *dst = w);
                        set_state(&|st| st.selected = s as u8);
                    }
                }
                if tool2(ui, &mut x, 64.0, "Freeze", gui.freeze, "Freeze the scope.") {
                    gui.freeze = !gui.freeze;
                }
                // user waves
                let ux = 372.0;
                painter.text(at(ux, y2 + hb * 0.5), Align2::LEFT_CENTER, "USER WAVES", pf(10.0), DIM);
                for i in 0..N_SLOTS {
                    let r = rect(ux + 76.0 + i as f32 * 40.0, y2, 36.0, hb);
                    let slot = st.slots.get(i).cloned().flatten();
                    let resp = ui.allocate_rect(r, Sense::click());
                    let tip = match &slot {
                        Some(s) => format!("{} · {}\nClick: load into {}{}\nShift-click: store the current shape here · right-click: clear", SLOT_NOTES[i], Kind::ALL[s.shaper as usize].name(), Kind::ALL[s.shaper as usize].name(), if s.band > 0 || p.sh[s.shaper as usize].multiband.value() { format!(" ({} band)", BAND_NAMES[s.band as usize].to_lowercase()) } else { String::new() }),
                        None => format!("{} · empty\nClick: store the current shape here", SLOT_NOTES[i]),
                    };
                    let resp = resp.on_hover_text(tip);
                    let scol = slot.as_ref().map(|s| kc(Kind::ALL[s.shaper as usize])).unwrap_or(FAINT);
                    painter.rect_filled(r, CornerRadius::same(5), if resp.hovered() { PANEL2 } else { PANEL });
                    painter.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0f32, if slot.is_some() { scol.gamma_multiply(0.8) } else { LINE }), StrokeKind::Inside);
                    if let Some(s) = &slot {
                        let tr = r.shrink2(Vec2::new(4.0, 5.0));
                        let path = wave_path(&s.wave, tr, 40.0);
                        painter.add(Shape::line(path, PathStroke::new(1.2f32, scol)));
                    } else {
                        painter.text(r.center(), Align2::CENTER_CENTER, SLOT_NOTES[i], pf(10.0), FAINT);
                    }
                    if !modal {
                        let shift = ui.input(|i| i.modifiers.shift);
                        if resp.clicked() {
                            if slot.is_none() || shift {
                                let w = cur_wave.clone();
                                set_state(&|s| s.slots[i] = Some(Slot { shaper: sel as u8, band: band as u8, wave: w.clone() }));
                                gui.status = Some((format!("Stored in {}", SLOT_NOTES[i]), now));
                            } else if let Some(s) = slot.clone() {
                                let (si, b) = (s.shaper as usize, s.band as usize);
                                let old = p.state.read().waves[si][b].clone();
                                push_undo(gui, si, b, &old);
                                edit_wave(p, &shared, si, b, |w| *w = s.wave);
                                set_state(&|st| {
                                    st.selected = si as u8;
                                    st.band = b as u8;
                                });
                            }
                        }
                        if resp.secondary_clicked() {
                            set_state(&|s| s.slots[i] = None);
                        }
                    }
                }
                let msw = p.midi_switch.value();
                if btn(ui, rect(ux + 76.0 + 9.0 * 40.0 + 6.0, y2, 96.0, hb), "MIDI Switch", msw, col, "MIDI notes C# .. A load the user waves into their shapers;\nonly C notes trigger the LFOs then.").clicked() && !modal {
                    set_b(setter, &p.midi_switch, !msw);
                }
                let pos_in_chain = st.order.iter().position(|&i| i as usize == sel).unwrap_or(0);
                if btn(ui, rect(BASE_W - 14.0 - 30.0, y2, 30.0, hb), "▶", false, col, "Move this shaper later in the chain.").clicked() && pos_in_chain + 1 < N_SHAPERS && !modal {
                    set_state(&|s| s.order.swap(pos_in_chain, pos_in_chain + 1));
                }
                if btn(ui, rect(BASE_W - 14.0 - 66.0, y2, 30.0, hb), "◀", false, col, "Move this shaper earlier in the chain.").clicked() && pos_in_chain > 0 && !modal {
                    set_state(&|s| s.order.swap(pos_in_chain, pos_in_chain - 1));
                }
                painter.text(at(BASE_W - 14.0 - 74.0, y2 + hb * 0.5), Align2::RIGHT_CENTER, "CHAIN", pf(10.0), DIM);

                // ================= row A: shaper controls + envelope / compressor
                let ya = 422.0;
                let ha = 156.0;
                let kr = 19.0;
                let ky = ya + 70.0;
                let sid = |s: &str| format!("{s}{sel}");
                let wk = 640.0;
                let rk = rect(14.0, ya, wk, ha);
                panel(ui, rk, &kind.name().to_uppercase(), col);
                // quick presets
                {
                    let q = presets::quick(kind);
                    let names: Vec<&str> = q.iter().map(|(n, _)| *n).collect();
                    if let Some(i) = combo(ui, &sid("quick"), rect(14.0 + wk - 12.0 - 140.0, ya + 4.0, 140.0, 22.0), &names, usize::MAX, "Quick presets", "Set up this shaper in one click (only this shaper changes).") {
                        let (n, d) = &q[i];
                        presets::apply(setter, p, d, Some(kind));
                        bump(&shared);
                        gui.status = Some((format!("{}: {}", kind.name(), n), now));
                    }
                }
                let mut hx = 14.0 + 130.0;
                match kind {
                    Kind::Filter => {
                        enum_combo::<FilterType>(ui, setter, &p.f_type, "ftype", rect(hx, ya + 4.0, 160.0, 22.0), "", "6 / 12 / 24 dB low-pass, high-pass, band-pass, notch, peak, and phasers with 2-8 notches or peaks.");
                    }
                    Kind::Pan => {
                        enum_combo::<PanMode>(ui, setter, &p.p_mode, "pmode", rect(hx, ya + 4.0, 150.0, 22.0), "", "Balance: the far side gets quieter, the near side up to +N dB louder.\nCombined: the far side is folded into the near one.");
                    }
                    Kind::Time => {
                        enum_combo::<TimeRange>(ui, setter, &p.t_range, "trange", rect(hx, ya + 4.0, 150.0, 22.0), "Range", "How late the bottom of the shape is: one cycle, 1-8 bars, or 2.5 / 10 / 20 ms (flanging, chorus, pitch wobble).");
                    }
                    Kind::Noise => {
                        enum_combo::<NoiseType>(ui, setter, &p.n_type, "ntype", rect(hx, ya + 4.0, 140.0, 22.0), "", "Noise colour / character.");
                        hx += 146.0;
                        enum_combo::<NoiseEnv>(ui, setter, &p.n_env, "nenv", rect(hx, ya + 4.0, 110.0, 22.0), "Env", "Follow: the noise follows the input level · Duck: the noise drops while the input plays.");
                        hx += 116.0;
                        let o = p.n_only.value();
                        if btn(ui, rect(hx, ya + 4.0, 90.0, 22.0), "Noise only", o, col, "Mute the input: only the noise is heard.").clicked() && !modal {
                            set_b(setter, &p.n_only, !o);
                        }
                    }
                    Kind::Crush => {
                        let d = p.c_dither.value();
                        if btn(ui, rect(hx, ya + 4.0, 66.0, 22.0), "Dither", d, col, "Dither before the bit reduction: less distortion, some noise.").clicked() && !modal {
                            set_b(setter, &p.c_dither, !d);
                        }
                        hx += 72.0;
                        let a = p.c_aa.value();
                        if btn(ui, rect(hx, ya + 4.0, 86.0, 22.0), "Anti-alias", a, col, "Low-pass before the resampling: smoother, less aliasing.").clicked() && !modal {
                            set_b(setter, &p.c_aa, !a);
                        }
                    }
                    Kind::Drive => {
                        enum_combo::<DriveStyle>(ui, setter, &p.d_style, "dstyle", rect(hx, ya + 4.0, 140.0, 22.0), "", "Distortion curve: clip, square, rectify or fold.");
                    }
                    _ => {}
                }
                let kn = kind_knobs(p, kind);
                let n = 1 + kn.len();
                let step = ((wk - 24.0) / n.max(4) as f32).min(86.0);
                let kx = |i: usize| 14.0 + 12.0 + step * (i as f32 + 0.5);
                let vw = (step - 8.0).min(70.0);
                knob(ui, setter, &sp.amount, "Amount", &sid("amt"), at(kx(0), ky), kr, amount_tip(kind), false, col, vw);
                for (i, (fp, label, id, tip, bip)) in kn.into_iter().enumerate() {
                    knob(ui, setter, fp, label, id, at(kx(i + 1), ky), kr, tip, bip, col, vw);
                }
                let (tp, bt) = kind.axis();
                painter.text(at(26.0, ya + ha - 13.0), Align2::LEFT_CENTER, format!("top of the shape: {}  ·  bottom: {}", tp.to_lowercase(), bt.to_lowercase()), pf(10.0), FAINT);

                // envelope / compressor
                let xe = 14.0 + wk + 10.0;
                let we = BASE_W - 14.0 - xe;
                let re = rect(xe, ya, we, ha);
                match kind {
                    Kind::Volume => {
                        panel(ui, re, "COMPRESSOR", col);
                        let c_on = p.v_comp.value();
                        if btn(ui, rect(xe + we - 12.0 - 44.0, ya + 4.0, 44.0, 22.0), "On", c_on, col, "Compressor on / off (after the shape, per band in multiband).").clicked() && !modal {
                            set_b(setter, &p.v_comp, !c_on);
                        }
                        let au = p.v_auto.value();
                        if btn(ui, rect(xe + we - 12.0 - 44.0 - 6.0 - 54.0, ya + 4.0, 54.0, 22.0), "Auto", au, col, "Automatic makeup gain.").clicked() && !modal {
                            set_b(setter, &p.v_auto, !au);
                        }
                        let ks: [(&FloatParam, &str, &str, &str); 6] = [
                            (&p.v_thr, "Thresh", "vthr", "Level above which it compresses."),
                            (&p.v_ratio, "Ratio", "vratio", "1:1 (none) to ∞:1 (limiting)."),
                            (&p.v_att, "Attack", "vatt", "0.01-200 ms."),
                            (&p.v_hold, "Hold", "vhold", "Keeps the gain reduction this long before releasing."),
                            (&p.v_rel, "Release", "vrel", "50 ms - 2 s."),
                            (&p.v_makeup, "Makeup", "vmk", "Makeup gain (on top of Auto)."),
                        ];
                        let st_ = (we - 24.0 - 30.0) / 6.0;
                        for (i, (fp, l, id, tip)) in ks.into_iter().enumerate() {
                            knob(ui, setter, fp, l, id, at(xe + 12.0 + st_ * (i as f32 + 0.5), ky), kr - 2.0, tip, false, col, st_ - 4.0);
                        }
                        let gr = shared.gr.load(Ordering::Relaxed);
                        vmeter(ui, rect(xe + we - 26.0, ya + 34.0, 10.0, ha - 60.0), gr / 24.0, HOT, "GR", true);
                    }
                    Kind::Time => {
                        panel(ui, re, "ENVELOPE", col);
                        painter.text(re.center(), Align2::CENTER_CENTER, "Time has no envelope follower.\nSmooth (in Shaper) softens the jumps.", pf(11.5), FAINT);
                    }
                    _ => {
                        panel(ui, re, "ENVELOPE FOLLOWER", col);
                        let e_on = sp.env_on.value();
                        if btn(ui, rect(xe + we - 12.0 - 44.0, ya + 4.0, 44.0, 22.0), "On", e_on, col, "The input level moves the shape's value too.").clicked() && !modal {
                            set_b(setter, &sp.env_on, !e_on);
                        }
                        if matches!(kind, Kind::Filter | Kind::Width) {
                            enum_combo::<EnvMode>(ui, setter, &sp.env_mode, &sid("emode"), rect(xe + we - 12.0 - 44.0 - 6.0 - 110.0, ya + 4.0, 110.0, 22.0), "", "Add: shape + envelope · Multiply: the shape scales the envelope.");
                        }
                        let ks: [(&FloatParam, &str, &str, &str, bool); 5] = [
                            (&sp.env_amt, "Amount", "eamt", "How far the envelope moves the value, ±200 %.", true),
                            (&sp.env_att, "Attack", "eatt", "How fast the envelope rises.", false),
                            (&sp.env_hold, "Hold", "ehold", "Keeps the peak this long.", false),
                            (&sp.env_rel, "Release", "erel", "How fast it falls.", false),
                            (&sp.env_thr, "Thresh", "ethr", "Ignores the signal below this level.", false),
                        ];
                        let st_ = (we - 24.0 - 30.0) / 5.0;
                        for (i, (fp, l, id, tip, bip)) in ks.into_iter().enumerate() {
                            knob(ui, setter, fp, l, &sid(id), at(xe + 12.0 + st_ * (i as f32 + 0.5), ky), kr - 1.0, tip, bip, col, st_ - 6.0);
                        }
                        let ev = shared.env[sel].load(Ordering::Relaxed);
                        let thr = sp.env_thr.value().min(-0.5);
                        let e = ((20.0 * ev.max(1e-6).log10() - thr) / -thr).clamp(0.0, 1.0);
                        vmeter(ui, rect(xe + we - 26.0, ya + 34.0, 10.0, ha - 60.0), if e_on { e } else { 0.0 }, col, "ENV", false);
                    }
                }

                // ================= row B: LFO, shaper, multiband, output
                let yb2 = ya + ha + 10.0;
                let hb2 = BASE_H - 14.0 - yb2;
                let kyb = yb2 + 68.0;
                let mut xp = 14.0;
                // LFO
                let w1 = 340.0;
                panel(ui, rect(xp, yb2, w1, hb2), "LFO", col);
                let mode = sp.mode.value();
                let trig = sp.trigger.value();
                enum_combo(ui, setter, &sp.mode, &sid("mode"), rect(xp + 12.0, yb2 + 30.0, 128.0, 21.0), "Mode", "Beat: tempo and song position · Hz: free running · Pitch: speed = the MIDI note played.");
                enum_combo(ui, setter, &sp.trigger, &sid("trig"), rect(xp + 12.0, yb2 + 56.0, 128.0, 21.0), "Trig", "Free: loops · MIDI / Audio: restarts on every note / hit · 1-shot: plays once and holds the end.");
                if mode == Mode::Beat {
                    enum_combo(ui, setter, &sp.rate, &sid("rate"), rect(xp + 12.0, yb2 + 82.0, 128.0, 21.0), "Length", "Length of one cycle: 1/128 to 32 bars.");
                    enum_combo(ui, setter, &sp.feel, &sid("feel"), rect(xp + 12.0, yb2 + 108.0, 128.0, 21.0), "", "Straight, dotted (×1.5) or triplet (×2/3).");
                }
                let mut kx2 = xp + 186.0;
                if mode == Mode::Hz {
                    knob(ui, setter, &sp.hz, "Speed", &sid("hz"), at(kx2, kyb - 6.0), kr, "Speed in Hz: 0.02 Hz to 5 kHz.", false, col, 64.0);
                    kx2 += 76.0;
                }
                if trig.audio() {
                    knob(ui, setter, &p.trig_db, "Trig level", "atrig", at(kx2, kyb - 6.0), kr, "Input level a hit must reach to restart the cycle.", false, col, 64.0);
                    kx2 += 76.0;
                }
                if kx2 < xp + w1 - 30.0 {
                    knob(ui, setter, &sp.phase, "Phase", &sid("ph"), at(kx2, kyb - 6.0), kr, "Moves the shape against the beat.", false, col, 64.0);
                }
                painter.text(at(xp + 160.0, yb2 + hb2 - 30.0), Align2::LEFT_CENTER, cycle_text(sp, tempo), pf(11.0), TEXT);
                let hint = match (mode, trig) {
                    (Mode::Beat, Trigger::Off) => "locked to the song position",
                    (_, Trigger::Off) => "free running",
                    (_, t) if t.midi() => "restarts on MIDI notes",
                    _ => "restarts on hits of the input",
                };
                painter.text(at(xp + 160.0, yb2 + hb2 - 14.0), Align2::LEFT_CENTER, hint, pf(10.0), DIM);
                xp += w1 + 10.0;

                // shaper mix / smooth
                let w2 = 150.0;
                panel(ui, rect(xp, yb2, w2, hb2), "SHAPER", col);
                knob(ui, setter, &sp.mix, "Mix", &sid("mix"), at(xp + 40.0, kyb), kr, "Dry / processed balance of this shaper (Volume / Pan / Width: works like an amount).", false, col, 64.0);
                knob(ui, setter, &sp.smooth, "Smooth", &sid("sm"), at(xp + 110.0, kyb), kr, "Softens hard corners and jumps of the shape (clicks).", false, col, 64.0);
                xp += w2 + 10.0;

                // multiband
                let w3 = 260.0;
                panel(ui, rect(xp, yb2, w3, hb2), "MULTIBAND", col);
                if btn(ui, rect(xp + w3 - 12.0 - 44.0, yb2 + 4.0, 44.0, 22.0), "On", multi, col, "Three bands, each with its own shape.").clicked() && !modal {
                    set_b(setter, &sp.multiband, !multi);
                }
                knob(ui, setter, &sp.x_lo, "Low / Mid", &sid("xl"), at(xp + 70.0, kyb - 10.0), kr, "Split between the low and the mid band.", false, col, 64.0);
                knob(ui, setter, &sp.x_hi, "Mid / High", &sid("xh"), at(xp + 190.0, kyb - 10.0), kr, "Split between the mid and the high band.", false, col, 64.0);
                let solo = shared.solo[sel].load(Ordering::Relaxed);
                let bwid = (w3 - 24.0 - 12.0) / 3.0;
                for b in 0..N_BANDS {
                    let r = rect(xp + 12.0 + b as f32 * (bwid + 6.0), yb2 + hb2 - 32.0, bwid - 26.0, 22.0);
                    let sr_ = rect(xp + 12.0 + b as f32 * (bwid + 6.0) + bwid - 24.0, yb2 + hb2 - 32.0, 24.0, 22.0);
                    if multi {
                        if btn(ui, r, BAND_NAMES[b], b == band, col, "Edit the shape of this band.").clicked() && !modal {
                            set_state(&|s| s.band = b as u8);
                        }
                        let so = solo & (1 << b) != 0;
                        if btn(ui, sr_, "S", so, GOLD, "Solo this band (click another S to switch).").clicked() && !modal {
                            let m = if so { solo & !(1 << b) } else { 1 << b };
                            shared.solo[sel].store(m, Ordering::Relaxed);
                        }
                    } else {
                        for rr in [r, sr_] {
                            painter.rect_stroke(rr, CornerRadius::same(5), Stroke::new(1.0f32, Color32::from_rgb(0x20, 0x24, 0x2b)), StrokeKind::Inside);
                        }
                        painter.text(r.center(), Align2::CENTER_CENTER, BAND_NAMES[b], pf(11.0), FAINT);
                    }
                }
                xp += w3 + 10.0;

                // output
                let w4 = BASE_W - 14.0 - xp;
                panel(ui, rect(xp, yb2, w4, hb2), "OUTPUT", Color32::from_rgb(0x7f, 0xd6, 0xa0));
                let s4 = (w4 - 24.0) / 3.0;
                let ocol = Color32::from_rgb(0xc8, 0xcd, 0xd8);
                knob(ui, setter, &p.in_gain, "Input", "in", at(xp + 12.0 + s4 * 0.5, kyb), kr, "Level into the chain.", true, ocol, s4 - 8.0);
                knob(ui, setter, &p.mix, "Master mix", "gmix", at(xp + 12.0 + s4 * 1.5, kyb), kr, "Blends the end of the chain with the untouched input.", false, ocol, s4 - 8.0);
                knob(ui, setter, &p.out_gain, "Output", "out", at(xp + 12.0 + s4 * 2.5, kyb), kr, "Output level.", true, ocol, s4 - 8.0);

                // status line
                if let Some((msg, t)) = &gui.status {
                    if now - t < 2.5 {
                        painter.text(at(BASE_W * 0.5, 145.0 - 4.0), Align2::CENTER_BOTTOM, msg, pf(10.5), GOLD);
                    } else {
                        gui.status = None;
                    }
                }

                // ================= preset browser
                if gui.browser {
                    let br = rect(14.0, 50.0, BASE_W - 28.0, BASE_H - 64.0);
                    let blocker = ui.interact(br, egui::Id::new("browser_block"), Sense::click_and_drag());
                    let _ = blocker;
                    painter.rect_filled(br, CornerRadius::same(10), Color32::from_rgb(0x14, 0x17, 0x1c));
                    painter.rect_stroke(br, CornerRadius::same(10), Stroke::new(1.0f32, col), StrokeKind::Inside);
                    painter.text(br.min + Vec2::new(18.0, 22.0), Align2::LEFT_CENTER, "Presets", FontId::proportional(17.0), TEXT);
                    if btn(ui, Rect::from_min_size(br.right_top() + Vec2::new(-92.0, 10.0), Vec2::new(78.0, 24.0)), "Close", false, col, "").clicked() {
                        gui.browser = false;
                    }
                    if btn(ui, Rect::from_min_size(br.right_top() + Vec2::new(-92.0 - 84.0, 10.0), Vec2::new(78.0, 24.0)), "Init", false, col, "Start from scratch: every shaper off, default shapes.").clicked() {
                        let mut b = presets::Builder::new();
                        let d = b.finish("Init", "");
                        presets::apply(setter, p, &d, None);
                        bump(&shared);
                    }
                    if btn(ui, Rect::from_min_size(br.right_top() + Vec2::new(-92.0 - 84.0 - 84.0, 10.0), Vec2::new(78.0, 24.0)), "Rescan", false, col, "Read the user preset folder again.").clicked() {
                        if let Some(l) = gui.lib.as_mut() {
                            l.user = presets::list_user();
                        }
                    }
                    // search
                    let sr_ = Rect::from_min_size(br.min + Vec2::new(110.0, 10.0), Vec2::new(260.0, 24.0));
                    ui.put(sr_, egui::TextEdit::singleline(&mut gui.search).hint_text("Search").font(pf(12.5)).margin(egui::Margin::symmetric(8, 4)));
                    // filters
                    let mut filters: Vec<String> = vec!["All".into(), "★ Favourites".into(), "Factory".into(), "User".into()];
                    filters.extend(CATEGORIES.iter().map(|c| c.to_string()));
                    for (i, f) in filters.iter().enumerate() {
                        let r = Rect::from_min_size(br.min + Vec2::new(14.0, 50.0 + i as f32 * 30.0), Vec2::new(170.0, 25.0));
                        if i == 4 {
                            painter.hline(r.x_range(), r.top() - 3.0, Stroke::new(1.0f32, LINE));
                        }
                        if btn(ui, r, f, gui.filter == i, col, "").clicked() {
                            gui.filter = i;
                        }
                    }
                    if let Some(d) = presets::dir() {
                        painter.text(br.left_bottom() + Vec2::new(16.0, -14.0), Align2::LEFT_CENTER, format!("User presets: {}", d.display()), pf(9.5), FAINT);
                    }
                    // list
                    let lr = Rect::from_min_max(br.min + Vec2::new(198.0, 48.0), br.max - Vec2::new(14.0, 30.0));
                    painter.rect_filled(lr, CornerRadius::same(6), EDIT_BG);
                    let heads = [("NAME", 34.0), ("CATEGORY", 360.0), ("SHAPERS", 500.0), ("BY", 720.0)];
                    for (h, xo) in heads {
                        painter.text(lr.min + Vec2::new(xo, 12.0), Align2::LEFT_CENTER, h, pf(9.5), DIM);
                    }
                    let list_rect = Rect::from_min_max(lr.min + Vec2::new(0.0, 24.0), lr.max);
                    let mut to_load: Option<PresetData> = None;
                    let mut fav_toggle: Option<String> = None;
                    let mut delete: Option<String> = None;
                    let favs = gui.lib.as_ref().map(|l| l.favs.clone()).unwrap_or_default();
                    ui.allocate_new_ui(UiBuilder::new().max_rect(list_rect), |ui| {
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            for (is_user, d) in filtered.iter() {
                                let (r, resp) = ui.allocate_exact_size(Vec2::new(list_rect.width() - 10.0, 26.0), Sense::click());
                                let cur = d.name == st.preset;
                                let pnt = ui.painter();
                                if cur {
                                    pnt.rect_filled(r, CornerRadius::same(4), col.gamma_multiply(0.18));
                                } else if resp.hovered() {
                                    pnt.rect_filled(r, CornerRadius::same(4), PANEL2);
                                }
                                let key = Library::key(*is_user, &d.name);
                                let fav = favs.contains(&key);
                                let star = Rect::from_center_size(r.left_center() + Vec2::new(16.0, 0.0), Vec2::splat(20.0));
                                pnt.text(star.center(), Align2::CENTER_CENTER, if fav { "★" } else { "☆" }, pf(13.0), if fav { GOLD } else { FAINT });
                                pnt.text(r.left_center() + Vec2::new(34.0, 0.0), Align2::LEFT_CENTER, &d.name, pf(12.5), if cur { col } else { TEXT });
                                pnt.text(r.left_center() + Vec2::new(360.0, 0.0), Align2::LEFT_CENTER, &d.category, pf(11.5), DIM);
                                for (j, k) in d.kinds_on().into_iter().enumerate() {
                                    let sq = Rect::from_min_size(r.left_center() + Vec2::new(500.0 + j as f32 * 16.0, -6.0), Vec2::splat(12.0));
                                    pnt.rect_filled(sq, CornerRadius::same(2), kc(Kind::ALL[k]));
                                }
                                pnt.text(r.left_center() + Vec2::new(720.0, 0.0), Align2::LEFT_CENTER, &d.author, pf(11.0), FAINT);
                                let trash = Rect::from_center_size(r.right_center() - Vec2::new(18.0, 0.0), Vec2::splat(20.0));
                                if *is_user {
                                    pnt.text(trash.center(), Align2::CENTER_CENTER, "✕", pf(12.0), FAINT);
                                }
                                if resp.clicked() {
                                    let pos = resp.interact_pointer_pos().unwrap_or_default();
                                    if star.contains(pos) {
                                        fav_toggle = Some(key);
                                    } else if *is_user && trash.contains(pos) {
                                        delete = Some(d.name.clone());
                                    } else {
                                        to_load = Some(d.clone());
                                    }
                                }
                            }
                            if filtered.is_empty() {
                                ui.add_space(20.0);
                                ui.label(egui::RichText::new("   Nothing here yet.").font(pf(12.0)).color(DIM));
                            }
                        });
                    });
                    if let Some(k) = fav_toggle {
                        let l = gui.lib.as_mut().unwrap();
                        if l.favs.contains(&k) {
                            l.favs.retain(|f| f != &k);
                        } else {
                            l.favs.push(k);
                        }
                        presets::save_favorites(&l.favs);
                    }
                    if let Some(n) = delete {
                        presets::delete_user(&n);
                        if let Some(l) = gui.lib.as_mut() {
                            l.user = presets::list_user();
                        }
                        gui.status = Some((format!("Deleted {n}"), now));
                    }
                    if let Some(d) = to_load {
                        presets::apply(setter, p, &d, None);
                        gui.undo.clear();
                        gui.redo.clear();
                        bump(&shared);
                    }
                }

                // ================= save dialog
                if gui.save_open {
                    let dr = Rect::from_center_size(at(BASE_W * 0.5, 250.0), Vec2::new(420.0, 190.0));
                    let _ = ui.interact(full, egui::Id::new("save_block"), Sense::click_and_drag());
                    painter.rect_filled(full, CornerRadius::ZERO, Color32::from_black_alpha(120));
                    painter.rect_filled(dr, CornerRadius::same(10), PANEL2);
                    painter.rect_stroke(dr, CornerRadius::same(10), Stroke::new(1.0f32, col), StrokeKind::Inside);
                    painter.text(dr.min + Vec2::new(18.0, 22.0), Align2::LEFT_CENTER, "Save preset", FontId::proportional(16.0), TEXT);
                    painter.text(dr.min + Vec2::new(18.0, 56.0), Align2::LEFT_CENTER, "Name", pf(11.0), DIM);
                    let ner = Rect::from_min_size(dr.min + Vec2::new(100.0, 44.0), Vec2::new(300.0, 26.0));
                    let te = ui.put(ner, egui::TextEdit::singleline(&mut gui.save_name).hint_text("My preset").font(pf(13.0)).margin(egui::Margin::symmetric(8, 4)));
                    if !te.has_focus() && gui.save_name.is_empty() {
                        te.request_focus();
                    }
                    painter.text(dr.min + Vec2::new(18.0, 94.0), Align2::LEFT_CENTER, "Category", pf(11.0), DIM);
                    let cats: Vec<&str> = CATEGORIES.to_vec();
                    if let Some(i) = combo(ui, "savecat", Rect::from_min_size(dr.min + Vec2::new(100.0, 82.0), Vec2::new(200.0, 24.0)), &cats, gui.save_cat, cats[gui.save_cat.min(cats.len() - 1)], "") {
                        gui.save_cat = i;
                    }
                    let exists = gui.lib.as_ref().map(|l| l.user.iter().any(|d| d.name.trim() == gui.save_name.trim())).unwrap_or(false);
                    if exists {
                        painter.text(dr.min + Vec2::new(100.0, 122.0), Align2::LEFT_CENTER, "A user preset with this name exists: it will be replaced.", pf(10.0), GOLD);
                    }
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    let name = gui.save_name.trim().to_string();
                    let save = btn(ui, Rect::from_min_size(dr.right_bottom() - Vec2::new(18.0 + 90.0, 40.0), Vec2::new(90.0, 26.0)), "Save", !name.is_empty(), col, "").clicked() || enter;
                    if btn(ui, Rect::from_min_size(dr.right_bottom() - Vec2::new(18.0 + 90.0 + 96.0, 40.0), Vec2::new(90.0, 26.0)), "Cancel", false, col, "").clicked() || esc {
                        gui.save_open = false;
                    } else if save && !name.is_empty() {
                        let d = presets::capture(p, &name, CATEGORIES[gui.save_cat.min(CATEGORIES.len() - 1)]);
                        match presets::save_user(&d) {
                            Ok(_) => {
                                p.state.write().preset = name.clone();
                                if let Some(l) = gui.lib.as_mut() {
                                    l.user = presets::list_user();
                                }
                                gui.status = Some((format!("Saved \"{name}\""), now));
                            }
                            Err(e) => gui.status = Some((format!("Could not save: {e}"), now)),
                        }
                        gui.save_open = false;
                    }
                }

                // resize grip
                let grip = Rect::from_min_size(full.max - Vec2::splat(18.0), Vec2::splat(18.0));
                let r = ui.interact(grip, egui::Id::new("wc_grip"), Sense::drag()).on_hover_text("Drag to resize");
                if r.dragged() {
                    if let Some(pos) = r.interact_pointer_pos() {
                        let ppp = if cfg!(target_os = "macos") { size_state.size().0 as f32 / BASE_W } else { ctx.pixels_per_point() };
                        let w = ((pos.x - full.min.x + 9.0) * ppp).clamp(820.0, 2800.0);
                        size_state.set_requested_size((w.round() as u32, (w * BASE_H / BASE_W).round() as u32));
                    }
                }
                let gc = if r.hovered() || r.dragged() { TEXT } else { FAINT };
                for k in [5.0f32, 9.0, 13.0] {
                    painter.line_segment([Pos2::new(full.max.x - 3.0, full.max.y - k), Pos2::new(full.max.x - k, full.max.y - 3.0)], Stroke::new(1.0f32, gc));
                }
            });
        },
    )
}
