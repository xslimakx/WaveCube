//! WaveCube GUI.
//!
//! Top to bottom: preset bar; the chain (any kind, any order, duplicates, drag to reorder,
//! right-click for more); the selected item with its lanes and bands; the shape tools and the
//! shape editor with the scope behind it (or the tool's own view); custom waves; and the
//! LFO / envelope / item / bands / output panels. The preset browser, the save dialogs and the
//! settings open over the editor.

mod browser;
mod panels;
mod wave_ed;
pub mod widgets;

use nih_plug::prelude::*;
use nih_plug_egui::{
    create_egui_editor,
    egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2},
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::presets::{self, ItemPreset, PresetData};
use crate::wave::{Pen, Wave};
use crate::{Kind, Shared, State, WaveCubeParams, N_BANDS, N_SLOTS, SCOPE_N};
use widgets::*;

pub const BASE_W: f32 = 1180.0;
pub const BASE_H: f32 = 820.0;

pub const BAND_NAMES: [&str; N_BANDS] = ["LOW", "MID", "HIGH"];

#[derive(Clone, Copy, PartialEq)]
pub enum Tool {
    Pointer,
    Select,
    Pen(Pen),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Drag {
    Point(usize),
    Curve(usize, f32, f32),
    Paint,
    Box(Pos2),
    MoveSel(Pos2),
    End,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SaveMode {
    Preset,
    Item,
}

pub struct Library {
    pub factory: Vec<PresetData>,
    pub user: Vec<PresetData>,
    pub favs: Vec<String>,
    /// item presets per kind: factory, then user
    pub items: Vec<Vec<ItemPreset>>,
}

impl Library {
    fn load() -> Self {
        let mut l = Self { factory: presets::factory(), user: presets::list_user(), favs: presets::load_favorites(), items: vec![] };
        l.reload_items();
        l
    }
    pub fn reload_items(&mut self) {
        self.items = Kind::ALL
            .iter()
            .map(|&k| {
                let mut v = presets::items(k);
                v.extend(presets::list_user_items(k));
                v
            })
            .collect();
    }
    /// (is_user, index) of every preset
    pub fn entries(&self) -> impl Iterator<Item = (bool, usize)> + '_ {
        (0..self.factory.len()).map(|i| (false, i)).chain((0..self.user.len()).map(|i| (true, i)))
    }
    pub fn get(&self, user: bool, i: usize) -> &PresetData {
        if user {
            &self.user[i]
        } else {
            &self.factory[i]
        }
    }
    pub fn key(user: bool, name: &str) -> String {
        format!("{}:{name}", if user { "U" } else { "F" })
    }
}

pub struct Gui {
    pub in_lv: f32,
    pub out_lv: f32,
    pub scope_in: [f32; SCOPE_N],
    pub scope_out: [f32; SCOPE_N],
    pub freeze: bool,
    pub drag: Option<Drag>,
    pub sel_pts: Vec<usize>,
    pub move_orig: Option<Wave>,
    pub undo: Vec<Snapshot>,
    pub redo: Vec<Snapshot>,
    ck_key: String,
    ck_t: f64,
    pub clip: Option<Wave>,
    pub item_clip: Option<ItemPreset>,
    pub tool: Tool,
    pub snap: bool,
    pub lib: Option<Library>,
    pub browser: bool,
    pub save: Option<SaveMode>,
    pub save_name: String,
    pub save_cat: usize,
    pub save_focus: bool,
    pub settings: bool,
    pub search: String,
    pub filter: usize,
    pub kind_filter: Option<Kind>,
    pub status: Option<(String, f64)>,
    last_switch: u32,
    pub seed: u32,
    tab_drag: Option<usize>,
    lane_drag: Option<(usize, f32)>,
    pub ab: [Option<PresetData>; 2],
    pub ab_cur: usize,
    provided: [u64; N_SLOTS],
    pub gr_hist: Vec<f32>,
    pub env_hist: Vec<f32>,
    pub noise_files: Vec<String>,
    pub ir_files: Vec<String>,
}

impl Gui {
    fn new() -> Self {
        Self {
            in_lv: 0.0,
            out_lv: 0.0,
            scope_in: [0.0; SCOPE_N],
            scope_out: [0.0; SCOPE_N],
            freeze: false,
            drag: None,
            sel_pts: vec![],
            move_orig: None,
            undo: vec![],
            redo: vec![],
            ck_key: String::new(),
            ck_t: -10.0,
            clip: None,
            item_clip: None,
            tool: Tool::Pointer,
            snap: true,
            lib: None,
            browser: false,
            save: None,
            save_name: String::new(),
            save_cat: 0,
            save_focus: false,
            settings: false,
            search: String::new(),
            filter: 0,
            kind_filter: None,
            status: None,
            last_switch: 0,
            seed: 1,
            tab_drag: None,
            lane_drag: None,
            ab: [None, None],
            ab_cur: 0,
            provided: [0; N_SLOTS],
            gr_hist: vec![0.0; 240],
            env_hist: vec![0.0; 240],
            noise_files: vec![],
            ir_files: vec![],
        }
    }

    /// Remembers the state for undo. Edits with the same `key` close together are one step.
    pub fn checkpoint(&mut self, cx: &Cx, key: &str, now: f64) {
        if !key.is_empty() && self.ck_key == key && now - self.ck_t < 0.8 {
            self.ck_t = now;
            return;
        }
        self.ck_key = key.to_string();
        self.ck_t = now;
        if self.undo.last().map(|u| u.st == cx.st).unwrap_or(false) {
            return;
        }
        self.undo.push(Snapshot::take(cx));
        self.redo.clear();
        if self.undo.len() > 120 {
            self.undo.remove(0);
        }
    }

    pub fn say(&mut self, s: impl Into<String>, now: f64) {
        self.status = Some((s.into(), now));
    }

    pub fn rescan_media(&mut self) {
        self.noise_files = crate::dsp::media::noise_dir().map(|d| crate::dsp::media::list(&d)).unwrap_or_default();
        self.ir_files = crate::dsp::media::ir_dir().map(|d| crate::dsp::media::list(&d)).unwrap_or_default();
    }
}

/// An undo step: the saved state and the automatable controls of every slot.
pub struct Snapshot {
    pub st: State,
    slots: Vec<[f32; 17]>,
    /// a whole preset changed: every control comes back
    pub full: bool,
}

impl Snapshot {
    fn take(cx: &Cx) -> Self {
        let slots = cx
            .p
            .slots
            .iter()
            .map(|sp| {
                let mut v = [0.0; 17];
                v[0] = sp.on.unmodulated_normalized_value();
                v[1] = sp.tile_a.unmodulated_normalized_value();
                v[2] = sp.tile_b.unmodulated_normalized_value();
                v[3] = sp.mix.unmodulated_normalized_value();
                for b in 0..N_BANDS {
                    v[4 + b] = sp.band_mix(b).unmodulated_normalized_value();
                }
                for k in 0..crate::N_KNOBS {
                    v[7 + k] = sp.knob(k).unmodulated_normalized_value();
                }
                v
            })
            .collect();
        Self { st: cx.st.clone(), slots, full: false }
    }

    /// Back to this step. The knobs of a slot come back only where the slot changed kind or
    /// came / went (knob tweaks are not undo steps of their own).
    fn restore(&self, cx: &Cx) {
        for s in 0..N_SLOTS {
            let was = (self.st.slots[s].kind, self.st.chain.contains(&(s as u8)));
            let now = (cx.st.slots[s].kind, cx.st.chain.contains(&(s as u8)));
            if was == now && !self.full {
                continue;
            }
            let sp = &cx.p.slots[s];
            let v = &self.slots[s];
            set_n(cx.setter, &sp.on, v[0]);
            set_n(cx.setter, &sp.tile_a, v[1]);
            set_n(cx.setter, &sp.tile_b, v[2]);
            set_n(cx.setter, &sp.mix, v[3]);
            for b in 0..N_BANDS {
                set_n(cx.setter, sp.band_mix(b), v[4 + b]);
            }
            for k in 0..crate::N_KNOBS {
                set_n(cx.setter, sp.knob(k), v[7 + k]);
            }
        }
        let st = self.st.clone();
        commit(cx.p, cx.shared, |x| *x = st);
    }
}

pub fn set_n<P: Param>(setter: &ParamSetter, p: &P, v: f32) {
    if (p.unmodulated_normalized_value() - v).abs() > 1e-7 {
        setter.begin_set_parameter(p);
        setter.set_parameter_normalized(p, v);
        setter.end_set_parameter(p);
    }
}

/// Applies a change to the saved state, repairs it and tells the audio thread.
pub fn commit(p: &WaveCubeParams, shared: &Shared, f: impl FnOnce(&mut State)) {
    let mut st = p.state.write();
    f(&mut st);
    st.sanitize();
    p.sync_kinds(&st);
    drop(st);
    shared.bump_settings();
    shared.bump_waves();
}

/// Changes a shape: one band in multiband mode, else all three (so they start alike when
/// multiband is switched on).
pub fn edit_wave(p: &WaveCubeParams, shared: &Shared, s: usize, lane: usize, band: usize, multi: bool, f: impl FnOnce(&mut Wave)) {
    commit(p, shared, |st| {
        let b = if multi { band } else { 0 };
        f(&mut st.waves[s][lane][b]);
        st.waves[s][lane][b].sanitize();
        if !multi {
            let w = st.waves[s][lane][0].clone();
            st.waves[s][lane][1] = w.clone();
            st.waves[s][lane][2] = w;
        }
    });
}

pub fn set_b(setter: &ParamSetter, p: &BoolParam, v: bool) {
    setter.begin_set_parameter(p);
    setter.set_parameter(p, v);
    setter.end_set_parameter(p);
}


/// Name of an item in the chain ("Drive", "Drive 2", ...).
pub fn item_name(st: &State, s: usize, short: bool) -> String {
    let k = st.slots[s].kind;
    let name = if short && k == Kind::Compressor { "Comp" } else { k.name() };
    let same: Vec<u8> = st.chain.iter().copied().filter(|&c| st.slots[c as usize].kind == k).collect();
    match same.iter().position(|&c| c as usize == s) {
        Some(i) if same.len() > 1 => format!("{name} {}", i + 1),
        _ => name.to_string(),
    }
}

/// Everything a part of the frame needs.
pub struct Cx<'a> {
    pub p: &'a WaveCubeParams,
    pub shared: &'a Shared,
    pub setter: &'a ParamSetter<'a>,
    pub o: Pos2,
    pub st: State,
    pub sel: usize,
    pub kind: Kind,
    pub col: Color32,
    pub lane: usize,
    pub band: usize,
    pub multi: bool,
    pub modal: bool,
    pub now: f64,
    pub tempo: f32,
}

impl Cx<'_> {
    pub fn r(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(self.o.x + x, self.o.y + y), Vec2::new(w, h))
    }
    pub fn at(&self, x: f32, y: f32) -> Pos2 {
        Pos2::new(self.o.x + x, self.o.y + y)
    }
    /// The bands an LFO / envelope edit goes to.
    pub fn bands(&self, linked_field: bool) -> Vec<usize> {
        if !self.multi || (linked_field && self.st.slots[self.sel].link[self.lane]) {
            (0..N_BANDS).collect()
        } else {
            vec![self.band]
        }
    }
    pub fn in_chain(&self) -> bool {
        self.st.chain.contains(&(self.sel as u8))
    }
}

pub fn create(params: Arc<WaveCubeParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    let editor_state = params.editor_state.clone();
    let size_state = editor_state.clone();
    create_egui_editor(
        editor_state,
        Gui::new(),
        |ctx, _| {
            visuals(ctx);
            install_fonts(ctx);
        },
        move |ctx, setter, gui| {
            ctx.request_repaint_after(Duration::from_millis(16));
            let p = &*params;
            let now = ctx.input(|i| i.time);
            if gui.lib.is_none() {
                gui.lib = Some(Library::load());
                gui.rescan_media();
            }
            demo_setup(p, &shared, setter, gui, &size_state);

            // MIDI Switch from the audio thread: show the loaded custom wave in the editor
            let sw = shared.switched.load(Ordering::Acquire);
            if sw != gui.last_switch {
                gui.last_switch = sw;
                let slot = (sw & 0xff) as usize;
                let mut st = p.state.write();
                if let Some(Some(c)) = st.custom.get(slot).cloned() {
                    st.waves[c.slot as usize][c.lane as usize][c.band as usize] = c.wave;
                }
                drop(st);
            }

            let st = p.state.read().clone();
            // the big buffers the chain needs (made here, never on the audio thread)
            crate::dsp::provide_heavy(&st, &shared, &mut gui.provided);

            let sel = (st.selected as usize).min(N_SLOTS - 1);
            let kind = st.slots[sel].kind;
            let multi = st.slots[sel].multiband;
            let lane = (st.lane as usize).min(kind.n_lanes().max(1) - 1);
            let band = if multi { (st.band as usize).min(N_BANDS - 1) } else { 0 };
            shared.view.store(sel as u32, Ordering::Relaxed);
            shared.view_lane.store(lane as u32, Ordering::Relaxed);
            shared.view_band.store(band as u32, Ordering::Relaxed);

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
            gui.gr_hist.remove(0);
            gui.gr_hist.push(shared.gr[sel].load(Ordering::Relaxed));
            gui.env_hist.remove(0);
            gui.env_hist.push(shared.env[sel][lane].load(Ordering::Relaxed));
            let tempo = shared.tempo.load(Ordering::Relaxed);

            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG)).show(ctx, |ui| {
                let full = ui.max_rect();
                let modal = gui.browser || gui.save.is_some() || gui.settings;
                let cx = Cx { p, shared: &shared, setter, o: full.min, st, sel, kind, col: kc(kind), lane, band, multi, modal, now, tempo };

                header(ui, &cx, gui, &size_state);
                chain_bar(ui, &cx, gui);
                if cx.st.chain.is_empty() {
                    empty_chain(ui, &cx, gui);
                } else {
                    item_header(ui, &cx, gui);
                    if kind.is_tool() {
                        panels::tool_view(ui, &cx, gui);
                    } else {
                        wave_ed::toolbar(ui, &cx, gui);
                        wave_ed::editor(ui, &cx, gui);
                        wave_ed::custom_row(ui, &cx, gui);
                    }
                    panels::bottom(ui, &cx, gui);
                }
                shortcuts(ui, &cx, gui);

                if gui.browser {
                    browser::browser(ui, &cx, gui);
                }
                if gui.save.is_some() {
                    browser::save_dialog(ui, &cx, gui);
                }
                if gui.settings {
                    browser::settings(ui, &cx, gui, &size_state);
                }

                // status line
                if let Some((s, t)) = gui.status.clone() {
                    if now - t < 3.0 {
                        let r = cx.r(BASE_W * 0.5 - 200.0, BASE_H - 30.0, 400.0, 22.0);
                        ui.painter().rect_filled(r, CornerRadius::same(6), PANEL2);
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, s, pf(11.5), TEXT);
                    } else {
                        gui.status = None;
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
                    ui.painter().line_segment([Pos2::new(full.max.x - 3.0, full.max.y - k), Pos2::new(full.max.x - k, full.max.y - 3.0)], Stroke::new(1.0f32, gc));
                }
            });
        },
    )
}

/// Developer helper for screenshots:
/// `WAVECUBE_DEMO_SET="preset=<factory index>,sel=<chain position>,lane=1,band=0,tool=2,browser=1,save=1,settings=1,size=1320"`
fn demo_setup(p: &WaveCubeParams, shared: &Shared, setter: &ParamSetter, gui: &mut Gui, size: &nih_plug_egui::EguiState) {
    static DEMO: std::sync::Once = std::sync::Once::new();
    DEMO.call_once(|| {
        for kv in std::env::var("WAVECUBE_DEMO_SET").unwrap_or_default().split(',') {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let Ok(v) = v.trim().parse::<f32>() else { continue };
            match k.trim() {
                "preset" => {
                    if let Some(d) = gui.lib.as_ref().and_then(|l| l.factory.get(v as usize)).cloned() {
                        presets::apply(setter, p, &d);
                    }
                }
                "sel" => {
                    let mut st = p.state.write();
                    if let Some(&s) = st.chain.get(v as usize) {
                        st.selected = s;
                    }
                }
                "lane" => p.state.write().lane = v as u8,
                "band" => p.state.write().band = v as u8,
                "tool" => {
                    gui.tool = match v as usize {
                        0 => Tool::Pointer,
                        1 => Tool::Select,
                        n => Tool::Pen(Pen::ALL[(n - 2).min(5)]),
                    }
                }
                "browser" => gui.browser = v > 0.5,
                "save" => {
                    gui.save = if v > 0.5 { Some(SaveMode::Preset) } else { None };
                    gui.save_name = "My pump".into();
                }
                "settings" => gui.settings = v > 0.5,
                "size" => size.set_requested_size((v as u32, (v * BASE_H / BASE_W) as u32)),
                _ => {}
            }
        }
        commit(p, shared, |_| {});
    });
}

fn load_preset(cx: &Cx, gui: &mut Gui, d: &PresetData) {
    gui.ck_key.clear();
    gui.undo.push(Snapshot { full: true, ..Snapshot::take(cx) });
    gui.redo.clear();
    presets::apply(cx.setter, cx.p, d);
    cx.shared.bump_settings();
    cx.shared.bump_waves();
    gui.sel_pts.clear();
}

// ------------------------------------------------------------------------------------------
// Header: logo, presets, A / B, settings, meters, bypass
// ------------------------------------------------------------------------------------------

fn header(ui: &mut Ui, cx: &Cx, gui: &mut Gui, size: &nih_plug_egui::EguiState) {
    let painter = ui.painter().clone();
    let col = cx.col;
    let modal = cx.modal;
    logo(ui, cx.at(28.0, 28.0), 13.0);
    painter.text(cx.at(48.0, 28.0), Align2::LEFT_CENTER, "WaveCube", FontId::proportional(20.0), TEXT);

    let lib = gui.lib.as_ref().unwrap();
    let filtered = browser::filtered(lib, gui);
    let cur_idx = filtered.iter().position(|&(u, i)| lib.get(u, i).name == cx.st.preset);
    let cat = lib.entries().map(|(u, i)| lib.get(u, i)).find(|d| d.name == cx.st.preset).map(|d| d.category.clone()).unwrap_or_default();
    let cur_is_user = lib.user.iter().any(|d| d.name == cx.st.preset);
    let cur_key = Library::key(cur_is_user, &cx.st.preset);
    let cur_fav = lib.favs.contains(&cur_key);
    let x0 = 196.0;
    let mut load: Option<PresetData> = None;
    if btn(ui, cx.r(x0, 15.0, 28.0, 26.0), "◀", false, col, "Previous preset").clicked() && !filtered.is_empty() && !modal {
        let i = cur_idx.map(|i| (i + filtered.len() - 1) % filtered.len()).unwrap_or(0);
        load = Some(lib.get(filtered[i].0, filtered[i].1).clone());
    }
    let nr = cx.r(x0 + 32.0, 15.0, 268.0, 26.0);
    let nresp = ui.allocate_rect(nr, Sense::click()).on_hover_text("Open the preset browser");
    painter.rect_filled(nr, CornerRadius::same(5), if nresp.hovered() || gui.browser { PANEL2 } else { EDIT_BG });
    painter.rect_stroke(nr, CornerRadius::same(5), Stroke::new(1.0f32, if gui.browser { col } else { LINE }), StrokeKind::Inside);
    if !cat.is_empty() {
        painter.text(nr.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, cat.to_uppercase(), pf(8.5), DIM);
    }
    painter.text(nr.center() + Vec2::new(if cat.is_empty() { 0.0 } else { 28.0 }, 0.0), Align2::CENTER_CENTER, &cx.st.preset, pf(12.5), TEXT);
    painter.text(nr.right_center() - Vec2::new(10.0, 0.0), Align2::RIGHT_CENTER, if gui.browser { "▴" } else { "▾" }, pf(11.0), DIM);
    if btn(ui, cx.r(x0 + 304.0, 15.0, 28.0, 26.0), "▶", false, col, "Next preset").clicked() && !filtered.is_empty() && !modal {
        let i = cur_idx.map(|i| (i + 1) % filtered.len()).unwrap_or(0);
        load = Some(lib.get(filtered[i].0, filtered[i].1).clone());
    }
    if btn_f(ui, cx.r(x0 + 336.0, 15.0, 28.0, 26.0), "⚄", false, col, "A random preset (from the browser's current filter)", 15.0).clicked() && !filtered.is_empty() && !modal {
        gui.seed = gui.seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let mut i = (gui.seed >> 8) as usize % filtered.len();
        if Some(i) == cur_idx && filtered.len() > 1 {
            i = (i + 1) % filtered.len();
        }
        load = Some(lib.get(filtered[i].0, filtered[i].1).clone());
    }
    if nresp.clicked() {
        gui.browser = !gui.browser;
        gui.save = None;
        gui.settings = false;
        if gui.browser {
            if let Some(l) = gui.lib.as_mut() {
                l.user = presets::list_user();
            }
        }
    }
    if btn(ui, cx.r(x0 + 370.0, 15.0, 54.0, 26.0), "SAVE", gui.save == Some(SaveMode::Preset), col, "Save everything as a preset").clicked() {
        browser::open_save(cx, gui, SaveMode::Preset);
    }
    if btn(ui, cx.r(x0 + 428.0, 15.0, 30.0, 26.0), if cur_fav { "★" } else { "☆" }, cur_fav, GOLD, "Favourite").clicked() {
        let l = gui.lib.as_mut().unwrap();
        if cur_fav {
            l.favs.retain(|f| f != &cur_key);
        } else {
            l.favs.push(cur_key.clone());
        }
        presets::save_favorites(&l.favs);
    }
    // A / B: two complete settings to compare
    for (i, name) in ["A", "B"].iter().enumerate() {
        let r = cx.r(x0 + 466.0 + i as f32 * 32.0, 15.0, 30.0, 26.0);
        let tip = "A / B: two complete settings to compare.\nClick the other letter to switch (the first switch copies the current settings).\nShift-click: copy the current settings into that letter.";
        if btn(ui, r, name, gui.ab_cur == i, GOLD, tip).clicked() && !modal {
            let shift = ui.input(|k| k.modifiers.shift);
            let now_state = presets::capture(cx.p, &cx.st.preset, "");
            if shift {
                gui.ab[i] = Some(now_state);
                gui.say(format!("Copied into {name}"), cx.now);
            } else if gui.ab_cur != i {
                let cur = gui.ab_cur;
                gui.ab[cur] = Some(now_state.clone());
                let target = gui.ab[i].clone().unwrap_or(now_state);
                gui.ab[i] = Some(target.clone());
                gui.ab_cur = i;
                load = Some(target);
            }
        }
    }
    // undo / redo of everything (chain, settings, shapes)
    let can_undo = !gui.undo.is_empty();
    if btn(ui, cx.r(x0 + 540.0, 15.0, 30.0, 26.0), "↶", false, col, "Undo (Ctrl+Z)").clicked() && can_undo && !modal {
        undo(cx, gui);
    }
    if btn(ui, cx.r(x0 + 572.0, 15.0, 30.0, 26.0), "↷", false, col, "Redo (Ctrl+Shift+Z)").clicked() && !modal {
        redo(cx, gui);
    }
    if btn_f(ui, cx.r(x0 + 610.0, 15.0, 34.0, 26.0), "⚙", gui.settings, col, "Settings: audio trigger, MIDI, window size, folders.", 15.0).clicked() {
        gui.settings = !gui.settings;
        gui.browser = false;
        gui.save = None;
        let _ = size;
    }
    if let Some(d) = load.take() {
        load_preset(cx, gui, &d);
    }
    // meters + bypass
    let byp = cx.p.bypass.value();
    if btn(ui, cx.r(BASE_W - 14.0 - 76.0, 15.0, 76.0, 26.0), "BYPASS", byp, HOT, "Bypass the plugin.").clicked() {
        set_b(cx.setter, &cx.p.bypass, !byp);
    }
    hmeter(ui, cx.r(BASE_W - 14.0 - 76.0 - 14.0 - 110.0, 17.0, 110.0, 7.0), gui.in_lv, "IN");
    hmeter(ui, cx.r(BASE_W - 14.0 - 76.0 - 14.0 - 110.0, 32.0, 110.0, 7.0), gui.out_lv, "OUT");
}

pub fn undo(cx: &Cx, gui: &mut Gui) {
    if let Some(s) = gui.undo.pop() {
        let mut r = Snapshot::take(cx);
        r.full = s.full;
        gui.redo.push(r);
        s.restore(cx);
        gui.ck_key.clear();
        gui.sel_pts.clear();
    }
}

pub fn redo(cx: &Cx, gui: &mut Gui) {
    if let Some(s) = gui.redo.pop() {
        let mut r = Snapshot::take(cx);
        r.full = s.full;
        gui.undo.push(r);
        s.restore(cx);
        gui.ck_key.clear();
        gui.sel_pts.clear();
    }
}

fn shortcuts(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    if cx.modal || ui.ctx().wants_keyboard_input() {
        return;
    }
    let (u, r, del, esc) = ui.input(|i| {
        let z = i.modifiers.command && i.key_pressed(egui::Key::Z);
        (z && !i.modifiers.shift, (z && i.modifiers.shift) || (i.modifiers.command && i.key_pressed(egui::Key::Y)), i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::Escape))
    });
    if u {
        undo(cx, gui);
    }
    if r {
        redo(cx, gui);
    }
    if esc {
        gui.sel_pts.clear();
    }
    if del && !gui.sel_pts.is_empty() && !cx.kind.is_tool() {
        gui.checkpoint(cx, "", cx.now);
        let mut idx = gui.sel_pts.clone();
        idx.sort_unstable();
        edit_wave(cx.p, cx.shared, cx.sel, cx.lane, cx.band, cx.multi, |w| {
            for &i in idx.iter().rev() {
                if i > 0 && i + 1 < w.pts.len() {
                    w.remove(i);
                }
            }
        });
        gui.sel_pts.clear();
    }
}

// ------------------------------------------------------------------------------------------
// The chain
// ------------------------------------------------------------------------------------------

/// Adds a new item of `kind` to the chain (after position `at`, or at the end).
pub fn add_item(cx: &Cx, gui: &mut Gui, kind: Kind, at: Option<usize>) {
    let Some(s) = cx.st.free_slot() else {
        gui.say("The chain is full (12 items)", cx.now);
        return;
    };
    gui.checkpoint(cx, "", cx.now);
    presets::reset_item(cx.setter, cx.p, s, kind);
    set_b(cx.setter, &cx.p.slots[s].on, true);
    commit(cx.p, cx.shared, |st| {
        let pos = at.map(|a| (a + 1).min(st.chain.len())).unwrap_or(st.chain.len());
        st.chain.insert(pos, s as u8);
        st.selected = s as u8;
        st.lane = 0;
        st.band = 0;
    });
    gui.sel_pts.clear();
}

pub fn duplicate_item(cx: &Cx, gui: &mut Gui, s: usize) {
    let Some(to) = cx.st.free_slot() else {
        gui.say("The chain is full (12 items)", cx.now);
        return;
    };
    gui.checkpoint(cx, "", cx.now);
    presets::copy_item(cx.setter, cx.p, s, to);
    set_b(cx.setter, &cx.p.slots[to].on, cx.p.slots[s].on.value());
    commit(cx.p, cx.shared, |st| {
        let pos = st.chain.iter().position(|&c| c as usize == s).map(|i| i + 1).unwrap_or(st.chain.len());
        st.chain.insert(pos, to as u8);
        st.selected = to as u8;
    });
}

pub fn remove_item(cx: &Cx, gui: &mut Gui, s: usize) {
    gui.checkpoint(cx, "", cx.now);
    commit(cx.p, cx.shared, |st| {
        let pos = st.chain.iter().position(|&c| c as usize == s).unwrap_or(0);
        st.chain.retain(|&c| c as usize != s);
        if st.selected as usize == s {
            st.selected = st.chain.get(pos.min(st.chain.len().saturating_sub(1))).copied().unwrap_or(0);
        }
        // the custom waves that pointed here have nowhere to go
        for c in st.custom.iter_mut() {
            if c.as_ref().map(|c| c.slot as usize == s).unwrap_or(false) {
                *c = None;
            }
        }
    });
    if cx.shared.env_listen.load(Ordering::Relaxed) as usize == s + 1 {
        cx.shared.env_listen.store(0, Ordering::Relaxed);
    }
    cx.shared.solo[s].store(0, Ordering::Relaxed);
    gui.sel_pts.clear();
}

pub fn replace_item(cx: &Cx, gui: &mut Gui, s: usize, kind: Kind) {
    gui.checkpoint(cx, "", cx.now);
    presets::reset_item(cx.setter, cx.p, s, kind);
    commit(cx.p, cx.shared, |st| {
        st.selected = s as u8;
        st.lane = 0;
    });
}

pub fn kind_menu_items(with_tools: bool) -> Vec<(String, Color32)> {
    let mut v: Vec<(String, Color32)> = vec![("──Shapers".into(), Color32::TRANSPARENT)];
    v.extend(Kind::SHAPERS.iter().map(|&k| (k.name().to_string(), kc(k))));
    if with_tools {
        v.push(("──Tools".into(), Color32::TRANSPARENT));
        v.push((Kind::Compressor.name().into(), kc(Kind::Compressor)));
        v.push((Kind::Scope.name().into(), kc(Kind::Scope)));
    }
    v
}

/// Index in [`kind_menu_items`] -> kind.
pub fn kind_of_menu(i: usize) -> Option<Kind> {
    match i {
        0 => None,
        1..=10 => Some(Kind::SHAPERS[i - 1]),
        12 => Some(Kind::Compressor),
        13 => Some(Kind::Scope),
        _ => None,
    }
}

fn chain_bar(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    let painter = ui.painter().clone();
    let st = &cx.st;
    let ty = 54.0;
    let th = 60.0;
    let gap = 5.0;
    let plus_w = 44.0;
    let tw = (BASE_W - 28.0 - plus_w - gap * N_SLOTS as f32) / N_SLOTS as f32;
    let chain: Vec<usize> = st.chain.iter().map(|&i| i as usize).collect();
    let slot_x = |pos: usize| 14.0 + pos as f32 * (tw + gap);
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let o = cx.o;
    let drop_pos = match (gui.tab_drag, pointer) {
        (Some(_), Some(pp)) => Some((((pp.x - o.x - 14.0) / (tw + gap)).floor().max(0.0) as usize).min(chain.len().saturating_sub(1))),
        _ => None,
    };
    let modal = cx.modal;
    let mut action: Option<Box<dyn FnOnce(&Cx, &mut Gui)>> = None;
    for (pos, &si) in chain.iter().enumerate() {
        let k = st.slots[si].kind;
        let kcol = kc(k);
        let r = cx.r(slot_x(pos), ty, tw, th);
        let on = cx.p.slots[si].on.value();
        let is_sel = si == cx.sel;
        let dragging_me = gui.tab_drag == Some(si);
        let name = item_name(st, si, true);
        // the tile first, the small buttons after it (the later one gets the click)
        let resp = ui.interact(r, egui::Id::new(("tab", si)), Sense::click_and_drag()).on_hover_text(format!(
            "{}: {}\nClick: edit · drag: move it in the chain · right-click: duplicate, replace, presets, remove.",
            item_name(st, si, false),
            k.about()
        ));
        let dot = Rect::from_center_size(Pos2::new(r.right() - 11.0, r.top() + 13.0), Vec2::splat(16.0));
        let dresp = ui.allocate_rect(dot, Sense::click()).on_hover_text(if on { "Switch off" } else { "Switch on" });
        let hovering = resp.hovered() || dresp.hovered();
        let xr = Rect::from_center_size(Pos2::new(r.right() - 11.0, r.bottom() - 11.0), Vec2::splat(15.0));
        let xresp = ui.allocate_rect(xr, Sense::click()).on_hover_text("Remove from the chain");
        if !modal {
            if dresp.clicked() {
                set_b(cx.setter, &cx.p.slots[si].on, !on);
            } else if xresp.clicked() && (hovering || xresp.hovered()) {
                action = Some(Box::new(move |cx, gui| remove_item(cx, gui, si)));
            } else if resp.drag_started() {
                gui.tab_drag = Some(si);
            } else if resp.clicked() && !is_sel {
                commit(cx.p, cx.shared, |s| {
                    s.selected = si as u8;
                    s.lane = 0;
                });
                gui.sel_pts.clear();
            }
        }
        resp.context_menu(|ui| {
            ui.set_min_width(190.0);
            ui.label(egui::RichText::new(item_name(st, si, false)).font(pf(12.0)).color(kcol));
            ui.separator();
            if ui.button("Duplicate").clicked() {
                action = Some(Box::new(move |cx, gui| duplicate_item(cx, gui, si)));
                ui.close_menu();
            }
            ui.menu_button("Replace with", |ui| {
                for &nk in Kind::ALL.iter() {
                    if ui.button(egui::RichText::new(nk.name()).color(kc(nk))).clicked() {
                        action = Some(Box::new(move |cx, gui| replace_item(cx, gui, si, nk)));
                        ui.close_menu();
                    }
                }
            });
            ui.menu_button("Add after", |ui| {
                for &nk in Kind::ALL.iter() {
                    if ui.button(egui::RichText::new(nk.name()).color(kc(nk))).clicked() {
                        action = Some(Box::new(move |cx, gui| add_item(cx, gui, nk, Some(pos))));
                        ui.close_menu();
                    }
                }
            });
            if ui.button("Copy item").clicked() {
                gui.item_clip = Some(presets::capture_item(cx.p, si, "Clipboard"));
                ui.close_menu();
            }
            let can_paste = gui.item_clip.is_some();
            if ui.add_enabled(can_paste, egui::Button::new("Paste item here")).clicked() {
                if let Some(it) = gui.item_clip.clone() {
                    action = Some(Box::new(move |cx, gui| {
                        gui.checkpoint(cx, "", cx.now);
                        if let Some(u) = gui.undo.last_mut() {
                            u.full = true;
                        }
                        presets::apply_item(cx.setter, cx.p, si, &it);
                        commit(cx.p, cx.shared, |s| s.selected = si as u8);
                    }));
                }
                ui.close_menu();
            }
            if ui.button("Reset").clicked() {
                action = Some(Box::new(move |cx, gui| replace_item(cx, gui, si, k)));
                ui.close_menu();
            }
            ui.separator();
            if ui.button(if on { "Switch off" } else { "Switch on" }).clicked() {
                set_b(cx.setter, &cx.p.slots[si].on, !on);
                ui.close_menu();
            }
            if ui.button(egui::RichText::new("Remove").color(HOT)).clicked() {
                action = Some(Box::new(move |cx, gui| remove_item(cx, gui, si)));
                ui.close_menu();
            }
        });
        let r = if dragging_me {
            let px = pointer.map(|pp| pp.x - tw * 0.5).unwrap_or(r.left());
            Rect::from_min_size(Pos2::new(px.clamp(o.x + 14.0, o.x + BASE_W - 14.0 - tw), r.top() - 3.0), r.size())
        } else {
            r
        };
        let painter = if dragging_me { ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("dragged_tab"))) } else { painter.clone() };
        let fill = if is_sel || dragging_me { PANEL2 } else if resp.hovered() { Color32::from_rgb(0x1c, 0x20, 0x27) } else { PANEL };
        painter.rect_filled(r, CornerRadius::same(7), fill);
        painter.rect_stroke(r, CornerRadius::same(7), Stroke::new(if is_sel || dragging_me { 1.5f32 } else { 1.0f32 }, if is_sel || dragging_me { kcol } else { LINE }), StrokeKind::Inside);
        if on {
            painter.rect_filled(Rect::from_min_size(r.min + Vec2::new(8.0, 0.0), Vec2::new(r.width() - 16.0, 2.5)), CornerRadius::same(1), kcol);
        }
        painter.text(Pos2::new(r.left() + 8.0, r.top() + 13.0), Align2::LEFT_CENTER, &name, pf(11.0), if on { TEXT } else { DIM });
        let dc = Pos2::new(r.right() - 11.0, r.top() + 13.0);
        if on {
            painter.circle_filled(dc, 4.5, kcol);
        } else {
            painter.circle_stroke(dc, 4.5, Stroke::new(1.3f32, if dresp.hovered() { kcol } else { FAINT }));
        }
        let thumb_r = Rect::from_min_max(Pos2::new(r.left() + 7.0, r.top() + 25.0), Pos2::new(r.right() - 7.0, r.bottom() - 6.0));
        let tc = if on { kcol } else { FAINT };
        if k.is_tool() {
            // tools: their own little picture
            let g = cx.shared.gr[si].load(Ordering::Relaxed);
            if k == Kind::Compressor {
                let t = (g / 24.0).clamp(0.0, 1.0);
                painter.rect_filled(Rect::from_min_size(thumb_r.min, Vec2::new(thumb_r.width(), 6.0)), CornerRadius::same(2), EDIT_BG);
                painter.rect_filled(Rect::from_min_size(thumb_r.min, Vec2::new(thumb_r.width() * t, 6.0)), CornerRadius::same(2), tc);
                painter.text(thumb_r.left_bottom() + Vec2::new(0.0, -6.0), Align2::LEFT_CENTER, format!("GR {:.1} dB", g), pf(9.0), DIM);
            } else {
                let pts: Vec<Pos2> = (0..=30).map(|i| Pos2::new(thumb_r.left() + thumb_r.width() * i as f32 / 30.0, thumb_r.center().y - (i as f32 * 0.9).sin() * thumb_r.height() * 0.35 * (1.0 - i as f32 / 40.0))).collect();
                painter.add(egui::Shape::line(pts, egui::epaint::PathStroke::new(1.2f32, tc)));
            }
        } else {
            let w = &st.waves[si][0][if st.slots[si].multiband { 1 } else { 0 }];
            let path = wave_path(w, thumb_r, 60.0);
            painter.add(fill_under(&path, thumb_r.bottom(), tc, if on { 80 } else { 25 }, thumb_r.height()));
            painter.add(egui::Shape::line(path, egui::epaint::PathStroke::new(1.2f32, tc)));
            if on {
                let lfo = st.slots[si].lfo[0][0];
                let ph = (cx.shared.phase[si][0].load(Ordering::Relaxed) + lfo.phase / 360.0).rem_euclid(1.0);
                painter.vline(thumb_r.left() + ph * thumb_r.width(), thumb_r.y_range(), Stroke::new(1.0f32, TEXT.gamma_multiply(0.5)));
            }
        }
        if (hovering || xresp.hovered()) && !dragging_me {
            painter.rect_filled(xr, CornerRadius::same(3), if xresp.hovered() { PANEL2 } else { fill });
            painter.text(xr.center(), Align2::CENTER_CENTER, "×", pf(13.0), if xresp.hovered() { HOT } else { DIM });
        }
        if st.slots[si].multiband {
            painter.text(Pos2::new(r.right() - 22.0, r.top() + 13.0), Align2::RIGHT_CENTER, "MB", pf(8.0), DIM);
        }
    }
    // drop marker / finish the drag
    if let (Some(si), Some(dp)) = (gui.tab_drag, drop_pos) {
        let from = chain.iter().position(|&c| c == si).unwrap_or(0);
        if dp != from {
            let xm = if dp > from { slot_x(dp) + tw + gap * 0.5 } else { slot_x(dp) - gap * 0.5 };
            painter.vline(o.x + xm, (o.y + ty - 2.0)..=(o.y + ty + th + 2.0), Stroke::new(2.5f32, kc(st.slots[si].kind)));
        }
        if ui.input(|i| i.pointer.any_released()) {
            if dp != from {
                gui.checkpoint(cx, "", cx.now);
                commit(cx.p, cx.shared, |s| {
                    let v = s.chain.remove(from);
                    s.chain.insert(dp.min(s.chain.len()), v);
                });
            }
            gui.tab_drag = None;
        }
    }
    if gui.tab_drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
        gui.tab_drag = None;
    }
    // "+": add any kind, as often as you like
    if chain.len() < N_SLOTS {
        let pr = cx.r(slot_x(chain.len()), ty, plus_w, th);
        if let Some(i) = menu_btn(ui, "addkind", pr, "+", &kind_menu_items(true), TEXT, "Add an item to the chain (the same kind as often as you like).", 22.0).filter(|_| !modal) {
            if let Some(k) = kind_of_menu(i) {
                let at = chain.iter().position(|&c| c == cx.sel).filter(|_| ui.input(|i| i.modifiers.shift));
                action = Some(Box::new(move |cx, gui| add_item(cx, gui, k, at)));
            }
        }
    }
    if let Some(a) = action {
        a(cx, gui);
    }
}

fn empty_chain(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    let r = cx.r(14.0, 130.0, BASE_W - 28.0, BASE_H - 144.0);
    ui.painter().rect_filled(r, CornerRadius::same(8), EDIT_BG);
    ui.painter().text(r.center() - Vec2::new(0.0, 60.0), Align2::CENTER_CENTER, "The chain is empty", pf(17.0), TEXT);
    ui.painter().text(r.center() - Vec2::new(0.0, 34.0), Align2::CENTER_CENTER, "Add items with + above, or pick one here. Any kind, any order, as many as you like (12 in all).", pf(12.0), DIM);
    let kinds = Kind::ALL;
    let bw = 120.0;
    let x0 = r.center().x - (6.0 * bw + 5.0 * 6.0) * 0.5;
    for (i, &k) in kinds.iter().enumerate() {
        let (row, colx) = (i / 6, i % 6);
        let br = Rect::from_min_size(Pos2::new(x0 + colx as f32 * (bw + 6.0), r.center().y + row as f32 * 36.0), Vec2::new(bw, 30.0));
        if btn(ui, br, k.name(), false, kc(k), k.about()).clicked() && !cx.modal {
            add_item(cx, gui, k, None);
        }
        ui.painter().circle_filled(br.left_center() + Vec2::new(12.0, 0.0), 4.0, kc(k));
    }
}

// ------------------------------------------------------------------------------------------
// The selected item: name, lanes (with their offset), bands, item presets
// ------------------------------------------------------------------------------------------

fn item_header(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    let painter = ui.painter().clone();
    let col = cx.col;
    let y = 122.0;
    let h = 32.0;
    painter.rect_filled(cx.r(14.0, y + 4.0, 4.0, h - 8.0), CornerRadius::same(2), col);
    painter.text(cx.at(26.0, y + h * 0.5), Align2::LEFT_CENTER, item_name(&cx.st, cx.sel, false), FontId::proportional(16.0), TEXT);

    // lanes: click to edit, drag up / down to move the whole shape (offset)
    let lanes = cx.kind.lanes();
    let mut x = 150.0;
    for (l, ln) in lanes.iter().enumerate() {
        let r = cx.r(x, y, 168.0, h);
        let tile = cx.p.slots[cx.sel].tile(l);
        let off = tile.unmodulated_plain_value();
        let resp = ui.interact(r, egui::Id::new(("lane", cx.sel, l)), Sense::click_and_drag()).on_hover_text(format!(
            "{}: click to edit its shape.\nDrag up / down: offset (moves the whole shape) · double-click: no offset.",
            ln.name
        ));
        let is = l == cx.lane;
        if !cx.modal {
            if resp.clicked() && !is {
                commit(cx.p, cx.shared, |s| s.lane = l as u8);
                gui.sel_pts.clear();
            }
            if resp.drag_started() {
                cx.setter.begin_set_parameter(tile);
                gui.lane_drag = Some((l, off));
            }
            if resp.dragged() {
                if let Some((dl, v)) = gui.lane_drag {
                    if dl == l {
                        let fine = ui.input(|i| i.modifiers.shift);
                        let nv = (v - resp.drag_delta().y / if fine { 600.0 } else { 120.0 }).clamp(-1.0, 1.0);
                        gui.lane_drag = Some((l, nv));
                        cx.setter.set_parameter(tile, nv);
                    }
                }
            }
            if resp.drag_stopped() {
                cx.setter.end_set_parameter(tile);
                gui.lane_drag = None;
            }
            if resp.double_clicked() {
                cx.setter.begin_set_parameter(tile);
                cx.setter.set_parameter(tile, 0.0);
                cx.setter.end_set_parameter(tile);
            }
        }
        painter.rect_filled(r, CornerRadius::same(6), if is { PANEL2 } else if resp.hovered() { Color32::from_rgb(0x1c, 0x20, 0x27) } else { PANEL });
        painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(if is { 1.5f32 } else { 1.0 }, if is { col } else { LINE }), StrokeKind::Inside);
        // mini shape
        let tr = Rect::from_min_max(r.min + Vec2::new(6.0, 5.0), Pos2::new(r.left() + 52.0, r.bottom() - 5.0));
        painter.rect_filled(tr, CornerRadius::same(3), EDIT_BG);
        thumb(ui, &cx.st.waves[cx.sel][l][cx.band], tr.shrink(2.0), if is { col } else { DIM }, 70);
        painter.text(Pos2::new(r.left() + 60.0, r.center().y), Align2::LEFT_CENTER, ln.name, pf(12.0), if is { TEXT } else { DIM });
        // offset bar
        let br = Rect::from_min_max(Pos2::new(r.right() - 10.0, r.top() + 5.0), Pos2::new(r.right() - 6.0, r.bottom() - 5.0));
        painter.rect_filled(br, CornerRadius::same(2), EDIT_BG);
        let mid = br.center().y;
        let yv = mid - off * br.height() * 0.5;
        painter.rect_filled(Rect::from_x_y_ranges(br.x_range(), mid.min(yv)..=mid.max(yv)), CornerRadius::same(1), col);
        if off.abs() > 0.004 {
            painter.text(Pos2::new(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, format!("{:+.0}%", off * 100.0), pf(10.0), col);
        }
        x += 174.0;
    }
    if lanes.is_empty() {
        painter.text(cx.at(150.0, y + h * 0.5), Align2::LEFT_CENTER, cx.kind.about(), pf(11.5), DIM);
    }

    // bands
    if cx.multi && !cx.kind.is_tool() {
        let bx = 510.0;
        painter.text(cx.at(bx, y + h * 0.5), Align2::LEFT_CENTER, "BAND", pf(9.5), DIM);
        for (b, n) in BAND_NAMES.iter().enumerate() {
            let r = cx.r(bx + 38.0 + b as f32 * 54.0, y + 3.0, 50.0, h - 6.0);
            if btn(ui, r, n, b == cx.band, col, "Edit this band's shape and LFO").clicked() && !cx.modal {
                commit(cx.p, cx.shared, |s| s.band = b as u8);
                gui.sel_pts.clear();
            }
        }
    }

    // on / item presets
    let on = cx.p.slots[cx.sel].on.value();
    if btn(ui, cx.r(BASE_W - 14.0 - 140.0 - 6.0 - 52.0, y + 3.0, 52.0, h - 6.0), if on { "ON" } else { "OFF" }, on, col, "Switch the item on / off").clicked() && !cx.modal {
        set_b(cx.setter, &cx.p.slots[cx.sel].on, !on);
    }
    let lib = gui.lib.as_ref().unwrap();
    let list = &lib.items[cx.kind.index()];
    let mut items: Vec<(String, Color32)> = vec![("──Factory".into(), Color32::TRANSPARENT)];
    let mut map: Vec<Option<usize>> = vec![None];
    for (i, it) in list.iter().enumerate().filter(|(_, it)| !it.user) {
        items.push((it.name.clone(), col));
        map.push(Some(i));
    }
    if list.iter().any(|it| it.user) {
        items.push(("──User".into(), Color32::TRANSPARENT));
        map.push(None);
        for (i, it) in list.iter().enumerate().filter(|(_, it)| it.user) {
            items.push((it.name.clone(), GOLD));
            map.push(Some(i));
        }
    }
    items.push(("──".into(), Color32::TRANSPARENT));
    map.push(None);
    let save_i = items.len();
    items.push((format!("Save {} preset…", cx.kind.name()), Color32::TRANSPARENT));
    let open_i = items.len();
    items.push(("Open the folder".into(), Color32::TRANSPARENT));
    if let Some(i) = menu_btn(ui, "itempresets", cx.r(BASE_W - 14.0 - 140.0, y + 3.0, 140.0, h - 6.0), &format!("{} presets ▾", cx.kind.name()), &items, col, "Presets of this item only (the rest of the chain stays).", 11.0) {
        if i == save_i {
            browser::open_save(cx, gui, SaveMode::Item);
        } else if i == open_i {
            if let Some(d) = presets::dir() {
                let d = d.join("Items").join(cx.kind.name());
                let _ = std::fs::create_dir_all(&d);
                open_folder(&d);
            }
        } else if let Some(Some(j)) = map.get(i) {
            let it = list[*j].clone();
            gui.checkpoint(cx, "", cx.now);
            if let Some(u) = gui.undo.last_mut() {
                u.full = true;
            }
            presets::apply_item(cx.setter, cx.p, cx.sel, &it);
            commit(cx.p, cx.shared, |_| {});
            gui.say(format!("{}: {}", cx.kind.name(), it.name), cx.now);
        }
    }
}

pub fn open_folder(d: &std::path::Path) {
    let cmd = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(cmd).arg(d).spawn();
}

