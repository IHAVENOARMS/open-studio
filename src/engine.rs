//! Everything that turns a recording + edits into per-frame camera and cursor
//! state. Springs are pre-simulated at a fixed step over the whole timeline so
//! the preview and the export see exactly the same motion for any time `t`.

use crate::edit::{AnimationSpeed, EditConfig, MaskKind, ZoomCurve, ZoomFocus, ZoomSegment};
use crate::project::Recording;

const DT: f64 = 1.0 / 240.0;

// Automatic zoom generation.
// Gap allowed between two padded click intervals before they become
// separate zooms (so clicks ~5s apart still share one).
const MERGE_GAP: f64 = 2.5;
const LEAD_IN: f64 = 0.4;
const HOLD_AFTER: f64 = 2.5;
const MIN_LENGTH: f64 = 1.5;
// Clicks this close to the end don't start a zoom, and zooms end this early.
const END_IGNORE: f64 = 1.0;
const END_MARGIN: f64 = 0.8;

/// Clicks are grouped into bursts; each burst becomes one zoom segment that
/// starts a little before the first click and lingers after the last.
pub fn auto_zoom_segments(rec: &Recording, scale: f64) -> Vec<ZoomSegment> {
    let mut times: Vec<f64> = rec.events.clicks.iter().filter(|c| c.down).map(|c| c.t).collect();
    times.retain(|t| *t >= 0.0 && *t <= rec.duration - END_IGNORE);
    let last_end = (rec.duration - END_MARGIN).max(0.0);
    times.sort_by(f64::total_cmp);
    let mut segs: Vec<ZoomSegment> = Vec::new();
    for t in times {
        let start = (t - LEAD_IN).max(0.0);
        let end = (t + HOLD_AFTER).min(last_end);
        match segs.last_mut() {
            Some(last) if start - last.end <= MERGE_GAP => {
                last.end = last.end.max(end);
            }
            _ => segs.push(ZoomSegment::new(start, end, scale, crate::edit::ZoomKind::Zoom)),
        }
    }
    for s in &mut segs {
        if s.end - s.start < MIN_LENGTH {
            s.end = (s.start + MIN_LENGTH).min(last_end);
        }
    }
    segs.retain(|s| s.end - s.start > 0.3);
    segs
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Device frame bezel, as a fraction of the screen's longer side.
pub const BEZEL: f64 = 0.022;

/// True when a phone frame is drawn around the recording.
pub fn device_frame(rec: &Recording, edit: &EditConfig) -> bool {
    rec.source.kind == "device" && edit.device_frame.enabled
}

/// Scene layout in canvas units: the canvas is `aspect` wide and 1 tall.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub aspect: f64,
    /// The rounded card (screen plus inset frame).
    pub card: Rect,
    /// Where the (cropped) video sits.
    pub content: Rect,
    /// Kept part of the recording: x, y, w, h as fractions of it.
    pub crop: [f64; 4],
    pub video_w: f64,
    pub video_h: f64,
    /// A phone frame is drawn around the screen (card = phone body).
    pub device: bool,
}

impl Layout {
    pub fn new(rec: &Recording, edit: &EditConfig) -> Self {
        let crop = clamp_crop(edit.crop);
        let vr = (rec.width as f64 * crop[2]) / (rec.height.max(1) as f64 * crop[3]);
        let aspect = edit.aspect.ratio(vr);
        let pad = edit.padding * aspect.min(1.0);
        let (aw, ah) = ((aspect - 2.0 * pad).max(0.05), (1.0 - 2.0 * pad).max(0.05));
        let (w, h) = if aw / ah > vr { (ah * vr, ah) } else { (aw, aw / vr) };
        let mut card = Rect { x: (aspect - w) * 0.5, y: (1.0 - h) * 0.5, w, h };
        let mut content = card;
        if device_frame(rec, edit) {
            // The phone: the screen keeps its place and size, the body is a
            // bezel wider all round (shrunk to fit if needed).
            let b = BEZEL * h.max(w);
            let fit = (aw / (w + 2.0 * b)).min(ah / (h + 2.0 * b)).min(1.0);
            let (sw, sh, b) = (w * fit, h * fit, b * fit);
            content = Rect { x: (aspect - sw) * 0.5, y: (1.0 - sh) * 0.5, w: sw, h: sh };
            card = Rect { x: content.x - b, y: content.y - b, w: sw + 2.0 * b, h: sh + 2.0 * b };
        } else if edit.inset.enabled && edit.inset.size > 0.0 {
            // Shrink the video inside the card, keeping its aspect; the
            // position setting moves it towards an edge.
            let i = edit.inset.size * h;
            let (aw, ah) = ((w - 2.0 * i).max(0.01), (h - 2.0 * i).max(0.01));
            let (iw, ih) = if aw / ah > vr { (ah * vr, ah) } else { (aw, aw / vr) };
            let [ox, oy] = edit.inset.offset.map(|v| v.clamp(-1.0, 1.0));
            content = Rect {
                x: card.x + (w - iw) * 0.5 + ox * ((w - iw) * 0.5),
                y: card.y + (h - ih) * 0.5 + oy * ((h - ih) * 0.5),
                w: iw,
                h: ih,
            };
        }
        Self { aspect, card, content, crop, video_w: rec.width as f64, video_h: rec.height as f64, device: device_frame(rec, edit) }
    }

    /// Recording pixels to canvas units (through the crop).
    pub fn video_to_canvas(&self, x: f64, y: f64) -> (f64, f64) {
        self.uv_to_canvas(x / self.video_w, y / self.video_h)
    }

    /// Recording fractions (0..1 across the whole recording) to canvas units.
    pub fn uv_to_canvas(&self, u: f64, v: f64) -> (f64, f64) {
        let [cx, cy, cw, ch] = self.crop;
        (self.content.x + (u - cx) / cw * self.content.w, self.content.y + (v - cy) / ch * self.content.h)
    }

    pub fn canvas_to_uv(&self, x: f64, y: f64) -> (f64, f64) {
        let [cx, cy, cw, ch] = self.crop;
        (cx + (x - self.content.x) / self.content.w * cw, cy + (y - self.content.y) / self.content.h * ch)
    }
}

/// A crop that is inside the recording and not degenerate.
pub fn clamp_crop(c: [f64; 4]) -> [f64; 4] {
    let w = c[2].clamp(0.02, 1.0);
    let h = c[3].clamp(0.02, 1.0);
    [c[0].clamp(0.0, 1.0 - w), c[1].clamp(0.0, 1.0 - h), w, h]
}

/// Camera: centre point in canvas units and zoom factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub cx: f64,
    pub cy: f64,
    pub scale: f64,
}

impl Camera {
    pub fn identity(layout: &Layout) -> Self {
        Self { cx: layout.aspect * 0.5, cy: 0.5, scale: 1.0 }
    }
    fn lerp(a: Self, b: Self, f: f64) -> Self {
        Self { cx: a.cx + (b.cx - a.cx) * f, cy: a.cy + (b.cy - a.cy) * f, scale: a.scale + (b.scale - a.scale) * f }
    }
}

/// Tension/friction/mass spring, the model Screen Studio exposes.
#[derive(Clone, Copy)]
struct Spring {
    stiffness: f64,
    damping: f64,
    mass: f64,
}

impl Spring {
    /// Same damping ratio, response time scaled by `1/sqrt(speed)`.
    fn faster(self, speed: f64) -> Self {
        Self { stiffness: self.stiffness * speed, damping: self.damping * speed.sqrt(), mass: self.mass }
    }
    fn step(&self, x: &mut f64, v: &mut f64, target: f64, dt: f64) {
        let a = (self.stiffness * (target - *x) - self.damping * *v) / self.mass;
        *v += a * dt;
        *x += *v * dt;
    }
}

fn speed_factor(speed: AnimationSpeed) -> f64 {
    match speed {
        AnimationSpeed::Slow => 0.45,
        AnimationSpeed::Mellow => 1.0,
        AnimationSpeed::Quick => 2.0,
        AnimationSpeed::Rapid => 4.0,
    }
}

/// Slightly underdamped (ratio ~0.94): settles in ~0.8s at Mellow.
fn zoom_spring(speed: AnimationSpeed) -> Spring {
    Spring { stiffness: 200.0, damping: 40.0, mass: 2.25 }.faster(speed_factor(speed))
}

fn cursor_spring(speed: AnimationSpeed) -> Spring {
    Spring { stiffness: 470.0, damping: 70.0, mass: 3.0 }.faster(speed_factor(speed))
}

/// Pre-simulated tracks for one (recording, edit) pair.
pub struct Tracks {
    pub layout: Layout,
    cameras: Vec<Camera>,
    cursor: Vec<(f64, f64)>,
    duration: f64,
    /// Shortcuts shown by the keycap overlay.
    pub shortcuts: Vec<crate::shortcuts::Shortcut>,
    /// Caption lines (empty when captions are off or there is no transcript).
    pub captions: Vec<crate::transcript::Line>,
    /// Speech spans, for "Hide during silence" (empty when off).
    speech: Vec<(f64, f64)>,
    /// Face positions in the camera, for face tracking.
    face: Option<crate::face::FaceTrack>,
    /// Hide sensitive data and Hide faces covers.
    covers: std::sync::Arc<Vec<crate::privacy::Cover>>,
}

impl Tracks {
    pub fn build(rec: &Recording, edit: &EditConfig) -> Self {
        let layout = Layout::new(rec, edit);
        let n = (rec.duration / DT).ceil() as usize + 2;

        // Cursor first: the camera follows the smoothed cursor.
        let mut cursor = Vec::with_capacity(n);
        let start = rec.cursor_at(0.0).unwrap_or((layout.video_w * 0.5, layout.video_h * 0.5));
        let (mut x, mut y, mut vx, mut vy) = (start.0, start.1, 0.0, 0.0);
        let cs = cursor_spring(edit.cursor.speed);
        // Feed the spring a slightly future target so the smoothed cursor
        // stays in sync with the video instead of trailing it.
        let lead = cs.damping / cs.stiffness;
        let style = &edit.cursor;
        // Shake removal: the target only moves once the pointer leaves a
        // small dead zone around it, which drops hand tremor.
        let dead = layout.video_h * 0.006;
        let mut held = start;
        for i in 0..n {
            let t = i as f64 * DT;
            let mut ts = if style.smooth { t + lead } else { t };
            if style.stop_at_end > 0.0 {
                ts = ts.min((rec.duration - style.stop_at_end).max(0.0));
            }
            let (mut tx, mut ty) = rec.cursor_at(ts).unwrap_or(start);
            if style.remove_shakes {
                let (dx, dy) = (tx - held.0, ty - held.1);
                let d = (dx * dx + dy * dy).sqrt();
                if d > dead {
                    held = (tx - dx / d * dead, ty - dy / d * dead);
                }
                (tx, ty) = held;
            }
            if style.loop_position && style.loop_duration > 0.0 {
                // Glide back to where the cursor started so the video loops.
                let f = ((t - (rec.duration - style.loop_duration)) / style.loop_duration).clamp(0.0, 1.0);
                let e = f * f * (3.0 - 2.0 * f);
                (tx, ty) = (tx + (start.0 - tx) * e, ty + (start.1 - ty) * e);
            }
            // Slices set to "Disable smooth mouse movement" follow the real cursor.
            if edit.cursor.smooth && !edit.slice_props(t).raw_cursor {
                cs.step(&mut x, &mut vx, tx, DT);
                cs.step(&mut y, &mut vy, ty, DT);
            } else {
                (x, y) = (tx, ty);
                (vx, vy) = (0.0, 0.0);
            }
            cursor.push((x, y));
        }

        let curve = edit.zoom_curve.unwrap_or(ZoomCurve::from_speed(edit.zoom_speed));
        let spring = match curve {
            ZoomCurve::Spring { tension, friction, mass } => Spring { stiffness: tension, damping: friction, mass },
            // With a bezier curve the spring only smooths cursor following
            // inside a zoom.
            ZoomCurve::Bezier { .. } => zoom_spring(AnimationSpeed::Quick),
        };
        // Bezier transitions: (from, start time) per channel, restarted when
        // the zoom level or segment changes. Targets may move during one.
        let mut bez: Option<((f64, f64, f64), f64)> = None;
        let mut bez_target_scale = 1.0f64;
        let mut cam = Camera::identity(&layout);
        let (mut vcx, mut vcy, mut vls) = (0.0, 0.0, 0.0);
        let mut ls = 0.0f64; // ln(scale): zoom feels linear in log space
        let mut follow = (cam.cx, cam.cy);
        let mut active: Option<usize> = None;
        let mut cameras = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 * DT;
            let seg_idx = edit.zoom_segments.iter().position(|s| s.kind == crate::edit::ZoomKind::Zoom && t >= s.start && t < s.end);
            let (target, target_scale) = match seg_idx {
                Some(si) => {
                    let seg = &edit.zoom_segments[si];
                    let scale = seg.scale.max(1.0);
                    let focus = match seg.focus {
                        ZoomFocus::Auto => {
                            // Aim at where the cursor will be shortly, so the
                            // camera doesn't trail behind it.
                            let ahead = ((t + 0.15) / DT) as usize;
                            let (fx, fy) = cursor[ahead.min(n - 1)];
                            layout.video_to_canvas(fx, fy)
                        }
                        ZoomFocus::Manual { x, y } => layout.uv_to_canvas(x, y),
                    };
                    let (vw, vh) = (layout.aspect / scale, 1.0 / scale);
                    if active != Some(si) {
                        follow = focus;
                    } else {
                        // Edge push: only move once the cursor leaves the
                        // middle of the view.
                        let (mx, my) = (vw * 0.25, vh * 0.35);
                        follow.0 = follow.0.clamp(focus.0 - mx, focus.0 + mx);
                        follow.1 = follow.1.clamp(focus.1 - my, focus.1 + my);
                    }
                    (clamp_center(follow, vw, vh, &layout), scale)
                }
                None => ((layout.aspect * 0.5, 0.5), 1.0),
            };
            let segment_changed = active != seg_idx;
            active = seg_idx;
            match curve {
                _ if edit.instant_animation => {
                    // Cut straight to the target, then follow the cursor.
                    if segment_changed || i == 0 {
                        (cam.cx, cam.cy, ls) = (target.0, target.1, target_scale.ln());
                        (vcx, vcy, vls) = (0.0, 0.0, 0.0);
                    } else {
                        spring.step(&mut cam.cx, &mut vcx, target.0, DT);
                        spring.step(&mut cam.cy, &mut vcy, target.1, DT);
                        ls = target_scale.ln();
                    }
                }
                ZoomCurve::Spring { .. } => {
                    spring.step(&mut cam.cx, &mut vcx, target.0, DT);
                    spring.step(&mut cam.cy, &mut vcy, target.1, DT);
                    spring.step(&mut ls, &mut vls, target_scale.ln(), DT);
                }
                ZoomCurve::Bezier { x1, y1, x2, y2, duration } => {
                    if segment_changed || (target_scale - bez_target_scale).abs() > 1e-9 || bez.is_none() {
                        bez = Some(((cam.cx, cam.cy, ls), t));
                        bez_target_scale = target_scale;
                    }
                    let ((fx, fy, fl), t0) = bez.unwrap();
                    let p = ((t - t0) / duration.max(0.05)).clamp(0.0, 1.0);
                    if p < 1.0 {
                        let e = crate::edit::cubic_bezier(x1, y1, x2, y2, p);
                        cam.cx = fx + (target.0 - fx) * e;
                        cam.cy = fy + (target.1 - fy) * e;
                        ls = fl + (target_scale.ln() - fl) * e;
                        (vcx, vcy, vls) = (0.0, 0.0, 0.0);
                    } else {
                        // Settled: follow the cursor with a quick spring.
                        spring.step(&mut cam.cx, &mut vcx, target.0, DT);
                        spring.step(&mut cam.cy, &mut vcy, target.1, DT);
                        ls = target_scale.ln();
                    }
                }
            }
            cam.scale = ls.exp().max(1.0);
            // Never show beyond the canvas edge.
            let (cx, cy) = clamp_in_canvas(cam.cx, cam.cy, cam.scale, &layout);
            cameras.push(Camera { cx, cy, scale: cam.scale });
        }
        let o = &edit.shortcuts;
        let shortcuts = if o.show {
            crate::shortcuts::detect(rec, o.single_keys, &o.custom).into_iter().filter(|s| !o.disabled.contains(&s.id)).collect()
        } else {
            vec![]
        };
        let captions = match &rec.transcript {
            // About 42 characters across 16:9; fewer in narrow (phone) frames.
            Some(t) if edit.captions.show => crate::transcript::lines(t, edit, caption_chars(layout.aspect)),
            _ => vec![],
        };
        let speech = if edit.camera.hide_during_silence && rec.camera.is_some() { crate::voice::speech_spans(rec) } else { vec![] };
        let face = if edit.camera.face_tracking { crate::face::FaceTrack::load(&rec.dir) } else { None };
        let covers = crate::privacy::covers(rec, &edit.privacy);
        Self { layout, cameras, cursor, duration: rec.duration, shortcuts, captions, speech, face, covers }
    }

    fn index(&self, t: f64) -> (usize, f64) {
        let f = (t.clamp(0.0, self.duration) / DT).max(0.0);
        let i = (f.floor() as usize).min(self.cameras.len() - 2);
        (i, (f - i as f64).clamp(0.0, 1.0))
    }

    pub fn camera(&self, t: f64) -> Camera {
        let (i, f) = self.index(t);
        Camera::lerp(self.cameras[i], self.cameras[i + 1], f)
    }

    /// Smoothed cursor position in video pixels.
    pub fn cursor(&self, t: f64) -> (f64, f64) {
        let (i, f) = self.index(t);
        let (a, b) = (self.cursor[i], self.cursor[i + 1]);
        (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)
    }
}

fn clamp_center(c: (f64, f64), vw: f64, vh: f64, layout: &Layout) -> (f64, f64) {
    let r = &layout.content;
    let axis = |v: f64, size: f64, lo: f64, len: f64, mid: f64| {
        if size >= len { mid } else { v.clamp(lo + size * 0.5, lo + len - size * 0.5) }
    };
    (axis(c.0, vw, r.x, r.w, layout.aspect * 0.5), axis(c.1, vh, r.y, r.h, 0.5))
}

fn clamp_in_canvas(cx: f64, cy: f64, scale: f64, layout: &Layout) -> (f64, f64) {
    let (hw, hh) = (layout.aspect * 0.5 / scale, 0.5 / scale);
    (cx.clamp(hw, layout.aspect - hw), cy.clamp(hh, 1.0 - hh))
}

/// Shutter length at full blur strength, in frames.
pub const SHUTTER_FRAMES: f64 = 1.0;

/// Everything the compositor needs for one output frame.
#[derive(Clone, Debug)]
pub struct FrameState {
    pub camera: Camera,
    /// Camera at the start and end of the shutter interval, for motion blur.
    /// Equal to `camera` when blur is off.
    pub blur_from: Camera,
    pub blur_to: Camera,
    pub cursor: Option<CursorState>,
    pub masks: Vec<ActiveMask>,
    pub click: Option<ClickFx>,
    pub keys: Option<KeyOverlay>,
    pub caption: Option<CaptionFrame>,
    pub loupe: Option<LoupeFrame>,
    /// Camera picture (webcam / phone camera) in canvas units.
    pub camera_view: Option<CameraFrame>,
    /// Where the screen card is drawn: canvas point p goes to
    /// `(ox + s * p.x, oy + s * p.y)` (Split layout moves and shrinks it).
    pub screen_xf: [f64; 3],
}

/// The camera picture for one frame.
#[derive(Clone, Copy, Debug)]
pub struct CameraFrame {
    /// Canvas units.
    pub rect: Rect,
    /// Corner radius, canvas units.
    pub radius: f64,
    pub opacity: f64,
    pub mirror: bool,
    pub shadow: f64,
    /// Part of the camera picture shown: x, y, w, h (normalized).
    pub crop: [f64; 4],
    /// 0..1, background removed (uses the matte).
    pub cutout: f64,
}

/// Camera crop at camera time `t`: the chosen crop, moved to keep the face
/// centred when face tracking is on.
fn camera_crop(edit: &EditConfig, tracks: &Tracks, t: f64) -> [f64; 4] {
    let [x, y, w, h] = edit.camera.crop;
    let (w, h) = (w.clamp(0.1, 1.0), h.clamp(0.1, 1.0));
    let (mut x, mut y) = (x.clamp(0.0, 1.0 - w), y.clamp(0.0, 1.0 - h));
    if let Some((fx, fy)) = tracks.face.as_ref().and_then(|f| f.at(t)) {
        // Centre on the face, a little above it so the shoulders show.
        x = (fx - w / 2.0).clamp(0.0, 1.0 - w);
        y = (fy + h * 0.08 - h / 2.0).clamp(0.0, 1.0 - h);
    }
    [x, y, w, h]
}

/// Glass Loupe lens: magnifies a spot of the video in place.
#[derive(Clone, Copy, Debug)]
pub struct LoupeFrame {
    /// Lens centre, normalized video coordinates.
    pub x: f64,
    pub y: f64,
    /// Radius as a fraction of the video height.
    pub radius: f64,
    pub magnification: f64,
    /// 0..1 grow-in / shrink-out.
    pub amount: f64,
}

/// The caption line on screen.
#[derive(Clone, Debug)]
pub struct CaptionFrame {
    pub words: Vec<String>,
    /// Right to left (the sentence begins with Arabic).
    pub rtl: bool,
    /// How many words have been spoken.
    pub spoken: usize,
    pub opacity: f64,
    /// Slide-in offset as a fraction of the caption height.
    pub slide: f64,
}

/// Click highlight drawn under the cursor.
#[derive(Clone, Copy, Debug)]
pub struct ClickFx {
    /// Canvas units.
    pub x: f64,
    pub y: f64,
    /// 0..1 through the animation.
    pub progress: f64,
    pub kind: crate::edit::ClickVisual,
}

/// Keycaps for the shortcut being shown.
#[derive(Clone, Debug)]
pub struct KeyOverlay {
    pub keys: Vec<&'static str>,
    /// Fade and pop-in, 0..1.
    pub opacity: f64,
    pub scale: f64,
    /// Size multiplier from the settings.
    pub size: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct ActiveMask {
    pub kind: MaskKind,
    /// Normalized video rect: x, y, w, h.
    pub rect: [f64; 4],
    /// Effect strength including fades (highlight dim amount, or 1 for blur).
    pub amount: f64,
}

/// Most masks the compositor takes in one frame.
pub const MAX_MASKS: usize = 64;

/// Masks in effect at `t`. Highlights fade in and out over 0.3s; blurs are
/// fully on for their whole span so nothing sensitive leaks at the edges.
/// Order: the user's blurs, Hide sensitive data and Hide faces covers, then
/// highlights, so a crowded frame drops highlights before anything private.
fn active_masks(edit: &EditConfig, tracks: &Tracks, t: f64) -> Vec<ActiveMask> {
    const FADE: f64 = 0.3;
    let user = |blur: bool| {
        edit.masks.iter().filter(move |m| !m.disabled && t >= m.start && t < m.end && (m.kind == MaskKind::Blur) == blur).map(move |m| {
            let amount = match m.kind {
                MaskKind::Highlight => {
                    let f = ((t - m.start) / FADE).min((m.end - t) / FADE).clamp(0.0, 1.0);
                    m.opacity * f * f * (3.0 - 2.0 * f)
                }
                _ => 1.0,
            };
            ActiveMask { kind: m.kind, rect: m.rect, amount }
        })
    };
    let covers = tracks.covers.iter().filter_map(|c| {
        let kind = match c.kind {
            crate::privacy::CoverKind::Text => MaskKind::Cover,
            crate::privacy::CoverKind::Face => MaskKind::Face,
        };
        c.at(t).map(|rect| ActiveMask { kind, rect, amount: 1.0 })
    });
    user(true).chain(covers).chain(user(false)).take(MAX_MASKS).collect()
}

#[derive(Clone, Debug)]
pub struct CursorState {
    /// Canvas units.
    pub x: f64,
    pub y: f64,
    /// Multiplier including click squish.
    pub scale: f64,
    pub opacity: f64,
    pub shape: CursorShape,
    /// Tilt in radians (rotate while moving).
    pub rotation: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CursorShape {
    /// Canonical cursor name, drawn from `theme` (an XCursor theme
    /// directory) or the built-in vector set.
    Named { name: String, theme: Option<String> },
    /// Captured image: path, hotspot in pixels, capture scale.
    Custom { path: std::path::PathBuf, hotspot: (f64, f64), scale: f64 },
    /// Soft touch circle (phone recordings).
    Circle,
}

fn cursor_shape(rec: &Recording, edit: &EditConfig, t: f64) -> Option<CursorShape> {
    let named = |name: &str| CursorShape::Named { name: name.to_string(), theme: edit.cursor.theme.clone() };
    if edit.cursor.circle {
        return Some(CursorShape::Circle);
    }
    if edit.cursor.always_default {
        return Some(named("default"));
    }
    let mut s = rec.shape_at(t);
    if s == "hidden" {
        // Older recordings have brief false "hidden" readings (the pointer
        // moved out of the sampled patch); only long ones are real.
        let ev = &rec.events.shapes;
        let i = ev.partition_point(|e| e.t <= t).saturating_sub(1);
        let end = ev.get(i + 1).map_or(rec.duration, |e| e.t);
        let start = ev.get(i).map_or(0.0, |e| e.t);
        if end - start >= 2.0 {
            return None;
        }
        s = ev[..i].iter().rev().map(|e| e.shape.as_str()).find(|n| *n != "hidden").unwrap_or("default");
    }
    if let Some(i) = s.strip_prefix("custom:").and_then(|i| i.parse::<usize>().ok()) {
        if let Some(c) = rec.custom_cursors.get(i) {
            return Some(CursorShape::Custom { path: rec.dir.join(&c.file), hotspot: (c.hotspot[0], c.hotspot[1]), scale: c.scale });
        }
    }
    Some(named(s))
}

pub fn frame_state(rec: &Recording, edit: &EditConfig, tracks: &Tracks, t: f64, fps: f64) -> FrameState {
    let camera = tracks.camera(t);
    // Camera at both ends of a shutter interval centred on `t`. The renderer
    // splits the motion between them into zoom and pan parts and weights each
    // by its own strength.
    let (blur_from, blur_to) = if edit.motion_blur && (edit.zoom_blur > 0.0 || edit.pan_blur > 0.0) {
        let s = SHUTTER_FRAMES / fps;
        (tracks.camera(t - s / 2.0), tracks.camera(t + s / 2.0))
    } else {
        (camera, camera)
    };
    let cursor = if rec.cursor_embedded || rec.events.moves.is_empty() || edit.cursor.hidden || edit.slice_props(t).hide_cursor {
        None
    } else {
        cursor_shape(rec, edit, t).map(|shape| {
            let (vx, vy) = tracks.cursor(t);
            let (x, y) = tracks.layout.video_to_canvas(vx, vy);
            let rotation = cursor_tilt(tracks, edit, t);
            CursorState { x, y, scale: edit.cursor.size * click_squish(rec, edit, t), opacity: idle_opacity(rec, edit, t), shape, rotation }
        })
    };
    // Click effects also show with a hidden cursor (taps on phone recordings).
    let click = if rec.cursor_embedded || rec.events.moves.is_empty() { None } else { click_fx(rec, edit, tracks, t) };
    let caption = caption_frame(rec, edit, tracks, t);
    let loupe = loupe_frame(edit, tracks, t);
    let mut fs = FrameState {
        camera,
        blur_from,
        blur_to,
        cursor,
        masks: active_masks(edit, tracks, t),
        click,
        keys: key_overlay(edit, tracks, t),
        caption,
        loupe,
        camera_view: None,
        screen_xf: [0.0, 0.0, 1.0],
    };
    if let Some(track) = &rec.camera {
        apply_camera_layout(&mut fs, edit, &tracks.layout, track.width as f64 / track.height.max(1) as f64, t);
        if let Some(view) = &mut fs.camera_view {
            if edit.camera.hide_during_silence {
                view.opacity *= crate::voice::visibility(&tracks.speech, t);
            }
            view.crop = camera_crop(edit, tracks, t - track.offset - edit.camera.delay);
        }
        if fs.camera_view.as_ref().is_some_and(|v| v.opacity <= 0.001) {
            fs.camera_view = None;
        }
    }
    fs
}

// ---- Camera layouts ---------------------------------------------------------

/// Layout changes animate over this long.
pub const LAYOUT_TRANSITION: f64 = 0.45;

/// Where things go for one camera layout, before transitions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub camera: Rect,
    pub radius: f64,
    pub opacity: f64,
    /// Screen card transform (ox, oy, scale).
    pub screen: [f64; 3],
    /// 1 = the screen zoom is switched off (split, fullscreen).
    pub no_zoom: f64,
    /// 1 = background removed (Cutout layout).
    pub cutout: f64,
}

impl Placement {
    fn lerp(a: Self, b: Self, f: f64) -> Self {
        let l = |x: f64, y: f64| x + (y - x) * f;
        Self {
            camera: Rect { x: l(a.camera.x, b.camera.x), y: l(a.camera.y, b.camera.y), w: l(a.camera.w, b.camera.w), h: l(a.camera.h, b.camera.h) },
            radius: l(a.radius, b.radius),
            opacity: l(a.opacity, b.opacity),
            screen: [l(a.screen[0], b.screen[0]), l(a.screen[1], b.screen[1]), l(a.screen[2], b.screen[2])],
            no_zoom: l(a.no_zoom, b.no_zoom),
            cutout: l(a.cutout, b.cutout),
        }
    }
}

/// The layout in effect at `t`: an enabled segment, or the project default.
fn layout_at(edit: &EditConfig, t: f64) -> (crate::edit::CameraLayoutKind, [u8; 2], Option<f64>) {
    use crate::edit::{CameraDefaultLayout, CameraLayoutKind};
    let cam = &edit.camera;
    if let Some(seg) = edit.camera_layouts.iter().find(|s| !s.disabled && t >= s.start && t < s.end) {
        return (seg.kind, seg.position.unwrap_or(cam.position), seg.custom_size);
    }
    let kind = match cam.layout {
        CameraDefaultLayout::Overlay => CameraLayoutKind::Default,
        CameraDefaultLayout::Split => CameraLayoutKind::Split,
        CameraDefaultLayout::Cutout => CameraLayoutKind::Cutout,
    };
    (kind, cam.position, None)
}

/// Placement of camera and screen for one layout. `zoom` is the screen zoom
/// scale (the camera shrinks while zoomed in).
pub fn placement(
    layout: &Layout,
    edit: &EditConfig,
    camera_aspect: f64,
    kind: crate::edit::CameraLayoutKind,
    position: [u8; 2],
    size: Option<f64>,
    zoom_progress: f64,
) -> Placement {
    use crate::edit::CameraLayoutKind;
    let cam = &edit.camera;
    let a = layout.aspect;
    let ar = cam.aspect.ratio(camera_aspect).clamp(0.3, 3.0);
    let pad = edit.padding * a.min(1.0);
    let screen_radius = edit.radius / 1080.0;
    let identity = [0.0, 0.0, 1.0];
    let overlay = |shrink: f64| {
        let mut h = size.unwrap_or(cam.size);
        // Shrinks in step with the screen zoom, on the zoom's own curve.
        if cam.shrink_on_zoom && size.is_none() {
            h += (cam.zoom_size - h) * zoom_progress.clamp(0.0, 1.0);
        }
        let mut w = h * ar;
        if w > a * 0.9 {
            w = a * 0.9;
            h = w / ar;
        }
        let m = 0.04;
        let x = [m, (a - w) / 2.0, a - w - m][position[0].min(2) as usize];
        let y = [m, (1.0 - h) / 2.0, 1.0 - h - m][position[1].min(2) as usize];
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let (w, h) = (w * shrink, h * shrink);
        Rect { x: cx - w / 2.0, y: cy - h / 2.0, w, h }
    };
    let round = |r: &Rect| cam.roundness.clamp(0.0, 1.0) * r.w.min(r.h) / 2.0;
    match kind {
        CameraLayoutKind::Default => {
            let r = overlay(1.0);
            Placement { radius: round(&r), camera: r, opacity: 1.0, screen: identity, no_zoom: 0.0, cutout: 0.0 }
        }
        CameraLayoutKind::Cutout => {
            // The person stands on the bottom edge, larger than the bubble,
            // in the camera's own proportions so nothing is cut off.
            let car = (camera_aspect * cam.crop[2] / cam.crop[3].max(0.01)).clamp(0.3, 3.0);
            let mut h = size.unwrap_or(cam.cutout_size).clamp(0.15, 0.98);
            let mut w = h * car;
            if w > a * 0.7 {
                w = a * 0.7;
                h = w / car;
            }
            let m = 0.02;
            let x = [m, (a - w) / 2.0, a - w - m][position[0].min(2) as usize];
            let r = Rect { x, y: 1.0 - h, w, h };
            Placement { radius: 0.0, camera: r, opacity: 1.0, screen: identity, no_zoom: 0.0, cutout: 1.0 }
        }
        CameraLayoutKind::Hide => {
            let r = overlay(0.8);
            Placement { radius: round(&r), camera: r, opacity: 0.0, screen: identity, no_zoom: 0.0, cutout: 0.0 }
        }
        CameraLayoutKind::Fullscreen => {
            let r = Rect { x: pad, y: pad, w: a - 2.0 * pad, h: 1.0 - 2.0 * pad };
            Placement { camera: r, radius: screen_radius, opacity: 1.0, screen: identity, no_zoom: 1.0, cutout: 0.0 }
        }
        CameraLayoutKind::Split => {
            let area = Rect { x: pad, y: pad, w: a - 2.0 * pad, h: 1.0 - 2.0 * pad };
            let gap = (pad * 0.5).max(0.02);
            let card = layout.card;
            let card_ar = card.w / card.h;
            let fit = |w: f64, h: f64| if w / h > card_ar { (h * card_ar, h) } else { (w, w / card_ar) };
            let (screen, camera) = if a >= 1.0 {
                // Side by side: camera column on the right, as tall as the screen.
                let cw = area.w * 0.34;
                let (sw, sh) = fit(area.w - cw - gap, area.h);
                let x = area.x + (area.w - (sw + gap + cw)) / 2.0;
                let y = area.y + (area.h - sh) / 2.0;
                (Rect { x, y, w: sw, h: sh }, Rect { x: x + sw + gap, y, w: cw, h: sh })
            } else {
                // Stacked: camera below the screen, as wide as it.
                let ch = area.h * 0.36;
                let (sw, sh) = fit(area.w, area.h - ch - gap);
                let x = area.x + (area.w - sw) / 2.0;
                let y = area.y + (area.h - (sh + gap + ch)) / 2.0;
                (Rect { x, y, w: sw, h: sh }, Rect { x, y: y + sh + gap, w: sw, h: ch })
            };
            let s = screen.w / card.w;
            Placement {
                camera,
                radius: screen_radius,
                opacity: 1.0,
                screen: [screen.x - s * card.x, screen.y - s * card.y, s],
                no_zoom: 1.0,
                cutout: 0.0,
            }
        }
    }
}

/// Placement at `t`, animating for [`LAYOUT_TRANSITION`] after each layout change.
pub fn placement_at(layout: &Layout, edit: &EditConfig, camera_aspect: f64, t: f64, zoom_progress: f64) -> Placement {
    let at = |t: f64| {
        let (kind, pos, size) = layout_at(edit, t);
        placement(layout, edit, camera_aspect, kind, pos, size, zoom_progress)
    };
    let now = at(t);
    let last_change = edit
        .camera_layouts
        .iter()
        .filter(|s| !s.disabled)
        .flat_map(|s| [s.start, s.end])
        .filter(|b| *b <= t && t - *b < LAYOUT_TRANSITION)
        .fold(f64::NEG_INFINITY, f64::max);
    if last_change.is_finite() {
        let before = at(last_change - 1e-4);
        if before != now {
            let p = crate::edit::cubic_bezier(0.32, 0.72, 0.0, 1.0, (t - last_change) / LAYOUT_TRANSITION);
            return Placement::lerp(before, now, p);
        }
    }
    now
}

/// How far the screen zoom is through its animation, 0 (none) to 1 (fully
/// zoomed in): the zoom's log scale over that of the zoom it is heading to
/// or coming back from. The zoom curve eases log scale, so this follows the
/// same curve.
fn zoom_progress(edit: &EditConfig, scale: f64, t: f64) -> f64 {
    if scale <= 1.0005 {
        return 0.0;
    }
    let zooms = edit.zoom_segments.iter().filter(|s| s.kind == crate::edit::ZoomKind::Zoom);
    // The zoom in effect, or else the last one that ended (zooming out).
    let target = zooms
        .clone()
        .find(|s| t >= s.start && t < s.end)
        .or_else(|| zooms.filter(|s| s.end <= t).max_by(|a, b| a.end.total_cmp(&b.end)))
        .map_or(scale, |s| s.scale.max(scale));
    if target <= 1.0005 { 0.0 } else { (scale.ln() / target.ln()).clamp(0.0, 1.0) }
}

fn apply_camera_layout(fs: &mut FrameState, edit: &EditConfig, layout: &Layout, camera_aspect: f64, t: f64) {
    let p = placement_at(layout, edit, camera_aspect, t, zoom_progress(edit, fs.camera.scale, t));
    // Split and fullscreen show the whole screen card: blend the zoom out.
    let id = Camera::identity(layout);
    fs.camera = Camera::lerp(fs.camera, id, p.no_zoom);
    fs.blur_from = Camera::lerp(fs.blur_from, id, p.no_zoom);
    fs.blur_to = Camera::lerp(fs.blur_to, id, p.no_zoom);
    let [ox, oy, s] = p.screen;
    fs.screen_xf = p.screen;
    if let Some(c) = &mut fs.cursor {
        (c.x, c.y) = (ox + s * c.x, oy + s * c.y);
        c.scale *= s;
    }
    if let Some(c) = &mut fs.click {
        (c.x, c.y) = (ox + s * c.x, oy + s * c.y);
    }
    if !edit.camera.hidden && p.opacity > 0.001 {
        fs.camera_view = Some(CameraFrame {
            rect: p.camera,
            radius: p.radius,
            opacity: p.opacity,
            mirror: edit.camera.mirror,
            shadow: edit.camera.shadow,
            crop: edit.camera.crop,
            cutout: p.cutout,
        });
    }
}

/// The loupe active at `t`: grows in and out over 0.3s with a soft overshoot
/// and follows the smoothed cursor (Auto) or stays on its point (Manual).
fn loupe_frame(edit: &EditConfig, tracks: &Tracks, t: f64) -> Option<LoupeFrame> {
    const GROW: f64 = 0.3;
    let seg = edit.zoom_segments.iter().find(|s| s.kind == crate::edit::ZoomKind::Loupe && t >= s.start && t < s.end)?;
    let f = ((t - seg.start) / GROW).min((seg.end - t) / GROW).clamp(0.0, 1.0);
    let amount = crate::edit::cubic_bezier(0.34, 1.3, 0.64, 1.0, f);
    let (x, y) = match seg.focus {
        ZoomFocus::Auto => {
            let (vx, vy) = tracks.cursor(t);
            (vx / tracks.layout.video_w, vy / tracks.layout.video_h)
        }
        ZoomFocus::Manual { x, y } => (x, y),
    };
    Some(LoupeFrame { x, y, radius: 0.15 * seg.size, magnification: seg.scale.max(1.0), amount })
}

/// The caption line around `t`; lines stay up briefly after their last word.
fn caption_frame(rec: &Recording, edit: &EditConfig, tracks: &Tracks, t: f64) -> Option<CaptionFrame> {
    const HOLD: f64 = 0.6;
    const FADE: f64 = 0.15;
    let transcript = rec.transcript.as_ref()?;
    let lines = &tracks.captions;
    let i = lines.partition_point(|l| l.start <= t);
    let line = lines[..i].last()?;
    let next_start = lines.get(i).map_or(f64::INFINITY, |n| n.start);
    let end = (line.end + HOLD).min(next_start);
    if t >= end {
        return None;
    }
    let words = &transcript.words;
    let texts = line.words.iter().map(|w| edit.captions.word_text(*w, &words[*w].text).to_string()).collect();
    let spoken = line.words.iter().filter(|w| words[**w].start <= t).count();
    let (fade_in, fade_out) = (((t - line.start) / FADE).min(1.0), ((end - t) / FADE).min(1.0));
    // Lines that follow straight on from the previous one don't re-animate.
    let joined = i >= 2 && lines[i - 2].end + HOLD >= line.start;
    let (opacity, slide) = match edit.captions.animation {
        crate::edit::CaptionAnimation::Appear => (1.0, 0.0),
        crate::edit::CaptionAnimation::Fade => (if joined { fade_out } else { fade_in.min(fade_out) }, 0.0),
        crate::edit::CaptionAnimation::Slide => {
            let p = if joined { 1.0 } else { fade_in };
            (p.min(fade_out), (1.0 - p).powi(3) * 0.6)
        }
    };
    Some(CaptionFrame { words: texts, rtl: line.rtl, spoken, opacity, slide })
}

/// Longest caption line, in characters, for a canvas of this aspect ratio.
fn caption_chars(aspect: f64) -> usize {
    ((42.0 * aspect / (16.0 / 9.0)).round() as usize).clamp(12, 42)
}

/// Tilt following horizontal speed, up to about 25 degrees at full setting.
fn cursor_tilt(tracks: &Tracks, edit: &EditConfig, t: f64) -> f64 {
    if edit.cursor.rotate <= 0.0 {
        return 0.0;
    }
    let h = 0.03;
    let (a, b) = (tracks.cursor(t - h), tracks.cursor(t + h));
    // Screen heights per second.
    let v = (b.0 - a.0) / (2.0 * h) / tracks.layout.video_h;
    (v * 0.35).tanh() * 0.44 * edit.cursor.rotate
}

/// Circle or ripple at the most recent click, for 0.5s.
fn click_fx(rec: &Recording, edit: &EditConfig, tracks: &Tracks, t: f64) -> Option<ClickFx> {
    const LEN: f64 = 0.5;
    if edit.cursor.click_visual == crate::edit::ClickVisual::None {
        return None;
    }
    let clicks = &rec.events.clicks;
    let i = clicks.partition_point(|c| c.t <= t);
    let c = clicks[..i].iter().rev().find(|c| c.down)?;
    let progress = (t - c.t) / LEN;
    if progress >= 1.0 {
        return None;
    }
    // Drawn where the rendered cursor is, so smoothing doesn't detach it.
    let (vx, vy) = tracks.cursor(c.t);
    let (x, y) = tracks.layout.video_to_canvas(vx, vy);
    Some(ClickFx { x, y, progress, kind: edit.cursor.click_visual })
}

/// The shortcut on screen at `t`: shown for 1.4s or until the next one.
/// How long a shortcut stays on screen (unless the next one comes sooner).
pub const SHORTCUT_SHOW: f64 = 1.4;

fn key_overlay(edit: &EditConfig, tracks: &Tracks, t: f64) -> Option<KeyOverlay> {
    const SHOW: f64 = SHORTCUT_SHOW;
    const FADE_IN: f64 = 0.12;
    const FADE_OUT: f64 = 0.25;
    let list = &tracks.shortcuts;
    let i = list.partition_point(|s| s.t <= t);
    let s = list[..i].last()?;
    let end = list.get(i).map_or(s.t + SHOW, |n| n.t.min(s.t + SHOW));
    let age = t - s.t;
    if t >= end {
        return None;
    }
    let fin = (age / FADE_IN).min(1.0);
    let fout = ((end - t) / FADE_OUT).min(1.0);
    // Back-out pop: 0.9 -> ~1.02 -> 1.
    let pop = 1.0 - 0.1 * (1.0 - fin).powi(2) + 0.02 * (fin * std::f64::consts::PI).sin();
    Some(KeyOverlay { keys: s.keys.clone(), opacity: fin.min(fout), scale: pop, size: edit.shortcuts.size })
}

fn click_squish(rec: &Recording, edit: &EditConfig, t: f64) -> f64 {
    if !edit.cursor.click_effect {
        return 1.0;
    }
    let clicks = &rec.events.clicks;
    let i = clicks.partition_point(|c| c.t <= t);
    let Some(c) = clicks[..i].iter().rev().find(|c| c.down) else { return 1.0 };
    let d = t - c.t;
    let press = 0.13;
    let amount = if d < press { (d / press).powf(0.5) } else { (-(d - press) * 8.0).exp() };
    1.0 - 0.2 * amount
}

fn idle_opacity(rec: &Recording, edit: &EditConfig, t: f64) -> f64 {
    if !edit.cursor.hide_when_idle {
        return 1.0;
    }
    // Time since the cursor last actually moved.
    let m = &rec.events.moves;
    let i = m.partition_point(|s| s[0] <= t);
    let mut last_move = f64::NEG_INFINITY;
    for k in (1..i).rev() {
        if (m[k][1] - m[k - 1][1]).abs() + (m[k][2] - m[k - 1][2]).abs() > 0.5 {
            last_move = m[k][0];
            break;
        }
    }
    let idle = t - last_move;
    (1.0 - (idle - 2.0) / 0.4).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{EditConfig, ZoomCurve, ZoomSegment};
    use crate::project::{Events, Recording, Source};

    fn rec() -> Recording {
        Recording {
            version: 1,
            video: "screen.mp4".into(),
            width: 1920,
            height: 1080,
            fps: 60,
            duration: 6.0,
            cursor_embedded: false,
            source: Source::default(),
            events: Events { moves: vec![[0.0, 960.0, 540.0], [6.0, 960.0, 540.0]], ..Default::default() },
            custom_cursors: vec![],
            audio: vec![],
            camera: None,
            dir: Default::default(),
            transcript: None,
        }
    }

    fn scales(curve: ZoomCurve) -> Vec<f64> {
        let edit = EditConfig {
            zoom_segments: vec![ZoomSegment::new(1.0, 5.0, 2.0, crate::edit::ZoomKind::Zoom)],
            zoom_curve: Some(curve),
            ..Default::default()
        };
        let tracks = Tracks::build(&rec(), &edit);
        (0..=60).map(|i| tracks.camera(1.0 + i as f64 * 0.025).scale).collect()
    }

    #[test]
    fn bezier_zoom_follows_curve_and_duration() {
        let ease = scales(ZoomCurve::Bezier { x1: 0.16, y1: 1.0, x2: 0.3, y2: 1.0, duration: 1.0 });
        assert!(ease[20] > 1.5 && ease[20] < 2.0, "halfway {}", ease[20]);
        assert!((ease[40] - 2.0).abs() < 1e-6, "done at 1.0s: {}", ease[40]);
        let back = scales(ZoomCurve::Bezier { x1: 0.34, y1: 1.56, x2: 0.64, y2: 1.0, duration: 1.0 });
        let peak = back.iter().cloned().fold(0.0, f64::max);
        assert!(peak > 2.05, "back-out should overshoot, peak {peak}");
        assert!((back[40] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn spring_presets_match_previous_behaviour() {
        let s = scales(ZoomCurve::from_speed(crate::edit::AnimationSpeed::Mellow));
        assert!(s[0] < 1.05 && s[40] > 1.9);
    }

    fn moving_rec() -> Recording {
        let mut r = rec();
        r.events.moves = (0..=60).map(|i| [i as f64 * 0.1, 100.0 + i as f64 * 20.0, 540.0]).collect();
        r
    }

    #[test]
    fn cursor_options() {
        let r = moving_rec();
        let mut edit = EditConfig { zoom_segments: vec![], ..Default::default() };
        edit.cursor.smooth = false;
        edit.cursor.stop_at_end = 2.0;
        let t = Tracks::build(&r, &edit);
        assert!((t.cursor(5.5).0 - t.cursor(4.2).0).abs() < 1.0, "stopped in the last 2s");

        edit.cursor.stop_at_end = 0.0;
        edit.cursor.loop_position = true;
        edit.cursor.loop_duration = 1.0;
        let t = Tracks::build(&r, &edit);
        assert!((t.cursor(6.0).0 - 100.0).abs() < 1.0, "back at start: {}", t.cursor(6.0).0);

        edit.cursor.hidden = true;
        assert!(frame_state(&r, &edit, &t, 1.0, 60.0).cursor.is_none());
    }

    #[test]
    fn instant_animation_cuts() {
        let mut edit = EditConfig {
            zoom_segments: vec![ZoomSegment::new(1.0, 5.0, 2.0, crate::edit::ZoomKind::Zoom)],
            ..Default::default()
        };
        edit.instant_animation = true;
        let t = Tracks::build(&rec(), &edit);
        assert!((t.camera(1.01).scale - 2.0).abs() < 1e-6);
        assert!((t.camera(5.01).scale - 1.0).abs() < 1e-6);
    }

    #[test]
    fn camera_layout_placement_and_transition() {
        use crate::edit::{CameraLayoutKind, CameraLayoutSegment};
        let r = rec();
        let mut edit = EditConfig { zoom_segments: vec![], ..Default::default() };
        let layout = Layout::new(&r, &edit);
        let ar = 16.0 / 9.0;
        // Default: bottom-right corner, inside the canvas.
        let d = placement(&layout, &edit, ar, CameraLayoutKind::Default, [2, 2], None, 0.0);
        assert!((d.camera.x + d.camera.w - (layout.aspect - 0.04)).abs() < 1e-9);
        assert!((d.camera.y + d.camera.h - 0.96).abs() < 1e-9);
        assert_eq!(d.screen, [0.0, 0.0, 1.0]);
        // Zoomed in, the camera shrinks.
        let z = placement(&layout, &edit, ar, CameraLayoutKind::Default, [2, 2], None, 1.0);
        assert!(z.camera.h < d.camera.h);
        // Split: screen shrinks to the left, camera to its right.
        let s = placement(&layout, &edit, ar, CameraLayoutKind::Split, [2, 2], None, 0.0);
        assert!(s.screen[2] < 1.0 && s.no_zoom == 1.0);
        let screen_right = s.screen[0] + s.screen[2] * (layout.card.x + layout.card.w);
        assert!(s.camera.x > screen_right);

        // A Fullscreen layout from 2s: animates in, then holds.
        edit.camera_layouts.push(CameraLayoutSegment::new(2.0, 4.0, CameraLayoutKind::Fullscreen));
        let full = placement(&layout, &edit, ar, CameraLayoutKind::Fullscreen, [2, 2], None, 0.0);
        let mid = placement_at(&layout, &edit, ar, 2.0 + LAYOUT_TRANSITION / 2.0, 0.0);
        assert!(mid.camera.w > d.camera.w && mid.camera.w < full.camera.w);
        let done = placement_at(&layout, &edit, ar, 2.0 + LAYOUT_TRANSITION + 0.01, 0.0);
        assert_eq!(done, full);
        // Disabled layouts are ignored.
        edit.camera_layouts[0].disabled = true;
        assert_eq!(placement_at(&layout, &edit, ar, 3.0, 0.0), d);
    }

    #[test]
    fn camera_shrinks_with_zoom_progress() {
        let mut edit = EditConfig {
            zoom_segments: vec![ZoomSegment::new(1.0, 5.0, 2.0, crate::edit::ZoomKind::Zoom)],
            ..Default::default()
        };
        edit.zoom_curve = Some(ZoomCurve::SMOOTH);
        assert_eq!(zoom_progress(&edit, 1.0, 0.5), 0.0);
        assert!((zoom_progress(&edit, 2.0f64.sqrt(), 1.2) - 0.5).abs() < 1e-9);
        assert!((zoom_progress(&edit, 2.0, 3.0) - 1.0).abs() < 1e-9);
        // Zooming back out after the segment: measured against that zoom.
        assert!((zoom_progress(&edit, 2.0f64.sqrt(), 5.2) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn brief_hidden_cursor_readings_are_ignored() {
        use crate::project::ShapeEvent;
        let mut r = rec();
        let ev = |t: f64, shape: &str| ShapeEvent { t, shape: shape.into() };
        r.events.shapes = vec![ev(0.0, "text"), ev(1.0, "hidden"), ev(1.4, "default"), ev(3.0, "hidden")];
        let edit = EditConfig::default();
        // 0.4 s of "hidden" keeps the shape from before it.
        assert!(matches!(cursor_shape(&r, &edit, 1.2), Some(CursorShape::Named { name, .. }) if name == "text"));
        // 3 s of "hidden" (to the end) really hides it.
        assert!(cursor_shape(&r, &edit, 4.0).is_none());
    }
}

#[cfg(test)]
mod device_frame_tests {
    use super::*;
    use crate::project::{Events, Recording, Source};

    #[test]
    fn device_frame_wraps_the_screen() {
        let mut rec = Recording {
            version: 1,
            video: "screen.mp4".into(),
            width: 1080,
            height: 2340,
            fps: 30,
            duration: 2.0,
            cursor_embedded: true,
            source: Source { kind: "device".into(), ..Default::default() },
            events: Events::default(),
            custom_cursors: vec![],
            audio: vec![],
            camera: None,
            dir: Default::default(),
            transcript: None,
        };
        let edit = EditConfig::default();
        let l = Layout::new(&rec, &edit);
        assert!(l.device);
        let b = (l.card.w - l.content.w) / 2.0;
        assert!(b > 0.0 && ((l.card.h - l.content.h) / 2.0 - b).abs() < 1e-9, "even bezel all round");
        assert!((b - BEZEL * l.content.h).abs() < 1e-9);
        // The screen keeps its shape; the phone fits the canvas.
        assert!((l.content.w / l.content.h - 1080.0 / 2340.0).abs() < 1e-9);
        assert!(l.card.x >= 0.0 && l.card.y >= 0.0 && l.card.x + l.card.w <= l.aspect + 1e-9 && l.card.y + l.card.h <= 1.0 + 1e-9);
        // Turned off, or a desktop recording: no frame.
        let mut off = edit.clone();
        off.device_frame.enabled = false;
        let l = Layout::new(&rec, &off);
        assert!(!l.device && l.card.w == l.content.w);
        rec.source.kind = "monitor".into();
        assert!(!Layout::new(&rec, &edit).device);
    }
}
