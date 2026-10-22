//! Settings panel on the right: a vertical icon tab strip plus a rounded card.

use super::widgets;
use super::timeline::{Selection, Track};
use crate::edit::{ZoomKind, AnimationSpeed, Background, ClickSound, ClickVisual, EditConfig, Mask, MaskKind, MicMode, ScreenAnimation, ShadowStyle, ZoomFocus};
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
use egui::{Color32, CornerRadius, Rect, RichText, Sense, Ui, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Background,
    Cursor,
    CameraLayout,
    CameraStyle,
    Captions,
    Transcript,
    Audio,
    Shortcuts,
    Animation,
}

/// The editor's right rail, in Screen Studio's order (Ctrl+1..9).
pub const TABS: [(Tab, Icon, &str); 9] = [
    (Tab::Background, Icon::RailBackground, "Background & Screen"),
    (Tab::Cursor, Icon::RailCursor, "Cursor"),
    (Tab::CameraLayout, Icon::RailCameraLayout, "Camera Layout"),
    (Tab::CameraStyle, Icon::RailCameraStyle, "Camera Style"),
    (Tab::Captions, Icon::RailCaptions, "Captions"),
    (Tab::Transcript, Icon::RailTranscript, "Transcript"),
    (Tab::Audio, Icon::RailAudio, "Audio"),
    (Tab::Shortcuts, Icon::RailShortcuts, "Keyboard shortcuts"),
    (Tab::Animation, Icon::RailAnimation, "Animations"),
];

/// Tabs with nothing to edit for this recording are shown disabled.
fn tab_enabled(t: Tab, rec: &Recording) -> bool {
    match t {
        Tab::CameraLayout | Tab::CameraStyle => rec.camera.is_some(),
        Tab::Captions | Tab::Transcript => rec.audio.iter().any(|a| a.kind == "microphone"),
        Tab::Shortcuts => !rec.events.keys.is_empty(),
        _ => true,
    }
}

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
    picking_music: Option<Receiver<Vec<std::path::PathBuf>>>,
    pub captions: super::captions_ui::State,
    pub camera: super::camera_ui::State,
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
    selected: &mut Option<Selection>,
    rec: &Recording,
    time: f64,
) -> Option<f64> {
    let mut seek = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        // Icon rail: plain icons, the active one lavender with a small dot.
        ui.vertical(|ui| {
            ui.add_space(6.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            // Ctrl+1..9 switch tabs, unless a text field has focus.
            if ui.memory(|m| m.focused().is_none()) {
                const KEYS: [egui::Key; 9] = [
                    egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5,
                    egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9,
                ];
                for (i, key) in KEYS.iter().enumerate() {
                    if ui.input_mut(|inp| inp.consume_key(egui::Modifiers::COMMAND, *key)) {
                        if let Some((t, ..)) = TABS.get(i).filter(|(t, ..)| tab_enabled(*t, rec)) {
                            *tab = *t;
                            *selected = None;
                        }
                    }
                }
            }
            for (i, (t, icon, name)) in TABS.iter().copied().enumerate() {
                let enabled = tab_enabled(t, rec);
                let active = *tab == t && selected.is_none();
                let (rect, resp) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::click());
                let color = if !enabled {
                    Color32::from_rgb(0x53, 0x53, 0x55)
                } else if active {
                    theme::KNOB
                } else if resp.hovered() {
                    Color32::WHITE
                } else {
                    // Screen Studio's idle icons are white, dimmed slightly
                    // so hover still reads.
                    Color32::from_rgb(0xec, 0xec, 0xef)
                };
                // Icons ~15pt in a 23pt box; active tab gets the lavender dot
                // level with the icon's centre, as in the v4 rail.
                icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(23.0, 23.0)), icon, color);
                if active {
                    ui.painter().circle_filled(pos2(rect.center().x + 13.5, rect.center().y), 2.1, theme::KNOB);
                }
                let tip = format!("{name}  (Ctrl+{})", i + 1);
                let resp = if enabled { resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp.on_hover_text(format!("{name}: {}", disabled_reason(t))) };
                if enabled && resp.clicked() {
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
                    if let Some(sel) = *selected {
                        match sel.track {
                            Track::Zoom if sel.index < edit.zoom_segments.len() => {
                                zoom_segment(ui, edit, sel.index, selected);
                                return;
                            }
                            Track::Mask if sel.index < edit.masks.len() => {
                                seek = mask_editor(ui, edit, sel.index, selected);
                                return;
                            }
                            Track::Camera if sel.index < edit.camera_layouts.len() => {
                                super::camera_ui::segment_editor(ui, edit, sel.index, selected);
                                return;
                            }
                            _ => *selected = None,
                        }
                    }
                    match tab {
                        Tab::Background => background(ui, state, edit),
                        Tab::Cursor => cursor(ui, state, edit, rec),
                        Tab::CameraLayout | Tab::CameraStyle => super::camera_ui::show(ui, &mut state.camera, edit, rec, *tab == Tab::CameraLayout),
                        Tab::Captions => {
                            if let super::captions_ui::CaptionsAction::OpenTranscript = super::captions_ui::captions(ui, &mut state.captions, edit, rec) {
                                *tab = Tab::Transcript;
                            }
                        }
                        Tab::Transcript => {
                            let act = super::captions_ui::transcript(ui, &mut state.captions, edit, rec, time);
                            if act.seek.is_some() {
                                seek = act.seek;
                            }
                            if act.close {
                                *tab = Tab::Captions;
                            }
                        }
                        Tab::Audio => audio(ui, state, edit, rec),
                        Tab::Shortcuts => shortcuts(ui, edit, rec),
                        Tab::Animation => animation(ui, edit, rec, selected),
                    }
                }));
            });
    });
    seek
}

fn disabled_reason(t: Tab) -> &'static str {
    match t {
        Tab::CameraLayout | Tab::CameraStyle => "record with a camera to use this",
        Tab::Captions | Tab::Transcript => "record with a microphone to use this",
        Tab::Shortcuts => "no keyboard shortcuts were recorded",
        _ => "",
    }
}

/// A bordered sub-card for options that belong to the setting above it.
pub(super) fn sub_card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x1b, 0x1d, 0x22))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// Label with an optional help line on the left, switch on the right.
pub(super) fn toggle_help(ui: &mut Ui, label: &str, help_text: &str, on: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 56.0);
            ui.label(RichText::new(label).size(13.5));
            if !help_text.is_empty() {
                ui.label(RichText::new(help_text).size(12.0).color(theme::TEXT_DIM));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = widgets::toggle(ui, on).changed();
        });
    });
    changed
}

pub(super) fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(theme::semibold(15.0)));
    ui.add_space(4.0);
}

pub(super) fn help(ui: &mut Ui, text: &str) {
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
    widgets::divider(ui);
    toggle_help(ui, "Inset", "Add inset around the screen.", &mut edit.inset.enabled);
    if edit.inset.enabled {
        let inset = &mut edit.inset;
        sub_card(ui, |ui| {
            ui.label(RichText::new("Inset size").size(13.5));
            widgets::slider(ui, &mut inset.size, 0.005..=0.12, Some(0.03));
            ui.horizontal(|ui| {
                ui.label(RichText::new("Inset color").size(13.5));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.color_edit_button_rgb(&mut inset.color);
                });
            });
            ui.label(RichText::new(format!("Inset opacity  ·  {:.0}%", inset.opacity * 100.0)).size(13.5));
            widgets::slider(ui, &mut inset.opacity, 0.0..=1.0, Some(1.0));
            ui.label(RichText::new("Inset position").size(13.5));
            position_pad(ui, &mut inset.offset);
        });
    }
    widgets::divider(ui);
    theme::section(ui, &format!("Shadow  ·  {:.0}%", edit.shadow * 100.0));
    widgets::slider(ui, &mut edit.shadow, 0.0..=1.0, Some(0.6));
    widgets::segmented(ui, &mut edit.shadow_style, &[(ShadowStyle::Classic, "Classic"), (ShadowStyle::Directional, "Directional")]);
    if edit.shadow_style == ShadowStyle::Directional {
        sub_card(ui, |ui| {
            ui.label(RichText::new(format!("Angle  ·  {:.0}°", edit.shadow_angle)).size(13.5));
            widgets::slider(ui, &mut edit.shadow_angle, -180.0..=180.0, Some(0.0));
            ui.label(RichText::new(format!("Distance  ·  {:.0}%", edit.shadow_distance * 100.0)).size(13.5));
            widgets::slider(ui, &mut edit.shadow_distance, 0.0..=1.0, Some(0.4));
            ui.label(RichText::new(format!("Softness  ·  {:.0}%", edit.shadow_softness * 100.0)).size(13.5));
            widgets::slider(ui, &mut edit.shadow_softness, 0.0..=1.0, Some(0.6));
        });
    }
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
    let c = &mut edit.cursor;
    theme::section(ui, "Cursor size");
    widgets::slider(ui, &mut c.size, 0.5..=4.0, Some(1.5));
    super::cursors::style_picker(ui, &mut state.cursors, edit);
    let c = &mut edit.cursor;
    toggle_help(ui, "Always use pointer cursor", "Don't change cursor, even if selecting text, etc.", &mut c.always_default);
    widgets::divider(ui);
    toggle_help(ui, "Hide cursor if not moving", "", &mut c.hide_when_idle);
    toggle_help(ui, "Loop cursor position", "Near the end of the video, cursor will move back to its initial position", &mut c.loop_position);
    if c.loop_position {
        sub_card(ui, |ui| {
            ui.label(RichText::new("Loop cursor position duration").size(13.5));
            help(ui, &format!("How long before the end of the recording to reset position of the mouse  ·  {:.1}s", c.loop_duration));
            widgets::slider(ui, &mut c.loop_duration, 0.3..=4.0, Some(1.0));
        });
    }
    toggle_help(ui, "Hide cursor", "", &mut c.hidden);
    widgets::divider(ui);
    theme::section(ui, "Click effect");
    widgets::segmented(ui, &mut c.click_visual, &[(ClickVisual::None, "None"), (ClickVisual::Circle, "Circle"), (ClickVisual::Ripple, "Ripple")]);
    theme::section(ui, "Click sound");
    widgets::segmented(ui, &mut c.click_sound, &[(ClickSound::None, "None"), (ClickSound::MagicMouse, "Magic Mouse"), (ClickSound::MxMaster, "MX Master")]);
    if c.click_sound != ClickSound::None {
        sub_card(ui, |ui| {
            ui.label(RichText::new(format!("Click sound volume  ·  {:.0}%", c.click_volume * 100.0)).size(13.5));
            widgets::slider(ui, &mut c.click_volume, 0.0..=1.0, Some(0.6));
            if widgets::wide_button_icon(ui, Some(Icon::Play), "Preview click sound").clicked() {
                crate::clicks::preview(c.click_sound, c.click_volume);
            }
        });
    }
    widgets::divider(ui);
    egui::CollapsingHeader::new(RichText::new("Advanced").size(13.5)).default_open(false).show(ui, |ui| {
        theme::section(ui, &format!("Rotate cursor while moving  ·  {:.0}%", c.rotate * 100.0));
        widgets::slider(ui, &mut c.rotate, 0.0..=1.0, Some(0.0));
        let label = if c.stop_at_end > 0.0 { format!("{:.1}s", c.stop_at_end) } else { "Off".into() };
        theme::section(ui, &format!("Stop cursor movement at the end  ·  {label}"));
        widgets::slider(ui, &mut c.stop_at_end, 0.0..=3.0, Some(0.0));
        toggle_help(ui, "Remove cursor shakes", "Smooths out small, unintended movements.", &mut c.remove_shakes);
        let mut optimize = !c.always_default;
        if toggle_help(ui, "Optimize original cursor types", "Redraw text, link and resize cursors from what apps showed.", &mut optimize) {
            c.always_default = !optimize;
        }
        widgets::toggle_row(ui, "Click squish animation", &mut c.click_effect);
    });
}

fn animation(ui: &mut Ui, edit: &mut EditConfig, rec: &Recording, selected: &mut Option<Selection>) {
    heading(ui, "Animations");
    theme::section(ui, "Screen animation");
    let before = edit.screen_animation;
    let custom = edit.zoom_curve.is_some_and(|c| c != edit.screen_animation.curve());
    big_choice(ui, &mut edit.screen_animation, &[
        (ScreenAnimation::Focused, Icon::Zoom, "Focused"),
        (ScreenAnimation::Smooth, Icon::RailAnimation, "Smooth"),
    ], custom);
    if edit.screen_animation != before || (edit.zoom_curve.is_none() && !custom) {
        if edit.screen_animation != before {
            edit.zoom_curve = Some(edit.screen_animation.curve());
        }
    }
    help(ui, match edit.screen_animation {
        ScreenAnimation::Focused => "Animation stabilizes quickly making it easier to follow and read the content.",
        ScreenAnimation::Smooth => "Longer, softer camera moves that glide into place.",
    });
    egui::CollapsingHeader::new(RichText::new(if custom { "Custom curve" } else { "Customize curve" }).size(13.5))
        .default_open(custom)
        .show(ui, |ui| super::curve::show(ui, edit));
    widgets::divider(ui);
    theme::section(ui, "Cursor animation");
    let mut style = if !edit.cursor.smooth {
        3
    } else {
        match edit.cursor.speed {
            AnimationSpeed::Slow | AnimationSpeed::Mellow => 0,
            AnimationSpeed::Quick => 1,
            AnimationSpeed::Rapid => 2,
        }
    };
    if widgets::segmented(ui, &mut style, &[(0, "Smooth"), (1, "Medium"), (2, "Rapid"), (3, "None")]) {
        edit.cursor.smooth = style != 3;
        edit.cursor.speed = match style {
            0 => AnimationSpeed::Mellow,
            1 => AnimationSpeed::Quick,
            _ => AnimationSpeed::Rapid,
        };
    }
    widgets::divider(ui);
    let mut blur = if edit.motion_blur { edit.zoom_blur } else { 0.0 };
    theme::section(ui, &format!("Motion blur  ·  {:.0}%", blur * 100.0));
    help(ui, "While mouse cursor or screen is moving, cinematic motion blur effect will be applied");
    if widgets::slider(ui, &mut blur, 0.0..=1.0, Some(0.5)) {
        edit.motion_blur = blur > 0.0;
        edit.zoom_blur = blur;
        edit.pan_blur = blur * 0.5;
    }
    toggle_help(ui, "Instant animation", "Zoom in and out instantly, without animation.", &mut edit.instant_animation);
    widgets::divider(ui);
    theme::section(ui, &format!("Default zoom level  ·  {:.0}%", edit.default_zoom * 100.0));
    widgets::slider(ui, &mut edit.default_zoom, 1.0..=4.0, Some(2.0));
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

/// Large icon chips (Screen Studio's animation style picker).
fn big_choice<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, opts: &[(T, Icon, &str)], none_selected: bool) {
    let w = (ui.available_width() - 8.0 * (opts.len() as f32 - 1.0)) / opts.len() as f32;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for (v, icon, label) in opts {
            let (r, resp) = ui.allocate_exact_size(vec2(w, 74.0), Sense::click());
            let sel = *value == *v && !none_selected;
            let fill = if sel { theme::SURFACE_HOVER } else if resp.hovered() { Color32::from_rgb(0x21, 0x23, 0x28) } else { theme::SURFACE };
            ui.painter().rect_filled(r, CornerRadius::same(10), fill);
            if sel {
                ui.painter().rect_stroke(r, CornerRadius::same(10), egui::Stroke::new(1.0, Color32::from_rgb(0x4f, 0x4d, 0x6f)), egui::StrokeKind::Inside);
            }
            let col = if sel { theme::KNOB } else { theme::TEXT };
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.center().x, r.top() + 27.0), vec2(22.0, 22.0)), *icon, col);
            ui.painter().text(pos2(r.center().x, r.bottom() - 17.0), egui::Align2::CENTER_CENTER, *label, egui::FontId::proportional(13.0), col);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                *value = *v;
            }
        }
    });
}

fn audio(ui: &mut Ui, state: &mut State, edit: &mut EditConfig, rec: &Recording) {
    heading(ui, "Audio");
    let has_mic = rec.audio.iter().any(|a| a.kind == "microphone");
    let has_sys = rec.audio.iter().any(|a| a.kind == "system");
    let a = &mut edit.audio;
    if has_mic {
        theme::section(ui, "Microphone");
        let opts: Vec<_> = MicMode::ALL.iter().map(|m| (*m, m.label())).collect();
        widgets::segmented(ui, &mut a.mic_mode, &opts);
        help(ui, a.mic_mode.help());
        toggle_help(ui, "Mute microphone", "", &mut a.mute_microphone);
        if !a.mute_microphone {
            theme::section(ui, &format!("Microphone volume  ·  {:.0}%", a.microphone_volume * 100.0));
            widgets::slider(ui, &mut a.microphone_volume, 0.0..=2.0, Some(1.0));
        }
        widgets::divider(ui);
    }
    if has_sys {
        toggle_help(ui, "Mute system audio", "Sound the computer played while recording.", &mut a.mute_system);
        if !a.mute_system {
            theme::section(ui, &format!("System audio volume  ·  {:.0}%", a.system_volume * 100.0));
            widgets::slider(ui, &mut a.system_volume, 0.0..=2.0, Some(1.0));
        }
        widgets::divider(ui);
    }
    if !has_mic && !has_sys {
        help(ui, "This recording has no microphone or system audio. Choose a microphone or system audio in the recording bar next time.");
        widgets::divider(ui);
    }
    theme::section(ui, "Background music");
    if let Some(rx) = &state.picking_music {
        match rx.try_recv() {
            Ok(files) => {
                if let Some(f) = files.into_iter().next() {
                    a.music = Some(f.to_string_lossy().into_owned());
                }
                state.picking_music = None;
            }
            Err(TryRecvError::Disconnected) => state.picking_music = None,
            Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)),
        }
    }
    match a.music.clone() {
        None => {
            help(ui, "Your own MP3, WAV, OGG or video file. It loops to fit the video.");
            if widgets::wide_button_icon(ui, Some(Icon::Plus), "Add background music").clicked() && state.picking_music.is_none() {
                let filter = ashpd::desktop::file_chooser::FileFilter::new("Audio")
                    .mimetype("audio/*")
                    .mimetype("video/mp4")
                    .mimetype("video/webm");
                state.picking_music = Some(dialogs::open_files(ui.ctx().clone(), "Add background music", Some(filter), false));
            }
        }
        Some(path) => {
            let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            ui.label(RichText::new(name).size(13.5));
            toggle_help(ui, "Mute background music", "", &mut a.mute_music);
            theme::section(ui, &format!("Background music volume  ·  {:.0}%", a.music_volume * 100.0));
            widgets::slider(ui, &mut a.music_volume, 0.0..=1.0, Some(0.35));
            if widgets::wide_button_icon(ui, Some(Icon::Trash), "Remove background music").clicked() {
                a.music = None;
            }
        }
    }
}

fn shortcuts(ui: &mut Ui, edit: &mut EditConfig, _rec: &Recording) {
    // Laid out as Screen Studio's Keyboard shortcuts panel.
    ui.label(RichText::new("Shortcuts").size(12.5).color(theme::TEXT_DIM));
    ui.add_space(4.0);
    let o = &mut edit.shortcuts;
    toggle_help(ui, "Show shortcuts", "Show pressed shortcut labels in the video.", &mut o.show);
    ui.add_space(8.0);
    theme::section(ui, "Shortcut labels size");
    widgets::slider(ui, &mut o.size, 0.5..=2.0, Some(1.0));
    ui.add_space(8.0);
    toggle_help(
        ui,
        "Show single key shortcuts",
        "By default, only 2+ keys shortcuts are shown. Enable this option to show single letter shortcuts as well.",
        &mut o.single_keys,
    );
    widgets::divider(ui);
    ui.add_space(8.0);
    let label = if o.timeline { "Hide shortcuts timeline" } else { "Show shortcuts timeline" };
    if widgets::wide_button_icon(ui, Some(Icon::Keyboard), label).clicked() {
        o.timeline = !o.timeline;
    }
    if o.timeline {
        help(ui, "Click a shortcut on the timeline to hide or show it. Right-click it to hide or show all of the same shortcut.");
    }
}

fn mask_editor(ui: &mut Ui, edit: &mut EditConfig, i: usize, selected: &mut Option<Selection>) -> Option<f64> {
    let mut seek = None;
    if icons::button(ui, Icon::Back, "Close Mask editor", egui::FontId::proportional(13.5), theme::TEXT, Some(theme::SURFACE), vec2(0.0, 34.0))
        .clicked()
    {
        *selected = None;
        return None;
    }
    ui.add_space(8.0);
    let m = &mut edit.masks[i];
    ui.label(RichText::new("Mask type").size(12.5).color(theme::TEXT_DIM));
    widgets::segmented(ui, &mut m.kind, &[(MaskKind::Blur, "Sensitive Data"), (MaskKind::Highlight, "Highlight")]);
    ui.add_space(4.0);
    if m.kind == MaskKind::Blur {
        let amber = Color32::from_rgb(0xe8, 0xa8, 0x3d);
        egui::Frame::new()
            .fill(Color32::from_rgb(0x2b, 0x22, 0x12))
            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(0x6b, 0x4f, 0x1c)))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Mask, amber);
                ui.label(
                    RichText::new("Make sure the mask covers the entire part of the timeline where sensitive data is present.")
                        .color(amber),
                );
            });
    }
    if widgets::wide_button(ui, "Focus the timeline on the mask start").clicked() {
        seek = Some(m.start);
    }
    widgets::divider(ui);
    if m.kind == MaskKind::Highlight {
        theme::section(ui, "Highlight mask opacity");
        widgets::slider(ui, &mut m.opacity, 0.0..=1.0, Some(Mask::DEFAULT_OPACITY));
        widgets::divider(ui);
    }
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 56.0);
            ui.label(RichText::new("Disable").size(13.5));
            help(ui, "When disabled, the mask will not be applied, but it will still be present in the timeline.");
        });
        widgets::toggle(ui, &mut m.disabled);
    });
    ui.add_space(8.0);
    if icons::button(ui, Icon::Trash, "Remove mask", egui::FontId::proportional(13.5), theme::TEXT, Some(theme::SURFACE), vec2(ui.available_width(), 38.0))
        .clicked()
    {
        edit.masks.remove(i);
        *selected = None;
    }
    seek
}

fn zoom_segment(ui: &mut Ui, edit: &mut EditConfig, i: usize, selected: &mut Option<Selection>) {
    let close = icons::button(ui, Icon::Back, "Close   Esc", egui::FontId::proportional(13.5), theme::TEXT, Some(theme::SURFACE), vec2(0.0, 34.0));
    if close.clicked() {
        *selected = None;
        return;
    }
    ui.add_space(8.0);
    let default_zoom = edit.default_zoom;
    let seg = &mut edit.zoom_segments[i];
    theme::section(ui, "Type");
    if widgets::tabs(ui, &mut seg.kind, &[(ZoomKind::Zoom, "Zoom"), (ZoomKind::Loupe, "Glass Loupe")]) && seg.kind == ZoomKind::Loupe {
        seg.scale = seg.scale.max(1.5);
    }
    if seg.kind == ZoomKind::Loupe {
        help(ui, "Magnify a detail under a glass lens. Viewers see it up close, along with everything around it. Use it where a classic zoom would move too much of your screen.");
        theme::section(ui, "Lens position");
        let mut manual = matches!(seg.focus, ZoomFocus::Manual { .. });
        if widgets::tabs(ui, &mut manual, &[(false, "Follow cursor"), (true, "Manual")]) {
            seg.focus = if manual { ZoomFocus::Manual { x: 0.5, y: 0.5 } } else { ZoomFocus::Auto };
        }
        if manual {
            help(ui, "Click on the preview to place the lens.");
        }
        theme::section(ui, &format!("Magnification  ·  {:.1}×", seg.scale));
        widgets::slider(ui, &mut seg.scale, 1.2..=4.0, Some(2.0));
        theme::section(ui, &format!("Lens size  ·  {:.0}%", seg.size * 100.0));
        widgets::slider(ui, &mut seg.size, 0.4..=2.5, Some(1.0));
        widgets::divider(ui);
        let remove = ui.add(
            egui::Button::new(RichText::new("Remove loupe").size(13.5).color(theme::RECORD))
                .fill(theme::SURFACE)
                .corner_radius(CornerRadius::same(8))
                .min_size(vec2(ui.available_width(), 38.0)),
        );
        if remove.clicked() {
            edit.zoom_segments.remove(i);
            *selected = None;
        }
        return;
    }
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
        for s in edit.zoom_segments.iter_mut().filter(|s| s.kind == ZoomKind::Zoom) {
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

/// 2-D pad with a ring handle (Screen Studio's inset position control).
fn position_pad(ui: &mut Ui, v: &mut [f64; 2]) {
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 110.0), Sense::click_and_drag());
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(10), Color32::from_rgb(0x12, 0x13, 0x17));
    p.rect_stroke(r, CornerRadius::same(10), egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
    p.line_segment([pos2(r.center().x, r.top() + 8.0), pos2(r.center().x, r.bottom() - 8.0)], egui::Stroke::new(1.0, Color32::from_white_alpha(14)));
    p.line_segment([pos2(r.left() + 8.0, r.center().y), pos2(r.right() - 8.0, r.center().y)], egui::Stroke::new(1.0, Color32::from_white_alpha(14)));
    let inner = r.shrink(14.0);
    if let Some(pp) = resp.interact_pointer_pos() {
        v[0] = (((pp.x - inner.center().x) / (inner.width() / 2.0)) as f64).clamp(-1.0, 1.0);
        v[1] = (((pp.y - inner.center().y) / (inner.height() / 2.0)) as f64).clamp(-1.0, 1.0);
    }
    if resp.double_clicked() {
        *v = [0.0, 0.0];
    }
    let h = pos2(inner.center().x + v[0] as f32 * inner.width() / 2.0, inner.center().y + v[1] as f32 * inner.height() / 2.0);
    p.circle_stroke(h, 8.0, egui::Stroke::new(2.0, theme::KNOB));
    p.circle_filled(h, 3.0, theme::KNOB);
    resp.on_hover_cursor(egui::CursorIcon::Grab).on_hover_text("Double-click to centre");
}
