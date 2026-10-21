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

/// Panel state: look thumbnails made from the current camera frame, the
/// face-tracking analysis and the LUT file picker.
#[derive(Default)]
pub struct State {
    /// Latest camera frame, set by the editor each frame.
    pub frame: Option<std::sync::Arc<super::player::Frame>>,
    thumbs: Option<(i64, Vec<egui::TextureHandle>, egui::TextureHandle)>,
    face_job: Option<(std::sync::Arc<std::sync::atomic::AtomicU32>, std::sync::mpsc::Receiver<Result<(), String>>)>,
    face_error: Option<String>,
    /// Set when face_track.json was written; the editor rebuilds its tracks.
    pub face_ready: bool,
    picking_lut: Option<std::sync::mpsc::Receiver<Vec<std::path::PathBuf>>>,
    cutout_job: Option<(std::sync::Arc<crate::cutout::Progress>, std::sync::mpsc::Receiver<Result<(), String>>)>,
    cutout_error: Option<String>,
    /// Set when the background-removal matte was written; the editor
    /// reloads the recording and opens the matte.
    pub cutout_ready: bool,
}

impl State {
    /// Starts removing the camera background in the background, if it
    /// isn't done or running already.
    pub fn start_cutout(&mut self, rec: &Recording, ctx: &egui::Context) {
        if self.cutout_job.is_some() || rec.camera.as_ref().is_none_or(|c| c.matte.is_some()) {
            return;
        }
        let progress = std::sync::Arc::new(crate::cutout::Progress::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let (r, p, ctx) = (rec.clone(), progress.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(crate::cutout::process(&r, &p).map(|_| ()).map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.cutout_job = Some((progress, rx));
        self.cutout_error = None;
    }

    /// Polls the job; returns its progress 0..1 while running.
    pub fn poll_cutout(&mut self, ctx: &egui::Context) -> Option<f32> {
        let (p, rx) = self.cutout_job.as_ref()?;
        match rx.try_recv() {
            Ok(Ok(())) => {
                self.cutout_job = None;
                self.cutout_ready = true;
                None
            }
            Ok(Err(e)) => {
                self.cutout_error = Some(e);
                self.cutout_job = None;
                None
            }
            Err(_) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
                Some(p.permille.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0)
            }
        }
    }
}

/// Progress or error line for background removal.
fn cutout_status(ui: &mut Ui, st: &mut State) {
    if let Some(p) = st.poll_cutout(ui.ctx()) {
        let w = ui.available_width();
        let (r, _) = ui.allocate_exact_size(vec2(w, 6.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(3), theme::TRACK);
        ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(w * p.max(0.02), 6.0)), CornerRadius::same(3), theme::ACCENT);
        help(ui, &if p == 0.0 { "Preparing background removal…".to_string() } else { format!("Removing background…  {:.0}%", p * 100.0) });
        if widgets::wide_button(ui, "Cancel").clicked() {
            if let Some((p, _)) = &st.cutout_job {
                p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    if let Some(e) = &st.cutout_error {
        ui.label(RichText::new(e).size(12.0).color(theme::RECORD));
    }
}

pub fn show(ui: &mut Ui, st: &mut State, edit: &mut EditConfig, rec: &Recording, layout: bool) {
    if rec.camera.is_none() {
        heading(ui, if layout { "Camera Layout" } else { "Camera Style" });
        help(ui, "Record with a camera to use this.");
        return;
    }
    if layout {
        camera_layout(ui, st, edit, rec);
    } else {
        camera_style(ui, st, edit, rec);
    }
}

fn camera_layout(ui: &mut Ui, st: &mut State, edit: &mut EditConfig, rec: &Recording) {
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
            (Some(CameraDefaultLayout::Cutout), "Camera cutout", true),
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
                    if v == CameraDefaultLayout::Cutout {
                        st.start_cutout(rec, ui.ctx());
                    }
                }
                let col = if !enabled { Color32::from_rgb(0x53, 0x53, 0x55) } else if selected { theme::TEXT } else { theme::TEXT_DIM };
                ui.label(RichText::new(label).size(12.5).color(col));
            });
        }
    });
    if c.layout == CameraDefaultLayout::Cutout {
        cutout_status(ui, st);
    }
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
    widgets::divider(ui);
    let has_mic = rec.audio.iter().any(|a| a.kind == "microphone");
    if has_mic {
        toggle_help(ui, "Hide during silence", "Hide your camera while you're not talking.", &mut c.hide_during_silence);
    } else {
        ui.add_enabled_ui(false, |ui| {
            let mut off = false;
            toggle_help(ui, "Hide during silence", "Record with a microphone to use this.", &mut off);
        });
    }
}

fn camera_style(ui: &mut Ui, st: &mut State, edit: &mut EditConfig, rec: &Recording) {
    heading(ui, "Camera Style");
    {
        let c = &mut edit.camera;
        theme::section(ui, "Camera aspect ratio");
        let opts: Vec<_> = CameraAspect::ALL.iter().map(|a| (*a, a.label())).collect();
        widgets::segmented(ui, &mut c.aspect, &opts);
        theme::section(ui, "Roundness");
        widgets::slider(ui, &mut c.roundness, 0.0..=1.0, Some(0.45));
        toggle_help(ui, "Mirror camera", "", &mut c.mirror);
        theme::section(ui, "Shadow");
        widgets::slider(ui, &mut c.shadow, 0.0..=1.0, Some(0.6));
    }
    widgets::divider(ui);
    crop_editor(ui, st, edit);
    widgets::divider(ui);
    face_tracking(ui, st, edit, rec);
    widgets::divider(ui);
    color_grading(ui, st, edit);
    widgets::divider(ui);
    {
        let c = &mut edit.camera;
        let mut on = c.layout == CameraDefaultLayout::Cutout;
        if toggle_help(ui, "Camera cutout", "Remove your background and place you in front of your screen. Processed on this computer.", &mut on) {
            c.layout = if on { CameraDefaultLayout::Cutout } else { CameraDefaultLayout::Overlay };
            if on {
                st.start_cutout(rec, ui.ctx());
            }
        }
        if on {
            cutout_status(ui, st);
        }
    }
    widgets::divider(ui);
    let c = &mut edit.camera;
    egui::CollapsingHeader::new(RichText::new("Advanced").size(13.5)).id_salt("camera-advanced").show(ui, |ui| {
        theme::section(ui, &format!("Camera delay  ·  {:+.0} ms", c.delay * 1000.0));
        help(ui, "Shift the camera picture if it isn't in sync with your voice. Positive values show it later.");
        widgets::slider(ui, &mut c.delay, -0.5..=0.5, Some(0.0));
    });
}

fn frame_image(f: &super::player::Frame, max_w: usize) -> egui::ColorImage {
    // Nearest-neighbour downscale; thumbnails are small.
    let (w, h) = (f.width as usize, f.height as usize);
    let tw = max_w.min(w).max(1);
    let th = (h * tw / w).max(1);
    let mut px = Vec::with_capacity(tw * th * 4);
    for y in 0..th {
        for x in 0..tw {
            let o = ((y * h / th) * w + x * w / tw) * 4;
            px.extend_from_slice(&f.rgba[o..o + 4]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([tw, th], &px)
}

/// Thumbnails of the current camera frame: the plain frame, then one per look.
fn ensure_thumbs(ui: &Ui, st: &mut State) {
    let Some(f) = st.frame.clone() else { return };
    // Refresh every few seconds of video, not every frame.
    if st.thumbs.as_ref().is_some_and(|(i, _, _)| (f.index - i).abs() < 90) {
        return;
    }
    let base = frame_image(&f, 120);
    let plain = ui.ctx().load_texture("camera-thumb", frame_image(&f, 320), egui::TextureOptions::LINEAR);
    let looks = crate::render::looks::Look::ALL
        .iter()
        .map(|look| {
            let g = look.grade();
            let mut img = base.clone();
            for p in img.pixels.iter_mut() {
                let c = g.apply([p.r() as f32 / 255.0, p.g() as f32 / 255.0, p.b() as f32 / 255.0]);
                *p = Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
            }
            ui.ctx().load_texture(format!("look-{}", look.label()), img, egui::TextureOptions::LINEAR)
        })
        .collect();
    st.thumbs = Some((f.index, looks, plain));
}

/// Camera crop: the camera picture with the kept part outlined; drag it to
/// move, and the zoom slider sets how much is kept.
fn crop_editor(ui: &mut Ui, st: &mut State, edit: &mut EditConfig) {
    theme::section(ui, "Camera crop");
    help(ui, "Keep only part of your camera picture.");
    ensure_thumbs(ui, st);
    let c = &mut edit.camera;
    let mut zoom = 1.0 / c.crop[2].max(0.1);
    if let Some((_, _, plain)) = &st.thumbs {
        let size = plain.size_vec2();
        let w = ui.available_width();
        let h = w * size.y / size.x;
        let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::drag());
        let p = ui.painter();
        p.image(plain.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let [x, y, cw, ch] = c.crop;
        let kept = Rect::from_min_size(pos2(r.left() + x as f32 * w, r.top() + y as f32 * h), vec2(cw as f32 * w, ch as f32 * h));
        // Dim what is cropped away.
        let dim = Color32::from_black_alpha(150);
        p.rect_filled(Rect::from_min_max(r.min, pos2(r.right(), kept.top())), 0.0, dim);
        p.rect_filled(Rect::from_min_max(pos2(r.left(), kept.bottom()), r.max), 0.0, dim);
        p.rect_filled(Rect::from_min_max(pos2(r.left(), kept.top()), pos2(kept.left(), kept.bottom())), 0.0, dim);
        p.rect_filled(Rect::from_min_max(pos2(kept.right(), kept.top()), pos2(r.right(), kept.bottom())), 0.0, dim);
        p.rect_stroke(kept, CornerRadius::same(4), Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
        if resp.dragged() {
            let d = resp.drag_delta();
            c.crop[0] = (c.crop[0] + (d.x / w) as f64).clamp(0.0, 1.0 - c.crop[2]);
            c.crop[1] = (c.crop[1] + (d.y / h) as f64).clamp(0.0, 1.0 - c.crop[3]);
        }
        resp.on_hover_cursor(egui::CursorIcon::Grab);
    } else {
        help(ui, "Play or seek the video to show the camera picture here.");
    }
    theme::section(ui, &format!("Crop zoom  ·  {:.1}×", zoom));
    if widgets::slider(ui, &mut zoom, 1.0..=3.0, Some(1.0)) {
        let s = 1.0 / zoom;
        let (cx, cy) = (c.crop[0] + c.crop[2] / 2.0, c.crop[1] + c.crop[3] / 2.0);
        c.crop = [(cx - s / 2.0).clamp(0.0, 1.0 - s), (cy - s / 2.0).clamp(0.0, 1.0 - s), s, s];
    }
}

fn face_tracking(ui: &mut Ui, st: &mut State, edit: &mut EditConfig, rec: &Recording) {
    let c = &mut edit.camera;
    let was = c.face_tracking;
    toggle_help(ui, "Face tracking", "Keep your face centred in the camera as you move. Works with Camera crop zoomed in.", &mut c.face_tracking);
    // Analyse the camera video the first time it's turned on.
    if c.face_tracking && !was && crate::face::FaceTrack::load(&rec.dir).is_none() && st.face_job.is_none() {
        let progress = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        let (r, p, ctx) = (rec.clone(), progress.clone(), ui.ctx().clone());
        std::thread::spawn(move || {
            let _ = tx.send(crate::face::analyze(&r, &p).map(|_| ()).map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        st.face_job = Some((progress, rx));
        st.face_error = None;
    }
    if c.face_tracking && c.crop[2] > 0.99 {
        // Tracking moves the crop, so it needs room: zoom in a little.
        c.crop = [0.2, 0.2, 0.6, 0.6];
    }
    if let Some((p, rx)) = &st.face_job {
        match rx.try_recv() {
            Ok(Ok(())) => {
                st.face_job = None;
                st.face_ready = true;
            }
            Ok(Err(e)) => {
                st.face_error = Some(e);
                st.face_job = None;
            }
            Err(_) => {
                let v = p.load(std::sync::atomic::Ordering::Relaxed) as f32 / 10.0;
                help(ui, &format!("Finding your face…  {v:.0}%"));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
    }
    if let Some(e) = &st.face_error {
        ui.label(RichText::new(e).size(12.0).color(theme::RECORD));
    }
}

fn color_grading(ui: &mut Ui, st: &mut State, edit: &mut EditConfig) {
    use crate::render::looks::Look;
    theme::section(ui, "Color grading");
    help(ui, "Pick a look for your camera and set how strong it is, or load your own LUT file.");
    ensure_thumbs(ui, st);
    let c = &mut edit.camera;
    let cols = 4;
    let gap = 6.0;
    let w = ((ui.available_width() - gap * (cols - 1) as f32) / cols as f32).floor();
    let looks = std::iter::once(Look::None).chain(Look::ALL);
    let all: Vec<Look> = looks.collect();
    for row in all.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for look in row {
                ui.vertical(|ui| {
                    ui.set_width(w);
                    let (r, resp) = ui.allocate_exact_size(vec2(w, w * 0.75), Sense::click());
                    let p = ui.painter();
                    let tex = st.thumbs.as_ref().and_then(|(_, t, plain)| match look {
                        Look::None => Some(plain.clone()),
                        l => Look::ALL.iter().position(|x| x == l).and_then(|i| t.get(i).cloned()),
                    });
                    match tex {
                        Some(t) => {
                            // Cover-crop the thumbnail to the tile.
                            let s = t.size_vec2();
                            let (ta, ra) = (s.x / s.y, r.width() / r.height());
                            let uv = if ta > ra {
                                let f = ra / ta;
                                Rect::from_min_max(pos2((1.0 - f) / 2.0, 0.0), pos2((1.0 + f) / 2.0, 1.0))
                            } else {
                                let f = ta / ra;
                                Rect::from_min_max(pos2(0.0, (1.0 - f) / 2.0), pos2(1.0, (1.0 + f) / 2.0))
                            };
                            p.add(egui::epaint::RectShape::filled(r, CornerRadius::same(8), Color32::WHITE).with_texture(t.id(), uv));
                        }
                        None => {
                            p.rect_filled(r, CornerRadius::same(8), theme::SURFACE);
                        }
                    }
                    let sel = c.look == *look && c.lut.is_none();
                    if sel {
                        p.rect_stroke(r.expand(1.5), CornerRadius::same(9), Stroke::new(2.0, theme::KNOB), StrokeKind::Outside);
                    } else if resp.hovered() {
                        p.rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0, Color32::from_white_alpha(60)), StrokeKind::Inside);
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        c.look = *look;
                        c.lut = None;
                    }
                    ui.label(RichText::new(look.label()).size(11.5).color(if sel { theme::TEXT } else { theme::TEXT_DIM }));
                });
            }
        });
    }
    if c.look != Look::None || c.lut.is_some() {
        theme::section(ui, &format!("Strength  ·  {:.0}%", c.look_strength * 100.0));
        widgets::slider(ui, &mut c.look_strength, 0.0..=1.0, Some(1.0));
    }
    // LUT file.
    if let Some(rx) = &st.picking_lut {
        match rx.try_recv() {
            Ok(files) => {
                if let Some(f) = files.into_iter().next() {
                    match crate::render::looks::load_cube(&f) {
                        Ok(_) => {
                            c.lut = Some(f.to_string_lossy().into_owned());
                            c.look = Look::None;
                        }
                        Err(e) => st.face_error = Some(format!("Couldn't load the LUT: {e}")),
                    }
                }
                st.picking_lut = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => st.picking_lut = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)),
        }
    }
    match &c.lut {
        Some(path) => {
            let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            help(ui, &format!("LUT: {name}"));
            if widgets::wide_button_icon(ui, Some(Icon::Trash), "Remove LUT").clicked() {
                c.lut = None;
            }
        }
        None => {
            if widgets::wide_button_icon(ui, Some(Icon::Plus), "Load LUT").clicked() && st.picking_lut.is_none() {
                let filter = ashpd::desktop::file_chooser::FileFilter::new("3D LUT (.cube)").glob("*.cube");
                st.picking_lut = Some(crate::dialogs::open_files(ui.ctx().clone(), "Load LUT", Some(filter), false));
            }
        }
    }
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
            (CameraLayoutKind::Cutout, Icon::CamCutout, "Cutout"),
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
            CameraLayoutKind::Cutout => "Show only you, without your background, in front of the screen",
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
