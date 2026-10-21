//! Settings panel on the right: a vertical icon tab strip plus a rounded card.

use super::widgets;
use crate::edit::{AnimationSpeed, Background, EditConfig, ZoomFocus};
use crate::project::Recording;
use crate::render::background::PRESETS;
use crate::dialogs;
use crate::wallpapers;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use crate::icons::{self, Icon};
use crate::theme;
use egui::{Color32, CornerRadius, Rect, RichText, Sense, Ui, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Background,
    Cursor,
    Animation,
}

const TABS: [(Tab, Icon, &str); 3] = [
    (Tab::Background, Icon::Image, "Background"),
    (Tab::Cursor, Icon::Cursor, "Cursor"),
    (Tab::Animation, Icon::Zoom, "Zoom & animation"),
];

struct Download {
    total: usize,
    done: AtomicUsize,
    finished: AtomicBool,
    errors: Mutex<Vec<String>>,
}

/// UI state the background panel keeps between frames.
#[derive(Default)]
pub struct State {
    category: Option<String>,
    thumbs: HashMap<String, Option<egui::TextureHandle>>,
    download: Option<Arc<Download>>,
    picking: Option<Receiver<Vec<std::path::PathBuf>>>,
    /// Files chosen for import as custom wallpapers.
    importing: Option<Receiver<Vec<std::path::PathBuf>>>,
    /// Custom wallpapers being converted; receives the id of each one added.
    converting: Option<Receiver<Option<String>>>,
    /// Cursor theme UI state.
    pub cursors: super::cursors::State,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BgKind {
    Wallpaper,
    Gradient,
    Color,
    Image,
}

pub fn show(
    ui: &mut Ui,
    state: &mut State,
    edit: &mut EditConfig,
    tab: &mut Tab,
    selected: &mut Option<usize>,
    rec: &Recording,
) {
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
                icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(20.0, 20.0)), icon, color);
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
                        Tab::Background => background(ui, state, edit),
                        Tab::Cursor => cursor(ui, state, edit, rec),
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

fn background(ui: &mut Ui, state: &mut State, edit: &mut EditConfig) {
    heading(ui, "Background");
    let kind = match edit.background {
        Background::Wallpaper(_) => BgKind::Wallpaper,
        Background::Gradient(_) => BgKind::Gradient,
        Background::Color(_) => BgKind::Color,
        Background::Image(_) => BgKind::Image,
    };
    let mut new_kind = kind;
    widgets::segmented(
        ui,
        &mut new_kind,
        &[(BgKind::Wallpaper, "Wallpaper"), (BgKind::Gradient, "Gradient"), (BgKind::Color, "Color"), (BgKind::Image, "Image")],
    );
    if new_kind != kind {
        edit.background = match new_kind {
            BgKind::Wallpaper => match wallpapers::manifest().into_iter().find(|w| wallpapers::installed(&w.id)) {
                Some(w) => Background::Wallpaper(w.id.clone()),
                None => Background::Wallpaper(String::new()),
            },
            BgKind::Gradient => Background::Gradient(0),
            BgKind::Color => Background::Color([0.11, 0.11, 0.13]),
            BgKind::Image => Background::Image(String::new()),
        };
    }
    ui.add_space(6.0);
    match &mut edit.background {
        Background::Wallpaper(id) => wallpaper_grid(ui, state, id),
        Background::Gradient(sel) => {
            if widgets::wide_button_icon(ui, Some(Icon::Sparkle), "Pick random gradient").clicked() {
                *sel = (*sel + 1 + random() % (PRESETS.len() - 1)) % PRESETS.len();
            }
            ui.add_space(4.0);
            let size = grid_cell(ui);
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
            let size = grid_cell(ui);
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
        Background::Image(path) => {
            if let Some(rx) = &state.picking {
                match rx.try_recv() {
                    Ok(files) => {
                        if let Some(p) = files.first() {
                            *path = p.to_string_lossy().into_owned();
                        }
                        state.picking = None;
                    }
                    Err(TryRecvError::Disconnected) => state.picking = None,
                    Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)),
                }
            }
            if widgets::wide_button_icon(ui, Some(Icon::Image), "Choose image").clicked() && state.picking.is_none() {
                state.picking = Some(dialogs::open_files(ui.ctx().clone(), "Choose background image", Some(dialogs::images_filter()), false));
            }
            let name = std::path::Path::new(path.as_str()).file_name().map(|n| n.to_string_lossy().to_string());
            help(ui, &name.unwrap_or_else(|| "No image selected. JPEG, PNG or WebP.".into()));
        }
    }
    if matches!(edit.background, Background::Wallpaper(_) | Background::Image(_)) {
        theme::section(ui, "Background blur");
        widgets::slider(ui, &mut edit.background_blur, 0.0..=1.0, Some(0.0));
    }
    widgets::divider(ui);
    theme::section(ui, "Padding");
    widgets::slider(ui, &mut edit.padding, 0.0..=0.25, Some(0.08));
    theme::section(ui, "Rounded corners");
    widgets::slider(ui, &mut edit.radius, 0.0..=48.0, Some(12.0));
    theme::section(ui, "Shadow");
    widgets::slider(ui, &mut edit.shadow, 0.0..=1.0, Some(0.6));
}

fn cursor(ui: &mut Ui, state: &mut State, edit: &mut EditConfig, rec: &Recording) {
    heading(ui, "Cursor");
    if rec.cursor_embedded {
        help(ui, "This recording has the system cursor baked into the video, so cursor styling is unavailable.");
        return;
    }
    if rec.events.moves.is_empty() {
        help(ui, "No cursor movement was captured for this recording.");
        return;
    }
    super::cursors::style_picker(ui, &mut state.cursors, edit);
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
    widgets::toggle_row(ui, "Always use default cursor", &mut edit.cursor.always_default);
    if !edit.cursor.always_default && !rec.events.shapes.is_empty() {
        help(ui, "Text, link and resize cursors are redrawn from what the apps showed while recording.");
    }
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
    if edit.motion_blur {
        theme::section(ui, &format!("Zoom blur  ·  {:.0}%", edit.zoom_blur * 100.0));
        help(ui, "Radial blur while the camera zooms in or out.");
        widgets::slider(ui, &mut edit.zoom_blur, 0.0..=1.0, Some(0.5));
        theme::section(ui, &format!("Movement blur  ·  {:.0}%", edit.pan_blur * 100.0));
        help(ui, "Directional blur while the zoomed camera follows the cursor.");
        widgets::slider(ui, &mut edit.pan_blur, 0.0..=1.0, Some(0.25));
    }
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
    let close = icons::button(ui, Icon::Back, "Close   Esc", egui::FontId::proportional(13.5), theme::TEXT, Some(theme::SURFACE), vec2(0.0, 34.0));
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

fn random() -> usize {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as usize).unwrap_or(0)
}

/// Width of one cell in an 8-column grid.
fn grid_cell(ui: &Ui) -> f32 {
    ((ui.available_width() - 7.0 * 6.0) / 8.0).floor().max(24.0)
}

fn wallpaper_grid(ui: &mut Ui, state: &mut State, id: &mut String) {
    let all = wallpapers::manifest();
    poll_imports(ui, state, id);
    let missing = wallpapers::missing().len();
    let builtin_total = all.iter().filter(|w| w.category != wallpapers::CUSTOM_CATEGORY).count();
    if let Some(d) = &state.download {
        let done = d.done.load(Ordering::Relaxed);
        if d.finished.load(Ordering::Relaxed) {
            let errors = d.errors.lock().unwrap();
            if !errors.is_empty() {
                help(ui, &format!("{} wallpapers failed to download.", errors.len()));
            }
            drop(errors);
            state.download = None;
            state.thumbs.retain(|_, t| t.is_some());
        } else {
            ui.add(
                egui::ProgressBar::new(done as f32 / d.total.max(1) as f32)
                    .text(format!("Downloading wallpapers  {done} / {}", d.total))
                    .fill(theme::ACCENT),
            );
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            // Let finished thumbnails appear while the rest download.
            state.thumbs.retain(|_, t| t.is_some());
        }
    } else if missing > 0 {
        let label = if missing == builtin_total {
            format!("Download wallpapers ({missing})")
        } else {
            format!("Download {missing} more wallpapers")
        };
        if widgets::wide_button_icon(ui, Some(Icon::Export), &label).clicked() {
            let d = Arc::new(Download { total: missing, done: AtomicUsize::new(0), finished: AtomicBool::new(false), errors: Mutex::new(vec![]) });
            let d2 = d.clone();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let errs = wallpapers::fetch_missing(&d2.done);
                *d2.errors.lock().unwrap() = errs;
                d2.finished.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            });
            state.download = Some(d);
        }
        if missing == builtin_total {
            help(ui, "Openly licensed wallpapers are downloaded once and stored locally. Credits are in WALLPAPERS.md.");
        }
    }

    // Category chips, defaulting to the category of the current wallpaper.
    let cats = wallpapers::categories();
    let current_cat = wallpapers::get(id).map(|w| w.category);
    let cat = state.category.clone().or(current_cat).unwrap_or_else(|| cats[0].clone());
    let mut chosen = cat.clone();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for c in &cats {
            if widgets::chip(ui, c, *c == chosen).clicked() {
                chosen = c.clone();
            }
        }
    });
    if chosen != cat || state.category.is_none() {
        state.category = Some(chosen.clone());
    }
    widgets::divider(ui);
    let installed: Vec<_> = all.iter().filter(|w| wallpapers::installed(&w.id)).collect();
    if widgets::wide_button_icon(ui, Some(Icon::Sparkle), "Pick random wallpaper").clicked() && !installed.is_empty() {
        let w = installed[random() % installed.len()];
        *id = w.id.clone();
        state.category = Some(w.category.clone());
    }
    ui.add_space(4.0);
    let size = grid_cell(ui);
    let custom = chosen == wallpapers::CUSTOM_CATEGORY;
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        if custom {
            let busy = state.importing.is_some() || state.converting.is_some();
            if add_tile(ui, size, busy).on_hover_text("Add your own wallpapers (JPEG, PNG, WebP)").clicked() && !busy {
                state.importing = Some(dialogs::open_files(ui.ctx().clone(), "Add wallpapers", Some(dialogs::images_filter()), true));
            }
        }
        for w in installed.iter().filter(|w| w.category == chosen) {
            let tex = state.thumbs.entry(w.id.clone()).or_insert_with(|| load_thumb(ui.ctx(), &w.id)).clone();
            let Some(tex) = tex else { continue };
            let resp = thumb(ui, &tex, *id == w.id, size);
            let tip = if w.author.is_empty() { w.title.clone() } else { format!("{} by {} ({})", w.title, w.author, w.license) };
            let resp = resp.on_hover_text(tip);
            if resp.clicked() {
                *id = w.id.clone();
            }
            if custom {
                resp.context_menu(|ui| {
                    if ui.button("Remove wallpaper").clicked() {
                        remove = Some(w.id.clone());
                        ui.close();
                    }
                });
            }
        }
    });
    if let Some(r) = remove {
        let _ = wallpapers::remove_custom(&r);
        state.thumbs.remove(&r);
    }
    if custom && !all.iter().any(|w| w.category == wallpapers::CUSTOM_CATEGORY) {
        help(ui, "Add your own images. They are stored with the other wallpapers and available in every project.");
    }
}

/// Picks up files chosen in the import dialog and converts them on a worker
/// thread; selects each wallpaper as it is added.
fn poll_imports(ui: &Ui, state: &mut State, id: &mut String) {
    if let Some(rx) = &state.importing {
        match rx.try_recv() {
            Ok(files) => {
                state.importing = None;
                if !files.is_empty() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        for f in files {
                            let _ = tx.send(wallpapers::add_custom(&f).ok().map(|w| w.id));
                            ctx.request_repaint();
                        }
                    });
                    state.converting = Some(rx);
                }
            }
            Err(TryRecvError::Disconnected) => state.importing = None,
            Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)),
        }
    }
    if let Some(rx) = &state.converting {
        loop {
            match rx.try_recv() {
                Ok(Some(new_id)) => *id = new_id,
                Ok(None) => {}
                Err(TryRecvError::Empty) => {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    state.converting = None;
                    break;
                }
            }
        }
    }
}

/// Dashed "+" tile for adding custom wallpapers.
fn add_tile(ui: &mut Ui, size: f32, busy: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let col = if resp.hovered() { theme::TEXT } else { theme::TEXT_DIM };
    let fill = if resp.hovered() { theme::SURFACE_HOVER } else { theme::SURFACE };
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    let stroke = egui::Stroke::new(1.0, col.gamma_multiply(0.6));
    let r = rect.shrink(0.5);
    for (a, b) in [(r.left_top(), r.right_top()), (r.right_top(), r.right_bottom()), (r.right_bottom(), r.left_bottom()), (r.left_bottom(), r.left_top())] {
        ui.painter().extend(egui::Shape::dashed_line(&[a, b], stroke, 3.0, 3.0));
    }
    if busy {
        egui::Spinner::new().size(size * 0.4).paint_at(ui, Rect::from_center_size(rect.center(), vec2(size * 0.4, size * 0.4)));
    } else {
        icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(size * 0.42, size * 0.42)), Icon::Plus, col);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn load_thumb(ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
    let img = image::open(wallpapers::thumb_path(id)).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture(format!("thumb-{id}"), color, egui::TextureOptions::LINEAR))
}

/// Square, centre-cropped thumbnail with rounded corners.
fn thumb(ui: &mut Ui, tex: &egui::TextureHandle, selected: bool, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let [w, h] = tex.size();
    let a = w as f32 / h as f32;
    let uv = if a > 1.0 {
        let m = (1.0 - 1.0 / a) / 2.0;
        Rect::from_min_max(egui::pos2(m, 0.0), egui::pos2(1.0 - m, 1.0))
    } else {
        let m = (1.0 - a) / 2.0;
        Rect::from_min_max(egui::pos2(0.0, m), egui::pos2(1.0, 1.0 - m))
    };
    let inner = rect.shrink(if selected { 3.0 } else { 0.0 });
    egui::Image::new((tex.id(), inner.size()))
        .uv(uv)
        .corner_radius(CornerRadius::same(if selected { 5 } else { 7 }))
        .paint_at(ui, inner);
    if selected {
        ui.painter().rect_stroke(rect, CornerRadius::same(8), egui::Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        ui.painter().rect_stroke(rect, CornerRadius::same(7), egui::Stroke::new(1.0, Color32::from_white_alpha(90)), egui::StrokeKind::Inside);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
