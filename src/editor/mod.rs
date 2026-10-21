//! The editor window: preview canvas, settings sidebar and timeline.

pub mod player;
mod sidebar;
mod timeline;
mod widgets;

use crate::edit::{EditConfig, ZoomFocus};
use crate::engine::{Tracks, frame_state};
use crate::export::{self, ExportOptions, Progress};
use crate::project::{ProjectPaths, Recording};
use crate::render::{Compositor, FORMAT};
use crate::theme;
use anyhow::Result;
use egui::{Color32, CornerRadius, Key, Rect, Sense, Vec2};
use egui_wgpu::wgpu;
use player::Player;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub use sidebar::Tab;

struct PreviewTarget {
    _texture: wgpu::Texture,
    /// sRGB view we render into.
    view: wgpu::TextureView,
    size: (u32, u32),
    id: egui::TextureId,
}

struct ExportJob {
    out: PathBuf,
    progress: Arc<Progress>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
}

struct ExportDialog {
    height: u32,
    fps: u32,
    job: Option<ExportJob>,
}

pub enum Action {
    None,
    GoHome,
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
    comp: Compositor,
    target: Option<PreviewTarget>,
    shown_index: i64,
    time: f64,
    playing: Option<(f64, Instant)>,
    selected: Option<usize>,
    tab: Tab,
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
            comp: Compositor::new(rs.device.clone(), rs.queue.clone()),
            target: None,
            shown_index: -1,
            time: start,
            playing: None,
            selected: None,
            tab: Tab::Background,
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
        } else {
            self.playing = None;
            self.player.pause();
            self.player.seek(self.time);
        }
    }

    fn seek(&mut self, t: f64) {
        self.time = t.clamp(0.0, self.rec.duration);
        if self.playing.is_some() {
            let now = Instant::now();
            self.playing = Some((self.time, now));
            self.player.play(self.time, now);
        } else {
            self.player.seek(self.time);
        }
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

        egui::Panel::top("topbar")
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(egui::RichText::new("🗀  Projects").size(13.5)).frame(false)).clicked() {
                        action = Action::GoHome;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::primary_button(ui, "  ⮉  Export  ").clicked() && self.export.is_none() {
                            self.export = Some(ExportDialog { height: 1080, fps: 60, job: None });
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
            .exact_size(196.0)
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
                    sidebar::show(ui, &mut self.edit, &mut self.tab, &mut self.selected, &self.rec);
                });
                let main_rect = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x - side_w - 16.0, full.max.y));
                let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(main_rect).layout(egui::Layout::top_down(egui::Align::Min)));
                ui.horizontal(|ui| {
                    let label = format!("▭  {}", self.edit.aspect.label());
                    ui.menu_button(egui::RichText::new(label).size(13.5), |ui| {
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
            if let Some(i) = self.selected.take() {
                if i < self.edit.zoom_segments.len() {
                    self.edit.zoom_segments.remove(i);
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
        // Clicking the preview sets the focus point of a manual zoom.
        if let (Some(i), Some(pos)) = (self.selected, resp.interact_pointer_pos()) {
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
        if let Some(ZoomFocus::Manual { x, y }) = self.selected.and_then(|i| self.edit.zoom_segments.get(i)).map(|s| s.focus)
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
        let mut close = false;
        let mut start = false;
        #[allow(deprecated)]
        let modal_id = egui::Id::new("export");
        egui::Modal::new(modal_id).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(egui::RichText::new("Export").font(theme::semibold(16.0)));
            ui.add_space(4.0);
            match &dlg.job {
                None => {
                    theme::section(ui, "Resolution");
                    widgets::segmented(
                        ui,
                        &mut dlg.height,
                        &[(720, "720p"), (1080, "1080p"), (1440, "1440p"), (2160, "4K")],
                    );
                    theme::section(ui, "Frame rate");
                    widgets::segmented(ui, &mut dlg.fps, &[(30, "30 fps"), (60, "60 fps")]);
                    let (w, h) = export::output_size(self.tracks.layout.aspect, dlg.height);
                    ui.label(egui::RichText::new(format!("MP4 (H.264), {w} × {h}")).color(theme::TEXT_DIM));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, "  Export video  ").clicked() {
                            start = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
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
                        Some(Ok(())) => {
                            ui.label("Export finished.");
                            ui.label(egui::RichText::new(job.out.display().to_string()).color(theme::TEXT_DIM).small());
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
        if start {
            let out = crate::util::recordings_dir().join(format!("{}.mp4", self.name));
            let progress = Arc::new(Progress::default());
            let result = Arc::new(Mutex::new(None));
            let (p, r) = (progress.clone(), result.clone());
            let (dir, rec, edit) = (self.dir.clone(), self.rec.clone(), self.edit.clone());
            let opts = ExportOptions { out: out.clone(), height: dlg.height, fps: dlg.fps };
            let (device, queue) = (self.comp.device().clone(), self.comp.queue().clone());
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let mut comp = Compositor::new(device, queue);
                let res = export::export(&mut comp, &dir, &rec, &edit, &opts, &p).map_err(|e| format!("{e:#}"));
                *r.lock().unwrap() = Some(res);
                ctx2.request_repaint();
            });
            dlg.job = Some(ExportJob { out, progress, result });
        }
        if close {
            self.export = None;
        }
    }
}
