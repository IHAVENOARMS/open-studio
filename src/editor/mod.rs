//! The editor window: preview canvas, settings sidebar and timeline.

mod audio;
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
}

pub struct Editor {
    pub dir: PathBuf,
    name: String,
    rec: Recording,
    edit: EditConfig,
    tracks: Tracks,
    tracks_for: EditConfig,
    saved: EditConfig,
    changed_at: Option<Instant>,
    player: Player,
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
        Ok(Self {
            name: paths.name(),
            dir,
            tracks,
            tracks_for: edit.clone(),
            saved: edit.clone(),
            changed_at: None,
            edit,
            player,
            audio: audio::AudioPreview::new(&rec),
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
        } else {
            self.playing = None;
            self.player.pause();
            self.audio.pause();
            self.player.seek(self.time);
        }
    }

    fn seek(&mut self, t: f64) {
        self.time = t.clamp(0.0, self.rec.duration);
        if self.playing.is_some() {
            let now = Instant::now();
            self.playing = Some((self.time, now));
            self.player.play(self.time, now);
            self.audio.play(self.time);
        } else {
            self.player.seek(self.time);
        }
    }

    /// Selects the mask under the playhead, or adds a new one there.
    fn add_mask_at_playhead(&mut self) {
        if let Some(i) = self.edit.masks.iter().position(|m| self.time >= m.start && self.time < m.end) {
            self.selected = Some(Selection { track: Track::Mask, index: i });
            return;
        }
        let start = self.time.min(self.rec.duration - 0.3).max(0.0);
        let Some((s, e)) = timeline::free_span_on(&self.edit, Track::Mask, start, 3.0, self.rec.duration) else { return };
        let index = timeline::add_mask(&mut self.edit, crate::edit::Mask::new(s, e));
        self.selected = Some(Selection { track: Track::Mask, index });
    }

    pub fn save(&mut self) {
        if self.edit != self.saved {
            if let Err(e) = self.edit.save(&self.dir) {
                eprintln!("open-studio: saving project failed: {e:#}");
            }
            self.saved = self.edit.clone();
        }
        self.changed_at = None;
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
            }
            ctx.request_repaint();
        }

        self.handle_keys(&ctx);
        self.audio.set_volumes(self.edit.audio.microphone_volume, self.edit.audio.system_volume, self.edit.audio.mute);

        egui::Panel::top("topbar")
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if icons::button(ui, Icon::Folder, "", egui::FontId::proportional(15.0), theme::TEXT, None, Vec2::ZERO)
                        .on_hover_text("Projects")
                        .clicked()
                    {
                        action = Action::GoHome;
                    }
                    ui.add_space(4.0);
                    if icons::button(ui, Icon::Trash, "", egui::FontId::proportional(15.0), theme::TEXT, None, Vec2::ZERO)
                        .on_hover_text("Delete project (moves it to the trash)")
                        .clicked()
                    {
                        self.save();
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
                        ui.centered_and_justified(|ui| {
                            let mut job = egui::text::LayoutJob::default();
                            job.append(&self.name, 0.0, egui::TextFormat::simple(egui::FontId::proportional(14.0), theme::TEXT));
                            job.append(".openstudio", 0.0, egui::TextFormat::simple(egui::FontId::proportional(14.0), theme::TEXT_DIM));
                            ui.label(job);
                        });
                    });
                });
            });

        egui::Panel::bottom("timeline")
            .resizable(false)
            .exact_size(278.0)
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
                    if let Some(t) = sidebar::show(ui, &mut self.sidebar, &mut self.edit, &mut self.tab, &mut self.selected, &self.rec) {
                        self.seek(t);
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
                    let mask_active = matches!(self.selected, Some(Selection { track: Track::Mask, .. }));
                    let col = if mask_active { theme::KNOB } else { theme::TEXT };
                    let mb = icons::button(ui, Icon::Mask, "Mask", egui::FontId::proportional(13.5), col, Some(theme::SURFACE), egui::vec2(0.0, 30.0))
                        .on_hover_text("Add a blur or highlight mask at the playhead");
                    if mb.clicked() {
                        self.add_mask_at_playhead();
                    }
                });
                let bar_h = 48.0;
                let avail = ui.available_rect_before_wrap();
                let preview_rect = egui::Rect::from_min_max(avail.min, egui::pos2(avail.max.x, avail.max.y - bar_h));
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

        // Rebuild motion tracks when edits change, autosave shortly after.
        if self.edit != self.tracks_for {
            self.tracks = Tracks::build(&self.rec, &self.edit);
            self.tracks_for = self.edit.clone();
            self.changed_at = Some(Instant::now());
        }
        if self.changed_at.is_some_and(|t| t.elapsed().as_secs_f64() > 1.0) {
            self.save();
        } else if self.changed_at.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        action
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
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
