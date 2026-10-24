//! Custom controls drawn to match Screen Studio's look: chip groups, thin
//! sliders with a lavender knob, iOS-style switches and wallpaper swatches.

use crate::theme;
use egui::{Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

/// Row of chips; returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (v, label) in options {
            if chip(ui, label, *value == *v).clicked() && *value != *v {
                *value = *v;
                changed = true;
            }
        }
    });
    changed
}

pub fn chip(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), egui::FontId::proportional(13.0), theme::TEXT);
    let size = vec2(galley.size().x + 26.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if selected {
        theme::SURFACE_HOVER
    } else if resp.hovered() {
        Color32::from_rgb(0x21, 0x23, 0x28)
    } else {
        theme::SURFACE
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(8), fill);
    if selected {
        p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3c, 0x44)), StrokeKind::Inside);
    }
    let color = if selected { theme::TEXT } else { Color32::from_rgb(0xb4, 0xb6, 0xbd) };
    p.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Two-option control with an underline, as in the zoom mode picker.
pub fn tabs<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    let w = ui.available_width();
    let (rect, whole) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    let cw = w / options.len() as f32;
    for (i, (v, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + cw * i as f32, rect.top()), vec2(cw, rect.height()));
        // Ids from this control's own id: two tab rows in one panel (zoom
        // type and zoom mode) mustn't share them.
        let resp = ui.interact(r, whole.id.with(("tab", i)), Sense::click());
        let sel = *value == *v;
        if sel {
            ui.painter().rect_filled(
                Rect::from_min_max(pos2(r.left() + 1.0, r.bottom() - 2.0), pos2(r.right() - 1.0, r.bottom())),
                0.0,
                theme::ACCENT,
            );
        }
        let color = if sel { theme::KNOB } else if resp.hovered() { theme::TEXT } else { theme::TEXT_DIM };
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, *label, egui::FontId::proportional(13.5), color);
        if resp.clicked() && !sel {
            *value = *v;
            changed = true;
        }
    }
    changed
}

/// Screen Studio's manual zoom picker: the frame with a purple dot at the
/// focus point (`x`, `y` as fractions of the frame). Click or drag to move
/// it; it snaps to the middle within 1% unless Ctrl is held. True when it
/// moved.
pub fn focus_picker(ui: &mut Ui, image: Option<&egui::TextureHandle>, aspect: f32, x: &mut f64, y: &mut f64) -> bool {
    let w = ui.available_width();
    let (outer, base) = ui.allocate_exact_size(vec2(w, (w - 32.0) / aspect.max(0.2) + 32.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(outer, CornerRadius::same(14), theme::SURFACE);
    let pic = outer.shrink(16.0);
    let resp = ui.interact(pic.expand(14.0), base.id.with("focus-picker"), Sense::click_and_drag());
    match image {
        Some(t) => {
            p.image(t.id(), pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            p.rect_filled(pic, CornerRadius::same(6), Color32::from_rgb(0x0e, 0x0f, 0x12));
            p.text(pic.center(), egui::Align2::CENTER_CENTER, "Loading frame…", egui::FontId::proportional(12.5), theme::TEXT_DIM);
        }
    }
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked() || resp.drag_started()) {
        let free = ui.input(|i| i.modifiers.ctrl);
        let snap = |v: f64| if !free && (v - 0.5).abs() <= 0.01 { 0.5 } else { v };
        let nx = snap((((pos.x - pic.left()) / pic.width()) as f64).clamp(0.0, 1.0));
        let ny = snap((((pos.y - pic.top()) / pic.height()) as f64).clamp(0.0, 1.0));
        if (nx, ny) != (*x, *y) {
            (*x, *y) = (nx, ny);
            changed = true;
        }
    }
    let dot = pos2(pic.left() + pic.width() * *x as f32, pic.top() + pic.height() * *y as f32);
    let active = resp.dragged() || resp.hovered();
    p.circle_filled(dot + vec2(0.0, 2.0), 17.0, Color32::from_black_alpha(70));
    p.circle_filled(dot, if active { 17.0 } else { 15.0 }, Color32::from_rgb(0x4a, 0x32, 0xf5));
    if resp.dragged() {
        // Where it is, while it's being picked.
        let text = format!("{:.0}%  ·  {:.0}%", *x * 100.0, *y * 100.0);
        let g = p.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
        let at = pos2(dot.x - g.size().x / 2.0 - 8.0, (dot.y - 44.0).max(outer.top() + 2.0));
        let r = Rect::from_min_size(at, g.size() + vec2(16.0, 8.0));
        p.rect_filled(r, CornerRadius::same(6), Color32::from_black_alpha(190));
        p.galley(r.min + vec2(8.0, 4.0), g, Color32::WHITE);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    changed
}

/// Full-width slider. Shows a "Reset" link when `default` differs.
pub fn slider(ui: &mut Ui, value: &mut f64, range: std::ops::RangeInclusive<f64>, default: Option<f64>) -> bool {
    let (lo, hi) = (*range.start(), *range.end());
    let mut changed = false;
    ui.horizontal(|ui| {
        let reset_w = if default.is_some() { 52.0 } else { 0.0 };
        let w = (ui.available_width() - reset_w).max(60.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click_and_drag());
        let knob_r = 9.0;
        let x0 = rect.left() + knob_r;
        let x1 = rect.right() - knob_r;
        if let Some(p) = resp.interact_pointer_pos() {
            let f = ((p.x - x0) / (x1 - x0)).clamp(0.0, 1.0) as f64;
            let v = lo + (hi - lo) * f;
            if (v - *value).abs() > f64::EPSILON {
                *value = v;
                changed = true;
            }
        }
        let f = ((*value - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
        let kx = x0 + (x1 - x0) * f;
        let cy = rect.center().y;
        let p = ui.painter();
        p.line_segment([pos2(x0, cy), pos2(x1, cy)], Stroke::new(3.0, theme::TRACK));
        p.line_segment([pos2(x0, cy), pos2(kx, cy)], Stroke::new(3.0, theme::ACCENT));
        let hovered = resp.hovered() || resp.dragged();
        p.circle_filled(pos2(kx, cy), if hovered { knob_r + 1.0 } else { knob_r }, theme::KNOB);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if let Some(d) = default {
            let differs = (d - *value).abs() > 1e-6;
            let color = if differs { theme::TEXT_DIM } else { Color32::from_rgb(0x3a, 0x3c, 0x42) };
            let r = ui.add(egui::Label::new(egui::RichText::new("Reset").color(color)).sense(Sense::click()));
            if differs && r.clicked() {
                *value = d;
                changed = true;
            }
        }
    });
    changed
}

/// iOS-style switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = vec2(42.0, 24.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = theme::TRACK.lerp_to_gamma(theme::ACCENT, t);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(12), bg);
    let x = egui::lerp(rect.left() + 12.0..=rect.right() - 12.0, t);
    p.circle_filled(pos2(x, rect.center().y), 9.5, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Label on the left, switch on the right.
pub fn toggle_row(ui: &mut Ui, label: &str, on: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(13.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = toggle(ui, on).changed();
        });
    });
    changed
}

/// Wallpaper thumbnail painted as a four-corner gradient mesh.
pub fn swatch(ui: &mut Ui, corners: [Color32; 4], selected: bool, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let inner = rect.shrink(if selected { 3.0 } else { 0.0 });
    let mut mesh = egui::Mesh::default();
    let n = 6;
    // Subdivide so the bilinear blend looks smooth.
    for j in 0..=n {
        for i in 0..=n {
            let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
            let top = corners[0].lerp_to_gamma(corners[1], u);
            let bottom = corners[2].lerp_to_gamma(corners[3], u);
            let pos = pos2(egui::lerp(inner.left()..=inner.right(), u), egui::lerp(inner.top()..=inner.bottom(), v));
            mesh.colored_vertex(pos, top.lerp_to_gamma(bottom, v));
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = (j * (n + 1) + i) as u32;
            let b = a + 1;
            let c = a + (n + 1) as u32;
            mesh.add_triangle(a, b, c);
            mesh.add_triangle(b, c + 1, c);
        }
    }
    ui.painter().add(mesh);
    if selected {
        ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
    } else if resp.hovered() {
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, Color32::from_white_alpha(90)), StrokeKind::Inside);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A subtle full-width button like "Set as default".
pub fn wide_button(ui: &mut Ui, label: &str) -> Response {
    wide_button_icon(ui, None, label)
}

pub fn wide_button_icon(ui: &mut Ui, icon: Option<crate::icons::Icon>, label: &str) -> Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 38.0), Sense::click());
    let fill = if resp.hovered() { theme::SURFACE_HOVER } else { theme::SURFACE };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    crate::icons::paint_with_text(ui.painter(), rect, icon, 16.0, label, egui::FontId::proportional(13.5), theme::TEXT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn divider(ui: &mut Ui) {
    ui.add_space(10.0);
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, theme::BORDER);
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;

    fn run(ctx: &egui::Context, t: &mut f64, events: Vec<egui::Event>, f: &mut dyn FnMut(&mut Ui)) {
        *t += 0.05;
        // Keys held: from the pointer events' modifiers.
        let modifiers = events.iter().find_map(|e| match e {
            egui::Event::PointerButton { modifiers, .. } => Some(*modifiers),
            _ => None,
        });
        let mut events = events;
        events.insert(0, egui::Event::ModifiersChanged(modifiers.unwrap_or_default()));
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 600.0))), time: Some(*t), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| f(ui));
        out.textures_delta.clear();
    }

    fn click(pos: Pos2, mods: egui::Modifiers) -> Vec<Vec<egui::Event>> {
        vec![
            vec![egui::Event::PointerMoved(pos)],
            vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: mods }],
            vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: mods }],
        ]
    }

    /// Two tab rows in one panel (zoom type, zoom mode) are independent.
    #[test]
    fn tab_rows_dont_share_clicks() {
        let ctx = egui::Context::default();
        let mut t = 0.0;
        let (mut kind, mut manual) = (0u8, false);
        let mut rows = (Rect::NOTHING, Rect::NOTHING);
        let draw = |ui: &mut Ui, kind: &mut u8, manual: &mut bool, rows: &mut (Rect, Rect)| {
            let a = ui.cursor().min;
            tabs(ui, kind, &[(0, "Zoom"), (1, "Glass Loupe")]);
            let b = ui.cursor().min;
            tabs(ui, manual, &[(false, "Auto"), (true, "Manual")]);
            *rows = (Rect::from_min_size(a, vec2(ui.available_width(), 40.0)), Rect::from_min_size(b, vec2(ui.available_width(), 40.0)));
        };
        run(&ctx, &mut t, vec![], &mut |ui| draw(ui, &mut kind, &mut manual, &mut rows));
        // "Manual": the right half of the second row.
        let at = pos2(rows.1.left() + rows.1.width() * 0.75, rows.1.center().y);
        for ev in click(at, Default::default()) {
            run(&ctx, &mut t, ev, &mut |ui| draw(ui, &mut kind, &mut manual, &mut rows));
        }
        assert!(manual, "Manual wasn't picked");
        assert_eq!(kind, 0, "Glass Loupe got picked too");
    }

    /// The manual zoom picker: click to place the dot, it snaps to the middle
    /// within 1% (not with Ctrl), and dragging moves it.
    #[test]
    fn focus_picker_places_and_snaps() {
        let ctx = egui::Context::default();
        let mut t = 0.0;
        let (mut x, mut y) = (0.5, 0.5);
        let mut area = Rect::NOTHING;
        let draw = |ui: &mut Ui, x: &mut f64, y: &mut f64, area: &mut Rect| {
            let top = ui.cursor().min;
            let w = ui.available_width();
            focus_picker(ui, None, 16.0 / 9.0, x, y);
            // The picture inside the 16 px frame.
            *area = Rect::from_min_size(top + vec2(16.0, 16.0), vec2(w - 32.0, (w - 32.0) / (16.0 / 9.0)));
        };
        run(&ctx, &mut t, vec![], &mut |ui| draw(ui, &mut x, &mut y, &mut area));
        let pic = area;
        let at = move |fx: f32, fy: f32| pos2(pic.left() + pic.width() * fx, pic.top() + pic.height() * fy);
        for ev in click(at(0.25, 0.75), Default::default()) {
            run(&ctx, &mut t, ev, &mut |ui| draw(ui, &mut x, &mut y, &mut area));
        }
        assert!((x - 0.25).abs() < 0.01 && (y - 0.75).abs() < 0.01, "at {x:.3}, {y:.3}");
        for ev in click(at(0.505, 0.3), Default::default()) {
            run(&ctx, &mut t, ev, &mut |ui| draw(ui, &mut x, &mut y, &mut area));
        }
        assert_eq!(x, 0.5, "didn't snap to the middle");
        let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        for ev in click(at(0.505, 0.3), ctrl) {
            run(&ctx, &mut t, ev, &mut |ui| draw(ui, &mut x, &mut y, &mut area));
        }
        assert!((x - 0.505).abs() < 0.002, "Ctrl should turn snapping off: {x}");
        // Drag from there to the top right.
        let m = egui::Modifiers::default();
        let (a, b) = (at(0.5, 0.3), at(0.9, 0.1));
        run(&ctx, &mut t, vec![egui::Event::PointerMoved(a)], &mut |ui| draw(ui, &mut x, &mut y, &mut area));
        run(&ctx, &mut t, vec![egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: m }], &mut |ui| draw(ui, &mut x, &mut y, &mut area));
        for k in 1..=8 {
            let p = a + (b - a) * (k as f32 / 8.0);
            run(&ctx, &mut t, vec![egui::Event::PointerMoved(p)], &mut |ui| draw(ui, &mut x, &mut y, &mut area));
        }
        run(&ctx, &mut t, vec![egui::Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: false, modifiers: m }], &mut |ui| draw(ui, &mut x, &mut y, &mut area));
        assert!((x - 0.9).abs() < 0.01 && (y - 0.1).abs() < 0.01, "after drag {x:.3}, {y:.3}");
    }
}
