//! Camera Layout and Camera Style panels, and the editor for a selected
//! pill on the Layouts track.

use super::sidebar::{heading, help, toggle_help};
use super::timeline::Selection;
use super::widgets;
use crate::edit::{CameraAspect, CameraDefaultLayout, CameraLayoutKind, EditConfig};
use crate::icons::{self, Icon};
use crate::project::Recording;
use crate::theme;
use egui::{Color32, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

pub fn show(ui: &mut Ui, edit: &mut EditConfig, rec: &Recording, layout: bool) {
    if rec.camera.is_none() {
        heading(ui, if layout { "Camera Layout" } else { "Camera Style" });
        help(ui, "Record with a camera to use this.");
        return;
    }
    if layout {
        camera_layout(ui, edit);
    } else {
        camera_style(ui, edit);
    }
}

fn camera_layout(ui: &mut Ui, edit: &mut EditConfig) {
    let c = &mut edit.camera;
    // "Hide camera" with its eye-slash icon on the left.
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        icons::paint(ui.painter(), r, Icon::EyeSlash, theme::TEXT);
        ui.label(RichText::new("Hide camera").size(13.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            widgets::toggle(ui, &mut c.hidden);
        });
    });
    widgets::divider(ui);
    theme::section(ui, "Default layout");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let w = ((ui.available_width() - 16.0) / 3.0).max(60.0);
        for (value, label, enabled) in [
            (Some(CameraDefaultLayout::Overlay), "Camera overlay", true),
            (Some(CameraDefaultLayout::Split), "Split screen", true),
            (None, "Camera cutout", false),
        ] {
            ui.vertical(|ui| {
                ui.set_width(w);
                let selected = value == Some(c.layout);
                let (r, resp) = ui.allocate_exact_size(vec2(w, w * 0.74), if enabled { Sense::click() } else { Sense::hover() });
                layout_card(ui.painter(), r, label, selected, enabled, resp.hovered());
                let resp = if enabled {
                    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
                } else {
                    resp.on_hover_text("Needs background removal, not available yet")
                };
                if let (true, Some(v)) = (resp.clicked(), value) {
                    c.layout = v;
                }
                let col = if !enabled { Color32::from_rgb(0x53, 0x53, 0x55) } else if selected { theme::TEXT } else { theme::TEXT_DIM };
                ui.label(RichText::new(label).size(12.5).color(col));
            });
        }
    });
    widgets::divider(ui);
    theme::section(ui, "Camera Position");
    position_grid(ui, &mut c.position);
    widgets::divider(ui);
    theme::section(ui, "Size");
    help(ui, "Size of camera when recording is not zoomed in");
    widgets::slider(ui, &mut c.size, 0.12..=0.6, Some(0.3));
    toggle_help(ui, "Camera size during zoom", "Make the camera smaller while the screen is zoomed in.", &mut c.shrink_on_zoom);
    if c.shrink_on_zoom {
        widgets::slider(ui, &mut c.zoom_size, 0.08..=0.6, Some(0.2));
    }
}

fn camera_style(ui: &mut Ui, edit: &mut EditConfig) {
    let c = &mut edit.camera;
    heading(ui, "Camera Style");
    theme::section(ui, "Camera aspect ratio");
    let opts: Vec<_> = CameraAspect::ALL.iter().map(|a| (*a, a.label())).collect();
    widgets::segmented(ui, &mut c.aspect, &opts);
    theme::section(ui, "Roundness");
    widgets::slider(ui, &mut c.roundness, 0.0..=1.0, Some(0.45));
    toggle_help(ui, "Mirror camera", "", &mut c.mirror);
    theme::section(ui, "Shadow");
    widgets::slider(ui, &mut c.shadow, 0.0..=1.0, Some(0.6));
    widgets::divider(ui);
    egui::CollapsingHeader::new(RichText::new("Advanced").size(13.5)).id_salt("camera-advanced").show(ui, |ui| {
        theme::section(ui, &format!("Camera delay  ·  {:+.0} ms", c.delay * 1000.0));
        help(ui, "Shift the camera picture if it isn't in sync with your voice. Positive values show it later.");
        widgets::slider(ui, &mut c.delay, -0.5..=0.5, Some(0.0));
    });
}

/// Editor for a selected Layouts-track pill ("Close Camera Layout editor").
pub fn segment_editor(ui: &mut Ui, edit: &mut EditConfig, i: usize, selected: &mut Option<Selection>) {
    if icons::button(ui, Icon::Back, "Close Camera Layout editor", egui::FontId::proportional(13.5), theme::TEXT, Some(theme::SURFACE), vec2(0.0, 34.0))
        .clicked()
    {
        *selected = None;
        return;
    }
    ui.add_space(8.0);
    let default_size = edit.camera.size;
    let default_pos = edit.camera.position;
    let seg = &mut edit.camera_layouts[i];
    theme::section(ui, "Camera layout");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (kind, icon, label) in [
            (CameraLayoutKind::Default, Icon::CamPip, "Default"),
            (CameraLayoutKind::Split, Icon::CamSplit, "Split"),
            (CameraLayoutKind::Fullscreen, Icon::CamFull, "Fullscreen"),
            (CameraLayoutKind::Hide, Icon::CamHidden, "Hide"),
        ] {
            let sel = seg.kind == kind;
            let col = if sel { theme::TEXT } else { Color32::from_rgb(0xb4, 0xb6, 0xbd) };
            let fill = if sel { theme::SURFACE_HOVER } else { theme::SURFACE };
            let r = icons::button(ui, icon, label, egui::FontId::proportional(13.0), col, Some(fill), vec2(0.0, 32.0));
            if sel {
                ui.painter().rect_stroke(r.rect, CornerRadius::same(8), Stroke::new(1.0, Color32::from_rgb(0x4f, 0x4d, 0x6f)), StrokeKind::Inside);
            }
            if r.clicked() {
                seg.kind = kind;
            }
        }
    });
    help(
        ui,
        match seg.kind {
            CameraLayoutKind::Default => "Show both camera and screen",
            CameraLayoutKind::Split => "Show screen and camera side by side",
            CameraLayoutKind::Fullscreen => "Show camera in fullscreen mode",
            CameraLayoutKind::Hide => "Only show the screen and hide the camera",
        },
    );
    if seg.kind == CameraLayoutKind::Default {
        theme::section(ui, "Camera position");
        let mut pos = seg.position.unwrap_or(default_pos);
        if position_grid(ui, &mut pos) {
            seg.position = Some(pos);
        }
        let mut custom = seg.custom_size.is_some();
        if toggle_help(
            ui,
            "Custom camera size",
            "When enabled, you can set a custom camera size that will be used while this layout is active.",
            &mut custom,
        ) {
            seg.custom_size = custom.then_some(default_size);
        }
        if let Some(size) = &mut seg.custom_size {
            widgets::slider(ui, size, 0.12..=0.6, Some(default_size));
        }
    }
    toggle_help(ui, "Disable layout", "", &mut seg.disabled);
    ui.add_space(8.0);
    if widgets::wide_button_icon(ui, Some(Icon::Trash), "Remove layout").clicked() {
        edit.camera_layouts.remove(i);
        *selected = None;
    }
}

/// The 3x3 dot grid (no centre). Returns true when the position changed.
fn position_grid(ui: &mut Ui, pos: &mut [u8; 2]) -> bool {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 132.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(10), Color32::from_rgb(0x1b, 0x1d, 0x22));
    let inner = r.shrink2(vec2(22.0, 20.0));
    let mut changed = false;
    for row in 0..3u8 {
        for col in 0..3u8 {
            if (col, row) == (1, 1) {
                continue;
            }
            let c = pos2(inner.left() + inner.width() * col as f32 / 2.0, inner.top() + inner.height() * row as f32 / 2.0);
            let hit = Rect::from_center_size(c, vec2(34.0, 34.0));
            let resp = ui.interact(hit, ui.scope_id().with(("cam-pos", col, row)), Sense::click());
            let sel = *pos == [col, row];
            let p = ui.painter();
            let ring = if sel {
                theme::KNOB
            } else if resp.hovered() {
                Color32::from_rgb(0x3a, 0x38, 0x58)
            } else {
                Color32::from_rgb(0x2a, 0x29, 0x40)
            };
            p.circle_filled(c, 9.0, ring);
            p.circle_filled(c, 2.4, Color32::WHITE);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !sel {
                *pos = [col, row];
                changed = true;
            }
        }
    }
    changed
}

/// Preview card for a default layout: a wallpaper-like backdrop with a white
/// screen and the camera drawn in the layout's place.
fn layout_card(p: &egui::Painter, r: Rect, label: &str, selected: bool, enabled: bool, hovered: bool) {
    let dim = if enabled { 1.0 } else { 0.45 };
    p.rect_filled(r, CornerRadius::same(10), Color32::from_rgb(0x2a, 0x10, 0x22).gamma_multiply(dim));
    // A few diagonal colour bands, like the wallpaper in Screen Studio's cards.
    for (k, col) in [Color32::from_rgb(0xe0, 0x2a, 0x4a), Color32::from_rgb(0x8a, 0x16, 0x5a)].iter().enumerate() {
        let x = r.left() + r.width() * (0.15 + 0.4 * k as f32);
        p.add(egui::Shape::convex_polygon(
            vec![pos2(x, r.top()), pos2(x + r.width() * 0.18, r.top()), pos2(x - r.width() * 0.1, r.bottom()), pos2(x - r.width() * 0.28, r.bottom())],
            col.gamma_multiply(0.55 * dim),
            Stroke::NONE,
        ));
    }
    let white = Color32::WHITE.gamma_multiply(dim);
    let dark = Color32::from_rgb(0x1a, 0x1a, 0x1e).gamma_multiply(dim);
    let monitor = |p: &egui::Painter, c: egui::Pos2, col: Color32| {
        icons::paint(p, Rect::from_center_size(c, vec2(22.0, 22.0)), Icon::Display, col);
    };
    match label {
        "Split screen" => {
            let s = Rect::from_min_max(r.min, pos2(r.left() + r.width() * 0.62, r.bottom()));
            p.rect_filled(s, CornerRadius { nw: 10, sw: 10, ne: 0, se: 0 }, white);
            monitor(p, s.center(), dark);
            let cam = Rect::from_min_max(pos2(s.right(), r.top()), r.max);
            p.rect_filled(cam, CornerRadius { nw: 0, sw: 0, ne: 10, se: 10 }, dark);
            icons::paint(p, Rect::from_center_size(cam.center(), vec2(18.0, 18.0)), Icon::CamOnly, white);
        }
        _ => {
            let s = Rect::from_center_size(r.center() - vec2(r.width() * 0.04, 0.0), vec2(r.width() * 0.72, r.height() * 0.78));
            p.rect_filled(s, CornerRadius::same(6), white);
            monitor(p, s.center() - vec2(s.width() * 0.14, s.height() * 0.08), dark);
            if label == "Camera cutout" {
                // A person standing in front of the screen, no background.
                let base = s.right_bottom() + vec2(-s.width() * 0.16, r.height() * 0.11);
                let k = r.width() / 110.0;
                p.circle_filled(base - vec2(0.0, 30.0 * k), 8.5 * k, white);
                p.circle_stroke(base - vec2(0.0, 30.0 * k), 8.5 * k, Stroke::new(1.5, dark));
                p.add(egui::Shape::convex_polygon(
                    vec![base + vec2(-15.0 * k, 0.0), base + vec2(-12.0 * k, -14.0 * k), base + vec2(0.0, -19.0 * k), base + vec2(12.0 * k, -14.0 * k), base + vec2(15.0 * k, 0.0)],
                    white,
                    Stroke::new(1.5, dark),
                ));
            } else {
                let cam = Rect::from_center_size(s.right_bottom() - vec2(s.width() * 0.12, s.height() * 0.18), vec2(r.width() * 0.26, r.width() * 0.26));
                p.rect_filled(cam, CornerRadius::same(8), dark);
                icons::paint(p, Rect::from_center_size(cam.center(), vec2(18.0, 18.0)), Icon::CamOnly, white);
            }
        }
    }
    let border = if selected {
        Stroke::new(2.0, Color32::from_rgb(0xd8, 0xd6, 0xe4))
    } else if hovered && enabled {
        Stroke::new(1.0, Color32::from_white_alpha(80))
    } else {
        Stroke::new(1.0, Color32::from_white_alpha(18))
    };
    p.rect_stroke(r, CornerRadius::same(10), border, StrokeKind::Inside);
}
