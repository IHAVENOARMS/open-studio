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
    let (rect, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    let cw = w / options.len() as f32;
    for (i, (v, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + cw * i as f32, rect.top()), vec2(cw, rect.height()));
        let resp = ui.interact(r, ui.scope_id().with(("tab", i)), Sense::click());
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
    let w = ui.available_width();
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(13.5))
            .fill(theme::SURFACE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(w, 38.0)),
    )
}

pub fn divider(ui: &mut Ui) {
    ui.add_space(10.0);
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, theme::BORDER);
    ui.add_space(6.0);
}
