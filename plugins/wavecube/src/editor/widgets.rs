//! Drawing helpers and small widgets of the editor.

use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, epaint::PathStroke, Align2, Color32, CornerRadius, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2};
use std::sync::Arc;

use crate::kinds::KnobDef;
use crate::wave::{bend, Wave};
use crate::Kind;

pub const BG: Color32 = Color32::from_rgb(0x10, 0x12, 0x16);
pub const PANEL: Color32 = Color32::from_rgb(0x17, 0x1a, 0x20);
pub const PANEL2: Color32 = Color32::from_rgb(0x21, 0x25, 0x2d);
pub const EDIT_BG: Color32 = Color32::from_rgb(0x0c, 0x0e, 0x12);
pub const LINE: Color32 = Color32::from_rgb(0x2b, 0x30, 0x39);
pub const TEXT: Color32 = Color32::from_rgb(0xe8, 0xea, 0xf0);
pub const DIM: Color32 = Color32::from_rgb(0x8a, 0x91, 0xa0);
pub const FAINT: Color32 = Color32::from_rgb(0x4b, 0x51, 0x5d);
pub const HOT: Color32 = Color32::from_rgb(0xf2, 0x5c, 0x6a);
pub const GOLD: Color32 = Color32::from_rgb(0xf5, 0xc5, 0x42);
pub const OK: Color32 = Color32::from_rgb(0x7f, 0xd6, 0xa0);

/// The colour of each kind.
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
        Kind::Reverb => Color32::from_rgb(0x6f, 0x8c, 0xf2),
        Kind::Liquid => Color32::from_rgb(0x3f, 0xc8, 0x8e),
        Kind::Compressor => Color32::from_rgb(0xd8, 0xd2, 0xc0),
        Kind::Scope => Color32::from_rgb(0x9a, 0xa6, 0xb8),
    }
}

pub fn pf(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("dejavu".into(), Arc::new(FontData::from_static(include_bytes!("../fonts/DejaVuSans.ttf"))));
    for fam in [FontFamily::Monospace, FontFamily::Proportional] {
        if let Some(list) = fonts.families.get_mut(&fam) {
            list.push("dejavu".into());
        }
    }
    ctx.set_fonts(fonts);
}

pub fn visuals(ctx: &egui::Context) {
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
}

/// Toggle / action button. `col` = colour when on.
pub fn btn(ui: &mut Ui, rect: Rect, text: &str, on: bool, col: Color32, tip: &str) -> egui::Response {
    btn_f(ui, rect, text, on, col, tip, 11.5)
}

pub fn btn_f(ui: &mut Ui, rect: Rect, text: &str, on: bool, col: Color32, tip: &str, font: f32) -> egui::Response {
    let mut resp = ui.allocate_rect(rect, Sense::click());
    if !tip.is_empty() {
        resp = resp.on_hover_text(tip);
    }
    let p = ui.painter();
    let hov = resp.hovered();
    let fill = if on { col } else if hov { PANEL2 } else { PANEL };
    p.rect_filled(rect, CornerRadius::same(5), fill);
    p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0f32, if on { col } else if hov { FAINT } else { LINE }), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, text, pf(font), if on { BG } else { TEXT });
    resp
}

/// A row of buttons of which one is on; returns the clicked index.
pub fn segmented(ui: &mut Ui, rect: Rect, names: &[&str], cur: usize, col: Color32, tip: &str) -> Option<usize> {
    let n = names.len().max(1) as f32;
    let w = rect.width() / n;
    let mut out = None;
    for (i, name) in names.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.left() + i as f32 * w, rect.top()), Vec2::new(w - 2.0, rect.height()));
        if btn(ui, r, name, i == cur, col, tip).clicked() {
            out = Some(i);
        }
    }
    out
}

pub fn panel(ui: &Ui, rect: Rect, title: &str, col: Color32) {
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(8), PANEL);
    p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0f32, LINE), StrokeKind::Inside);
    if !title.is_empty() {
        p.circle_filled(Pos2::new(rect.left() + 14.0, rect.top() + 15.0), 3.0, col);
        p.text(Pos2::new(rect.left() + 24.0, rect.top() + 15.0), Align2::LEFT_CENTER, title, pf(11.0), DIM);
    }
}

pub fn label(ui: &Ui, pos: Pos2, text: &str, col: Color32, size: f32) {
    ui.painter().text(pos, Align2::LEFT_CENTER, text, pf(size), col);
}

/// A combo box over a list of names; returns the picked index. Names starting with "──" are headings.
pub fn combo(ui: &mut Ui, id: &str, rect: Rect, names: &[String], idx: usize, shown: &str, tip: &str) -> Option<usize> {
    let mut pick = None;
    ui.allocate_new_ui(UiBuilder::new().max_rect(rect), |ui| {
        let r = egui::ComboBox::from_id_salt(id)
            .width(rect.width() - 22.0)
            .height(520.0)
            .selected_text(egui::RichText::new(shown).font(pf(11.5)))
            .show_ui(ui, |ui| {
                ui.set_min_width(rect.width().max(150.0));
                for (i, n) in names.iter().enumerate() {
                    if let Some(h) = n.strip_prefix("──") {
                        ui.label(egui::RichText::new(h.trim()).font(pf(10.0)).color(DIM));
                        continue;
                    }
                    if ui.selectable_label(i == idx, egui::RichText::new(n.as_str()).font(pf(12.0))).clicked() {
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

pub fn combo_s(ui: &mut Ui, id: &str, rect: Rect, names: &[&str], idx: usize, prefix: &str, tip: &str) -> Option<usize> {
    let v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    let shown = if prefix.is_empty() { names.get(idx).copied().unwrap_or("").to_string() } else { format!("{prefix} {}", names.get(idx).copied().unwrap_or("")) };
    combo(ui, id, rect, &v, idx, &shown, tip)
}

/// A button that opens a menu below it; returns the picked item. Items starting with "──" are headings.
#[allow(clippy::too_many_arguments)]
pub fn menu_btn(ui: &mut Ui, id: &str, rect: Rect, label: &str, items: &[(String, Color32)], col: Color32, tip: &str, font: f32) -> Option<usize> {
    let popup_id = egui::Id::new(("menu", id));
    let open = ui.memory(|m| m.is_popup_open(popup_id));
    let resp = ui.allocate_rect(rect, Sense::click());
    let resp = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    let p = ui.painter();
    let hov = resp.hovered() || open;
    p.rect_filled(rect, CornerRadius::same(6), if hov { PANEL2 } else { PANEL });
    p.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0f32, if open { col } else if hov { FAINT } else { LINE }), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, label, pf(font), if hov { col } else { TEXT });
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    let mut pick = None;
    egui::popup::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(rect.width().max(170.0));
        menu_items(ui, items, &mut pick);
    });
    pick
}

/// The items of a menu (also used in context menus).
pub fn menu_items(ui: &mut Ui, items: &[(String, Color32)], pick: &mut Option<usize>) {
    egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| {
        for (i, (name, c)) in items.iter().enumerate() {
            if let Some(h) = name.strip_prefix("──") {
                ui.add_space(2.0);
                ui.label(egui::RichText::new(h.trim()).font(pf(10.0)).color(DIM));
                continue;
            }
            let (r, ir) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(160.0), 22.0), Sense::click());
            if ir.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(4), PANEL);
            }
            if *c != Color32::TRANSPARENT {
                ui.painter().circle_filled(r.left_center() + Vec2::new(10.0, 0.0), 4.0, *c);
            }
            ui.painter().text(r.left_center() + Vec2::new(22.0, 0.0), Align2::LEFT_CENTER, name, pf(12.0), TEXT);
            if ir.clicked() {
                *pick = Some(i);
                ui.close_menu();
            }
        }
    });
}

/// What a knob did this frame.
#[derive(Default)]
pub struct KnobOut {
    pub value: Option<f32>,
    pub started: bool,
    pub stopped: bool,
}

/// The knob itself: drag (shift = fine), wheel, double-click = default, click the value to type.
/// Works on a normalized value; `text` shows it, `parse` reads a typed value back.
#[allow(clippy::too_many_arguments)]
pub fn knob_core(
    ui: &mut Ui,
    id: egui::Id,
    c: Pos2,
    r: f32,
    norm: f32,
    default: f32,
    label: &str,
    text: &str,
    parse: &dyn Fn(&str) -> Option<f32>,
    tip: &str,
    bipolar: bool,
    col: Color32,
    enabled: bool,
) -> KnobOut {
    let mut out = KnobOut::default();
    let hit = Rect::from_center_size(c, Vec2::splat(2.0 * r + 8.0));
    let resp = ui.interact(hit, id, if enabled { Sense::click_and_drag() } else { Sense::hover() });
    let resp = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    let raw_id = id.with("raw");
    if resp.drag_started() {
        out.started = true;
        ui.data_mut(|d| d.insert_temp(raw_id, norm));
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let d = -resp.drag_delta().y / if fine { 1200.0 } else { 220.0 };
        let raw = ui.data(|dd| dd.get_temp::<f32>(raw_id)).unwrap_or(norm) + d;
        ui.data_mut(|dd| dd.insert_temp(raw_id, raw));
        out.value = Some(raw.clamp(0.0, 1.0));
    }
    if resp.drag_stopped() {
        out.stopped = true;
    }
    if resp.double_clicked() {
        out.started = true;
        out.value = Some(default);
        out.stopped = true;
    } else if resp.hovered() && enabled {
        let sc = ui.input(|i| i.smooth_scroll_delta.y);
        if sc != 0.0 {
            out.started = true;
            out.value = Some((norm + sc / 1500.0).clamp(0.0, 1.0));
            out.stopped = true;
        }
    }
    let shown = out.value.unwrap_or(norm);
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
    let col = if enabled { col } else { FAINT };
    painter.add(Shape::line(arc(0.0, 1.0), PathStroke::new(3.2f32, Color32::from_rgb(0x2a, 0x2f, 0x38))));
    let from = if bipolar { 0.5 } else { 0.0 };
    if (shown - from).abs() > 0.002 {
        painter.add(Shape::line(arc(from, shown), PathStroke::new(3.2f32, col)));
    }
    let body = r * 0.66;
    painter.circle_filled(c, body, if resp.hovered() && enabled { Color32::from_rgb(0x2e, 0x34, 0x3e) } else { Color32::from_rgb(0x26, 0x2b, 0x33) });
    painter.circle_stroke(c, body, Stroke::new(1.0f32, Color32::from_rgb(0x3a, 0x40, 0x4b)));
    painter.circle_filled(pt(body * 0.62, shown), 2.2, if enabled { TEXT } else { DIM });

    let vw = (2.0 * r + 26.0).max(56.0);
    let vr = Rect::from_center_size(Pos2::new(c.x, c.y + r + 9.0), Vec2::new(vw, 15.0));
    let edit_id = id.with("edit");
    let editing: Option<String> = ui.data(|d| d.get_temp::<String>(edit_id));
    ui.painter().text(Pos2::new(c.x, vr.bottom() + 8.0), Align2::CENTER_CENTER, label, pf(10.0), DIM);
    if let Some(mut buf) = editing {
        let te = egui::TextEdit::singleline(&mut buf).font(pf(10.5)).horizontal_align(egui::Align::Center).desired_width(vr.width()).margin(egui::Margin::symmetric(2, 1));
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
            if let Some(nv) = parse(&txt) {
                out.started = true;
                out.value = Some(nv.clamp(0.0, 1.0));
                out.stopped = true;
            }
            ui.data_mut(|d| d.remove::<String>(edit_id));
        } else {
            ui.data_mut(|d| d.insert_temp(edit_id, buf));
        }
    } else {
        let vresp = ui.interact(vr, id.with("val"), if enabled { Sense::click() } else { Sense::hover() });
        let vresp = if enabled { vresp.on_hover_text("Click to type a value") } else { vresp };
        let gw = ui.fonts(|f| f.layout_no_wrap(text.to_string(), pf(10.5), TEXT).size().x);
        let size = if gw > vr.width() - 4.0 { (10.5 * (vr.width() - 4.0) / gw).max(7.5) } else { 10.5 };
        ui.painter().text(vr.center(), Align2::CENTER_CENTER, text, pf(size), if !enabled { FAINT } else if vresp.hovered() { col } else { TEXT });
        if vresp.clicked() {
            let first = text.trim().split(' ').next().unwrap_or("").to_string();
            ui.data_mut(|d| d.insert_temp(edit_id, first));
        }
    }
    out
}

/// A knob on a host parameter.
#[allow(clippy::too_many_arguments)]
pub fn pknob<P: Param>(ui: &mut Ui, setter: &ParamSetter, p: &P, label: &str, c: Pos2, r: f32, tip: &str, bipolar: bool, col: Color32) {
    let norm = p.unmodulated_normalized_value();
    let text = p.normalized_value_to_string(norm, true);
    let id = egui::Id::new(("pknob", p.name()));
    let parse = |s: &str| p.string_to_normalized_value(s);
    let o = knob_core(ui, id, c, r, norm, p.default_normalized_value(), label, &text, &parse, tip, bipolar, col, true);
    if o.started {
        setter.begin_set_parameter(p);
    }
    if let Some(v) = o.value {
        setter.set_parameter_normalized(p, v);
    }
    if o.stopped {
        setter.end_set_parameter(p);
    }
}

/// A knob on a plain value of the saved state, mapped by a [`KnobDef`]; returns the new value.
pub fn sknob(ui: &mut Ui, id: &str, d: &KnobDef, v: f32, c: Pos2, r: f32, col: Color32, enabled: bool) -> Option<f32> {
    let norm = d.norm(v);
    let text = d.fmt(norm);
    let parse = |s: &str| d.parse(s);
    let o = knob_core(ui, egui::Id::new(("sknob", id)), c, r, norm, d.default_norm(), d.name, &text, &parse, d.tip, d.bipolar, col, enabled);
    o.value.map(|n| d.plain(n))
}

/// Horizontal peak meter, -36 .. 0 dBFS.
pub fn hmeter(ui: &Ui, rect: Rect, lin: f32, label: &str) {
    let p = ui.painter();
    p.text(Pos2::new(rect.left() - 6.0, rect.center().y), Align2::RIGHT_CENTER, label, pf(9.0), DIM);
    p.rect_filled(rect, CornerRadius::same(2), EDIT_BG);
    let db = 20.0 * lin.max(1e-6).log10();
    let t = ((db + 36.0) / 36.0).clamp(0.0, 1.0);
    let col = if db > -0.3 { HOT } else { OK };
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.left() + rect.width() * t, rect.bottom())), CornerRadius::same(2), col);
}

/// Vertical 0..1 meter (envelope, gain reduction).
pub fn vmeter(ui: &Ui, rect: Rect, t: f32, col: Color32, label: &str, from_top: bool) {
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(2), EDIT_BG);
    let h = rect.height() * t.clamp(0.0, 1.0);
    let r = if from_top { Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + h)) } else { Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - h), rect.max) };
    p.rect_filled(r, CornerRadius::same(2), col);
    if !label.is_empty() {
        p.text(Pos2::new(rect.center().x, rect.bottom() + 8.0), Align2::CENTER_CENTER, label, pf(9.0), DIM);
    }
}

/// The logo: an isometric cube, three faces in three colours, a wave across the front.
pub fn logo(ui: &Ui, c: Pos2, s: f32) {
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
pub fn wave_path(w: &Wave, inner: Rect, res: f32) -> Vec<Pos2> {
    let mut out = Vec::new();
    let px = |x: f32, y: f32| Pos2::new(inner.left() + x * inner.width(), inner.bottom() - y * inner.height());
    for s in 0..w.pts.len().saturating_sub(1) {
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
    if let Some(l) = w.pts.last() {
        out.push(px(l.x, l.y));
    }
    out
}

/// Gradient fill under a path down to `bottom`.
pub fn fill_under(path: &[Pos2], bottom: f32, col: Color32, top_a: u8, height: f32) -> Mesh {
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

/// A small wave picture (chain tiles, custom waves, menus).
pub fn thumb(ui: &Ui, w: &Wave, r: Rect, col: Color32, alpha: u8) {
    let path = wave_path(w, r, 80.0);
    ui.painter().add(fill_under(&path, r.bottom(), col, alpha, r.height()));
    ui.painter().add(Shape::line(path, PathStroke::new(1.2f32, col)));
}

/// A smooth outline through per-bin levels (lightly blurred, Catmull-Rom between bins):
/// points from left to right, `h(v)` = height in pixels above `base` (negative: below).
pub fn smooth_outline(vals: &[f32], left: f32, width: f32, base: f32, h: impl Fn(f32) -> f32) -> Vec<Pos2> {
    let n = vals.len();
    if n < 2 {
        return vec![];
    }
    // blur [1 2 1] (wraps: the scope is one cycle)
    let b: Vec<f32> = (0..n).map(|i| (vals[(i + n - 1) % n] + 2.0 * vals[i] + vals[(i + 1) % n]) * 0.25).collect();
    let y: Vec<f32> = b.iter().map(|&v| h(v)).collect();
    let at = |i: isize| y[i.rem_euclid(n as isize) as usize];
    let up = h(1.0) >= 0.0;
    let clip = |v: f32| if up { v.max(0.0) } else { v.min(0.0) };
    let sub = 3;
    let mut out = Vec::with_capacity(n * sub + 1);
    for i in 0..n as isize {
        let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        for k in 0..sub {
            let t = k as f32 / sub as f32;
            let v = 0.5 * (2.0 * p1 + (-p0 + p2) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t);
            let x = left + width * (i as f32 + 0.5 + t) / n as f32;
            out.push(Pos2::new(x.min(left + width), base - clip(v)));
        }
    }
    out.insert(0, Pos2::new(left, base - clip(at(0))));
    out.push(Pos2::new(left + width, base - clip(at(n as isize - 1))));
    out
}

/// Solid fill between an outline and a base line.
pub fn fill_to(path: &[Pos2], base: f32, col: Color32) -> Mesh {
    let mut m = Mesh::default();
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (b.x - a.x).abs() < 0.01 {
            continue;
        }
        let i = m.vertices.len() as u32;
        m.colored_vertex(a, col);
        m.colored_vertex(b, col);
        m.colored_vertex(Pos2::new(b.x, base), col);
        m.colored_vertex(Pos2::new(a.x, base), col);
        m.add_triangle(i, i + 1, i + 2);
        m.add_triangle(i, i + 2, i + 3);
    }
    m
}
