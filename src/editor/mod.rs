//! The editor window: preview canvas, settings sidebar and timeline.

mod audio;
mod camera_ui;
mod captions_ui;
mod cursors;
mod curve;
mod mask_overlay;
pub mod player;
pub use widgets::toggle;
mod sidebar;
mod timeline;
mod widgets;

use crate::edit::{EditConfig, ZoomFocus};
use crate::engine::{Tracks, frame_state};
use crate::dialogs;
use crate::export::{self, ExportOptions, Progress};
use crate::settings::{Destination, Format, Quality, Settings};
use crate::icons::{self, Icon};
use crate::project::{ProjectPaths, Recording};
use crate::render::{Compositor, FORMAT};
use crate::theme;
use anyhow::Result;
use egui::{Color32, CornerRadius, Key, Rect, Sense, Vec2};
use egui_wgpu::wgpu;
use player::Player;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub use sidebar::Tab;
use timeline::{Selection, Track};

struct PreviewTarget {
    _texture: wgpu::Texture,
    /// sRGB view we render into.
    view: wgpu::TextureView,
    size: (u32, u32),
    id: egui::TextureId,
}

struct ExportJob {
    out: PathBuf,
    destination: Destination,
    progress: Arc<Progress>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
}

struct ExportDialog {
    settings: Settings,
    job: Option<ExportJob>,
    /// Folder chooser for the export directory.
    picking_dir: Option<Receiver<Vec<PathBuf>>>,
    /// Save dialog for "Export to file".
    saving: Option<Receiver<Vec<PathBuf>>>,
}

pub enum Action {
    None,
    GoHome,
    /// The project was deleted; leave the editor.
    Deleted,
    /// Close the app (after the unsaved-changes prompt).
    Quit,
}

/// What the user was doing when asked about unsaved changes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Leaving {
    Home,
    Quit,
}

pub struct Editor {
    pub dir: PathBuf,
    name: String,
    rec: Recording,
    edit: EditConfig,
    tracks: Tracks,
    tracks_for: EditConfig,
    saved: EditConfig,
    /// Title being edited (click the project name to rename).
    renaming: Option<String>,
    /// Unsaved-changes prompt.
    leaving: Option<Leaving>,
    saving_as: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
    /// Detected shortcuts by single-key setting, for the shortcuts track.
    shortcuts_cache: Option<(bool, Vec<crate::shortcuts::Shortcut>)>,
    /// Short message under the title ("Saved", errors).
    status: Option<Toast>,
    player: Player,
    /// Camera track player and the frame index on the GPU.
    camera: Option<(Player, i64)>,
    audio: audio::AudioPreview,
    /// Waveform envelope for the clip track (computed in the background).
    waveform: Arc<Mutex<Option<Vec<f32>>>>,
    comp: Compositor,
    target: Option<PreviewTarget>,
    shown_index: i64,
    time: f64,
    playing: Option<(f64, Instant)>,
    selected: Option<Selection>,
    tab: Tab,
    sidebar: sidebar::State,
    timeline: timeline::State,
    export: Option<ExportDialog>,
}

impl Editor {
    pub fn open(dir: PathBuf, ctx: &egui::Context, rs: &egui_wgpu::RenderState) -> Result<Self> {
        let paths = ProjectPaths::new(&dir);
        let rec = Recording::load(&dir)?;
        let edit = EditConfig::load_or_create(&dir, &rec);
        let tracks = Tracks::build(&rec, &edit);
        let player = Player::new(paths.video(), (rec.width, rec.height), rec.fps, ctx.clone());
        let (start, _) = edit.trim_range(rec.duration);
        player.seek(start);
        let audio = audio::AudioPreview::new(&rec, &edit);
        let camera = rec.camera.as_ref().map(|c| {
            let p = Player::new(dir.join(&c.file), (c.width, c.height), c.fps, ctx.clone());
            p.seek((start - c.offset - edit.camera.delay).max(0.0));
            (p, -1)
        });
        Ok(Self {
            name: paths.name(),
            dir,
            tracks,
            tracks_for: edit.clone(),
            saved: edit.clone(),
            renaming: None,
            leaving: None,
            saving_as: None,
            shortcuts_cache: None,
            status: None,
            edit,
            player,
            camera,
            audio,
            waveform: audio::envelope_async(&rec, ctx.clone()),
            comp: Compositor::new(rs.device.clone(), rs.queue.clone()),
            target: None,
            shown_index: -1,
            time: start,
            playing: None,
            selected: None,
            tab: Tab::Background,
            sidebar: sidebar::State::default(),
            timeline: timeline::State::default(),
            export: None,
            rec,
        })
    }

    fn set_playing(&mut self, play: bool) {
        let (start, end) = self.edit.trim_range(self.rec.duration);
        if play {
            if self.time >= end - 0.05 {
                self.time = start;
            }
            let now = Instant::now();
            self.playing = Some((self.time, now));
            self.player.play(self.time, now);
            self.audio.play(self.time);
            self.camera_follow(Some(now));
        } else {
            self.playing = None;
            self.player.pause();
            self.audio.pause();
            self.player.seek(self.time);
            self.camera_follow(None);
        }
    }

    /// Keeps the camera player on the playhead: playing from `now`, or
    /// paused at the current time.
    fn camera_follow(&self, now: Option<Instant>) {
        let (Some((p, _)), Some(c)) = (&self.camera, &self.rec.camera) else { return };
        let t = (self.time - c.offset - self.edit.camera.delay).max(0.0);
        match now {
            Some(at) => p.play(t, at),
            None => {
                p.pause();
                p.seek(t);
            }
        }
    }

    fn seek(&mut self, t: f64) {
        self.time = t.clamp(0.0, self.rec.duration);
        if self.playing.is_some() {
            let now = Instant::now();
            self.playing = Some((self.time, now));
            self.player.play(self.time, now);
            self.audio.play(self.time);
            self.camera_follow(Some(now));
        } else {
            self.player.seek(self.time);
            self.camera_follow(None);
        }
    }

    /// Selects the mask under the playhead, or adds a new one of `kind` there.
    fn add_mask_at_playhead(&mut self, kind: crate::edit::MaskKind) {
        if let Some(i) = self.edit.masks.iter().position(|m| self.time >= m.start && self.time < m.end) {
            self.selected = Some(Selection { track: Track::Mask, index: i });
            return;
        }
        let start = self.time.min(self.rec.duration - 0.3).max(0.0);
        let Some((s, e)) = timeline::free_span_on(&self.edit, Track::Mask, start, 3.0, self.rec.duration) else { return };
        let mut mask = crate::edit::Mask::new(s, e);
        mask.kind = kind;
        let index = timeline::add_mask(&mut self.edit, mask);
        self.selected = Some(Selection { track: Track::Mask, index });
    }

    /// Sets the camera layout under the playhead to `kind`, or adds a layout
    /// of that kind there.
    fn add_layout_at_playhead(&mut self, kind: crate::edit::CameraLayoutKind) {
        if let Some(i) = self.edit.camera_layouts.iter().position(|l| self.time >= l.start && self.time < l.end) {
            self.edit.camera_layouts[i].kind = kind;
            self.selected = Some(Selection { track: Track::Camera, index: i });
            return;
        }
        let start = self.time.min(self.rec.duration - 0.3).max(0.0);
        let Some((s, e)) = timeline::free_span_on(&self.edit, Track::Camera, start, 3.0, self.rec.duration) else { return };
        let index = timeline::add_layout(&mut self.edit, crate::edit::CameraLayoutSegment::new(s, e, kind));
        self.selected = Some(Selection { track: Track::Camera, index });
    }

    /// Selects the zoom under the playhead, or adds a zoom or loupe there.
    fn add_zoom_at_playhead(&mut self, kind: crate::edit::ZoomKind) {
        if let Some(i) = self.edit.zoom_segments.iter().position(|z| self.time >= z.start && self.time < z.end) {
            self.selected = Some(Selection { track: Track::Zoom, index: i });
            return;
        }
        let start = self.time.min(self.rec.duration - 0.3).max(0.0);
        let Some((s, e)) = timeline::free_span_on(&self.edit, Track::Zoom, start, 3.0, self.rec.duration) else { return };
        let scale = if kind == crate::edit::ZoomKind::Loupe { 2.0 } else { self.edit.default_zoom };
        self.edit.zoom_segments.push(crate::edit::ZoomSegment::new(s, e, scale, kind));
        self.edit.zoom_segments.sort_by(|a, b| a.start.total_cmp(&b.start));
        let index = self.edit.zoom_segments.iter().position(|z| z.start == s).unwrap_or(0);
        self.selected = Some(Selection { track: Track::Zoom, index });
    }

    /// Screen Studio's toolbar next to the preview: add zooms, camera layouts
    /// and masks at the playhead.
    fn tool_pill(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        use crate::edit::{MaskKind, ZoomKind};
        let has_camera = self.rec.camera.is_some();
        let groups: [&[(Icon, &str, u8)]; 3] = [
            &[(Icon::ZoomAdd, "Add zoom", 0), (Icon::Loupe, "Add Glass Loupe: magnify a detail under a glass lens", 1)],
            &[
                (Icon::CamPip, "Camera overlay", 10),
                (Icon::CamSplit, "Split screen", 11),
                (Icon::CamCutout, "Camera cutout (needs background removal, not available yet)", 14),
                (Icon::CamFull, "Camera fullscreen", 12),
                (Icon::CamHidden, "Hide camera", 13),
            ],
            &[(Icon::MaskBlur, "Add sensitive data mask", 3), (Icon::MaskHighlight, "Add highlight mask", 2)],
        ];
        // Shrinks to fit short windows.
        let n: usize = groups.iter().map(|g| g.len()).sum();
        let gap = 16.0f32.min(rect.height() * 0.03);
        let step = ((rect.height() - 2.0 * gap - 16.0) / n as f32).clamp(22.0, 42.0);
        let h = n as f32 * step + 2.0 * gap + 16.0;
        let pill = egui::Rect::from_center_size(egui::pos2(rect.left() + 26.0, rect.center().y), egui::vec2(50.0, h));
        let p = ui.painter();
        p.rect_filled(pill, 16.0, egui::Color32::from_rgb(0x1b, 0x1d, 0x22));
        p.rect_stroke(pill, 16.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(0x26, 0x28, 0x2d)), egui::StrokeKind::Inside);
        let mut y = pill.top() + 8.0;
        let mut clicked = None;
        for (gi, g) in groups.iter().enumerate() {
            if gi > 0 {
                y += gap;
            }
            for (icon, tip, action) in g.iter().copied() {
                let r = egui::Rect::from_min_size(egui::pos2(pill.left() + 5.0, y + 1.0), egui::vec2(40.0, step - 2.0));
                let enabled = action < 10 || (has_camera && action != 14);
                let resp = ui.interact(r, ui.scope_id().with(("tool", tip)), egui::Sense::click());
                if enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 10.0, egui::Color32::from_white_alpha(14));
                }
                let col = if enabled { egui::Color32::WHITE } else { egui::Color32::from_rgb(0x53, 0x53, 0x55) };
                let size = (step * 0.52).clamp(14.0, 21.0);
                icons::paint(ui.painter(), egui::Rect::from_center_size(r.center(), egui::vec2(size, size)), icon, col);
                let resp = if enabled {
                    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
                } else {
                    resp.on_hover_text(if action == 14 { tip.to_string() } else { format!("{tip}: record with a camera to use this") })
                };
                if enabled && resp.clicked() {
                    clicked = Some(action);
                }
                y += step;
            }
        }
        match clicked {
            Some(0) => self.add_zoom_at_playhead(ZoomKind::Zoom),
            Some(1) => self.add_zoom_at_playhead(ZoomKind::Loupe),
            Some(2) => self.add_mask_at_playhead(MaskKind::Highlight),
            Some(3) => self.add_mask_at_playhead(MaskKind::Blur),
            Some(10) => self.add_layout_at_playhead(crate::edit::CameraLayoutKind::Default),
            Some(11) => self.add_layout_at_playhead(crate::edit::CameraLayoutKind::Split),
            Some(12) => self.add_layout_at_playhead(crate::edit::CameraLayoutKind::Fullscreen),
            Some(13) => self.add_layout_at_playhead(crate::edit::CameraLayoutKind::Hide),
            _ => {}
        }
    }

    /// Project name in the top bar; click it to rename. "Edited" shows while
    /// there are unsaved changes.
    fn title(&mut self, ui: &mut egui::Ui) {
        // The name is edited in place: same font, no frame, ".openstudio"
        // still beside it, the row staying centred while you type.
        let font = egui::FontId::proportional(14.0);
        let small = egui::FontId::proportional(12.5);
        let note = self.has_unsaved_changes().then(|| if self.is_new() { "   Not saved".to_string() } else { "   Edited".to_string() });
        let name = self.renaming.clone().unwrap_or_else(|| self.name.clone());
        // Every part is laid out at a fixed spot on one shared top line, so
        // switching to editing moves nothing.
        let galley = |text: &str, f: &egui::FontId, c: egui::Color32| ui.painter().layout_no_wrap(text.to_string(), f.clone(), c);
        let g_name = galley(&name, &font, theme::TEXT);
        let g_ext = galley(".openstudio", &font, theme::TEXT_DIM);
        let g_note = note.as_deref().map(|n| galley(n, &small, theme::TEXT_DIM));
        let name_w = g_name.size().x.max(8.0) + 1.0;
        let total = name_w + g_ext.size().x + g_note.as_ref().map_or(0.0, |g| g.size().x);
        let full = ui.max_rect();
        let top = full.center().y - g_ext.size().y / 2.0;
        let x0 = full.center().x - total / 2.0;
        let name_rect = egui::Rect::from_min_size(egui::pos2(x0, top), egui::vec2(name_w, g_ext.size().y));
        let ext_pos = egui::pos2(x0 + name_w, top);
        if let Some(text) = &mut self.renaming {
            let id = egui::Id::NULL.with("rename-project");
            let mut out = egui::TextEdit::singleline(text)
                .id(id)
                .font(font.clone())
                .text_color(theme::TEXT)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .min_size(name_rect.size())
                .desired_width(name_w)
                .show(&mut ui.new_child(egui::UiBuilder::new().max_rect(name_rect)));
            let r = out.response.response.clone();
            if !r.has_focus() && !r.lost_focus() {
                // First frame: focus and select the whole name.
                r.request_focus();
                let len = text.chars().count();
                out.state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(len))));
                out.state.store(ui.ctx(), id);
            }
            let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if esc {
                self.renaming = None;
            } else if enter || r.lost_focus() {
                let name = self.renaming.take().unwrap_or_default();
                if let Err(e) = self.rename(&name) {
                    self.notify(format!("Couldn't rename: {e:#}"));
                }
            }
        } else {
            ui.painter().galley(name_rect.min, g_name, theme::TEXT);
            let r = ui
                .interact(name_rect.union(egui::Rect::from_min_size(ext_pos, g_ext.size())), egui::Id::NULL.with("project-title"), Sense::click())
                .on_hover_text("Click to rename.  Ctrl+S saves, Ctrl+Shift+S saves to another folder.")
                .on_hover_cursor(egui::CursorIcon::Text);
            if r.clicked() {
                self.renaming = Some(self.name.clone());
            }
        }
        let ext_w = g_ext.size().x;
        ui.painter().galley(ext_pos, g_ext, theme::TEXT_DIM);
        if let Some(g) = g_note {
            // Smaller text, centred on the same line.
            let y = full.center().y - g.size().y / 2.0;
            ui.painter().galley(egui::pos2(ext_pos.x + ext_w, y), g, theme::TEXT_DIM);
        }
    }

    /// A new recording that hasn't been saved yet.
    pub fn is_new(&self) -> bool {
        self.dir.starts_with(crate::util::unsaved_dir())
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.is_new() || self.edit != self.saved
    }

    fn set_dir(&mut self, dir: PathBuf) {
        self.rec.dir = dir.clone();
        self.dir = dir;
    }

    /// Shows a toast at the top of the window. `msg` is "Title" or
    /// "Title: detail"; errors get a red mark.
    fn notify(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        let error = msg.starts_with("Saving failed") || msg.starts_with("Couldn't");
        let (title, detail) = match msg.split_once(": ") {
            Some((t, d)) => (t.to_string(), d.to_string()),
            None => (msg, String::new()),
        };
        self.status = Some(Toast { title, detail, error, folder: None, at: Instant::now() });
    }

    fn notify_saved(&mut self) {
        self.status = Some(Toast {
            title: "Project saved".into(),
            detail: tilde(&self.dir),
            error: false,
            folder: Some(self.dir.clone()),
            at: Instant::now(),
        });
    }

    /// The toast: slides down from under the top bar, stays a few seconds,
    /// fades out. Click to dismiss.
    fn toast(&mut self, ctx: &egui::Context) {
        const SHOW: f32 = 3.5;
        const ANIM: f32 = 0.22;
        let Some(t) = &self.status else { return };
        let age = t.at.elapsed().as_secs_f32();
        let life = if t.error { SHOW + 2.0 } else { SHOW };
        if age > life {
            self.status = None;
            return;
        }
        let a = (age / ANIM).min(((life - age) / ANIM).min(1.0)).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - a).powi(3);
        let mut dismiss = false;
        let mut open = None;
        egui::Area::new(egui::Id::NULL.with("toast"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 58.0 - 12.0 * (1.0 - ease)))
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_opacity(ease);
                let frame = egui::Frame::new()
                    .fill(Color32::from_rgb(0x1e, 0x20, 0x26))
                    .stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(22)))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin { left: 14, right: 16, top: 10, bottom: 10 })
                    .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(140) });
                let resp = frame
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            // Mark: a check in a lavender disc, or "!" in red.
                            let (r, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::hover());
                            let p = ui.painter();
                            if t.error {
                                p.circle_filled(r.center(), 11.0, theme::RECORD);
                                p.text(r.center(), egui::Align2::CENTER_CENTER, "!", theme::semibold(14.0), Color32::WHITE);
                            } else {
                                p.circle_filled(r.center(), 11.0, theme::ACCENT);
                                let c = r.center();
                                p.line(
                                    vec![c + egui::vec2(-4.5, 0.2), c + egui::vec2(-1.2, 3.4), c + egui::vec2(4.8, -3.4)],
                                    egui::Stroke::new(2.0, Color32::WHITE),
                                );
                            }
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(egui::RichText::new(&t.title).font(theme::semibold(13.5)).color(theme::TEXT));
                                if !t.detail.is_empty() {
                                    // Long paths are shortened from the left so the name stays visible.
                                    let max = 62;
                                    let d = if t.detail.chars().count() > max {
                                        format!("…{}", t.detail.chars().rev().take(max - 1).collect::<Vec<_>>().into_iter().rev().collect::<String>())
                                    } else {
                                        t.detail.clone()
                                    };
                                    ui.label(egui::RichText::new(d).size(12.0).color(theme::TEXT_DIM));
                                }
                            });
                            if let Some(f) = &t.folder {
                                ui.add_space(6.0);
                                let b = egui::Button::new(egui::RichText::new("Show in folder").size(12.5).color(theme::TEXT))
                                    .fill(Color32::from_white_alpha(14))
                                    .corner_radius(egui::CornerRadius::same(7));
                                if ui.add(b).clicked() {
                                    open = Some(f.clone());
                                }
                            }
                        });
                    })
                    .response;
                if resp.interact(Sense::click()).clicked() {
                    dismiss = true;
                }
            });
        if let Some(f) = open {
            let _ = std::process::Command::new("xdg-open").arg(&f).spawn();
            dismiss = true;
        }
        if dismiss {
            self.status = None;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    /// Ctrl+S: writes the project; a new recording moves into the projects
    /// folder under its name.
    pub fn save(&mut self) -> anyhow::Result<()> {
        if self.is_new() {
            let target = crate::util::unique_path(&crate::util::projects_dir(), &self.name);
            crate::util::move_dir(&self.dir, &target)?;
            self.set_dir(target);
        }
        self.edit.save(&self.dir)?;
        self.saved = self.edit.clone();
        Ok(())
    }

    /// Ctrl+Shift+S: saves into `folder`. A saved project is copied there and
    /// the editor continues on the copy.
    fn save_as(&mut self, folder: &std::path::Path) -> anyhow::Result<()> {
        let target = crate::util::unique_path(folder, &self.name);
        if self.is_new() {
            crate::util::move_dir(&self.dir, &target)?;
        } else {
            crate::util::copy_dir(&self.dir, &target)?;
        }
        self.set_dir(target);
        self.edit.save(&self.dir)?;
        self.saved = self.edit.clone();
        Ok(())
    }

    /// Renames the project; a saved project's folder is renamed with it.
    fn rename(&mut self, name: &str) -> anyhow::Result<()> {
        let name = name.trim().replace('/', "-");
        if name.is_empty() || name == self.name {
            return Ok(());
        }
        if !self.is_new() {
            let target = self.dir.with_file_name(&name);
            if target.exists() {
                anyhow::bail!("a project named \"{name}\" already exists");
            }
            std::fs::rename(&self.dir, &target)?;
            self.set_dir(target);
        }
        self.name = name;
        Ok(())
    }

    /// Leaves the editor, asking first when there are unsaved changes.
    pub fn request_leave(&mut self, why: Leaving) -> Action {
        if self.has_unsaved_changes() {
            self.leaving = Some(why);
            Action::None
        } else {
            leave_action(why)
        }
    }

    fn save_with_feedback(&mut self) {
        match self.save() {
            Ok(()) => self.notify_saved(),
            Err(e) => self.notify(format!("Saving failed: {e:#}")),
        }
    }

    /// The unsaved-changes prompt. Screen Studio asks whether to save or
    /// delete a new recording; for a saved project, whether to save changes.
    fn leave_prompt(&mut self, ctx: &egui::Context) -> Action {
        let Some(why) = self.leaving else { return Action::None };
        let mut out = Action::None;
        let new = self.is_new();
        // Drawn like the macOS alert Screen Studio shows: app icon, centred
        // bold title and message, full-width stacked buttons, default on top.
        let frame = egui::Frame::new()
            .fill(egui::Color32::from_rgb(0x26, 0x26, 0x29))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(22)))
            .corner_radius(egui::CornerRadius::same(16))
            .inner_margin(egui::Margin { left: 22, right: 22, top: 24, bottom: 20 })
            .shadow(egui::epaint::Shadow { offset: [0, 18], blur: 50, spread: 0, color: egui::Color32::from_black_alpha(150) });
        egui::Modal::new(egui::Id::NULL.with("leave-prompt"))
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(110))
            .show(ctx, |ui| {
                let w = 248.0;
                ui.set_width(w);
                ui.vertical_centered(|ui| {
                    // App icon: a lavender record ring on an indigo squircle.
                    let (r, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(r, egui::CornerRadius::same(14), egui::Color32::from_rgb(0x1d, 0x16, 0x3f));
                    p.rect_stroke(r, egui::CornerRadius::same(14), egui::Stroke::new(1.0, egui::Color32::from_white_alpha(30)), egui::StrokeKind::Inside);
                    p.circle_stroke(r.center(), 15.0, egui::Stroke::new(5.0, theme::KNOB));
                    p.circle_filled(r.center(), 6.0, theme::ACCENT);
                    ui.add_space(14.0);
                    let title = if new { "Save this recording?".to_string() } else { format!("Save changes to \u{201c}{}\u{201d}?", self.name) };
                    ui.label(egui::RichText::new(title).font(theme::semibold(14.0)).color(egui::Color32::WHITE));
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(if new {
                            "This recording hasn't been saved. If you don't save it, it will be deleted."
                        } else {
                            "Your changes will be lost if you don't save them."
                        })
                        .size(12.0)
                        .color(egui::Color32::from_rgb(0xb4, 0xb4, 0xb9)),
                    );
                    if let Some(Toast { title, detail, .. }) = &self.status {
                        let m = format!("{title}: {detail}");
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(m).size(11.5).color(theme::RECORD));
                    }
                    ui.add_space(18.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    let button = |ui: &mut egui::Ui, label: &str, fill: egui::Color32, text: egui::Color32| {
                        ui.add_sized(
                            [w, 30.0],
                            egui::Button::new(egui::RichText::new(label).size(13.0).color(text))
                                .fill(fill)
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(egui::CornerRadius::same(8)),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                    };
                    let grey = egui::Color32::from_rgb(0x3a, 0x3a, 0x3e);
                    if button(ui, "Save", theme::ACCENT, egui::Color32::WHITE).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        match self.save() {
                            Ok(()) => {
                                self.leaving = None;
                                out = leave_action(why);
                            }
                            Err(e) => self.notify(format!("Saving failed: {e:#}")),
                        }
                    }
                    let discard = if new { "Delete" } else { "Don't Save" };
                    if button(ui, discard, grey, egui::Color32::from_rgb(0xff, 0x6b, 0x61)).clicked() {
                        if new {
                            let _ = trash(&self.dir);
                        }
                        self.edit = self.saved.clone();
                        self.leaving = None;
                        out = leave_action(why);
                    }
                    if button(ui, "Cancel", grey, egui::Color32::WHITE).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        self.leaving = None;
                    }
                });
            });
        out
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, rs: &egui_wgpu::RenderState) -> Action {
        let ctx = ui.ctx().clone();
        let mut action = Action::None;

        // Clock.
        if let Some((from, at)) = self.playing {
            let (_, end) = self.edit.trim_range(self.rec.duration);
            self.time = from + at.elapsed().as_secs_f64();
            if self.time >= end {
                self.time = end;
                self.set_playing(false);
            } else if let Some(c) = self.edit.cut_at(self.time) {
                // Jump over cut stretches.
                self.seek(c.end.min(end));
            }
            ctx.request_repaint();
        }

        self.handle_keys(&ctx);
        // Track set changed (music, mutes, click sounds): rebuild the player.
        if !self.audio.is_current(&self.edit) {
            self.audio = audio::AudioPreview::new(&self.rec, &self.edit);
            if self.playing.is_some() {
                self.audio.play(self.time);
            }
        }
        self.audio.set_muted(self.edit.audio.mute);

        egui::Panel::top("topbar")
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if icons::button(ui, Icon::Folder, "", egui::FontId::proportional(15.0), theme::TEXT, None, Vec2::ZERO)
                        .on_hover_text("Projects")
                        .clicked()
                    {
                        action = self.request_leave(Leaving::Home);
                    }
                    ui.add_space(4.0);
                    if icons::button(ui, Icon::Trash, "", egui::FontId::proportional(15.0), theme::TEXT, None, Vec2::ZERO)
                        .on_hover_text("Delete project (moves it to the trash)")
                        .clicked()
                    {
                        match trash(&self.dir) {
                            Ok(()) => action = Action::Deleted,
                            Err(e) => eprintln!("open-studio: deleting project failed: {e:#}"),
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::button(ui, Icon::Export, "Export", theme::semibold(13.5), Color32::WHITE, Some(theme::ACCENT), egui::vec2(0.0, 32.0)).clicked()
                            && self.export.is_none()
                        {
                            self.export = Some(ExportDialog { settings: Settings::load(), job: None, picking_dir: None, saving: None });
                        }
                        ui.centered_and_justified(|ui| self.title(ui));
                    });
                });
            });

        // Detected shortcuts for the timeline track, recomputed only when
        // the single-key setting changes.
        let shortcuts = if self.edit.shortcuts.timeline {
            let single = self.edit.shortcuts.single_keys;
            if self.shortcuts_cache.as_ref().is_none_or(|(s, _)| *s != single) {
                self.shortcuts_cache = Some((single, crate::shortcuts::detect(&self.rec, single)));
            }
            self.shortcuts_cache.as_ref().map(|(_, l)| l.clone()).unwrap_or_default()
        } else {
            vec![]
        };
        egui::Panel::bottom("timeline")
            .resizable(false)
            .exact_size(
                if self.rec.camera.is_some() { 348.0 } else { 278.0 }
                    + if self.edit.shortcuts.timeline && !self.rec.events.keys.is_empty() { 44.0 } else { 0.0 },
            )
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left: 16, right: 16, top: 6, bottom: 14 }))
            .show(ui, |ui| {
                let out = timeline::show(
                    ui,
                    &mut self.timeline,
                    self.rec.duration,
                    &mut self.edit,
                    self.time,
                    self.playing.is_some(),
                    &mut self.selected,
                    self.waveform.lock().unwrap().as_deref(),
                    self.rec.camera.is_some(),
                    &shortcuts,
                );
                if let Some(t) = out.seek {
                    self.seek(t);
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left: 24, right: 16, top: 8, bottom: 0 }))
            .show(ui, |ui| {
                // Settings column on the right, preview + transport on the left.
                let full = ui.available_rect_before_wrap();
                let side_w = 404.0;
                let side_rect = egui::Rect::from_min_max(egui::pos2(full.max.x - side_w, full.min.y + 4.0), full.max - egui::vec2(0.0, 8.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(side_rect), |ui| {
                    self.sidebar.camera.frame = self.camera.as_ref().and_then(|(p, _)| p.latest());
                    if std::mem::take(&mut self.sidebar.camera.face_ready) {
                        self.tracks = Tracks::build(&self.rec, &self.edit);
                    }
                    if let Some(t) = sidebar::show(ui, &mut self.sidebar, &mut self.edit, &mut self.tab, &mut self.selected, &self.rec, self.time) {
                        self.seek(t);
                    }
                    if std::mem::take(&mut self.sidebar.captions.play_request) && self.playing.is_none() {
                        self.set_playing(true);
                    }
                    // A transcript just finished generating.
                    if let Some(t) = self.sidebar.captions.finished.take() {
                        self.rec.transcript = Some(t);
                        self.tracks = Tracks::build(&self.rec, &self.edit);
                    }
                });
                let main_rect = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x - side_w - 16.0, full.max.y));
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(main_rect).layout(egui::Layout::top_down(egui::Align::Min)));
                ui.horizontal(|ui| {
                    let label = format!("Aspect  {}", self.edit.aspect.label());
                    let resp = icons::button(ui, Icon::Aspect, &label, egui::FontId::proportional(13.5), theme::TEXT, Some(theme::SURFACE), egui::vec2(0.0, 30.0));
                    egui::Popup::menu(&resp).show(|ui| {
                        for a in crate::edit::Aspect::ALL {
                            if ui.selectable_label(self.edit.aspect == a, a.label()).clicked() {
                                self.edit.aspect = a;
                                ui.close();
                            }
                        }
                    });
                });
                let bar_h = 48.0;
                let avail = ui.available_rect_before_wrap();
                // Tool pill on the left, preview to its right.
                let pill_rect = egui::Rect::from_min_max(avail.min, egui::pos2(avail.min.x + 56.0, avail.max.y - bar_h));
                self.tool_pill(ui, pill_rect);
                let preview_rect = egui::Rect::from_min_max(egui::pos2(avail.min.x + 68.0, avail.min.y), egui::pos2(avail.max.x, avail.max.y - bar_h));
                ui.scope_builder(egui::UiBuilder::new().max_rect(preview_rect), |ui| self.preview(ui, rs));
                let bar_rect = egui::Rect::from_min_max(egui::pos2(avail.min.x, avail.max.y - bar_h), avail.max);
                ui.scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
                    let out = timeline::playback_bar(ui, self.time, self.rec.duration, self.playing.is_some());
                    if out.toggle_play {
                        self.set_playing(self.playing.is_none());
                    }
                    if let Some(t) = out.seek {
                        let (s, e) = self.edit.trim_range(self.rec.duration);
                        self.seek(t.clamp(s, e));
                    }
                });
            });

        self.export_dialog(&ctx);

        // Rebuild motion tracks when edits change. Nothing is written to
        // disk until the project is saved.
        if self.edit != self.tracks_for {
            if self.edit.camera.delay != self.tracks_for.camera.delay && self.playing.is_none() {
                self.camera_follow(None);
            }
            self.tracks = Tracks::build(&self.rec, &self.edit);
            self.tracks_for = self.edit.clone();
        }
        // Save As folder chosen.
        if let Some(rx) = &self.saving_as {
            match rx.try_recv() {
                Ok(folders) => {
                    self.saving_as = None;
                    if let Some(f) = folders.into_iter().next() {
                        match self.save_as(&f) {
                            Ok(()) => self.notify_saved(),
                            Err(e) => self.notify(format!("Saving failed: {e:#}")),
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.saving_as = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            }
        }
        if let Action::None = action {
            action = self.leave_prompt(&ctx);
        }
        self.toast(&ctx);
        action
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Ctrl+S saves, Ctrl+Shift+S asks where.
        let save_as = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::S));
        let save = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::S));
        if save_as && self.saving_as.is_none() {
            self.saving_as = Some(crate::dialogs::open_folder(ctx.clone(), "Save project to", Some(crate::util::projects_dir())));
        } else if save {
            self.save_with_feedback();
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (space, del, left, right, esc) = ctx.input(|i| {
            (
                i.key_pressed(Key::Space),
                i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::Escape),
            )
        });
        if space {
            self.set_playing(self.playing.is_none());
        }
        if del {
            if let Some(sel) = self.selected.take() {
                match sel.track {
                    Track::Zoom if sel.index < self.edit.zoom_segments.len() => {
                        self.edit.zoom_segments.remove(sel.index);
                    }
                    Track::Mask if sel.index < self.edit.masks.len() => {
                        self.edit.masks.remove(sel.index);
                    }
                    Track::Camera if sel.index < self.edit.camera_layouts.len() => {
                        self.edit.camera_layouts.remove(sel.index);
                    }
                    _ => {}
                }
            }
        }
        let step = 1.0 / self.rec.fps as f64;
        if left {
            self.seek(self.time - step);
        }
        if right {
            self.seek(self.time + step);
        }
        if esc {
            self.selected = None;
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui, rs: &egui_wgpu::RenderState) {
        let avail = ui.available_rect_before_wrap();
        let aspect = self.tracks.layout.aspect as f32;
        let mut size = avail.size();
        if size.x / size.y > aspect {
            size.x = size.y * aspect;
        } else {
            size.y = size.x / aspect;
        }
        let rect = Rect::from_center_size(avail.center(), size);
        let ppp = ui.ctx().pixels_per_point();
        let px = (((size.x * ppp) as u32).max(2), ((size.y * ppp) as u32).max(2));

        if let Some((p, shown)) = &mut self.camera {
            if let Some(f) = p.latest() {
                if f.index != *shown {
                    self.comp.upload_camera_frame(&f.rgba, f.width, f.height);
                    *shown = f.index;
                }
            }
        }
        if let Some(f) = self.player.latest() {
            if f.index != self.shown_index {
                self.comp.upload_frame(&f.rgba, f.width, f.height);
                self.shown_index = f.index;
            }
        }
        if !self.comp.has_frame() {
            ui.put(rect, egui::Spinner::new());
            return;
        }
        self.ensure_target(px, rs);
        let target = self.target.as_ref().unwrap();
        let fs = frame_state(&self.rec, &self.edit, &self.tracks, self.time, 60.0);
        self.comp.render(&target.view, target.size, &self.tracks.layout, &self.edit, &fs);

        let resp = ui.allocate_rect(rect, Sense::click());
        ui.painter().image(
            target.id,
            rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let zoom_sel = self.selected.filter(|s| s.track == Track::Zoom).map(|s| s.index);
        if let Some(Selection { track: Track::Mask, index }) = self.selected {
            if let Some(m) = self.edit.masks.get_mut(index) {
                let id = ui.scope_id().with(("mask-edit", index));
                mask_overlay::edit(ui, rect, &self.tracks.layout, fs.camera, id, &mut m.rect);
            }
        }
        // Clicking the preview sets the focus point of a manual zoom.
        if let (Some(i), Some(pos)) = (zoom_sel, resp.interact_pointer_pos()) {
            if resp.clicked() {
                if let Some(seg) = self.edit.zoom_segments.get_mut(i) {
                    if matches!(seg.focus, ZoomFocus::Manual { .. }) {
                        let l = &self.tracks.layout;
                        let cam = fs.camera;
                        let u = ((pos - rect.min) / rect.height()).to_pos2();
                        let qx = cam.cx + (u.x as f64 - l.aspect * 0.5) / cam.scale;
                        let qy = cam.cy + (u.y as f64 - 0.5) / cam.scale;
                        let x = ((qx - l.content.x) / l.content.w).clamp(0.0, 1.0);
                        let y = ((qy - l.content.y) / l.content.h).clamp(0.0, 1.0);
                        seg.focus = ZoomFocus::Manual { x, y };
                    }
                }
            }
        }
        if let Some(ZoomFocus::Manual { x, y }) = zoom_sel.and_then(|i| self.edit.zoom_segments.get(i)).map(|s| s.focus)
        {
            // Show the focus point while the camera is at rest (unzoomed).
            if (fs.camera.scale - 1.0).abs() < 0.02 {
                let l = &self.tracks.layout;
                let p = rect.min
                    + Vec2::new(
                        ((l.content.x + x * l.content.w) as f32) * rect.height(),
                        ((l.content.y + y * l.content.h) as f32) * rect.height(),
                    );
                ui.painter().circle(p, 9.0, theme::ZOOM.gamma_multiply(0.5), egui::Stroke::new(2.0, Color32::WHITE));
            }
        }
    }

    fn ensure_target(&mut self, size: (u32, u32), rs: &egui_wgpu::RenderState) {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let texture = rs.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // egui samples textures as gamma-encoded Rgba8Unorm; we render
            // through an sRGB view of the same memory so blending stays linear.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[FORMAT],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor { format: Some(FORMAT), ..Default::default() });
        let egui_view = texture.create_view(&Default::default());
        let mut renderer = rs.renderer.write();
        let id = match self.target.take() {
            Some(old) => {
                renderer.update_egui_texture_from_wgpu_texture(&rs.device, &egui_view, wgpu::FilterMode::Linear, old.id);
                old.id
            }
            None => renderer.register_native_texture(&rs.device, &egui_view, wgpu::FilterMode::Linear),
        };
        self.target = Some(PreviewTarget { _texture: texture, view, size, id });
    }

    fn export_dialog(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.export else { return };
        let native = self.rec.height.clamp(720, 2160);
        let mut close = false;
        let mut start: Option<PathBuf> = None;
        let height = dlg.settings.height.unwrap_or(native);

        // Folder chooser and save dialog results.
        if let Some(rx) = &dlg.picking_dir {
            match rx.try_recv() {
                Ok(dirs) => {
                    if let Some(d) = dirs.into_iter().next() {
                        dlg.settings.export_dir = d;
                        dlg.settings.save();
                    }
                    dlg.picking_dir = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => dlg.picking_dir = None,
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            }
        }
        if let Some(rx) = &dlg.saving {
            match rx.try_recv() {
                Ok(files) => {
                    dlg.saving = None;
                    if let Some(mut f) = files.into_iter().next() {
                        let ext = if dlg.settings.format == Format::Gif { "gif" } else { "mp4" };
                        if f.extension().is_none_or(|e| !e.eq_ignore_ascii_case(ext)) {
                            f.set_extension(ext);
                        }
                        if let Some(parent) = f.parent() {
                            dlg.settings.export_dir = parent.to_path_buf();
                        }
                        start = Some(f);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => dlg.saving = None,
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            }
        }

        #[allow(deprecated)]
        let modal_id = egui::Id::new("export");
        let modal = egui::Modal::new(modal_id).frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(egui::Margin::same(36)),
        );
        modal.show(ctx, |ui| {
            ui.set_width(640.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.label(egui::RichText::new("Export").font(theme::semibold(20.0)));
            ui.add_space(14.0);
            match &dlg.job {
                None => {
                    let s = &mut dlg.settings;
                    let col_w = (ui.available_width() - 32.0) / 2.0;
                    let header = |ui: &mut egui::Ui, icon: Icon, text: &str| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            let (r, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::hover());
                            icons::paint(ui.painter(), r, icon, theme::TEXT);
                            ui.label(egui::RichText::new(text).size(15.0));
                        });
                    };
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 32.0;
                        ui.vertical(|ui| {
                            ui.set_width(col_w);
                            header(ui, Icon::Video, "Format");
                            widgets::segmented(ui, &mut s.format, &[(Format::Mp4, "MP4"), (Format::Gif, "GIF")]);
                        });
                        ui.vertical(|ui| {
                            ui.set_width(col_w);
                            header(ui, Icon::Aperture, "Frame rate");
                            let opts: Vec<(u32, &str)> = if s.format == Format::Gif {
                                vec![(15, "15 FPS"), (24, "24 FPS"), (30, "30 FPS")]
                            } else {
                                vec![(24, "24 FPS"), (30, "30 FPS"), (60, "60 FPS")]
                            };
                            if !opts.iter().any(|(f, _)| *f == s.fps) {
                                s.fps = opts.last().unwrap().0;
                            }
                            widgets::segmented(ui, &mut s.fps, &opts);
                        });
                    });
                    ui.add_space(18.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 32.0;
                        ui.vertical(|ui| {
                            ui.set_width(col_w);
                            header(ui, Icon::Resolution, "Resolution");
                            let mut h = height;
                            let mut opts: Vec<(u32, &str)> = vec![(720, "720p"), (1080, "1080p"), (1440, "1440p"), (2160, "4K")];
                            if !opts.iter().any(|(x, _)| *x == native) {
                                opts.push((native, "Original"));
                            }
                            if widgets::segmented(ui, &mut h, &opts) {
                                s.height = Some(h);
                            }
                            let (w, hh) = export::output_size(self.tracks.layout.aspect, h);
                            ui.label(egui::RichText::new(format!("{w}px × {hh}px")).color(theme::TEXT_DIM));
                        });
                        ui.vertical(|ui| {
                            ui.set_width(col_w);
                            header(ui, Icon::Film, "Quality");
                            if s.format == Format::Gif {
                                ui.label(
                                    egui::RichText::new("GIFs use an optimised 256-colour palette. Keep them short; they get large quickly.")
                                        .color(theme::TEXT_DIM),
                                );
                            } else {
                                let opts: Vec<(Quality, &str)> = Quality::ALL.iter().map(|q| (*q, q.label())).collect();
                                widgets::segmented(ui, &mut s.quality, &opts);
                                ui.label(egui::RichText::new(s.quality.description()).color(theme::TEXT_DIM));
                            }
                        });
                    });
                    ui.add_space(26.0);
                    ui.label(egui::RichText::new("Export to").size(15.0));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let dest_chip = |ui: &mut egui::Ui, d: Destination, icon: Icon, label: &str, cur: &mut Destination| {
                            let fill = if *cur == d { theme::SURFACE_HOVER } else { theme::SURFACE };
                            let r = icons::button(ui, icon, label, egui::FontId::proportional(14.0), theme::TEXT, Some(fill), egui::vec2(0.0, 42.0));
                            if *cur == d {
                                ui.painter().rect_stroke(r.rect, CornerRadius::same(8), egui::Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3c, 0x44)), egui::StrokeKind::Inside);
                            }
                            if r.clicked() {
                                *cur = d;
                            }
                        };
                        dest_chip(ui, Destination::File, Icon::File, "File", &mut s.destination);
                        dest_chip(ui, Destination::Clipboard, Icon::Clipboard, "Clipboard", &mut s.destination);
                        let folder = icons::button(ui, Icon::Folder, "", egui::FontId::proportional(14.0), theme::TEXT, Some(theme::SURFACE), egui::vec2(42.0, 42.0))
                            .on_hover_text("Choose export folder");
                        if folder.clicked() && dlg.picking_dir.is_none() {
                            dlg.picking_dir = Some(dialogs::open_folder(ctx.clone(), "Choose export folder", Some(s.export_dir.clone())));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (icon, label) = match s.destination {
                                Destination::File => (Icon::Export, "Export to file"),
                                Destination::Clipboard => (Icon::Clipboard, "Export to clipboard"),
                            };
                            let go = icons::button(ui, icon, label, theme::semibold(14.0), Color32::WHITE, Some(theme::ACCENT), egui::vec2(0.0, 42.0));
                            if go.clicked() && dlg.saving.is_none() {
                                let ext = if s.format == Format::Gif { "gif" } else { "mp4" };
                                let name = format!("{}.{ext}", self.name);
                                match s.destination {
                                    Destination::File => {
                                        let _ = std::fs::create_dir_all(&s.export_dir);
                                        dlg.saving = Some(dialogs::save_file(ctx.clone(), "Export video", &s.export_dir, &name));
                                    }
                                    Destination::Clipboard => {
                                        let dir = std::env::temp_dir().join("open-studio-exports");
                                        let _ = std::fs::create_dir_all(&dir);
                                        start = Some(dir.join(name));
                                    }
                                }
                            }
                            if ui.button("Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
                    let where_ = match s.destination {
                        Destination::File => format!("The save dialog opens in {}", crate::util::tilde(&s.export_dir)),
                        Destination::Clipboard => "The video is copied as a file you can paste into chats, email or your file manager.".to_string(),
                    };
                    ui.label(egui::RichText::new(where_).size(12.0).color(theme::TEXT_DIM));
                    if s.height.is_none() {
                        s.height = None;
                    }
                }
                Some(job) => {
                    let done = job.result.lock().unwrap().clone();
                    match done {
                        None => {
                            let p = job.progress.permille.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
                            ui.add(egui::ProgressBar::new(p).show_percentage().fill(theme::ACCENT).corner_radius(CornerRadius::same(4)));
                            ctx.request_repaint_after(std::time::Duration::from_millis(100));
                            if ui.button("Cancel").clicked() {
                                job.progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        Some(Ok(())) => match job.destination {
                            Destination::File => {
                                ui.label("Export finished.");
                                ui.label(egui::RichText::new(crate::util::tilde(&job.out)).color(theme::TEXT_DIM).small());
                                ui.horizontal(|ui| {
                                    if theme::primary_button(ui, "  Show in folder  ").clicked() {
                                        if let Some(parent) = job.out.parent() {
                                            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
                                        }
                                    }
                                    if ui.button("Open video").clicked() {
                                        let _ = std::process::Command::new("xdg-open").arg(&job.out).spawn();
                                    }
                                    if ui.button("Done").clicked() {
                                        close = true;
                                    }
                                });
                            }
                            Destination::Clipboard => {
                                ui.label("Copied to the clipboard.");
                                ui.label(egui::RichText::new("Paste it into a chat, an email or a folder.").color(theme::TEXT_DIM));
                                if theme::primary_button(ui, "  Done  ").clicked() {
                                    close = true;
                                }
                            }
                        },
                        Some(Err(e)) => {
                            ui.colored_label(theme::RECORD, e);
                            if ui.button("Close").clicked() {
                                close = true;
                            }
                        }
                    }
                }
            }
        });
        if let Some(out) = start {
            dlg.settings.save();
            let progress = Arc::new(Progress::default());
            let result = Arc::new(Mutex::new(None));
            let (p, r) = (progress.clone(), result.clone());
            let (dir, rec, edit) = (self.dir.clone(), self.rec.clone(), self.edit.clone());
            let s = &dlg.settings;
            let opts = ExportOptions { out: out.clone(), height, fps: s.fps, format: s.format, quality: s.quality };
            let destination = s.destination;
            let (device, queue) = (self.comp.device().clone(), self.comp.queue().clone());
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let mut comp = Compositor::new(device, queue);
                let mut res = export::export(&mut comp, &dir, &rec, &edit, &opts, &p).map_err(|e| format!("{e:#}"));
                if res.is_ok() && destination == Destination::Clipboard {
                    res = copy_file_to_clipboard(&opts.out);
                }
                *r.lock().unwrap() = Some(res);
                ctx2.request_repaint();
            });
            dlg.job = Some(ExportJob { out, destination, progress, result });
        }
        if close {
            dlg.settings.save();
            self.export = None;
        }
    }
}

/// Moves a project to the desktop trash (recoverable), or deletes it when
/// no trash is available.
struct Toast {
    title: String,
    detail: String,
    error: bool,
    /// Offer "Show in folder" for this directory.
    folder: Option<PathBuf>,
    at: Instant,
}

/// Path with the home directory shown as ~.
fn tilde(p: &std::path::Path) -> String {
    match std::env::home_dir().and_then(|h| p.strip_prefix(&h).ok().map(|r| r.to_path_buf())) {
        Some(r) => format!("~/{}", r.display()),
        None => p.display().to_string(),
    }
}

fn leave_action(why: Leaving) -> Action {
    match why {
        Leaving::Home => Action::GoHome,
        Leaving::Quit => Action::Quit,
    }
}

fn trash(dir: &std::path::Path) -> anyhow::Result<()> {
    let ok = std::process::Command::new("gio").arg("trash").arg(dir).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

/// Puts a file on the Wayland clipboard as a URI list, which file managers,
/// browsers and chat apps accept as a pasted file.
pub fn copy_file_to_clipboard(path: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    let uri = format!("file://{}\n", path.display());
    let mut child = std::process::Command::new("wl-copy")
        .args(["--type", "text/uri-list"])
        .stdin(std::process::Stdio::piped())
        // wl-copy keeps serving the clipboard from a background process;
        // don't let it hold our stdout/stderr open.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("wl-copy not available: {e}"))?;
    child.stdin.take().unwrap().write_all(uri.as_bytes()).map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}
