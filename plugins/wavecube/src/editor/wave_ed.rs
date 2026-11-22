//! The shape tools, the shape editor (with the scope behind it) and the custom waves.

use nih_plug::prelude::Param;
use nih_plug_egui::egui::{self, epaint::PathStroke, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use std::sync::atomic::Ordering;

use super::widgets::*;
use super::{commit, edit_wave, set_b, Cx, Drag, Gui, Tool, BASE_W, BAND_NAMES};
use crate::kinds::LENGTHS;
use crate::wave::{bend, Pen, Wave};
use crate::{Custom, GRIDS, N_BANDS, N_CUSTOM, SCOPE_N};

const Y_TOOLS: f32 = 160.0;
const Y_ED: f32 = 190.0;
const H_ED: f32 = 262.0;
const Y_CUSTOM: f32 = 458.0;

pub fn toolbar(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    let col = cx.col;
    let y = Y_TOOLS;
    let h = 24.0;
    let cur = cx.st.waves[cx.sel][cx.lane][cx.band].clone();
    let (s, l, b, m) = (cx.sel, cx.lane, cx.band, cx.multi);
    let mut x = 14.0;

    // shapes of this kind, by category, plus the custom waves
    {
        let cats = cx.kind.wave_categories();
        let mut items: Vec<(String, Color32)> = Vec::new();
        let mut map: Vec<Option<Wave>> = Vec::new();
        for (c, list) in &cats {
            items.push((format!("──{c}"), Color32::TRANSPARENT));
            map.push(None);
            for w in list {
                items.push((w.name().to_string(), col));
                map.push(Some(Wave::preset(*w)));
            }
        }
        let customs: Vec<(usize, &Custom)> = cx.st.custom.iter().enumerate().filter_map(|(i, c)| c.as_ref().map(|c| (i, c))).collect();
        if !customs.is_empty() {
            items.push(("──Custom waves".into(), Color32::TRANSPARENT));
            map.push(None);
            for (i, c) in customs {
                items.push((format!("Custom {} ({})", i + 1, c.kind.name()), kc(c.kind)));
                map.push(Some(c.wave.clone()));
            }
        }
        if let Some(i) = menu_btn(ui, "shapes", cx.r(x, y, 96.0, h), "Shapes ▾", &items, col, "Load a ready-made shape into this lane / band.", 11.5) {
            if let Some(Some(w)) = map.get(i).cloned() {
                gui.checkpoint(cx, "", cx.now);
                edit_wave(cx.p, cx.shared, s, l, b, m, |dst| *dst = w);
                gui.sel_pts.clear();
            }
        }
        x += 102.0;
    }
    // tool
    {
        let mut names: Vec<String> = vec!["Pointer".into(), "Select".into()];
        names.extend(Pen::ALL.iter().map(|q| format!("Pen: {}", q.name())));
        let cur_i = match gui.tool {
            Tool::Pointer => 0,
            Tool::Select => 1,
            Tool::Pen(pn) => 2 + Pen::ALL.iter().position(|&z| z == pn).unwrap_or(0),
        };
        let tip = "Pointer: move points and bend segments.\nSelect: drag a box around points, then move them together or delete them (Del).\nPens: click or drag to draw into the grid cells; the height is where you click.";
        if let Some(i) = combo(ui, "tool", cx.r(x, y, 128.0, h), &names, cur_i, &format!("✎ {}", names[cur_i]), tip) {
            gui.tool = match i {
                0 => Tool::Pointer,
                1 => Tool::Select,
                n => Tool::Pen(Pen::ALL[n - 2]),
            };
            gui.sel_pts.clear();
        }
        x += 136.0;
    }
    let grid = cx.st.grid as f32;
    let tool = |ui: &mut Ui, x: &mut f32, w: f32, label: &str, tip: &str| -> (bool, bool) {
        let r = btn(ui, cx.r(*x, y, w, h), label, false, col, tip);
        *x += w + 4.0;
        (r.clicked() && !cx.modal, ui.input(|i| i.modifiers.shift))
    };
    let mut ed: Option<Box<dyn FnOnce(&mut Wave)>> = None;
    if tool(ui, &mut x, 52.0, "Invert", "Flip upside down.").0 {
        ed = Some(Box::new(|w| w.invert()));
    }
    if tool(ui, &mut x, 58.0, "Reverse", "Flip left to right.").0 {
        ed = Some(Box::new(|w| w.reverse()));
    }
    if tool(ui, &mut x, 28.0, "×2", "Twice in one cycle.").0 {
        ed = Some(Box::new(|w| w.repeat(2)));
    }
    if tool(ui, &mut x, 28.0, "×3", "Three times in one cycle.").0 {
        ed = Some(Box::new(|w| w.repeat(3)));
    }
    let (c, shift) = tool(ui, &mut x, 60.0, "Random", "A random shape on the grid.\nShift: keep the points where they are, new heights and bends only.");
    if c {
        gui.seed = gui.seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let seed = gui.seed;
        ed = Some(Box::new(move |w| w.randomize(grid as usize, seed, shift)));
    }
    let (c, shift) = tool(ui, &mut x, 24.0, "◀", "Move one grid step earlier (Shift: 1/64).");
    if c {
        let d = if shift { 1.0 / 64.0 } else { 1.0 / grid };
        ed = Some(Box::new(move |w| w.shift(-d)));
    }
    let (c, shift) = tool(ui, &mut x, 24.0, "▶", "Move one grid step later (Shift: 1/64).");
    if c {
        let d = if shift { 1.0 / 64.0 } else { 1.0 / grid };
        ed = Some(Box::new(move |w| w.shift(d)));
    }
    if tool(ui, &mut x, 44.0, "Clear", "A flat line at the top.").0 {
        ed = Some(Box::new(|w| *w = Wave::flat(1.0)));
    }
    if tool(ui, &mut x, 44.0, "Copy", "Copy this shape (paste it into any lane, band or item).").0 {
        gui.clip = Some(cur.clone());
        gui.say("Shape copied", cx.now);
    }
    if tool(ui, &mut x, 46.0, "Paste", "Paste the copied shape here.").0 {
        if let Some(c) = gui.clip.clone() {
            ed = Some(Box::new(move |w| *w = c));
        }
    }
    if !gui.sel_pts.is_empty() && tool(ui, &mut x, 70.0, &format!("Delete {}", gui.sel_pts.len()), "Delete the selected points (Del).").0 {
        let mut idx = gui.sel_pts.clone();
        idx.sort_unstable();
        ed = Some(Box::new(move |w| {
            for &i in idx.iter().rev() {
                if i > 0 && i + 1 < w.pts.len() {
                    w.remove(i);
                }
            }
        }));
    }
    if cx.multi && tool(ui, &mut x, 78.0, "To all bands", "Copy this band's shape to the other two bands.").0 {
        gui.checkpoint(cx, "", cx.now);
        let w = cur.clone();
        commit(cx.p, cx.shared, |st| {
            for bb in 0..N_BANDS {
                st.waves[s][l][bb] = w.clone();
            }
        });
    }
    if let Some(f) = ed {
        gui.checkpoint(cx, "", cx.now);
        edit_wave(cx.p, cx.shared, s, l, b, m, f);
        gui.sel_pts.clear();
    }

    // right side: scope, snap, grid
    let gnames: Vec<String> = ["1/4", "1/8", "1/12 (triplets)", "1/16", "1/24 (triplets)", "1/32", "1/64"].iter().map(|s| s.to_string()).collect();
    let gi = GRIDS.iter().position(|&g| g == cx.st.grid).unwrap_or(3);
    let rx = BASE_W - 14.0;
    if let Some(i) = combo(ui, "grid", cx.r(rx - 92.0, y, 92.0, h), &gnames, gi, &format!("Grid {}", gnames[gi].split(' ').next().unwrap()), "Editor grid (steps in one cycle). Triplet grids for swung and triplet patterns.") {
        let g = GRIDS[i];
        commit(cx.p, cx.shared, |st| st.grid = g);
    }
    if btn(ui, cx.r(rx - 92.0 - 4.0 - 48.0, y, 48.0, h), "Snap", gui.snap, col, "Snap to the grid (hold Shift to toggle while dragging).").clicked() {
        gui.snap = !gui.snap;
    }
    if btn(ui, cx.r(rx - 92.0 - 4.0 - 48.0 - 4.0 - 54.0, y, 54.0, h), "Freeze", gui.freeze, GOLD, "Freeze the scope behind the shape.").clicked() {
        gui.freeze = !gui.freeze;
    }
    let smodes = ["Scope: in + out", "Scope: output", "Scope: off"];
    if let Some(i) = combo_s(ui, "scopemode", cx.r(rx - 92.0 - 4.0 - 48.0 - 4.0 - 54.0 - 4.0 - 118.0, y, 118.0, h), &smodes, (cx.st.scope_mode as usize).min(2), "", "What the scope behind the shape shows: the item's input (dark) and output (light), the output only, or nothing.") {
        commit(cx.p, cx.shared, |st| st.scope_mode = i as u8);
    }
}

/// Moves the selected points of `orig` together (each stays between its unselected neighbours).
fn move_selection(orig: &Wave, sel: &[usize], dx: f32, dy: f32) -> Wave {
    let mut w = orig.clone();
    let n = w.pts.len();
    for &i in sel {
        if i >= n {
            continue;
        }
        let lo = (0..i).rev().find(|j| !sel.contains(j)).map(|j| orig.pts[j].x).unwrap_or(0.0);
        let hi = (i + 1..n).find(|j| !sel.contains(j)).map(|j| orig.pts[j].x).unwrap_or(1.0);
        if i != 0 && i != n - 1 {
            w.pts[i].x = (orig.pts[i].x + dx).clamp(lo, hi);
        }
        w.pts[i].y = (orig.pts[i].y + dy).clamp(0.0, 1.0);
    }
    w
}

pub fn editor(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    let painter = ui.painter().clone();
    let col = cx.col;
    let (s, l, b, m) = (cx.sel, cx.lane, cx.band, cx.multi);
    let set = cx.st.slots[s];
    let lfo = set.lfo_of(l, b);
    let er = cx.r(14.0, Y_ED, BASE_W - 28.0, H_ED);
    painter.rect_filled(er, CornerRadius::same(8), EDIT_BG);
    let inner = er.shrink2(Vec2::new(10.0, 14.0));
    let px = |x: f32, y: f32| Pos2::new(inner.left() + x * inner.width(), inner.bottom() - y * inner.height());

    // scope: input dark, output light
    let mode = cx.st.scope_mode;
    if mode < 2 {
        let bw = inner.width() / SCOPE_N as f32;
        let sc = |a: f32| (a.min(1.0).sqrt() * inner.height()).max(0.0);
        for i in 0..SCOPE_N {
            let x0 = inner.left() + i as f32 * bw;
            let hi = if mode == 0 { sc(gui.scope_in[i]) } else { 0.0 };
            let ho = sc(gui.scope_out[i]);
            if hi > 0.5 {
                painter.rect_filled(Rect::from_min_max(Pos2::new(x0, inner.bottom() - hi), Pos2::new(x0 + bw - 1.0, inner.bottom())), CornerRadius::ZERO, Color32::from_rgb(0x1b, 0x1f, 0x26));
            }
            if ho > 0.5 {
                painter.rect_filled(Rect::from_min_max(Pos2::new(x0, inner.bottom() - ho), Pos2::new(x0 + bw - 1.0, inner.bottom())), CornerRadius::ZERO, Color32::from_rgb(0x2c, 0x32, 0x3c));
            }
        }
    }
    // grid
    let g = cx.st.grid as usize;
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
    let ln = cx.kind.lanes()[l];
    let title = if m { format!("{} · {} band", ln.name, BAND_NAMES[b].to_lowercase()) } else { ln.name.to_string() };
    let len_txt = match lfo.mode {
        1 => format!("{:.2} Hz", lfo.hz),
        2 => format!("pitch {:+.0} st", lfo.transpose),
        _ => format!("{}{}", LENGTHS[lfo.length as usize], ["", " dotted", " triplet"][lfo.feel as usize]),
    };
    let trig = ["", " · MIDI trigger", " · audio trigger"][lfo.trigger as usize];

    // interaction
    let resp = ui.interact(er, egui::Id::new("wave_editor"), Sense::click_and_drag());
    let hover = if cx.modal { None } else { resp.hover_pos() };
    let shift = ui.input(|i| i.modifiers.shift);
    let snap_on = gui.snap != shift;
    let grid = cx.st.grid as f32;
    let to_xy = |pos: Pos2| -> (f32, f32) { (((pos.x - inner.left()) / inner.width()).clamp(0.0, 1.0), ((inner.bottom() - pos.y) / inner.height()).clamp(0.0, 1.0)) };
    let snap_x = |x: f32, snap: bool| if snap { (x * grid).round() / grid } else { x };
    let snap_y = |y: f32, snap: bool| if snap { (y * 16.0).round() / 16.0 } else { y };
    let cur = cx.st.waves[s][l][b].clone();
    let pts = cur.pts.clone();
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
        let (a, bb) = (pts[i], pts[i + 1]);
        if bb.x - a.x < 0.01 || (bb.y - a.y).abs() < 0.01 {
            return None;
        }
        Some(px(0.5 * (a.x + bb.x), a.y + (bb.y - a.y) * bend(0.5, a.c)))
    };
    let near_handle = |pos: Pos2| -> Option<usize> { (0..pts.len() - 1).find(|&i| handle_pos(i).map(|h| h.distance(pos) < 8.0).unwrap_or(false)) };
    // 1-shot end marker
    let end_x = inner.left() + lfo.end * inner.width();
    let end_handle = Rect::from_center_size(Pos2::new(end_x, er.top() + 7.0), Vec2::new(14.0, 14.0));
    let near_end = |pos: Pos2| !lfo.looping && end_handle.expand(3.0).contains(pos);
    let pointer = gui.tool != Tool::Select && !matches!(gui.tool, Tool::Pen(_));
    let selecting = gui.tool == Tool::Select;
    let hov_pt = if gui.tool == Tool::Pointer || selecting { hover.and_then(near_pt) } else { None };
    let hov_h = if !pointer || hov_pt.is_some() { None } else { hover.and_then(near_handle) };
    let paint = |gui: &mut Gui, pos: Pos2| {
        let (x, y) = to_xy(pos);
        let cell = (x * grid).floor().min(grid - 1.0);
        let y = snap_y(y, snap_on);
        let pen = if let Tool::Pen(p) = gui.tool { p } else { Pen::Step };
        edit_wave(cx.p, cx.shared, s, l, b, m, |w| w.paint(cell / grid, (cell + 1.0) / grid, y, pen));
    };

    if !cx.modal {
        if resp.drag_started() {
            // where the button went down (the drag is only noticed a few pixels later)
            let start = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or_default();
            gui.checkpoint(cx, "", cx.now);
            gui.drag = if near_end(start) {
                Some(Drag::End)
            } else if let Tool::Pen(_) = gui.tool {
                Some(Drag::Paint)
            } else if selecting {
                match near_pt(start) {
                    Some(i) => {
                        if !gui.sel_pts.contains(&i) {
                            if !shift {
                                gui.sel_pts.clear();
                            }
                            gui.sel_pts.push(i);
                        }
                        gui.move_orig = Some(cur.clone());
                        Some(Drag::MoveSel(start))
                    }
                    None => Some(Drag::Box(start)),
                }
            } else if let Some(i) = near_pt(start) {
                Some(Drag::Point(i))
            } else if let Some(i) = near_handle(start) {
                Some(Drag::Curve(i, pts[i].c, start.y))
            } else {
                let (x, y) = to_xy(start);
                let mut w = cur.clone();
                let idx = w.insert(snap_x(x, snap_on), snap_y(y, snap_on));
                if idx.is_some() {
                    edit_wave(cx.p, cx.shared, s, l, b, m, |dst| *dst = w);
                }
                idx.map(Drag::Point)
            };
        }
        if resp.dragged() {
            if let (Some(d), Some(pos)) = (gui.drag, resp.interact_pointer_pos()) {
                match d {
                    Drag::Point(i) => {
                        let (x, y) = to_xy(pos);
                        edit_wave(cx.p, cx.shared, s, l, b, m, |w| w.move_pt(i, snap_x(x, snap_on), snap_y(y, snap_on)));
                    }
                    Drag::Curve(i, c0, y0) => {
                        if i + 1 < pts.len() {
                            let dir = if pts[i + 1].y >= pts[i].y { 1.0 } else { -1.0 };
                            let c = (c0 + (pos.y - y0) / 110.0 * dir).clamp(-1.0, 1.0);
                            edit_wave(cx.p, cx.shared, s, l, b, m, |w| {
                                if i < w.pts.len() {
                                    w.pts[i].c = c
                                }
                            });
                        }
                    }
                    Drag::Paint => paint(gui, pos),
                    Drag::Box(_) => {}
                    Drag::MoveSel(start) => {
                        if let Some(orig) = gui.move_orig.clone() {
                            let mut dx = (pos.x - start.x) / inner.width();
                            let mut dy = (start.y - pos.y) / inner.height();
                            if snap_on {
                                dx = (dx * grid).round() / grid;
                                dy = (dy * 16.0).round() / 16.0;
                            }
                            let sel = gui.sel_pts.clone();
                            let w = move_selection(&orig, &sel, dx, dy);
                            commit(cx.p, cx.shared, |st| {
                                let bands: Vec<usize> = if m { vec![b] } else { (0..N_BANDS).collect() };
                                for bb in bands {
                                    st.waves[s][l][bb] = w.clone();
                                }
                            });
                        }
                    }
                    Drag::End => {
                        let (x, _) = to_xy(pos);
                        let e = snap_x(x, snap_on).max(1.0 / 64.0);
                        let bands = cx.bands(false);
                        commit(cx.p, cx.shared, |st| {
                            for bb in bands {
                                st.slots[s].lfo[l][bb].end = e;
                            }
                        });
                    }
                }
            }
        }
        if resp.drag_stopped() {
            if let (Some(Drag::Box(a)), Some(bpos)) = (gui.drag, resp.interact_pointer_pos()) {
                let r = Rect::from_two_pos(a, bpos);
                if !shift {
                    gui.sel_pts.clear();
                }
                for (i, q) in pts.iter().enumerate() {
                    if r.contains(px(q.x, q.y)) && !gui.sel_pts.contains(&i) {
                        gui.sel_pts.push(i);
                    }
                }
            }
            gui.drag = None;
            gui.move_orig = None;
        }
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                match gui.tool {
                    Tool::Pen(_) => {
                        gui.checkpoint(cx, "", cx.now);
                        paint(gui, pos);
                    }
                    Tool::Select => match near_pt(pos) {
                        Some(i) => {
                            if shift {
                                if let Some(k) = gui.sel_pts.iter().position(|&j| j == i) {
                                    gui.sel_pts.remove(k);
                                } else {
                                    gui.sel_pts.push(i);
                                }
                            } else {
                                gui.sel_pts = vec![i];
                            }
                        }
                        None => gui.sel_pts.clear(),
                    },
                    Tool::Pointer => {}
                }
            }
        }
        if resp.double_clicked() && pointer {
            if let Some(pos) = resp.interact_pointer_pos() {
                gui.checkpoint(cx, "", cx.now);
                if let Some(i) = near_pt(pos) {
                    edit_wave(cx.p, cx.shared, s, l, b, m, |w| w.remove(i));
                } else if let Some(i) = near_handle(pos) {
                    edit_wave(cx.p, cx.shared, s, l, b, m, |w| w.pts[i].c = 0.0);
                } else {
                    let (x, y) = to_xy(pos);
                    let (x, y) = (snap_x(x, snap_on), snap_y(y, snap_on));
                    edit_wave(cx.p, cx.shared, s, l, b, m, |w| {
                        w.insert(x, y);
                    });
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if let Some(i) = near_pt(pos) {
                    gui.checkpoint(cx, "", cx.now);
                    edit_wave(cx.p, cx.shared, s, l, b, m, |w| w.remove(i));
                    gui.sel_pts.clear();
                } else if let Some(i) = near_handle(pos) {
                    gui.checkpoint(cx, "", cx.now);
                    edit_wave(cx.p, cx.shared, s, l, b, m, |w| w.pts[i].c = 0.0);
                }
            }
        }
    }
    let _ = resp.on_hover_text(match gui.tool {
        Tool::Pen(_) => "Pen: click or drag to draw into the grid cells; the height is where you click.\nShift: toggle snapping.",
        Tool::Select => "Select: drag a box around points (Shift adds), drag selected points to move them together, Del deletes them, Esc clears.",
        Tool::Pointer => "Drag points · drag the small rings to bend a segment\nDrag on empty space or double-click: new point\nDouble-click / right-click a point: delete\nShift: toggle snapping · Ctrl+Z / Ctrl+Shift+Z: undo / redo",
    });

    // the shape (fresh from the state: the edits of this frame included)
    let st_now = cx.p.state.read();
    let w_now = st_now.waves[s][l][b].clone();
    let others: Vec<Wave> = if m { (0..N_BANDS).filter(|&x| x != b).map(|x| st_now.waves[s][l][x].clone()).collect() } else { vec![] };
    drop(st_now);
    let on = cx.p.slots[s].on.value();
    for o in &others {
        painter.add(Shape::line(wave_path(o, inner, 300.0), PathStroke::new(1.0f32, col.gamma_multiply(0.28))));
    }
    // the offset moves the whole shape: show where it really is
    let off = cx.p.slots[s].tile(l).unmodulated_plain_value();
    let path = wave_path(&w_now, inner, 700.0);
    let wc = if on { col } else { DIM };
    if off.abs() > 0.004 {
        let shifted: Vec<Pos2> = path.iter().map(|q| Pos2::new(q.x, (q.y - off * inner.height()).clamp(inner.top(), inner.bottom()))).collect();
        painter.add(Shape::dashed_line(&shifted, Stroke::new(1.2f32, wc.gamma_multiply(0.6)), 5.0, 4.0));
    }
    painter.add(fill_under(&path, inner.bottom(), wc, if on { 110 } else { 35 }, inner.height()));
    painter.add(Shape::line(path, PathStroke::new(2.2f32, wc)));
    if gui.tool != Tool::Pen(Pen::Step) && !matches!(gui.tool, Tool::Pen(_)) {
        if pointer {
            for i in 0..w_now.pts.len().saturating_sub(1) {
                let (a, bb) = (w_now.pts[i], w_now.pts[i + 1]);
                if bb.x - a.x < 0.01 || (bb.y - a.y).abs() < 0.01 {
                    continue;
                }
                let h = px(0.5 * (a.x + bb.x), a.y + (bb.y - a.y) * bend(0.5, a.c));
                let hot = hov_h == Some(i) || matches!(gui.drag, Some(Drag::Curve(j, ..)) if j == i);
                painter.circle_stroke(h, if hot { 5.0 } else { 3.5 }, Stroke::new(1.4f32, if hot { TEXT } else { wc.gamma_multiply(0.8) }));
            }
        }
        for (i, q) in w_now.pts.iter().enumerate() {
            let c = px(q.x, q.y);
            let picked = gui.sel_pts.contains(&i);
            let hot = hov_pt == Some(i) || matches!(gui.drag, Some(Drag::Point(j)) if j == i);
            let r = if hot { 6.5 } else { 5.0 };
            painter.circle_filled(c, r, if hot || picked { TEXT } else { EDIT_BG });
            painter.circle_stroke(c, r, Stroke::new(2.0f32, if picked { GOLD } else if hot { TEXT } else { wc }));
        }
    }
    // labels on top of the shape, on a dark backdrop
    {
        let tag = |pos: Pos2, align: Align2, text: String, c: Color32, size: f32| {
            let g = ui.fonts(|f| f.layout_no_wrap(text.clone(), pf(size), c));
            let r = align.anchor_size(pos, g.size()).expand2(Vec2::new(4.0, 1.0));
            painter.rect_filled(r, CornerRadius::same(3), Color32::from_rgba_unmultiplied(12, 14, 18, 210));
            painter.text(pos, align, text, pf(size), c);
        };
        tag(Pos2::new(er.left() + 24.0, inner.top() + 2.0), Align2::LEFT_CENTER, ln.top.to_string(), DIM, 9.0);
        tag(Pos2::new(er.left() + 24.0, inner.bottom() - 2.0), Align2::LEFT_CENTER, ln.bottom.to_string(), DIM, 9.0);
        tag(Pos2::new(er.right() - 10.0, inner.top() + 2.0), Align2::RIGHT_CENTER, title, col, 11.5);
        tag(Pos2::new(er.right() - 10.0, inner.top() + 17.0), Align2::RIGHT_CENTER, format!("{len_txt}{trig}{}", if lfo.looping { "" } else { " · 1-shot" }), DIM, 9.5);
    }
    if let (Some(Drag::Box(a)), Some(bpos)) = (gui.drag, ui.input(|i| i.pointer.interact_pos())) {
        let r = Rect::from_two_pos(a, bpos);
        painter.rect_filled(r, CornerRadius::same(2), GOLD.gamma_multiply(0.08));
        painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0f32, GOLD), StrokeKind::Inside);
    }
    // 1-shot end
    if !lfo.looping {
        painter.rect_filled(Rect::from_min_max(Pos2::new(end_x, inner.top()), inner.right_bottom()), CornerRadius::ZERO, Color32::from_black_alpha(90));
        painter.vline(end_x, er.y_range(), Stroke::new(1.5f32, GOLD));
        painter.rect_filled(end_handle, CornerRadius::same(3), GOLD);
        painter.text(end_handle.center(), Align2::CENTER_CENTER, "◆", pf(8.0), BG);
        painter.text(Pos2::new(end_x + 10.0, er.top() + 7.0), Align2::LEFT_CENTER, "END", pf(8.5), GOLD);
    }
    // help line
    if let Some(hp) = hover {
        let (x, y) = to_xy(hp);
        let (x, y) = (snap_x(x, snap_on), snap_y(y, snap_on));
        let steps = (x * grid).round() as i32;
        let txt = if (x * grid - steps as f32).abs() < 1e-3 { format!("{}/{}  ·  {:.0} %", steps, cx.st.grid, y * 100.0) } else { format!("{:.3}  ·  {:.0} %", x, y * 100.0) };
        painter.text(Pos2::new(er.right() - 10.0, inner.bottom() - 2.0), Align2::RIGHT_CENTER, txt, pf(9.5), DIM);
        if let Tool::Pen(_) = gui.tool {
            let cell = (x * grid).floor().min(grid - 1.0);
            let r = Rect::from_x_y_ranges(inner.left() + inner.width() * cell / grid..=inner.left() + inner.width() * (cell + 1.0) / grid, er.y_range());
            painter.rect_filled(r, CornerRadius::ZERO, col.gamma_multiply(0.08));
        }
    }
    // playhead
    if !cx.in_chain() {
    } else if on {
        let ph = (cx.shared.phase[s][l].load(Ordering::Relaxed) + lfo.phase / 360.0).rem_euclid(1.0);
        let yv = cx.shared.value[s][l][b].load(Ordering::Relaxed);
        let c = px(ph, yv);
        painter.vline(c.x, er.y_range(), Stroke::new(1.0f32, TEXT.gamma_multiply(0.35)));
        let spr = lfo.spread / 360.0;
        if spr.abs() > 0.001 {
            let pr = (ph + spr).rem_euclid(1.0);
            let cr = px(pr, w_now.eval(pr));
            painter.vline(cr.x, er.y_range(), Stroke::new(1.0f32, col.gamma_multiply(0.35)));
            painter.circle_stroke(cr, 5.0, Stroke::new(2.0f32, TEXT));
            painter.text(cr + Vec2::new(8.0, -10.0), Align2::LEFT_CENTER, "R", pf(9.0), DIM);
        }
        painter.circle_filled(c, 6.0, TEXT);
        painter.circle_filled(c, 3.5, col);
        // envelope follower on: its level on the left edge
        let env = set.env[l][if m { b } else { 0 }];
        if env.on && cx.kind.has_envelope() {
            let e = cx.shared.env[s][l].load(Ordering::Relaxed);
            let r = Rect::from_min_max(Pos2::new(er.left() + 2.0, inner.top()), Pos2::new(er.left() + 6.0, inner.bottom()));
            vmeter(ui, r, e, OK, "", false);
        }
    } else {
        painter.text(er.center(), Align2::CENTER_CENTER, "The item is off · switch it on with the dot on its tile", pf(13.0), DIM);
    }
    if gui.freeze {
        painter.text(Pos2::new(er.left() + 80.0, inner.top() + 2.0), Align2::LEFT_CENTER, "SCOPE FROZEN", pf(9.0), GOLD);
    }
    painter.rect_stroke(er, CornerRadius::same(8), Stroke::new(1.0f32, LINE), StrokeKind::Inside);
}

const NOTES: [&str; N_CUSTOM] = ["C#", "D", "D#", "E", "F", "F#", "G", "G#", "A"];

pub fn custom_row(ui: &mut Ui, cx: &Cx, gui: &mut Gui) {
    let painter = ui.painter().clone();
    let y = Y_CUSTOM;
    let h = 26.0;
    painter.text(cx.at(14.0, y + h * 0.5), Align2::LEFT_CENTER, "CUSTOM", pf(9.5), DIM);
    let w = 76.0;
    let cur = cx.st.waves[cx.sel][cx.lane][cx.band].clone();
    for i in 0..N_CUSTOM {
        let r = cx.r(68.0 + i as f32 * (w + 4.0), y, w, h);
        let c = cx.st.custom[i].clone();
        let tip = match &c {
            Some(c) => format!(
                "Custom wave {} (note {}) from {}: {}.\nClick: load it here · Shift-click: store the current shape here · right-click: more.\nWith MIDI Switch on, note {} loads it into {} while playing.",
                i + 1,
                NOTES[i],
                c.kind.name(),
                c.kind.lanes().get(c.lane as usize).map(|l| l.name).unwrap_or(""),
                NOTES[i],
                super::item_name(&cx.st, c.slot as usize, false)
            ),
            None => format!("Empty custom wave {} (note {}). Click: store the current shape here.", i + 1, NOTES[i]),
        };
        let resp = ui.allocate_rect(r, Sense::click()).on_hover_text(tip);
        let hov = resp.hovered();
        painter.rect_filled(r, CornerRadius::same(5), if hov { PANEL2 } else { PANEL });
        painter.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0f32, if c.as_ref().map(|c| c.wave == cur).unwrap_or(false) { cx.col } else { LINE }), StrokeKind::Inside);
        painter.text(r.left_center() + Vec2::new(6.0, 0.0), Align2::LEFT_CENTER, NOTES[i], pf(9.0), DIM);
        if let Some(c) = &c {
            let tr = Rect::from_min_max(r.min + Vec2::new(22.0, 4.0), r.max - Vec2::new(5.0, 4.0));
            thumb(ui, &c.wave, tr, kc(c.kind), 60);
        } else {
            painter.text(r.center() + Vec2::new(8.0, 0.0), Align2::CENTER_CENTER, "+", pf(12.0), FAINT);
        }
        if cx.modal {
            continue;
        }
        let shift = ui.input(|k| k.modifiers.shift);
        let store = |gui: &mut Gui| {
            gui.checkpoint(cx, "", cx.now);
            let cu = Custom { slot: cx.sel as u8, lane: cx.lane as u8, band: cx.band as u8, kind: cx.kind, wave: cur.clone() };
            commit(cx.p, cx.shared, |st| st.custom[i] = Some(cu));
            gui.say(format!("Stored as custom wave {} (note {})", i + 1, NOTES[i]), cx.now);
        };
        if resp.clicked() {
            match (&c, shift) {
                (None, _) | (Some(_), true) => store(gui),
                (Some(c), false) => {
                    gui.checkpoint(cx, "", cx.now);
                    let w = c.wave.clone();
                    edit_wave(cx.p, cx.shared, cx.sel, cx.lane, cx.band, cx.multi, |d| *d = w);
                }
            }
        }
        let mut act = 0;
        resp.context_menu(|ui| {
            if ui.button("Store the current shape here").clicked() {
                act = 1;
                ui.close_menu();
            }
            if ui.add_enabled(c.is_some(), egui::Button::new("Load it into this lane")).clicked() {
                act = 2;
                ui.close_menu();
            }
            if ui.add_enabled(c.is_some(), egui::Button::new("Point it at this lane")).on_hover_text("MIDI Switch will load it into the lane you are editing now.").clicked() {
                act = 4;
                ui.close_menu();
            }
            if ui.add_enabled(c.is_some(), egui::Button::new("Clear")).clicked() {
                act = 3;
                ui.close_menu();
            }
        });
        match act {
            1 => store(gui),
            2 => {
                if let Some(c) = &c {
                    gui.checkpoint(cx, "", cx.now);
                    let w = c.wave.clone();
                    edit_wave(cx.p, cx.shared, cx.sel, cx.lane, cx.band, cx.multi, |d| *d = w);
                }
            }
            3 => {
                gui.checkpoint(cx, "", cx.now);
                commit(cx.p, cx.shared, |st| st.custom[i] = None);
            }
            4 => {
                let (sl, la, ba, k) = (cx.sel as u8, cx.lane as u8, cx.band as u8, cx.kind);
                commit(cx.p, cx.shared, |st| {
                    if let Some(c) = st.custom[i].as_mut() {
                        c.slot = sl;
                        c.lane = la;
                        c.band = ba;
                        c.kind = k;
                    }
                });
            }
            _ => {}
        }
    }
    let x = 68.0 + N_CUSTOM as f32 * (w + 4.0) + 8.0;
    let ms = cx.p.midi_switch.value();
    if btn(ui, cx.r(x, y, 104.0, h), "MIDI Switch", ms, cx.col, "MIDI Switch: while playing, notes C# .. A load custom waves 1 .. 9 into the lanes they point at; C triggers the MIDI-triggered LFOs. Off: every note triggers.").clicked() && !cx.modal {
        set_b(cx.setter, &cx.p.midi_switch, !ms);
    }
    if ms {
        painter.text(cx.at(x + 112.0, y + h * 0.5), Align2::LEFT_CENTER, "C# .. A switch · C triggers", pf(9.5), DIM);
    }
}

