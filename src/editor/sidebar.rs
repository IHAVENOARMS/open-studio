//! Settings panel on the right: a vertical icon tab strip plus a rounded card.

use super::widgets;
use crate::edit::{AnimationSpeed, Background, EditConfig, ZoomFocus};
use crate::project::Recording;
use crate::render::background::PRESETS;
use crate::theme;
use egui::{Color32, CornerRadius, RichText, Sense, Ui, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Background,
    Cursor,
    Animation,
}

const TABS: [(Tab, &str, &str); 3] = [
    (Tab::Background, "🖼", "Background"),
    (Tab::Cursor, "🖱", "Cursor"),
    (Tab::Animation, "🔍", "Zoom & animation"),
];

pub fn show(ui: &mut Ui, edit: &mut EditConfig, tab: &mut Tab, selected: &mut Option<usize>, rec: &Recording) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        // Icon strip.
        ui.vertical(|ui| {
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.y = 14.0;
            for (t, icon, name) in TABS {
                let active = *tab == t && selected.is_none();
                let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
                if active {
                    ui.painter().rect_filled(rect, CornerRadius::same(8), theme::SURFACE_HOVER);
                }
                let color = if active { theme::KNOB } else if resp.hovered() { theme::TEXT } else { theme::TEXT_DIM };
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(18.0), color);
                if resp.on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    *tab = t;
                    *selected = None;
                }
            }
        });
        // Card.
        egui::Frame::new()
            .fill(theme::PANEL)
            .corner_radius(CornerRadius::same(14))
            .inner_margin(egui::Margin::same(18))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(ui.available_height());
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    if let Some(i) = *selected {
                        if i < edit.zoom_segments.len() {
                            zoom_segment(ui, edit, i, selected);
                            return;
                        }
                        *selected = None;
                    }
                    match tab {
                        Tab::Background => background(ui, edit),
                        Tab::Cursor => cursor(ui, edit, rec),
                        Tab::Animation => animation(ui, edit, rec, selected),
                    }
                }));
            });
    });
}

fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(theme::semibold(15.0)));
    ui.add_space(4.0);
}

fn help(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(theme::TEXT_DIM));
}

fn to_color32(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

fn background(ui: &mut Ui, edit: &mut EditConfig) {
    heading(ui, "Background");
    let mut kind = matches!(edit.background, Background::Gradient(_));
    if widgets::segmented(ui, &mut kind, &[(true, "Wallpaper"), (false, "Color")]) {
        edit.background = if kind { Background::Gradient(0) } else { Background::Color([0.11, 0.11, 0.13]) };
    }
    ui.add_space(6.0);
    match &mut edit.background {
        Background::Gradient(sel) => {
            if widgets::wide_button(ui, "✨  Pick random wallpaper").clicked() {
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as usize)
                    .unwrap_or(0);
                *sel = (*sel + 1 + seed % (PRESETS.len() - 1)) % PRESETS.len();
            }
            ui.add_space(4.0);
            let size = ((ui.available_width() - 7.0 * 6.0) / 8.0).floor().max(24.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (i, p) in PRESETS.iter().enumerate() {
                    if widgets::swatch(ui, p.corners.map(to_color32), *sel == i, size).on_hover_text(p.name).clicked() {
                        *sel = i;
                    }
                }
            });
        }
        Background::Color(rgb) => {
            const COLORS: [[f32; 3]; 8] = [
                [0.07, 0.07, 0.08],
                [0.96, 0.96, 0.97],
                [0.29, 0.19, 0.93],
                [0.93, 0.29, 0.45],
                [0.98, 0.62, 0.20],
                [0.20, 0.75, 0.47],
                [0.18, 0.55, 0.95],
                [0.55, 0.36, 0.96],
            ];
            let size = ((ui.available_width() - 7.0 * 6.0) / 8.0).floor().max(24.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for c in COLORS {
                    let col = Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
                    if widgets::swatch(ui, [col; 4], *rgb == c, size).clicked() {
                        *rgb = c;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Custom");
                ui.color_edit_button_rgb(rgb);
            });
        }
    }
    widgets::divider(ui);
    theme::section(ui, "Padding");
    widgets::slider(ui, &mut edit.padding, 0.0..=0.25, Some(0.08));
    theme::section(ui, "Rounded corners");
    widgets::slider(ui, &mut edit.radius, 0.0..=48.0, Some(12.0));
    theme::section(ui, "Shadow");
    widgets::slider(ui, &mut edit.shadow, 0.0..=1.0, Some(0.6));
}

fn cursor(ui: &mut Ui, edit: &mut EditConfig, rec: &Recording) {
    heading(ui, "Cursor");
    if rec.cursor_embedded {
        help(ui, "This recording has the system cursor baked into the video, so cursor styling is unavailable.");
        return;
    }
    if rec.events.moves.is_empty() {
        help(ui, "No cursor movement was captured for this recording.");
        return;
    }
    theme::section(ui, "Size");
    widgets::slider(ui, &mut edit.cursor.size, 0.5..=4.0, Some(1.5));
    widgets::divider(ui);
    widgets::toggle_row(ui, "Smooth movement", &mut edit.cursor.smooth);
    if edit.cursor.smooth {
        help(ui, "How quickly the smoothed cursor catches up with the real one.");
        let opts: Vec<_> = AnimationSpeed::ALL.iter().map(|s| (*s, s.label())).collect();
        widgets::segmented(ui, &mut edit.cursor.speed, &opts);
    }
    widgets::divider(ui);
    widgets::toggle_row(ui, "Hide when not moving", &mut edit.cursor.hide_when_idle);
    widgets::toggle_row(ui, "Click animation", &mut edit.cursor.click_effect);
}

fn animation(ui: &mut Ui, edit: &mut EditConfig, rec: &Recording, selected: &mut Option<usize>) {
    heading(ui, "Zoom");
    theme::section(ui, "Animation speed");
    let opts: Vec<_> = AnimationSpeed::ALL.iter().map(|s| (*s, s.label())).collect();
    widgets::segmented(ui, &mut edit.zoom_speed, &opts);
    help(ui, "Zooms and camera moves use spring physics, like a real camera operator.");
    widgets::divider(ui);
    widgets::toggle_row(ui, "Motion blur", &mut edit.motion_blur);
    widgets::divider(ui);
    theme::section(ui, &format!("Default zoom level  ·  {:.0}%", edit.default_zoom * 100.0));
    widgets::slider(ui, &mut edit.default_zoom, 1.0..=4.0, Some(2.0));
    ui.add_space(6.0);
    let clicks = rec.events.clicks.iter().filter(|c| c.down).count();
    if widgets::wide_button(ui, "Regenerate automatic zooms").clicked() {
        edit.zoom_segments = crate::engine::auto_zoom_segments(rec, edit.default_zoom);
        *selected = None;
    }
    help(ui, &format!("Zooms are created from your clicks ({clicks} recorded). This replaces the current zooms."));
    if widgets::wide_button(ui, "Remove all zooms").clicked() {
        edit.zoom_segments.clear();
        *selected = None;
    }
}

fn zoom_segment(ui: &mut Ui, edit: &mut EditConfig, i: usize, selected: &mut Option<usize>) {
    let close = ui.add(
        egui::Button::new(RichText::new("←  Close   Esc").size(13.5))
            .fill(theme::SURFACE)
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(0.0, 34.0)),
    );
    if close.clicked() {
        *selected = None;
        return;
    }
    ui.add_space(8.0);
    let default_zoom = edit.default_zoom;
    let seg = &mut edit.zoom_segments[i];
    theme::section(ui, "Zoom mode");
    let mut manual = matches!(seg.focus, ZoomFocus::Manual { .. });
    if widgets::tabs(ui, &mut manual, &[(false, "Auto"), (true, "Manual")]) {
        seg.focus = if manual { ZoomFocus::Manual { x: 0.5, y: 0.5 } } else { ZoomFocus::Auto };
    }
    help(
        ui,
        if manual {
            "Click on the preview to choose where the camera zooms in."
        } else {
            "Zoomed camera will automatically try to keep the mouse cursor visible."
        },
    );
    ui.add_space(6.0);
    theme::section(ui, &format!("Zoom level  ·  {:.0}%", seg.scale * 100.0));
    help(ui, "How close to zoom on the cursor during this zoom phase");
    widgets::slider(ui, &mut seg.scale, 1.0..=4.0, Some(default_zoom));
    ui.add_space(4.0);
    let scale = seg.scale;
    if widgets::wide_button(ui, "Set as default").clicked() {
        edit.default_zoom = scale;
    }
    if widgets::wide_button(ui, "Apply to all zooms").clicked() {
        for s in &mut edit.zoom_segments {
            s.scale = scale;
        }
    }
    widgets::divider(ui);
    let remove = ui.add(
        egui::Button::new(RichText::new("Remove zoom").size(13.5).color(theme::RECORD))
            .fill(theme::SURFACE)
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(ui.available_width(), 38.0)),
    );
    if remove.clicked() {
        edit.zoom_segments.remove(i);
        *selected = None;
    }
}
