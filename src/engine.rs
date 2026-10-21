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
            _ => segs.push(ZoomSegment { start, end, scale, focus: ZoomFocus::Auto }),
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

/// Scene layout in canvas units: the canvas is `aspect` wide and 1 tall.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub aspect: f64,
    pub content: Rect,
    pub video_w: f64,
    pub video_h: f64,
}

impl Layout {
    pub fn new(rec: &Recording, edit: &EditConfig) -> Self {
        let vr = rec.width as f64 / rec.height.max(1) as f64;
        let aspect = edit.aspect.ratio(vr);
        let pad = edit.padding * aspect.min(1.0);
        let (aw, ah) = ((aspect - 2.0 * pad).max(0.05), (1.0 - 2.0 * pad).max(0.05));
        let (w, h) = if aw / ah > vr { (ah * vr, ah) } else { (aw, aw / vr) };
        let content = Rect { x: (aspect - w) * 0.5, y: (1.0 - h) * 0.5, w, h };
        Self { aspect, content, video_w: rec.width as f64, video_h: rec.height as f64 }
    }

    pub fn video_to_canvas(&self, x: f64, y: f64) -> (f64, f64) {
        (self.content.x + x / self.video_w * self.content.w, self.content.y + y / self.video_h * self.content.h)
    }
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
        for i in 0..n {
            let t = i as f64 * DT;
            let (tx, ty) = rec.cursor_at(if edit.cursor.smooth { t + lead } else { t }).unwrap_or(start);
            if edit.cursor.smooth {
                cs.step(&mut x, &mut vx, tx, DT);
                cs.step(&mut y, &mut vy, ty, DT);
            } else {
                (x, y) = (tx, ty);
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
            let seg_idx = edit.zoom_segments.iter().position(|s| t >= s.start && t < s.end);
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
                        ZoomFocus::Manual { x, y } => layout.video_to_canvas(x * layout.video_w, y * layout.video_h),
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
        Self { layout, cameras, cursor, duration: rec.duration }
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
}

#[derive(Clone, Copy, Debug)]
pub struct ActiveMask {
    pub kind: MaskKind,
    /// Normalized video rect: x, y, w, h.
    pub rect: [f64; 4],
    /// Effect strength including fades (highlight dim amount, or 1 for blur).
    pub amount: f64,
}

/// Masks in effect at `t`. Highlights fade in and out over 0.3s; blurs are
/// fully on for their whole span so nothing sensitive leaks at the edges.
fn active_masks(edit: &EditConfig, t: f64) -> Vec<ActiveMask> {
    const FADE: f64 = 0.3;
    edit.masks
        .iter()
        .filter(|m| !m.disabled && t >= m.start && t < m.end)
        .map(|m| {
            let amount = match m.kind {
                MaskKind::Blur => 1.0,
                MaskKind::Highlight => {
                    let f = ((t - m.start) / FADE).min((m.end - t) / FADE).clamp(0.0, 1.0);
                    m.opacity * f * f * (3.0 - 2.0 * f)
                }
            };
            ActiveMask { kind: m.kind, rect: m.rect, amount }
        })
        .take(8)
        .collect()
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum CursorShape {
    /// Canonical cursor name, drawn from `theme` (an XCursor theme
    /// directory) or the built-in vector set.
    Named { name: String, theme: Option<String> },
    /// Captured image: path, hotspot in pixels, capture scale.
    Custom { path: std::path::PathBuf, hotspot: (f64, f64), scale: f64 },
}

fn cursor_shape(rec: &Recording, edit: &EditConfig, t: f64) -> Option<CursorShape> {
    let named = |name: &str| CursorShape::Named { name: name.to_string(), theme: edit.cursor.theme.clone() };
    if edit.cursor.always_default {
        return Some(named("default"));
    }
    let s = rec.shape_at(t);
    if s == "hidden" {
        return None;
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
    let cursor = if rec.cursor_embedded || rec.events.moves.is_empty() {
        None
    } else {
        cursor_shape(rec, edit, t).map(|shape| {
            let (vx, vy) = tracks.cursor(t);
            let (x, y) = tracks.layout.video_to_canvas(vx, vy);
            CursorState { x, y, scale: edit.cursor.size * click_squish(rec, edit, t), opacity: idle_opacity(rec, edit, t), shape }
        })
    };
    FrameState { camera, blur_from, blur_to, cursor, masks: active_masks(edit, t) }
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
    use crate::edit::{EditConfig, ZoomCurve, ZoomFocus, ZoomSegment};
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
            dir: Default::default(),
        }
    }

    fn scales(curve: ZoomCurve) -> Vec<f64> {
        let edit = EditConfig {
            zoom_segments: vec![ZoomSegment { start: 1.0, end: 5.0, scale: 2.0, focus: ZoomFocus::Auto }],
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
}
