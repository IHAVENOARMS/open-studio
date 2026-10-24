//! The editor window: preview canvas, settings sidebar and timeline.

mod audio;
mod camera_ui;
mod captions_ui;
mod commands;
mod cursors;
mod curve;
mod guide;
mod mask_overlay;
pub mod player;
mod privacy_ui;
pub use widgets::toggle;
mod scrub;
mod shuttle;
mod sidebar;
mod timeline;
mod transcript_ui;
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
    /// "Still working" prompt before leaving while tasks run.
    busy_prompt: Option<Leaving>,
    tasks_dismissed: bool,
    saving_as: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
    /// Detected shortcuts by single-key setting, for the shortcuts track.
    shortcuts_cache: Option<((bool, Vec<String>), Vec<crate::shortcuts::Shortcut>)>,
    /// Short message under the title ("Saved", errors).
    status: Option<Toast>,
    /// Short note at the bottom ("Removed 6.0s — Ctrl+Z to undo").
    hint: Option<(String, Instant)>,
    player: Player,
    /// For reopening players when the project folder moves.
    ctx: egui::Context,
    /// Preview size relative to the space available (zoom out / in).
    preview_zoom: f32,
    /// Preview resolution divisor (Preview quality).
    preview_quality: u32,
    crop_tool: Option<CropTool>,
    /// Undo history: settled states before each change, and undone ones.
    history: Vec<EditConfig>,
    future: Vec<EditConfig>,
    /// The last settled state (a change settles when no mouse button is held).
    committed: EditConfig,
    /// Camera track player and the frame index on the GPU.
    camera: Option<(Player, i64)>,
    /// Background-removal matte player, in step with the camera.
    matte: Option<(Player, i64)>,
    /// When the camera picture last changed during playback (watchdog).
    camera_changed: Instant,
    audio: audio::AudioPreview,
    /// Waveform envelope for the clip track (computed in the background).
    waveform: Arc<Mutex<Option<Vec<f32>>>>,
    comp: Compositor,
    target: Option<PreviewTarget>,
    shown_index: i64,
    time: f64,
    playing: Option<(f64, Instant)>,
    /// Shuttle speed while playing (J/K/L): 1 normal, 2..16 faster, negative
    /// backwards. On top of slice speeds.
    rate: f64,
    selected: Option<Selection>,
    tab: Tab,
    sidebar: sidebar::State,
    timeline: timeline::State,
    export: Option<ExportDialog>,
    scrubber: scrub::Scrubber,
    /// Ctrl+K command menu, while open.
    command: Option<commands::CommandMenu>,
    /// Typing stretches found in the recorded keys (Speed up typing).
    typing: Vec<crate::typing::Typing>,
    /// Ctrl+/ Shortcuts Guide, while open.
    guide: Option<guide::Guide>,
    presets: Vec<crate::presets::Preset>,
    /// "New preset name..." field.
    preset_name: String,
    /// Preset whose settings row is unfolded in the dropdown.
    preset_menu: Option<usize>,
    /// A file dialog for a command, and what to do with its answer.
    file_task: Option<(Receiver<Vec<PathBuf>>, commands::FileTask)>,
    /// Message when a command's file job finishes.
    file_done: Option<Receiver<String>>,
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
        let matte = open_matte(&dir, &rec, &edit, start, ctx);
        let saved_copy = edit.clone();
        Ok(Self {
            name: paths.name(),
            dir,
            tracks,
            tracks_for: edit.clone(),
            saved: edit.clone(),
            renaming: None,
            leaving: None,
            busy_prompt: None,
            tasks_dismissed: false,
            saving_as: None,
            shortcuts_cache: None,
            status: None,
            hint: None,
            edit,
            player,
            ctx: ctx.clone(),
            preview_zoom: 1.0,
            preview_quality: Settings::load().preview_quality.clamp(1, 4),
            crop_tool: None,
            history: vec![],
            future: vec![],
            committed: saved_copy,
            camera,
            matte,
            camera_changed: Instant::now(),
            audio,
            waveform: audio::envelope_async(&rec, ctx.clone()),
            comp: Compositor::new(rs.device.clone(), rs.queue.clone()),
            target: None,
            shown_index: -1,
            time: start,
            playing: None,
            rate: 1.0,
            selected: None,
            tab: Tab::Background,
            sidebar: sidebar::State::default(),
            timeline: timeline::State::default(),
            export: None,
            scrubber: Default::default(),
            command: None,
            guide: None,
            typing: crate::typing::detect(&rec.events.keys, rec.duration),
            presets: vec![],
            preset_name: String::new(),
            preset_menu: None,
            file_task: None,
            file_done: None,
            rec,
        })
    }

    /// Where the kept video starts and ends: the trim range without cuts
    /// at either end.
    fn playable(&self) -> (f64, f64) {
        let kept = self.edit.kept_ranges(self.rec.duration);
        match (kept.first(), kept.last()) {
            (Some(a), Some(b)) => (a.0, b.1),
            _ => self.edit.trim_range(self.rec.duration),
        }
    }

    fn set_playing(&mut self, play: bool) {
        let (start, end) = self.playable();
        if play {
            if self.rate > 0.0 && self.time >= end - 0.05 {
                self.time = start;
            }
            self.run_clock();
        } else {
            self.playing = None;
            self.rate = 1.0;
            self.player.pause();
            self.audio.pause();
            self.player.seek(self.time);
            self.camera_follow(None);
        }
    }

    /// Shuttle (J/K/L, the speed menu): plays at `rate`, or pauses at 0.
    fn set_rate(&mut self, rate: f64) {
        if rate == 0.0 {
            self.set_playing(false);
            return;
        }
        self.rate = rate;
        self.set_playing(true);
    }

    /// Starts every player from the playhead, now, at the slice speed times
    /// the shuttle rate.
    fn run_clock(&mut self) {
        let now = Instant::now();
        self.playing = Some((self.time, now));
        let mut slice = self.edit.slice_props(self.time);
        slice.speed *= self.rate;
        self.player.play(self.time, now, slice.speed);
        if shuttle::audible(self.rate) {
            self.audio.play(self.time, &slice);
        } else {
            self.audio.pause();
        }
        self.camera_follow(Some(now));
    }

    /// Keeps the camera player on the playhead: playing from `now`, or
    /// paused at the current time.
    fn camera_follow(&self, now: Option<Instant>) {
        let Some(c) = &self.rec.camera else { return };
        let t = (self.time - c.offset - self.edit.camera.delay).max(0.0);
        for (p, _) in [&self.camera, &self.matte].into_iter().flatten() {
            match now {
                Some(at) => p.play(t, at, self.edit.slice_props(self.time).speed * self.rate),
                None => {
                    p.pause();
                    p.seek(t);
                }
            }
        }
    }

    fn seek(&mut self, t: f64) {
        self.time = t.clamp(0.0, self.rec.duration);
        if self.playing.is_some() {
            self.run_clock();
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
                (Icon::CamCutout, "Camera cutout: you without your background", 14),
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
                let enabled = action < 10 || has_camera;
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
                    resp.on_hover_text(format!("{tip}: record with a camera to use this"))
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
            Some(14) => self.add_layout_at_playhead(crate::edit::CameraLayoutKind::Cutout),
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
                // Naming a brand-new recording saves it (into the projects
                // folder, under that name). A saved project is only renamed:
                // its unsaved edits stay unsaved until Ctrl+S.
                let was_new = self.is_new();
                match self.rename(&name) {
                    Ok(true) if was_new => self.save_with_feedback(),
                    Ok(_) => {}
                    Err(e) => self.notify(format!("Couldn't rename: {e:#}")),
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

    /// The project folder moved (saved, renamed, Save As): reopen
    /// everything that reads files from it at the new place.
    fn set_dir(&mut self, dir: PathBuf) {
        self.rec.dir = dir.clone();
        self.dir = dir;
        let paths = ProjectPaths::new(&self.dir);
        self.player = Player::new(paths.video(), (self.rec.width, self.rec.height), self.rec.fps, self.ctx.clone());
        self.shown_index = -1;
        self.camera = self.rec.camera.as_ref().map(|c| (Player::new(self.dir.join(&c.file), (c.width, c.height), c.fps, self.ctx.clone()), -1));
        self.matte = open_matte(&self.dir, &self.rec, &self.edit, self.time, &self.ctx);
        self.audio = audio::AudioPreview::new(&self.rec, &self.edit);
        // Resume where we were.
        if self.playing.is_some() {
            self.run_clock();
        } else {
            self.player.seek(self.time);
            self.camera_follow(None);
        }
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
    /// The bottom hint: fades in and out over 2.5 s.
    fn hint_toast(&mut self, ctx: &egui::Context) {
        const SHOW: f32 = 2.5;
        let Some((msg, at)) = &self.hint else { return };
        let age = at.elapsed().as_secs_f32();
        if age > SHOW {
            self.hint = None;
            return;
        }
        let a = (age / 0.15).min((SHOW - age) / 0.25).clamp(0.0, 1.0);
        egui::Area::new(egui::Id::NULL.with("hint-toast"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -26.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(a);
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x1c, 0x1d, 0x22))
                    .stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(26)))
                    .corner_radius(egui::CornerRadius::same(7))
                    .inner_margin(egui::Margin { left: 14, right: 14, top: 7, bottom: 7 })
                    .show(ui, |ui| ui.label(egui::RichText::new(msg).size(13.5).color(theme::TEXT)));
            });
        ctx.request_repaint();
    }

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
    /// Returns whether the name changed.
    fn rename(&mut self, name: &str) -> anyhow::Result<bool> {
        let name = name.trim().replace('/', "-");
        if name.is_empty() || name == self.name {
            return Ok(false);
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
        Ok(true)
    }

    /// Leaves the editor, asking first when there are unsaved changes.
    /// Background tasks still running (they stop when the editor closes).
    pub fn running_tasks(&self) -> Vec<&'static str> {
        let mut v = self.sidebar.camera.running();
        if self.sidebar.captions.transcribing() {
            v.insert(0, "Transcription");
        }
        v
    }

    pub fn request_leave(&mut self, why: Leaving) -> Action {
        if !self.running_tasks().is_empty() && !self.tasks_dismissed {
            self.busy_prompt = Some(why);
            return Action::None;
        }
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
    /// Shown when leaving while background tasks run: they would stop.
    fn busy_prompt(&mut self, ctx: &egui::Context) -> Action {
        let Some(why) = self.busy_prompt else { return Action::None };
        let tasks = self.running_tasks();
        if tasks.is_empty() {
            // Finished meanwhile: carry on leaving.
            self.busy_prompt = None;
            return self.request_leave(why);
        }
        let mut out = Action::None;
        let frame = egui::Frame::new()
            .fill(Color32::from_rgb(0x26, 0x26, 0x29))
            .stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(22)))
            .corner_radius(egui::CornerRadius::same(16))
            .inner_margin(egui::Margin { left: 22, right: 22, top: 22, bottom: 20 })
            .shadow(egui::epaint::Shadow { offset: [0, 18], blur: 50, spread: 0, color: Color32::from_black_alpha(150) });
        egui::Modal::new(egui::Id::NULL.with("busy-prompt")).frame(frame).backdrop_color(Color32::from_black_alpha(110)).show(ctx, |ui| {
            let w = 260.0;
            ui.set_width(w);
            ui.vertical_centered(|ui| {
                let what = tasks.join(" and ");
                let verb = if tasks.len() > 1 { "are" } else { "is" };
                ui.label(egui::RichText::new(format!("{what} {verb} still running")).font(theme::semibold(14.0)).color(Color32::WHITE));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(if why == Leaving::Quit {
                        "If you quit now it stops, and you'll have to start it again next time."
                    } else {
                        "If you leave this project now it stops, and you'll have to start it again."
                    })
                    .size(12.0)
                    .color(Color32::from_rgb(0xb4, 0xb4, 0xb9)),
                );
                ui.add_space(16.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                let button = |ui: &mut egui::Ui, label: &str, fill: Color32, text: Color32| {
                    ui.add_sized([w, 30.0], egui::Button::new(egui::RichText::new(label).size(13.0).color(text)).fill(fill).corner_radius(egui::CornerRadius::same(8)))
                };
                if button(ui, "Keep working", theme::ACCENT, Color32::WHITE).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.busy_prompt = None;
                }
                let label = if why == Leaving::Quit { "Quit anyway" } else { "Leave anyway" };
                if button(ui, label, Color32::from_rgb(0x3a, 0x3a, 0x3e), Color32::from_rgb(0xff, 0x6b, 0x61)).clicked() {
                    self.busy_prompt = None;
                    self.tasks_dismissed = true;
                    out = self.request_leave(why);
                }
            });
        });
        out
    }

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
                        // Staying: warn about running tasks again next time.
                        self.tasks_dismissed = false;
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
            // Stop where the kept video ends, not past a cut running to the end.
            let (start, end) = self.playable();
            let slice = self.edit.slice_props(from);
            self.time = from + at.elapsed().as_secs_f64() * slice.speed * self.rate;
            if self.time >= end {
                self.time = end;
                self.set_playing(false);
            } else if self.time <= start && self.rate < 0.0 {
                self.time = start;
                self.set_playing(false);
            } else if let Some(c) = self.edit.cut_at(self.time) {
                // Jump over cut stretches, whichever way we're going.
                self.seek(if self.rate < 0.0 { (c.start - 1e-3).max(start) } else { c.end.min(end) });
            } else if !self.edit.slice_props(self.time).same(&slice) {
                // Entered a slice with another speed or volume: restart from here.
                self.seek(self.time);
            }
            ctx.request_repaint();
        }

        self.handle_keys(&ctx);
        // Hide sensitive data / Hide faces: a finished pass brings new covers.
        if privacy_ui::tick(&mut self.sidebar.privacy, &self.edit, &self.rec, &ctx) {
            self.tracks = Tracks::build(&self.rec, &self.edit);
        }
        // Track set changed (music, mutes, click sounds): rebuild the player.
        if !self.audio.is_current(&self.edit) {
            self.audio = audio::AudioPreview::new(&self.rec, &self.edit);
            if self.playing.is_some() {
                self.audio.play(self.time, &self.edit.slice_props(self.time));
            }
        }
        self.audio.set_muted(self.edit.audio.mute);
        self.audio.set_levels(&self.edit, &self.edit.slice_props(self.time));

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
                        ui.add_space(8.0);
                        self.presets_button(ui);
                        bar_separator(ui);
                        // Redo and undo, dim when there is nothing to do.
                        let (can_undo, can_redo) = (!self.history.is_empty(), !self.future.is_empty());
                        if toolbar_button(ui, Icon::Redo, "", "", false, can_redo).on_hover_text("Redo  (Ctrl+Shift+Z)").clicked() {
                            self.redo();
                        }
                        if toolbar_button(ui, Icon::Undo, "", "", false, can_undo).on_hover_text("Undo  (Ctrl+Z)").clicked() {
                            self.undo();
                        }
                        bar_separator(ui);
                        if toolbar_button(ui, Icon::Command, "", "", false, true).on_hover_text("Command menu  (Ctrl+K)").clicked() {
                            self.open_command_menu();
                        }

                        ui.centered_and_justified(|ui| self.title(ui));
                    });
                });
            });

        // Detected shortcuts for the timeline track, recomputed only when
        // the single-key setting changes.
        let shortcuts = if self.edit.shortcuts.timeline {
            let key = (self.edit.shortcuts.single_keys, self.edit.shortcuts.custom.clone());
            if self.shortcuts_cache.as_ref().is_none_or(|(k, _)| *k != key) {
                let list = crate::shortcuts::detect(&self.rec, key.0, &key.1);
                self.shortcuts_cache = Some((key, list));
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
                    self.rec.audio.iter().any(|a| a.kind == "system"),
                    self.rec.audio.iter().any(|a| a.kind == "device"),
                    &shortcuts,
                    &self.rec.events.flags,
                    &self.typing,
                );
                if let Some(t) = out.seek {
                    self.seek(t);
                }
                // Scrubber: dragging the playhead plays the sound under it.
                if self.timeline.scrubber {
                    self.scrubber.prepare(&self.rec, &self.edit);
                    if out.scrubbing {
                        self.scrubber.scrub(self.time);
                    } else {
                        self.scrubber.end();
                    }
                }
                if let Some(m) = out.toast {
                    self.hint = Some((m, Instant::now()));
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
                    // Background removal: starts after the cutout stays in use a moment.
                    let in_use = camera_ui::cutout_in_use(&self.edit);
                    self.sidebar.camera.tick_cutout(&self.rec, ui.ctx(), in_use);
                    // Background removal finished: pick up the matte.
                    if std::mem::take(&mut self.sidebar.camera.cutout_ready) {
                        if let Ok(fresh) = Recording::load(&self.dir) {
                            self.rec.camera = fresh.camera;
                        }
                        let ctx = ui.ctx().clone();
                        self.matte = open_matte(&self.dir, &self.rec, &self.edit, self.time, &ctx);
                        self.camera_follow(self.playing.map(|_| Instant::now()));
                        self.notify("Background removed");
                    }
                    if let Some(t) = sidebar::show(ui, &mut self.sidebar, &mut self.edit, &mut self.tab, &mut self.selected, &self.rec, self.time) {
                        self.seek(t);
                    }
                    if std::mem::take(&mut self.sidebar.captions.play_request) && self.playing.is_none() {
                        self.set_playing(true);
                    }
                    // A transcript just finished generating.
                    if let Some(m) = self.sidebar.captions.toast.take() {
                        self.hint = Some((m, Instant::now()));
                    }
                    if let Some(t) = self.sidebar.captions.finished.take() {
                        self.rec.transcript = Some(t);
                        self.tracks = Tracks::build(&self.rec, &self.edit);
                    }
                });
                let main_rect = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x - side_w - 16.0, full.max.y));
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(main_rect).layout(egui::Layout::top_down(egui::Align::Min)));
                self.preview_toolbar(ui);
                ui.add_space(8.0);
                let bar_h = 48.0;
                let avail = ui.available_rect_before_wrap();
                // Tool pill on the left, preview to its right.
                let pill_rect = egui::Rect::from_min_max(avail.min, egui::pos2(avail.min.x + 56.0, avail.max.y - bar_h));
                self.tool_pill(ui, pill_rect);
                let preview_rect = egui::Rect::from_min_max(egui::pos2(avail.min.x + 68.0, avail.min.y), egui::pos2(avail.max.x, avail.max.y - bar_h));
                ui.scope_builder(egui::UiBuilder::new().max_rect(preview_rect), |ui| self.preview(ui, rs));
                let bar_rect = egui::Rect::from_min_max(egui::pos2(avail.min.x, avail.max.y - bar_h), avail.max);
                ui.scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
                    let out = timeline::playback_bar(ui, &mut self.timeline, self.time, self.rec.duration, self.playing.is_some(), self.rate);
                    if out.toggle_play {
                        self.set_playing(self.playing.is_none());
                    }
                    if let Some(r) = out.rate {
                        self.set_rate(r);
                    }
                    if let Some(t) = out.seek {
                        let (s, e) = self.edit.trim_range(self.rec.duration);
                        self.seek(t.clamp(s, e));
                    }
                });
            });

        self.export_dialog(&ctx);
        self.command_menu(&ctx);
        self.guide_window(&ctx);
        self.poll_file_tasks(&ctx);

        // Undo history: a change settles once no mouse button is held, so a
        // whole drag or slider move is one step.
        if self.edit != self.committed && !ctx.input(|i| i.pointer.any_down()) {
            let before = std::mem::replace(&mut self.committed, self.edit.clone());
            self.history.push(before);
            crate::presets::remember_last(&self.edit, self.rec.source.kind == "device");
            if self.history.len() > 200 {
                self.history.remove(0);
            }
            self.future.clear();
        }

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
            action = self.busy_prompt(&ctx);
        }
        if let Action::None = action {
            action = self.leave_prompt(&ctx);
        }
        self.toast(&ctx);
        self.hint_toast(&ctx);
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
        // Text fields keep their own undo, except the transcript, whose edits
        // are project edits.
        let in_transcript = ctx.memory(|m| m.focused()) == Some(transcript_ui::doc_id());
        if ctx.egui_wants_keyboard_input() && !in_transcript {
            return;
        }
        if self.command.is_none() && !in_transcript && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::K)) {
            self.open_command_menu();
            return;
        }
        if self.guide.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Slash)) {
            self.open_guide();
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::E)) && self.export.is_none() {
            self.export = Some(ExportDialog { settings: Settings::load(), job: None, picking_dir: None, saving: None });
            return;
        }
        // Ctrl+Z undoes; Ctrl+Shift+Z or Ctrl+Y redoes.
        let redo = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Z) || i.consume_key(egui::Modifiers::COMMAND, Key::Y));
        let undo = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Z));
        if redo {
            self.redo();
        } else if undo {
            self.undo();
        }
        if in_transcript {
            return;
        }
        // Ctrl+C copies the current frame as an image (egui turns the key
        // into a Copy event).
        if ctx.input_mut(|i| {
            let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            i.events.retain(|e| !matches!(e, egui::Event::Copy));
            i.consume_key(egui::Modifiers::COMMAND, Key::C) || copy
        }) {
            self.copy_frame(ctx);
        }
        // Alt+B/L/S: camera layouts (Screen Studio's Control+B/L/S; Ctrl+S
        // saves here).
        if self.rec.camera.is_some() {
            use crate::edit::CameraLayoutKind as K;
            for (key, kind) in [(Key::B, K::Default), (Key::L, K::Fullscreen), (Key::S, K::Hide)] {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, key)) {
                    self.add_layout_at_playhead(kind);
                }
            }
        }
        // C splits the clip at the playhead (as in Screen Studio); Shift+C crops.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::C)) {
            self.edit.split_at(self.time, self.rec.duration);
        }
        if self.crop_tool.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::C)) {
            self.open_crop_tool();
            return;
        }
        let steps = [1.0f32, 0.75, 0.5, 0.33];
        let zi = steps.iter().position(|s| (s - self.preview_zoom).abs() < 0.01).unwrap_or(0);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Minus)) && zi + 1 < steps.len() {
            self.preview_zoom = steps[zi + 1];
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Equals) || i.consume_key(egui::Modifiers::COMMAND, Key::Plus)) && zi > 0 {
            self.preview_zoom = steps[zi - 1];
        }
        if self.crop_tool.is_some() {
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
        // J/K/L shuttle. Holding K, J and L step a frame instead.
        let (j, k, l, k_held) = ctx.input(|i| {
            let plain = i.modifiers.is_none();
            (plain && i.key_pressed(Key::J), plain && i.key_pressed(Key::K), plain && i.key_pressed(Key::L), i.key_down(Key::K))
        });
        let rate = if self.playing.is_some() { self.rate } else { 0.0 };
        if k {
            self.set_rate(shuttle::press_k(rate));
        } else if k_held && (j || l) {
            self.set_playing(false);
            let step = 1.0 / self.rec.fps as f64;
            self.seek(self.time + if j { -step } else { step });
        } else if j {
            self.set_rate(shuttle::press_j(rate));
        } else if l {
            self.set_rate(shuttle::press_l(rate));
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
                    Track::Clip => {
                        if let Some(&(a, b)) = self.edit.slice_ranges(self.rec.duration).get(sel.index) {
                            self.edit.add_cut(a, b);
                            self.hint = Some((format!("Removed {:.1}s — Ctrl+Z to undo", b - a), Instant::now()));
                        }
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

    pub fn undo(&mut self) {
        if self.edit != self.committed {
            return; // a change is still in progress
        }
        if let Some(prev) = self.history.pop() {
            self.future.push(std::mem::replace(&mut self.committed, prev.clone()));
            self.edit = prev;
            self.selected = None;
        }
    }

    pub fn redo(&mut self) {
        if self.edit != self.committed {
            return;
        }
        if let Some(next) = self.future.pop() {
            self.history.push(std::mem::replace(&mut self.committed, next.clone()));
            self.edit = next;
            self.selected = None;
        }
    }

    /// The row above the preview, as in Screen Studio: aspect ratio and Crop
    /// on the left; Preview quality and preview zoom on the right.
    fn preview_toolbar(&mut self, ui: &mut egui::Ui) {
        use crate::edit::Aspect;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let a = self.edit.aspect;
            let resp = toolbar_button(ui, Icon::AspectLayout, a.name(), a.label(), true, true);
            egui::Popup::menu(&resp).show(|ui| {
                ui.set_min_width(200.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                for opt in Aspect::ALL {
                    if menu_item(ui, opt.name(), opt.label(), a == opt).clicked() {
                        self.edit.aspect = opt;
                        ui.close();
                    }
                }
            });
            if toolbar_button(ui, Icon::Crop, "Crop", "", false, true).on_hover_text("Crop the recording  (Shift+C)").clicked() {
                self.open_crop_tool();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                let steps = [1.0f32, 0.75, 0.5, 0.33];
                let i = steps.iter().position(|s| (s - self.preview_zoom).abs() < 0.01).unwrap_or(0);
                if toolbar_button(ui, Icon::ZoomInLens, "", "", false, i > 0).on_hover_text("Zoom in preview  (Ctrl+=)").clicked() && i > 0 {
                    self.preview_zoom = steps[i - 1];
                }
                if toolbar_button(ui, Icon::ZoomOutLens, "", "", false, i + 1 < steps.len()).on_hover_text("Zoom out preview  (Ctrl+-)").clicked() && i + 1 < steps.len() {
                    self.preview_zoom = steps[i + 1];
                }
                ui.add_space(6.0);
                let q = match self.preview_quality {
                    1 => "Full",
                    2 => "Half",
                    _ => "Quarter",
                };
                let resp = toolbar_button(ui, Icon::Quality, "Preview quality", q, true, true);
                egui::Popup::menu(&resp).show(|ui| {
                    ui.set_min_width(260.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (d, name, help) in [
                        (1u32, "Full", "Sharpest preview, looks exactly like the export."),
                        (2, "Half", "Half resolution, smoother playback."),
                        (4, "Quarter", "Fastest playback on slower computers."),
                    ] {
                        let r = menu_item(ui, name, "", self.preview_quality == d).on_hover_text(help);
                        if r.clicked() {
                            self.preview_quality = d;
                            let mut s = Settings::load();
                            s.preview_quality = d;
                            s.save();
                            ui.close();
                        }
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Only the preview; exports are always full quality.").size(11.5).color(theme::TEXT_DIM));
                    });
                    ui.add_space(2.0);
                });
            });
        });
    }

    fn open_crop_tool(&mut self) {
        if self.crop_tool.is_none() {
            self.set_playing(false);
            self.crop_tool = Some(CropTool { rect: crate::engine::clamp_crop(self.edit.crop), ratio: None, drag: None, tex: None, tex_index: -1 });
        }
    }

    /// Crop tool in place of the preview: drag the edges or the middle,
    /// pick a ratio or type a size, then Done.
    fn crop_ui(&mut self, ui: &mut egui::Ui) {
        let (vw, vh) = (self.rec.width as f64, self.rec.height as f64);
        let frame = self.player.latest();
        let Some(tool) = &mut self.crop_tool else { return };
        // Picture of the current frame (downscaled; it's only a guide).
        if let Some(f) = &frame {
            if tool.tex.is_none() || tool.tex_index != f.index {
                let (w, h) = (f.width as usize, f.height as usize);
                let tw = w.min(1600);
                let th = h * tw / w;
                let mut px = Vec::with_capacity(tw * th * 4);
                for y in 0..th {
                    for x in 0..tw {
                        let o = ((y * h / th) * w + x * w / tw) * 4;
                        px.extend_from_slice(&f.rgba[o..o + 4]);
                    }
                }
                tool.tex = Some(ui.ctx().load_texture("crop-frame", egui::ColorImage::from_rgba_unmultiplied([tw, th], &px), egui::TextureOptions::LINEAR));
                tool.tex_index = f.index;
            }
        }
        let full = ui.available_rect_before_wrap();
        let controls_h = 46.0;
        let area = Rect::from_min_max(full.min, egui::pos2(full.max.x, full.max.y - controls_h - 8.0)).shrink(12.0);
        let ar = (vw / vh) as f32;
        let mut size = area.size();
        if size.x / size.y > ar {
            size.x = size.y * ar;
        } else {
            size.y = size.x / ar;
        }
        let pic = Rect::from_center_size(area.center(), size);
        let p = ui.painter();
        if let Some(t) = &tool.tex {
            p.image(t.id(), pic, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        } else {
            p.rect_filled(pic, 0.0, theme::SURFACE);
        }
        let [x, y, w, h] = tool.rect;
        let kept = Rect::from_min_size(
            egui::pos2(pic.left() + x as f32 * pic.width(), pic.top() + y as f32 * pic.height()),
            egui::vec2(w as f32 * pic.width(), h as f32 * pic.height()),
        );
        let dim = Color32::from_black_alpha(160);
        p.rect_filled(Rect::from_min_max(pic.min, egui::pos2(pic.right(), kept.top())), 0.0, dim);
        p.rect_filled(Rect::from_min_max(egui::pos2(pic.left(), kept.bottom()), pic.max), 0.0, dim);
        p.rect_filled(Rect::from_min_max(egui::pos2(pic.left(), kept.top()), egui::pos2(kept.left(), kept.bottom())), 0.0, dim);
        p.rect_filled(Rect::from_min_max(egui::pos2(kept.right(), kept.top()), egui::pos2(pic.right(), kept.bottom())), 0.0, dim);
        // Thirds guides and the frame.
        let guide = egui::Stroke::new(1.0, Color32::from_white_alpha(70));
        for k in [1.0 / 3.0, 2.0 / 3.0] {
            let gx = kept.left() + kept.width() * k;
            let gy = kept.top() + kept.height() * k;
            p.line_segment([egui::pos2(gx, kept.top()), egui::pos2(gx, kept.bottom())], guide);
            p.line_segment([egui::pos2(kept.left(), gy), egui::pos2(kept.right(), gy)], guide);
        }
        p.rect_stroke(kept, 0.0, egui::Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Middle);
        // Corner and edge handles.
        let handles = [
            (kept.left_top(), (true, true, false, false)),
            (kept.right_top(), (false, true, true, false)),
            (kept.left_bottom(), (true, false, false, true)),
            (kept.right_bottom(), (false, false, true, true)),
            (kept.center_top(), (false, true, false, false)),
            (kept.center_bottom(), (false, false, false, true)),
            (kept.left_center(), (true, false, false, false)),
            (kept.right_center(), (false, false, true, false)),
        ];
        for (c, _) in handles {
            p.rect_filled(Rect::from_center_size(c, egui::vec2(9.0, 9.0)), 2.0, Color32::WHITE);
        }
        let resp = ui.interact(pic.expand(8.0), ui.scope_id().with("crop-drag"), Sense::drag());
        if resp.drag_started() {
            if let Some(pos) = resp.interact_pointer_pos() {
                tool.drag = handles
                    .iter()
                    .find(|(c, _)| c.distance(pos) < 12.0)
                    .map(|(_, e)| CropDrag::Resize(e.0, e.1, e.2, e.3))
                    .or_else(|| kept.contains(pos).then_some(CropDrag::Move));
            }
        }
        if let (Some(d), Some(pos)) = (tool.drag, resp.interact_pointer_pos()) {
            let u = ((pos.x - pic.left()) / pic.width()).clamp(0.0, 1.0) as f64;
            let v = ((pos.y - pic.top()) / pic.height()).clamp(0.0, 1.0) as f64;
            let [mut x0, mut y0, w0, h0] = tool.rect;
            let (mut x1, mut y1) = (x0 + w0, y0 + h0);
            match d {
                CropDrag::Move => {
                    let dlt = resp.drag_delta();
                    x0 = (x0 + (dlt.x / pic.width()) as f64).clamp(0.0, 1.0 - w0);
                    y0 = (y0 + (dlt.y / pic.height()) as f64).clamp(0.0, 1.0 - h0);
                    (x1, y1) = (x0 + w0, y0 + h0);
                }
                CropDrag::Resize(l, t, r, b) => {
                    let min = 0.05;
                    if l {
                        x0 = u.min(x1 - min);
                    }
                    if r {
                        x1 = u.max(x0 + min);
                    }
                    if t {
                        y0 = v.min(y1 - min);
                    }
                    if b {
                        y1 = v.max(y0 + min);
                    }
                    // Keep the locked ratio by adjusting the height.
                    if let Some(ratio) = tool.ratio {
                        let hh = ((x1 - x0) * vw / ratio / vh).min(1.0);
                        if t && !b {
                            y0 = (y1 - hh).max(0.0);
                        } else {
                            y1 = (y0 + hh).min(1.0);
                        }
                    }
                }
            }
            tool.rect = crate::engine::clamp_crop([x0, y0, x1 - x0, y1 - y0]);
        }
        if resp.drag_stopped() {
            tool.drag = None;
        }
        // Controls: ratios, size in pixels, Reset / Cancel / Done.
        let bar = Rect::from_min_max(egui::pos2(full.min.x, full.max.y - controls_h), full.max);
        let mut done = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (label, ratio) in [("Free", None), ("16:9", Some(16.0 / 9.0)), ("4:3", Some(4.0 / 3.0)), ("1:1", Some(1.0)), ("9:16", Some(9.0 / 16.0))] {
                if widgets::chip(ui, label, tool.ratio == ratio).clicked() {
                    tool.ratio = ratio;
                    if let Some(r) = ratio {
                        // Largest centred area with that ratio inside the current one.
                        let [x, y, w, h] = tool.rect;
                        let (pw, ph) = (w * vw, h * vh);
                        let (nw, nh) = if pw / ph > r { (ph * r, ph) } else { (pw, pw / r) };
                        let (nw, nh) = (nw / vw, nh / vh);
                        tool.rect = crate::engine::clamp_crop([x + (w - nw) / 2.0, y + (h - nh) / 2.0, nw, nh]);
                    }
                }
            }
            ui.add_space(10.0);
            let mut pw = (tool.rect[2] * vw).round();
            let mut ph = (tool.rect[3] * vh).round();
            ui.label(egui::RichText::new("W").size(12.5).color(theme::TEXT_DIM));
            let cw = ui.add(egui::DragValue::new(&mut pw).range(16.0..=vw).speed(2.0)).changed();
            ui.label(egui::RichText::new("H").size(12.5).color(theme::TEXT_DIM));
            let ch = ui.add(egui::DragValue::new(&mut ph).range(16.0..=vh).speed(2.0)).changed();
            if cw || ch {
                let [x, y, _, _] = tool.rect;
                tool.rect = crate::engine::clamp_crop([x, y, pw / vw, ph / vh]);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, "  Done  ").clicked() {
                    done = Some(true);
                }
                if widgets::chip(ui, "Cancel", false).clicked() {
                    done = Some(false);
                }
                if widgets::chip(ui, "Reset", false).clicked() {
                    tool.rect = [0.0, 0.0, 1.0, 1.0];
                    tool.ratio = None;
                }
            });
        });
        let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        if enter {
            done = Some(true);
        } else if esc {
            done = Some(false);
        }
        match done {
            Some(true) => {
                let r = tool.rect;
                self.edit.crop = r;
                self.crop_tool = None;
            }
            Some(false) => self.crop_tool = None,
            None => {}
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui, rs: &egui_wgpu::RenderState) {
        if self.crop_tool.is_some() {
            self.crop_ui(ui);
            return;
        }
        let full_avail = ui.available_rect_before_wrap();
        let avail = Rect::from_center_size(full_avail.center(), full_avail.size() * self.preview_zoom);
        let aspect = self.tracks.layout.aspect as f32;
        let mut size = avail.size();
        if size.x / size.y > aspect {
            size.x = size.y * aspect;
        } else {
            size.y = size.x / aspect;
        }
        let rect = Rect::from_center_size(avail.center(), size);
        let ppp = ui.ctx().pixels_per_point() / self.preview_quality.max(1) as f32;
        let px = (((size.x * ppp) as u32).max(2), ((size.y * ppp) as u32).max(2));

        if let Some((p, shown)) = &mut self.matte {
            if let Some(f) = p.latest() {
                if f.index != *shown {
                    self.comp.upload_matte_frame(&f.rgba, f.width, f.height);
                    *shown = f.index;
                }
            }
        }
        let mut camera_stalled = false;
        if let Some((p, shown)) = &mut self.camera {
            if let Some(f) = p.latest() {
                if f.index != *shown {
                    self.comp.upload_camera_frame(&f.rgba, f.width, f.height);
                    *shown = f.index;
                    self.camera_changed = Instant::now();
                }
            }
            // Watchdog: while playing, a camera picture that hasn't moved for
            // a while means its player lost the play command or stopped;
            // restart it at the playhead (what pausing and playing did).
            let cam_t = self.rec.camera.as_ref().map(|c| self.time - c.offset - self.edit.camera.delay).unwrap_or(0.0);
            if self.playing.is_some() && cam_t > 0.0 && self.camera_changed.elapsed().as_secs_f64() > 0.6 {
                camera_stalled = true;
                self.camera_changed = Instant::now();
            }
        }
        if camera_stalled {
            if std::env::var_os("OPEN_STUDIO_DEBUG_PLAYER").is_some() {
                eprintln!("camera watchdog: restarting at {:.2}", self.time);
            }
            self.camera_follow(Some(Instant::now()));
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
        resp.context_menu(|ui| {
            if menu_item(ui, "Copy current frame as image", "Ctrl+C", false).clicked() {
                self.copy_frame(ui.ctx());
                ui.close();
            }
        });
        let target = self.target.as_ref().unwrap();
        ui.painter().image(
            target.id,
            rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        // Background removal runs for a while: show it on the preview, from
        // whatever tab is open, until the cutout can be shown.
        let busy = self
            .sidebar
            .camera
            .poll_cutout(ui.ctx())
            .map(|p| (if p == 0.0 { "Preparing background removal…".to_string() } else { format!("Removing background…  {:.0}%", p * 100.0) }, p))
            .or_else(|| privacy_ui::status(&self.sidebar.privacy));
        if let Some((text, p)) = busy {
            let font = egui::FontId::proportional(13.0);
            let g = ui.painter().layout_no_wrap(text, font, Color32::WHITE);
            let size = egui::vec2(g.size().x + 28.0, 34.0);
            let pill = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 14.0 + size.y / 2.0), size);
            let p_ = ui.painter();
            p_.rect_filled(pill, 17.0, Color32::from_rgba_unmultiplied(20, 20, 26, 225));
            p_.rect_stroke(pill, 17.0, egui::Stroke::new(1.0, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);
            p_.galley(pill.center() - g.size() / 2.0 - egui::vec2(0.0, 2.0), g, Color32::WHITE);
            let bar = egui::Rect::from_min_size(egui::pos2(pill.left() + 14.0, pill.bottom() - 7.0), egui::vec2(pill.width() - 28.0, 2.0));
            p_.rect_filled(bar, 1.0, Color32::from_white_alpha(30));
            p_.rect_filled(egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * p.max(0.02), 2.0)), 1.0, theme::KNOB);
        }
        let zoom_sel = self.selected.filter(|s| s.track == Track::Zoom).map(|s| s.index);
        if let Some(Selection { track: Track::Mask, index }) = self.selected {
            if let Some(m) = self.edit.masks.get_mut(index) {
                let id = ui.scope_id().with(("mask-edit", index));
                let map = mask_overlay::Mapping { screen: rect, layout: &self.tracks.layout, cam: fs.camera, xf: fs.screen_xf };
                mask_overlay::edit(ui, &map, id, &mut m.rect);
            }
        }
        // Clicking the preview sets the focus point of a manual zoom.
        if let (Some(i), Some(pos)) = (zoom_sel, resp.interact_pointer_pos()) {
            if resp.clicked() {
                if let Some(seg) = self.edit.zoom_segments.get_mut(i) {
                    if matches!(seg.focus, ZoomFocus::Manual { .. }) {
                        let map = mask_overlay::Mapping { screen: rect, layout: &self.tracks.layout, cam: fs.camera, xf: fs.screen_xf };
                        let (x, y) = map.to_uv(pos);
                        seg.focus = ZoomFocus::Manual { x: x.clamp(0.0, 1.0), y: y.clamp(0.0, 1.0) };
                    }
                }
            }
        }
        if let Some(ZoomFocus::Manual { x, y }) = zoom_sel.and_then(|i| self.edit.zoom_segments.get(i)).map(|s| s.focus)
        {
            // Show the focus point while the camera is at rest (unzoomed).
            if (fs.camera.scale - 1.0).abs() < 0.02 {
                let map = mask_overlay::Mapping { screen: rect, layout: &self.tracks.layout, cam: fs.camera, xf: fs.screen_xf };
                let p = map.to_screen(x, y);
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
                    if self.edit.privacy.hide_sensitive || self.edit.privacy.hide_faces {
                        ui.add_space(18.0);
                        privacy_ui::review_note(ui);
                    }
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
            spawn_export(dlg, out, height, &self.dir, &self.rec, &self.edit, &self.comp, ctx);
        }
        if close {
            dlg.settings.save();
            self.export = None;
        }
    }
}

/// Starts rendering to `out` with the dialog's settings on a worker thread;
/// the dialog shows its progress.
#[allow(clippy::too_many_arguments)]
fn spawn_export(dlg: &mut ExportDialog, out: PathBuf, height: u32, dir: &std::path::Path, rec: &Recording, edit: &EditConfig, comp: &Compositor, ctx: &egui::Context) {
    let progress = Arc::new(Progress::default());
    let result = Arc::new(Mutex::new(None));
    let (p, r) = (progress.clone(), result.clone());
    let (dir, rec, edit) = (dir.to_path_buf(), rec.clone(), edit.clone());
    let s = &dlg.settings;
    let opts = ExportOptions { out: out.clone(), height, fps: s.fps, format: s.format, quality: s.quality };
    let destination = s.destination;
    let (device, queue) = (comp.device().clone(), comp.queue().clone());
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

/// Moves a project to the desktop trash (recoverable), or deletes it when
/// no trash is available.
/// The matte player, when the recording has a background-removal matte.
fn open_matte(dir: &std::path::Path, rec: &Recording, edit: &EditConfig, start: f64, ctx: &egui::Context) -> Option<(Player, i64)> {
    let c = rec.camera.as_ref()?;
    let file = c.matte.as_ref()?;
    let p = Player::new(dir.join(file), crate::cutout::matte_size(c.width, c.height), c.fps, ctx.clone());
    p.seek((start - c.offset - edit.camera.delay).max(0.0));
    Some((p, -1))
}

/// Thin vertical divider between top bar groups.
fn bar_separator(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(17.0, 22.0), Sense::hover());
    ui.painter().rect_filled(egui::Rect::from_center_size(r.center(), egui::vec2(1.0, 22.0)), 0.0, Color32::from_white_alpha(22));
}

/// Small text button for an unfolded settings row.
fn menu_item_small(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let g = ui.painter().layout_no_wrap(label.to_string(), egui::FontId::proportional(12.5), theme::TEXT);
    let (r, resp) = ui.allocate_exact_size(egui::vec2(g.size().x + 18.0, 28.0), Sense::click());
    let fill = if resp.hovered() { Color32::from_white_alpha(22) } else { Color32::from_white_alpha(10) };
    ui.painter().rect_filled(r, egui::CornerRadius::same(7), fill);
    ui.painter().galley(r.center() - g.size() / 2.0, g, theme::TEXT);
    resp
}

/// Dropdown row: roomy padding, a check on the selected item, a soft
/// hover fill, and an optional dim value on the right.
fn menu_item(ui: &mut egui::Ui, label: &str, value: &str, selected: bool) -> egui::Response {
    let w = ui.available_width().max(160.0);
    let (r, resp) = ui.allocate_exact_size(egui::vec2(w, 34.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(r, egui::CornerRadius::same(7), Color32::from_white_alpha(16));
    } else if selected {
        p.rect_filled(r, egui::CornerRadius::same(7), Color32::from_white_alpha(8));
    }
    if selected {
        let c = egui::pos2(r.left() + 18.0, r.center().y);
        p.line(
            vec![c + egui::vec2(-4.5, 0.0), c + egui::vec2(-1.3, 3.2), c + egui::vec2(4.5, -3.5)],
            egui::Stroke::new(1.8, theme::KNOB),
        );
    }
    let font = egui::FontId::proportional(14.0);
    p.text(egui::pos2(r.left() + 34.0, r.center().y), egui::Align2::LEFT_CENTER, label, font.clone(), theme::TEXT);
    if !value.is_empty() {
        p.text(egui::pos2(r.right() - 14.0, r.center().y), egui::Align2::RIGHT_CENTER, value, font, theme::TEXT_DIM);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A button in the row above the preview: icon, white label, dim value and
/// an optional chevron, with a soft hover fill (Screen Studio's style).
fn toolbar_button(ui: &mut egui::Ui, icon: Icon, label: &str, value: &str, chevron: bool, enabled: bool) -> egui::Response {
    let font = egui::FontId::proportional(14.0);
    let white = if enabled { theme::TEXT } else { Color32::from_rgb(0x3a, 0x3c, 0x42) };
    let dim = Color32::from_rgb(0x6c, 0x6e, 0x75);
    let gl = (!label.is_empty()).then(|| ui.painter().layout_no_wrap(label.to_string(), font.clone(), white));
    let gv = (!value.is_empty()).then(|| ui.painter().layout_no_wrap(value.to_string(), font.clone(), dim));
    let mut w = 10.0 + 18.0 + 10.0;
    if let Some(g) = &gl {
        w += g.size().x + 8.0;
    }
    if let Some(g) = &gv {
        w += g.size().x + 6.0;
    }
    if chevron {
        w += 16.0;
    }
    if gl.is_none() && gv.is_none() {
        w = 34.0;
    }
    let (r, resp) = ui.allocate_exact_size(egui::vec2(w, 32.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    if enabled && resp.hovered() {
        p.rect_filled(r, egui::CornerRadius::same(8), Color32::from_white_alpha(12));
    }
    let mut x = if gl.is_none() && gv.is_none() { r.center().x - 9.0 } else { r.left() + 10.0 };
    icons::paint(p, egui::Rect::from_min_size(egui::pos2(x, r.center().y - 9.0), egui::vec2(18.0, 18.0)), icon, white);
    x += 18.0 + 8.0;
    for (g, c) in [(gl, white), (gv, dim)].into_iter().filter_map(|(g, c)| g.map(|g| (g, c))) {
        let gw = g.size().x;
        p.galley(egui::pos2(x, r.center().y - g.size().y / 2.0), g, c);
        x += gw + 6.0;
    }
    if chevron {
        icons::paint(p, egui::Rect::from_min_size(egui::pos2(x, r.center().y - 6.0), egui::vec2(12.0, 12.0)), Icon::ChevronDown, dim);
    }
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// The crop tool: the whole recording with the kept area outlined.
struct CropTool {
    /// x, y, w, h as fractions of the recording.
    rect: [f64; 4],
    /// Locked aspect ratio (width / height in pixels), None = free.
    ratio: Option<f64>,
    drag: Option<CropDrag>,
    tex: Option<egui::TextureHandle>,
    tex_index: i64,
}

#[derive(Clone, Copy)]
enum CropDrag {
    Move,
    /// Edges being moved: left, top, right, bottom.
    Resize(bool, bool, bool, bool),
}

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

/// Puts a file on the clipboard as a URI list, which file managers,
/// browsers and chat apps accept as a pasted file.
pub fn copy_file_to_clipboard(path: &std::path::Path) -> Result<(), String> {
    crate::clipboard::copy("text/uri-list", format!("file://{}\n", path.display()).as_bytes())
}
