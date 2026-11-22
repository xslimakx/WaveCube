//! Over the editor: the preset browser, the save dialogs and the settings.

use nih_plug_egui::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2};
use nih_plug_egui::EguiState;

use super::widgets::*;
use super::{commit, load_preset, open_folder, set_b, Cx, Gui, Library, SaveMode, BASE_H, BASE_W};
use crate::kinds::{self, KnobDef, Scale::*, Unit};
use crate::presets::{self, PresetData, CATEGORIES};
use crate::Kind;

/// The presets the browser shows (search, filter, kinds used).
pub fn filtered(lib: &Library, gui: &Gui) -> Vec<(bool, usize)> {
    let q = gui.search.to_lowercase();
    lib.entries()
        .filter(|&(u, i)| {
            let d = lib.get(u, i);
            let cat_ok = match gui.filter {
                0 => true,
                1 => lib.favs.contains(&Library::key(u, &d.name)),
                2 => !u,
                3 => u,
                f => d.category == CATEGORIES[(f - 4).min(CATEGORIES.len() - 1)],
            };
            let kind_ok = gui.kind_filter.map(|k| d.kinds().contains(&k)).unwrap_or(true);
            cat_ok && kind_ok && (q.is_empty() || d.name.to_lowercase().contains(&q) || d.category.to_lowercase().contains(&q))
        })
        .collect()
}

pub fn browser(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    let col = cx.col;
    let br = cx.r(14.0, 50.0, BASE_W - 28.0, BASE_H - 64.0);
    let _ = ui.interact(br, egui::Id::new("browser_block"), Sense::click_and_drag());
    let painter = ui.painter().clone();
    painter.rect_filled(br, CornerRadius::same(10), Color32::from_rgb(0x14, 0x17, 0x1c));
    painter.rect_stroke(br, CornerRadius::same(10), Stroke::new(1.0f32, col), StrokeKind::Inside);
    painter.text(br.min + Vec2::new(18.0, 22.0), Align2::LEFT_CENTER, "Presets", FontId::proportional(17.0), TEXT);
    if btn(ui, Rect::from_min_size(br.right_top() + Vec2::new(-92.0, 10.0), Vec2::new(78.0, 24.0)), "Close", false, col, "").clicked() {
        gui.browser = false;
    }
    if btn(ui, Rect::from_min_size(br.right_top() + Vec2::new(-92.0 - 84.0, 10.0), Vec2::new(78.0, 24.0)), "Init", false, col, "Start from scratch: one Volume item, default shapes.").clicked() {
        let mut b = presets::Builder::new();
        b.add(Kind::Volume);
        let d = b.finish("Init", "");
        load_preset(cx, gui, &d);
        gui.browser = false;
    }
    if btn(ui, Rect::from_min_size(br.right_top() + Vec2::new(-92.0 - 168.0, 10.0), Vec2::new(78.0, 24.0)), "Rescan", false, col, "Read the user preset folder again.").clicked() {
        if let Some(l) = gui.lib.as_mut() {
            l.user = presets::list_user();
            l.reload_items();
        }
    }
    if btn(ui, Rect::from_min_size(br.right_top() + Vec2::new(-92.0 - 252.0, 10.0), Vec2::new(78.0, 24.0)), "Folder", false, col, "Open the user preset folder.").clicked() {
        if let Some(d) = presets::dir() {
            let _ = std::fs::create_dir_all(&d);
            open_folder(&d);
        }
    }
    let sr_ = Rect::from_min_size(br.min + Vec2::new(110.0, 10.0), Vec2::new(260.0, 24.0));
    ui.put(sr_, egui::TextEdit::singleline(&mut gui.search).hint_text("Search").font(pf(12.5)).margin(egui::Margin::symmetric(8, 4)));
    // filters
    let mut filters: Vec<String> = vec!["All".into(), "★ Favourites".into(), "Factory".into(), "User".into()];
    filters.extend(CATEGORIES.iter().map(|c| c.to_string()));
    for (i, f) in filters.iter().enumerate() {
        let r = Rect::from_min_size(br.min + Vec2::new(14.0, 50.0 + i as f32 * 28.0), Vec2::new(170.0, 24.0));
        if i == 4 {
            painter.hline(r.x_range(), r.top() - 2.0, Stroke::new(1.0f32, LINE));
        }
        if btn(ui, r, f, gui.filter == i, col, "").clicked() {
            gui.filter = i;
        }
    }
    // uses kind: filter by the items in the chain
    let ky = 50.0 + filters.len() as f32 * 28.0 + 12.0;
    painter.text(br.min + Vec2::new(16.0, ky), Align2::LEFT_CENTER, "USES", pf(9.5), DIM);
    for (i, &k) in Kind::ALL.iter().enumerate() {
        let r = Rect::from_min_size(br.min + Vec2::new(14.0 + (i % 2) as f32 * 86.0, ky + 12.0 + (i / 2) as f32 * 24.0), Vec2::new(82.0, 20.0));
        let on = gui.kind_filter == Some(k);
        if btn_f(ui, r, k.name(), on, kc(k), "Only presets that use this item. Click again: all.", 10.5).clicked() {
            gui.kind_filter = if on { None } else { Some(k) };
        }
    }
    if let Some(d) = presets::dir() {
        painter.text(br.left_bottom() + Vec2::new(16.0, -14.0), Align2::LEFT_CENTER, format!("User presets: {}", d.display()), pf(9.5), FAINT);
    }
    // list
    let lr = Rect::from_min_max(br.min + Vec2::new(198.0, 48.0), br.max - Vec2::new(14.0, 30.0));
    painter.rect_filled(lr, CornerRadius::same(6), EDIT_BG);
    for (h, xo) in [("NAME", 34.0), ("CATEGORY", 330.0), ("CHAIN", 450.0), ("BY", 790.0)] {
        painter.text(lr.min + Vec2::new(xo, 12.0), Align2::LEFT_CENTER, h, pf(9.5), DIM);
    }
    let list_rect = Rect::from_min_max(lr.min + Vec2::new(0.0, 24.0), lr.max);
    let mut to_load: Option<PresetData> = None;
    let mut fav_toggle: Option<String> = None;
    let mut delete: Option<String> = None;
    let rows = {
        let lib = gui.lib.as_ref().unwrap();
        filtered(lib, gui)
    };
    let lib = gui.lib.as_ref().unwrap();
    ui.allocate_new_ui(UiBuilder::new().max_rect(list_rect), |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for &(is_user, i) in rows.iter() {
                let d = lib.get(is_user, i);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(list_rect.width() - 10.0, 26.0), Sense::click());
                let cur = d.name == cx.st.preset;
                let pnt = ui.painter();
                if cur {
                    pnt.rect_filled(r, CornerRadius::same(4), col.gamma_multiply(0.18));
                } else if resp.hovered() {
                    pnt.rect_filled(r, CornerRadius::same(4), PANEL2);
                }
                let key = Library::key(is_user, &d.name);
                let fav = lib.favs.contains(&key);
                let star = Rect::from_center_size(r.left_center() + Vec2::new(16.0, 0.0), Vec2::splat(20.0));
                pnt.text(star.center(), Align2::CENTER_CENTER, if fav { "★" } else { "☆" }, pf(13.0), if fav { GOLD } else { FAINT });
                pnt.text(r.left_center() + Vec2::new(34.0, 0.0), Align2::LEFT_CENTER, &d.name, pf(12.5), if cur { col } else { TEXT });
                pnt.text(r.left_center() + Vec2::new(330.0, 0.0), Align2::LEFT_CENTER, &d.category, pf(11.0), DIM);
                for (j, k) in d.kinds().into_iter().enumerate().take(12) {
                    let sq = Rect::from_min_size(r.left_center() + Vec2::new(450.0 + j as f32 * 26.0, -7.0), Vec2::new(23.0, 14.0));
                    pnt.rect_filled(sq, CornerRadius::same(3), kc(k));
                    pnt.text(sq.center(), Align2::CENTER_CENTER, &k.name()[..2], pf(8.0), BG);
                }
                pnt.text(r.left_center() + Vec2::new(790.0, 0.0), Align2::LEFT_CENTER, &d.author, pf(11.0), FAINT);
                let trash = Rect::from_center_size(r.right_center() - Vec2::new(18.0, 0.0), Vec2::splat(20.0));
                if is_user {
                    pnt.text(trash.center(), Align2::CENTER_CENTER, "✕", pf(12.0), FAINT);
                }
                let resp = resp.on_hover_text(d.kinds().iter().map(|k| k.name()).collect::<Vec<_>>().join(" → "));
                if resp.clicked() {
                    let pos = resp.interact_pointer_pos().unwrap_or_default();
                    if star.contains(pos) {
                        fav_toggle = Some(key);
                    } else if is_user && trash.contains(pos) {
                        delete = Some(d.name.clone());
                    } else {
                        to_load = Some(d.clone());
                    }
                }
            }
            if rows.is_empty() {
                ui.add_space(20.0);
                ui.label(egui::RichText::new("   Nothing here yet.").font(pf(12.0)).color(DIM));
            }
        });
    });
    painter.text(br.right_bottom() + Vec2::new(-16.0, -14.0), Align2::RIGHT_CENTER, format!("{} presets", rows.len()), pf(9.5), FAINT);
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
        gui.say(format!("Deleted {n}"), cx.now);
    }
    if let Some(d) = to_load {
        load_preset(cx, gui, &d);
    }
}

pub fn open_save(cx: &Cx, gui: &mut Gui, mode: SaveMode) {
    if gui.save == Some(mode) {
        gui.save = None;
        return;
    }
    gui.save = Some(mode);
    gui.save_focus = true;
    gui.browser = false;
    gui.settings = false;
    gui.save_name = match mode {
        SaveMode::Preset => {
            if cx.st.preset == "Init" {
                String::new()
            } else {
                cx.st.preset.clone()
            }
        }
        SaveMode::Item => format!("My {}", cx.kind.name().to_lowercase()),
    };
}

pub fn save_dialog(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    let col = cx.col;
    let mode = gui.save.unwrap_or(SaveMode::Preset);
    let full = cx.r(0.0, 0.0, BASE_W, BASE_H);
    let dr = Rect::from_center_size(cx.at(BASE_W * 0.5, 260.0), Vec2::new(440.0, 196.0));
    let _ = ui.interact(full, egui::Id::new("save_block"), Sense::click_and_drag());
    let painter = ui.painter().clone();
    painter.rect_filled(full, CornerRadius::ZERO, Color32::from_black_alpha(120));
    painter.rect_filled(dr, CornerRadius::same(10), PANEL2);
    painter.rect_stroke(dr, CornerRadius::same(10), Stroke::new(1.0f32, col), StrokeKind::Inside);
    let title = match mode {
        SaveMode::Preset => "Save preset (everything)".to_string(),
        SaveMode::Item => format!("Save {} preset (this item only)", cx.kind.name()),
    };
    painter.text(dr.min + Vec2::new(18.0, 22.0), Align2::LEFT_CENTER, title, FontId::proportional(15.0), TEXT);
    painter.text(dr.min + Vec2::new(18.0, 58.0), Align2::LEFT_CENTER, "Name", pf(11.0), DIM);
    let ner = Rect::from_min_size(dr.min + Vec2::new(100.0, 46.0), Vec2::new(320.0, 26.0));
    let save_name = &mut gui.save_name;
    let mut out = ui
        .allocate_new_ui(UiBuilder::new().max_rect(ner), |ui| egui::TextEdit::singleline(save_name).hint_text("Name").font(pf(13.0)).margin(egui::Margin::symmetric(8, 4)).desired_width(ner.width()).show(ui))
        .inner;
    if gui.save_focus {
        out.response.request_focus();
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(gui.save_name.chars().count()));
        out.state.cursor.set_char_range(Some(all));
        out.state.store(ui.ctx(), out.response.id);
        gui.save_focus = false;
    }
    let name = gui.save_name.trim().to_string();
    let exists = match mode {
        SaveMode::Preset => {
            painter.text(dr.min + Vec2::new(18.0, 96.0), Align2::LEFT_CENTER, "Category", pf(11.0), DIM);
            if let Some(i) = combo_s(ui, "savecat", Rect::from_min_size(dr.min + Vec2::new(100.0, 84.0), Vec2::new(200.0, 24.0)), &CATEGORIES, gui.save_cat, "", "") {
                gui.save_cat = i;
            }
            gui.lib.as_ref().map(|l| l.user.iter().any(|d| d.name.trim() == name)).unwrap_or(false)
        }
        SaveMode::Item => {
            painter.text(dr.min + Vec2::new(18.0, 96.0), Align2::LEFT_CENTER, "Shows up in the item's preset menu, for every item of this kind.", pf(10.5), DIM);
            gui.lib.as_ref().map(|l| l.items[cx.kind.index()].iter().any(|d| d.user && d.name.trim() == name)).unwrap_or(false)
        }
    };
    if exists {
        painter.text(dr.min + Vec2::new(100.0, 124.0), Align2::LEFT_CENTER, "A user preset with this name exists: it will be replaced.", pf(10.0), GOLD);
    }
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let save = btn(ui, Rect::from_min_size(dr.right_bottom() - Vec2::new(18.0 + 90.0, 40.0), Vec2::new(90.0, 26.0)), "Save", !name.is_empty(), col, "").clicked() || enter;
    if btn(ui, Rect::from_min_size(dr.right_bottom() - Vec2::new(18.0 + 90.0 + 96.0, 40.0), Vec2::new(90.0, 26.0)), "Cancel", false, col, "").clicked() || esc {
        gui.save = None;
    } else if save && !name.is_empty() {
        let res = match mode {
            SaveMode::Preset => {
                let d = presets::capture(cx.p, &name, CATEGORIES[gui.save_cat.min(CATEGORIES.len() - 1)]);
                presets::save_user(&d).map(|_| {
                    cx.p.state.write().preset = name.clone();
                    if let Some(l) = gui.lib.as_mut() {
                        l.user = presets::list_user();
                    }
                })
            }
            SaveMode::Item => {
                let it = presets::capture_item(cx.p, cx.sel, &name);
                presets::save_user_item(&it).map(|_| {
                    if let Some(l) = gui.lib.as_mut() {
                        l.reload_items();
                    }
                })
            }
        };
        match res {
            Ok(_) => gui.say(format!("Saved \"{name}\""), cx.now),
            Err(e) => gui.say(format!("Could not save: {e}"), cx.now),
        }
        gui.save = None;
    }
}

const K_DETAIL: KnobDef = KnobDef::new("Sensitivity", "How small a hit still triggers.", 0.0, 1.0, 0.5, Lin, Unit::Pct, false);
const K_TSHIFT: KnobDef = KnobDef::new("Shift", "Delays the trigger (to line up with the hit).", 0.0, 4.0, 0.0, Lin, Unit::Ms, false);
const K_THP: KnobDef = KnobDef::new("HP", "High-pass on the trigger detector.", 20.0, 20000.0, 20.0, Log, Unit::Hz, false);
const K_TLP: KnobDef = KnobDef::new("LP", "Low-pass on the trigger detector.", 20.0, 20000.0, 20000.0, Log, Unit::Hz, false);

pub fn settings(ui: &mut Ui, cx: &Cx, gui: &mut Gui, size: &EguiState) {
    let col = cx.col;
    let full = cx.r(0.0, 0.0, BASE_W, BASE_H);
    let dr = Rect::from_center_size(cx.at(BASE_W * 0.5, 400.0), Vec2::new(720.0, 548.0));
    let blk = ui.interact(full, egui::Id::new("set_block"), Sense::click());
    let painter = ui.painter().clone();
    painter.rect_filled(full, CornerRadius::ZERO, Color32::from_black_alpha(120));
    painter.rect_filled(dr, CornerRadius::same(10), PANEL2);
    painter.rect_stroke(dr, CornerRadius::same(10), Stroke::new(1.0f32, col), StrokeKind::Inside);
    painter.text(dr.min + Vec2::new(18.0, 24.0), Align2::LEFT_CENTER, "Settings", FontId::proportional(16.0), TEXT);
    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if btn(ui, Rect::from_min_size(dr.right_top() + Vec2::new(-90.0, 12.0), Vec2::new(76.0, 24.0)), "Close", false, col, "").clicked() || esc || (blk.clicked() && !blk.interact_pointer_pos().map(|p| dr.contains(p)).unwrap_or(true)) {
        gui.settings = false;
    }
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(dr.min + Vec2::new(x, y), Vec2::new(w, h));
    let head = |y: f32, t: &str| {
        painter.text(dr.min + Vec2::new(18.0, y), Align2::LEFT_CENTER, t, pf(10.0), DIM);
        painter.hline((dr.left() + 18.0)..=(dr.right() - 18.0), dr.top() + y + 10.0, Stroke::new(1.0f32, LINE));
    };
    let trig = cx.st.trig;

    // ---- audio trigger
    head(56.0, "AUDIO TRIGGER  (LFOs set to Audio restart on every hit)");
    if btn(ui, at(18.0, 78.0, 150.0, 24.0), if trig.sidechain { "Listens to: sidechain" } else { "Listens to: input" }, trig.sidechain, col, "Trigger from the plugin's input, or from the sidechain input (route a kick into it).").clicked() {
        gui.checkpoint(cx, "", cx.now);
        commit(cx.p, cx.shared, |st| st.trig.sidechain = !st.trig.sidechain);
    }
    if let Some(i) = segmented(ui, at(176.0, 78.0, 210.0, 24.0), &kinds::TRIG_ALGOS, trig.algo as usize, col, "Drums: fast and tight · General: in between · Complex: slow, for dense material.") {
        gui.checkpoint(cx, "", cx.now);
        commit(cx.p, cx.shared, |st| st.trig.algo = i as u8);
    }
    let ky = dr.top() + 146.0;
    pknob(ui, cx.setter, &cx.p.trig_db, "Threshold", Pos2::new(dr.left() + 50.0, ky), 16.0, "Level a hit must reach.", false, col);
    let row: [(&str, &KnobDef, f32); 4] = [("tdet", &K_DETAIL, trig.detail), ("tsh", &K_TSHIFT, trig.shift), ("thp", &K_THP, trig.hp), ("tlp", &K_TLP, trig.lp)];
    for (i, (id, d, v)) in row.iter().enumerate() {
        if let Some(nv) = sknob(ui, id, d, *v, Pos2::new(dr.left() + 130.0 + i as f32 * 80.0, ky), 16.0, col, true) {
            gui.checkpoint(cx, id, cx.now);
            commit(cx.p, cx.shared, |st| match i {
                0 => st.trig.detail = nv,
                1 => st.trig.shift = nv,
                2 => st.trig.hp = nv,
                _ => st.trig.lp = nv,
            });
        }
    }

    // ---- MIDI
    head(214.0, "MIDI");
    let chans: Vec<String> = std::iter::once("All channels".to_string()).chain((1..=16).map(|c| format!("Channel {c}"))).collect();
    if let Some(i) = combo(ui, "midich", at(18.0, 236.0, 150.0, 24.0), &chans, cx.st.midi_channel as usize, &chans[cx.st.midi_channel as usize], "The MIDI channel the triggers, the pitch mode and MIDI Switch listen to.") {
        commit(cx.p, cx.shared, |st| st.midi_channel = i as u8);
    }
    let ms = cx.p.midi_switch.value();
    if btn(ui, at(176.0, 236.0, 110.0, 24.0), "MIDI Switch", ms, col, "Notes C# .. A load custom waves 1 .. 9; C triggers.").clicked() {
        set_b(cx.setter, &cx.p.midi_switch, !ms);
    }
    painter.text(dr.min + Vec2::new(296.0, 248.0), Align2::LEFT_CENTER, "Route MIDI to the plugin in your host to trigger or pitch the LFOs.", pf(10.0), FAINT);

    // ---- output
    head(282.0, "OUTPUT");
    let sc = cx.p.safe_clip.value();
    if btn(ui, at(18.0, 304.0, 150.0, 24.0), "Safety clipper", sc, col, "A soft clipper at the very end: no overs, whatever the chain does.").clicked() {
        set_b(cx.setter, &cx.p.safe_clip, !sc);
    }

    // ---- window
    head(350.0, "WINDOW SIZE");
    for (i, f) in [0.75f32, 1.0, 1.25, 1.5, 2.0].iter().enumerate() {
        if btn(ui, at(18.0 + i as f32 * 70.0, 372.0, 64.0, 24.0), &format!("{:.0} %", f * 100.0), false, col, "").clicked() {
            size.set_requested_size(((BASE_W * f) as u32, (BASE_H * f) as u32));
        }
    }
    painter.text(dr.min + Vec2::new(380.0, 384.0), Align2::LEFT_CENTER, "or drag the corner at the bottom right", pf(10.0), FAINT);

    // ---- folders
    head(418.0, "YOUR FILES");
    let folders: [(&str, Option<std::path::PathBuf>); 3] = [("Presets", presets::dir()), ("Noises (WAV)", crate::dsp::media::noise_dir()), ("Impulse responses (WAV)", crate::dsp::media::ir_dir())];
    for (i, (n, d)) in folders.iter().enumerate() {
        let y = 440.0 + i as f32 * 28.0;
        if btn(ui, at(18.0, y, 196.0, 24.0), &format!("Open {n}"), false, col, "").clicked() {
            if let Some(d) = d {
                let _ = std::fs::create_dir_all(d);
                open_folder(d);
            }
        }
        if let Some(d) = d {
            painter.text(dr.min + Vec2::new(224.0, y + 12.0), Align2::LEFT_CENTER, d.display().to_string(), pf(9.5), FAINT);
        }
    }
    if btn(ui, at(560.0, 440.0, 140.0, 24.0), "Rescan files", false, col, "Look for new noises, IRs and presets.").clicked() {
        gui.rescan_media();
        if let Some(l) = gui.lib.as_mut() {
            l.user = presets::list_user();
            l.reload_items();
        }
        crate::dsp::media::load_user_media(&cx.p.state.read(), cx.shared);
        gui.say(format!("{} noises, {} IRs", gui.noise_files.len(), gui.ir_files.len()), cx.now);
    }
    painter.text(dr.min + Vec2::new(560.0, 476.0), Align2::LEFT_CENTER, format!("{} noises · {} IRs found", gui.noise_files.len(), gui.ir_files.len()), pf(9.5), DIM);
    let _ = Pos2::ZERO;
}
