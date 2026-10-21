//! Timeline: ruler, amber clip track (with trim handles), the purple zoom
//! track and the pink mask track. Items on the two lower tracks are added,
//! moved, resized and selected the same way.

use crate::edit::{CameraLayoutKind, CameraLayoutSegment, EditConfig, Mask, MaskKind, ZoomFocus, ZoomSegment};
use crate::icons::{self, Icon};
use crate::theme;
use crate::util::format_time;
use egui::{Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

const EDGE: f32 = 7.0;
/// How far a hovered zoom segment rises, in points.
const LIFT: f32 = 4.0;
const MIN_SEG: f64 = 0.3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Zoom,
    Mask,
    /// Camera layouts (only with a camera track).
    Camera,
}

/// A selected timeline item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub track: Track,
    pub index: usize,
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    Scrub,
    Move { sel: Selection, grab: f64 },
    ResizeStart(Selection),
    ResizeEnd(Selection),
    TrimStart,
    TrimEnd,
}

impl Drag {
    fn item(self) -> Option<Selection> {
        match self {
            Drag::Move { sel, .. } | Drag::ResizeStart(sel) | Drag::ResizeEnd(sel) => Some(sel),
            _ => None,
        }
    }
}

fn spans(edit: &EditConfig, track: Track) -> Vec<(f64, f64)> {
    match track {
        Track::Zoom => edit.zoom_segments.iter().map(|z| (z.start, z.end)).collect(),
        Track::Mask => edit.masks.iter().map(|m| (m.start, m.end)).collect(),
        Track::Camera => edit.camera_layouts.iter().map(|l| (l.start, l.end)).collect(),
    }
}

fn set_span(edit: &mut EditConfig, sel: Selection, start: f64, end: f64) {
    match sel.track {
        Track::Zoom => {
            let z = &mut edit.zoom_segments[sel.index];
            (z.start, z.end) = (start, end);
        }
        Track::Mask => {
            let m = &mut edit.masks[sel.index];
            (m.start, m.end) = (start, end);
        }
        Track::Camera => {
            let l = &mut edit.camera_layouts[sel.index];
            (l.start, l.end) = (start, end);
        }
    }
}

/// Adds an item to a track (kept sorted) and returns its selection.
fn add_item(edit: &mut EditConfig, track: Track, start: f64, end: f64) -> Selection {
    let index = match track {
        Track::Zoom => {
            edit.zoom_segments.push(ZoomSegment::new(start, end, edit.default_zoom, crate::edit::ZoomKind::Zoom));
            edit.zoom_segments.sort_by(|a, b| a.start.total_cmp(&b.start));
            edit.zoom_segments.iter().position(|z| z.start == start).unwrap_or(0)
        }
        Track::Mask => add_mask(edit, Mask::new(start, end)),
        Track::Camera => add_layout(edit, CameraLayoutSegment::new(start, end, CameraLayoutKind::Fullscreen)),
    };
    Selection { track, index }
}

/// Inserts a camera layout keeping the list sorted; returns its index.
pub fn add_layout(edit: &mut EditConfig, seg: CameraLayoutSegment) -> usize {
    edit.camera_layouts.push(seg);
    edit.camera_layouts.sort_by(|a, b| a.start.total_cmp(&b.start));
    edit.camera_layouts.iter().position(|l| l.start == seg.start).unwrap_or(0)
}

/// Inserts a mask keeping the list sorted; returns its index.
pub fn add_mask(edit: &mut EditConfig, mask: Mask) -> usize {
    edit.masks.push(mask);
    edit.masks.sort_by(|a, b| a.start.total_cmp(&b.start));
    edit.masks.iter().position(|m| m.start == mask.start).unwrap_or(0)
}

/// Free span for a new item of up to `len` seconds at `t` on a track.
pub fn free_span_on(edit: &EditConfig, track: Track, t: f64, len: f64, duration: f64) -> Option<(f64, f64)> {
    let items = spans(edit, track);
    if items.iter().any(|(s, e)| t >= *s && t < *e) {
        return None;
    }
    let next = items.iter().map(|(s, _)| *s).filter(|s| *s > t).fold(duration, f64::min);
    let start = t;
    let end = (start + len).min(next);
    (end - start >= MIN_SEG).then_some((start, end))
}

#[derive(Default)]
pub struct State {
    drag: Option<Drag>,
}

#[derive(Default)]
pub struct Output {
    pub seek: Option<f64>,
    pub toggle_play: bool,
}

struct Geo {
    left: f32,
    width: f32,
    duration: f64,
}

impl Geo {
    fn x(&self, t: f64) -> f32 {
        self.left + (t / self.duration) as f32 * self.width
    }
    fn t(&self, x: f32) -> f64 {
        (((x - self.left) / self.width) as f64 * self.duration).clamp(0.0, self.duration)
    }
}

enum Hit {
    Item(Selection, Drag),
    TrimStart,
    TrimEnd,
    Empty(Track),
    Other,
}

/// Transport controls under the preview.
pub fn playback_bar(ui: &mut Ui, time: f64, duration: f64, playing: bool) -> Output {
    let mut out = Output::default();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    let c = rect.center();
    let p = ui.painter().clone();
    let dim = theme::TEXT_DIM;
    p.text(pos2(c.x - 120.0, c.y), egui::Align2::RIGHT_CENTER, format_time_precise(time), egui::FontId::proportional(14.0), dim);
    p.text(pos2(c.x + 120.0, c.y), egui::Align2::LEFT_CENTER, format_time_precise(duration), egui::FontId::proportional(14.0), dim);

    let button = |ui: &mut Ui, center: Pos2, id: &str| {
        let r = Rect::from_center_size(center, vec2(32.0, 32.0));
        let resp = ui.interact(r, ui.scope_id().with(id), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let col = if resp.hovered() { Color32::WHITE } else { theme::TEXT };
        (resp, col)
    };
    let icon_button = |ui: &mut Ui, center: Pos2, id: &str, icon: Icon, size: f32| {
        let (resp, col) = button(ui, center, id);
        icons::paint(&p, Rect::from_center_size(center, vec2(size, size)), icon, col);
        resp
    };
    if icon_button(ui, pos2(c.x - 64.0, c.y), "prev", Icon::SkipBack, 18.0).clicked() {
        out.seek = Some(0.0);
    }
    let play = icon_button(ui, c, "play", if playing { Icon::Pause } else { Icon::Play }, 14.0);
    p.circle_stroke(c, 14.0, Stroke::new(1.8, if play.hovered() { Color32::WHITE } else { theme::TEXT }));
    if play.clicked() {
        out.toggle_play = true;
    }
    if icon_button(ui, pos2(c.x + 64.0, c.y), "next", Icon::SkipForward, 18.0).clicked() {
        out.seek = Some(duration);
    }
    out
}

fn format_time_precise(t: f64) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u64;
    let s = t - m as f64 * 60.0;
    format!("{m}:{s:05.2}")
}

pub fn show(
    ui: &mut Ui,
    state: &mut State,
    duration: f64,
    edit: &mut EditConfig,
    time: f64,
    _playing: bool,
    selected: &mut Option<Selection>,
    waveform: Option<&[f32]>,
    has_camera: bool,
) -> Output {
    let mut out = Output::default();
    let full = ui.available_rect_before_wrap();
    let geo = Geo { left: full.left() + 10.0, width: full.width() - 20.0, duration: duration.max(0.01) };
    let ruler = Rect::from_min_size(full.min, vec2(full.width(), 30.0));
    let clip = Rect::from_min_max(pos2(full.left(), ruler.bottom() + 6.0), pos2(full.right(), ruler.bottom() + 66.0));
    let zoom = Rect::from_min_max(pos2(full.left(), clip.bottom() + 10.0), pos2(full.right(), clip.bottom() + 70.0));
    let mask = Rect::from_min_max(pos2(full.left(), zoom.bottom() + 10.0), pos2(full.right(), zoom.bottom() + 70.0));
    let cam = Rect::from_min_max(pos2(full.left(), mask.bottom() + 10.0), pos2(full.right(), mask.bottom() + 70.0));
    let track_rect = |t: Track| match t {
        Track::Zoom => zoom,
        Track::Mask => mask,
        Track::Camera => cam,
    };
    let tracks: &[Track] = if has_camera { &[Track::Zoom, Track::Mask, Track::Camera] } else { &[Track::Zoom, Track::Mask] };
    let bottom = if has_camera { cam.bottom() } else { mask.bottom() };
    let area = Rect::from_min_max(full.min, pos2(full.right(), bottom));
    let resp = ui.allocate_rect(area, Sense::click_and_drag());
    let p = ui.painter_at(area.expand(4.0));
    let (trim_s, trim_e) = edit.trim_range(duration);

    // Ruler.
    let px_per_s = geo.width as f64 / geo.duration;
    let step = [1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0]
        .into_iter()
        .find(|s| s * px_per_s >= 70.0)
        .unwrap_or(1200.0);
    let mut t = 0.0;
    while t <= geo.duration + 1e-6 {
        let x = geo.x(t);
        if t > 0.0 {
            p.text(pos2(x, ruler.top() + 10.0), egui::Align2::CENTER_CENTER, format_time(t).trim_end_matches(".0"), egui::FontId::proportional(12.5), theme::TEXT_DIM);
        }
        p.circle_filled(pos2(x, ruler.bottom() - 6.0), 1.5, theme::TEXT_DIM);
        let half = geo.x(t + step / 2.0);
        if t + step / 2.0 <= geo.duration {
            p.circle_filled(pos2(half, ruler.bottom() - 6.0), 1.5, Color32::from_rgb(0x4a, 0x4c, 0x52));
        }
        t += step;
    }

    // Clip track (trimmed region bright, the rest dimmed).
    let full_clip = Rect::from_x_y_ranges(geo.x(0.0)..=geo.x(duration), clip.y_range());
    gradient_bar(&p, full_clip, 10.0, &CLIP_STOPS, CLIP_EDGE, 0.5, CLIP_BORDER, 0.3);
    // Kept parts of the clip: the trim range split at the cuts.
    let kept = edit.kept_ranges(duration);
    for &(a, b) in &kept {
        let r = Rect::from_x_y_ranges(geo.x(a)..=geo.x(b), clip.y_range());
        // Neighbouring parts sit 2px apart so a cut reads as a split.
        let r = if kept.len() > 1 { r.shrink2(vec2(1.0, 0.0)) } else { r };
        gradient_bar(&p, r, 10.0, &CLIP_STOPS, CLIP_EDGE, 0.5, CLIP_BORDER, 1.0);
        if let Some(env) = waveform {
            paint_waveform(&p, r, &geo, env, a, b);
        }
        // Faint gridlines at every ruler tick (labelled and half steps),
        // drawn over the waveform as in Screen Studio.
        let grid_step = step / 2.0;
        let mut t = (a / grid_step).ceil() * grid_step;
        while t < b {
            let x = geo.x(t);
            if x > r.left() + 6.0 && x < r.right() - 6.0 && grid_step * px_per_s > 12.0 {
                p.line_segment([pos2(x, r.top() + 1.0), pos2(x, r.bottom() - 1.0)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xf0, 0xcc, 0x8c, 36)));
            }
            t += grid_step;
        }
        if r.width() > 60.0 {
            p.text(r.center() - vec2(0.0, 10.0), egui::Align2::CENTER_CENTER, "Clip", egui::FontId::proportional(12.5), Color32::from_rgb(0xe8, 0xc6, 0x86));
            p.text(r.center() + vec2(0.0, 9.0), egui::Align2::CENTER_CENTER, format!("{:.0}s  ·  1×", b - a), egui::FontId::proportional(13.5), Color32::WHITE);
        }
    }
    // Cut markers: a yellow pin with scissors; click to restore the cut.
    for c in edit.cuts.clone() {
        if c.end <= trim_s || c.start >= trim_e {
            continue;
        }
        let x = geo.x((c.start + c.end) * 0.5);
        let pin = Rect::from_center_size(pos2(x, clip.top() - 3.0), vec2(18.0, 18.0));
        let id = ui.scope_id().with(("cut", (c.start * 1000.0) as i64));
        let r = ui.interact(pin.expand(2.0), id, Sense::click());
        let fill = if r.hovered() { Color32::from_rgb(0xff, 0xd5, 0x5c) } else { Color32::from_rgb(0xf2, 0xb8, 0x2c) };
        p.circle_filled(pin.center(), 9.0, fill);
        p.add(egui::Shape::convex_polygon(
            vec![pin.center() + vec2(-5.0, 6.0), pin.center() + vec2(5.0, 6.0), pin.center() + vec2(0.0, 12.0)],
            fill,
            Stroke::NONE,
        ));
        crate::icons::paint(&p, pin.shrink(4.0), crate::icons::Icon::Scissors, Color32::from_rgb(0x3a, 0x26, 0x05));
        let r = r.on_hover_text(format!("{:.1}s cut. Click to restore.", c.end - c.start)).on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() {
            edit.restore(c.start, c.end);
        }
    }

    // Zoom and mask tracks.
    let hover = resp.hover_pos();
    let hit = |pos: Pos2, edit: &EditConfig| -> Hit {
        if clip.contains(pos) {
            if (pos.x - geo.x(trim_s)).abs() < EDGE {
                return Hit::TrimStart;
            }
            if (pos.x - geo.x(trim_e)).abs() < EDGE {
                return Hit::TrimEnd;
            }
        }
        for &track in tracks {
            if !track_rect(track).contains(pos) {
                continue;
            }
            for (i, (s, e)) in spans(edit, track).into_iter().enumerate() {
                let (x0, x1) = (geo.x(s), geo.x(e));
                if pos.x >= x0 - 2.0 && pos.x <= x1 + 2.0 {
                    let sel = Selection { track, index: i };
                    let d = if (pos.x - x0).abs() < EDGE {
                        Drag::ResizeStart(sel)
                    } else if (pos.x - x1).abs() < EDGE {
                        Drag::ResizeEnd(sel)
                    } else {
                        Drag::Move { sel, grab: geo.t(pos.x) - s }
                    };
                    return Hit::Item(sel, d);
                }
            }
            return Hit::Empty(track);
        }
        Hit::Other
    };

    // Items lift while hovered or being dragged, and settle back down when
    // the pointer leaves.
    let hovered_item = match (state.drag, hover.map(|h| hit(h, edit))) {
        (None, Some(Hit::Item(sel, _))) => Some(sel),
        _ => None,
    };
    let dragged_item = state.drag.and_then(Drag::item);
    for &track in tracks {
        for (i, (s, e)) in spans(edit, track).into_iter().enumerate() {
            let sel = Selection { track, index: i };
            let up = hovered_item == Some(sel) || dragged_item == Some(sel);
            let lift = smooth(ui, ui.scope_id().with(("lift", track as u8, i)), if up { 1.0 } else { 0.0 });
            let r = Rect::from_x_y_ranges(geo.x(s)..=geo.x(e), track_rect(track).y_range()).translate(vec2(0.0, -LIFT * lift));
            if lift > 0.01 {
                let shadow = egui::epaint::Shadow {
                    offset: [0, (3.0 + 3.0 * lift) as i8],
                    blur: (6.0 + 10.0 * lift) as u8,
                    spread: 0,
                    color: Color32::from_black_alpha((150.0 * lift) as u8),
                };
                p.add(shadow.as_shape(r, CornerRadius::same(10)));
            }
            let is_sel = *selected == Some(sel);
            match track {
                Track::Zoom => paint_zoom(&p, r, &edit.zoom_segments[i], is_sel),
                Track::Mask => paint_mask(&p, r, &edit.masks[i], is_sel),
                Track::Camera => paint_layout(&p, r, &edit.camera_layouts[i], is_sel),
            }
        }
    }
    let hints = [
        (Track::Zoom, "Click or drag to add zoom on cursor"),
        (Track::Mask, "Click to add a blur or highlight mask"),
        (Track::Camera, "Click to add a camera layout"),
    ];
    for (track, hint) in hints.into_iter().filter(|(t, _)| tracks.contains(t)) {
        let r = track_rect(track);
        if spans(edit, track).is_empty() && hover.is_none_or(|h| !r.contains(h)) {
            p.text(r.center(), egui::Align2::CENTER_CENTER, hint, egui::FontId::proportional(13.0), Color32::from_rgb(0x4a, 0x4c, 0x52));
        }
    }

    // Hover feedback.
    if state.drag.is_none() {
        if let Some(h) = hover {
            match hit(h, edit) {
                Hit::Empty(track) => {
                    let len = if track == Track::Zoom { 2.0 } else { 3.0 };
                    if let Some((s, e)) = free_span_on(edit, track, geo.t(h.x), len, duration) {
                        let r = Rect::from_x_y_ranges(geo.x(s)..=geo.x(e), track_rect(track).y_range());
                        let (stops, edge, border, label): (&[(f32, Color32)], _, _, _) = match track {
                            Track::Zoom => (&ZOOM_STOPS, ZOOM_EDGE, ZOOM_BORDER, "Zoom"),
                            Track::Mask => (&MASK_STOPS, MASK_EDGE, MASK_BORDER, "Mask"),
                            Track::Camera => (&LAYOUT_STOPS, LAYOUT_EDGE, LAYOUT_BORDER, "Layout"),
                        };
                        gradient_bar(&p, r, 10.0, stops, edge, 0.55, border, 0.35);
                        icons::paint_with_text(&p, r, Some(Icon::Plus), 13.0, label, egui::FontId::proportional(13.0), Color32::from_white_alpha(180));
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    }
                }
                Hit::Item(_, Drag::Move { .. }) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
                Hit::Item(..) | Hit::TrimStart | Hit::TrimEnd => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
                Hit::Other => {}
            }
        }
    }

    // Interaction.
    let press = ui.input(|i| i.pointer.press_origin());
    if resp.drag_started() {
        if let Some(pos) = press {
            state.drag = match hit(pos, edit) {
                Hit::Item(sel, d) => {
                    *selected = Some(sel);
                    Some(d)
                }
                Hit::TrimStart => Some(Drag::TrimStart),
                Hit::TrimEnd => Some(Drag::TrimEnd),
                Hit::Empty(_) => None,
                Hit::Other => Some(Drag::Scrub),
            };
        }
    }
    if let (Some(drag), Some(pos)) = (state.drag, resp.interact_pointer_pos()) {
        let t = geo.t(pos.x);
        match drag {
            Drag::Scrub => out.seek = Some(t),
            Drag::Move { sel, grab } => {
                let (s0, e0) = spans(edit, sel.track)[sel.index];
                let len = e0 - s0;
                let (lo, hi) = neighbours(edit, sel, duration);
                let start = (t - grab).clamp(lo, (hi - len).max(lo));
                set_span(edit, sel, start, start + len);
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
            Drag::ResizeStart(sel) => {
                let (lo, _) = neighbours(edit, sel, duration);
                let (_, e0) = spans(edit, sel.track)[sel.index];
                set_span(edit, sel, t.clamp(lo, e0 - MIN_SEG), e0);
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            Drag::ResizeEnd(sel) => {
                let (_, hi) = neighbours(edit, sel, duration);
                let (s0, _) = spans(edit, sel.track)[sel.index];
                set_span(edit, sel, s0, t.clamp(s0 + MIN_SEG, hi));
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            Drag::TrimStart => {
                edit.trim_start = t.min(trim_e - 0.5).max(0.0);
                out.seek = Some(edit.trim_start);
            }
            Drag::TrimEnd => {
                let e = t.max(trim_s + 0.5).min(duration);
                edit.trim_end = if duration - e < 1e-3 { None } else { Some(e) };
                out.seek = Some(e);
            }
        }
    }
    if resp.drag_stopped() {
        state.drag = None;
    }
    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            match hit(pos, edit) {
                Hit::Item(sel, _) => *selected = Some(sel),
                Hit::Empty(track) => {
                    let len = if track == Track::Zoom { 2.0 } else { 3.0 };
                    if let Some((s, e)) = free_span_on(edit, track, geo.t(pos.x), len, duration) {
                        *selected = Some(add_item(edit, track, s, e));
                    }
                }
                _ => {
                    out.seek = Some(geo.t(pos.x));
                    *selected = None;
                }
            }
        }
    }

    // Playhead.
    let x = geo.x(time);
    p.line_segment([pos2(x, ruler.top() + 4.0), pos2(x, bottom)], Stroke::new(1.5, theme::PLAYHEAD));
    p.circle_filled(pos2(x, ruler.top() + 4.0), 6.0, theme::PLAYHEAD);
    out
}

/// Allowed range for an item without overlapping its neighbours.
fn neighbours(edit: &EditConfig, sel: Selection, duration: f64) -> (f64, f64) {
    let items = spans(edit, sel.track);
    let lo = if sel.index > 0 { items[sel.index - 1].1 } else { 0.0 };
    let hi = items.get(sel.index + 1).map(|s| s.0).unwrap_or(duration);
    (lo, hi)
}

/// Mauve-grey "Layout" pills, as on Screen Studio's layouts track.
const LAYOUT_STOPS: [(f32, Color32); 3] = [
    (0.0, Color32::from_rgb(0x8f, 0x80, 0x87)),
    (0.5, Color32::from_rgb(0x7f, 0x71, 0x78)),
    (1.0, Color32::from_rgb(0x70, 0x63, 0x6a)),
];
const LAYOUT_EDGE: Color32 = Color32::from_rgb(0xa8, 0x9a, 0xa1);
const LAYOUT_BORDER: Color32 = Color32::from_rgb(0x5c, 0x51, 0x57);

fn paint_layout(p: &egui::Painter, r: Rect, seg: &CameraLayoutSegment, selected: bool) {
    let rad = 10.0f32.min(r.width() / 2.0);
    gradient_bar(p, r, rad, &LAYOUT_STOPS, LAYOUT_EDGE, 0.55, LAYOUT_BORDER, if seg.disabled { 0.4 } else { 1.0 });
    if selected {
        p.rect_stroke(r.expand(2.0), CornerRadius::same(rad as u8 + 2), Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
    }
    let icon = match seg.kind {
        CameraLayoutKind::Default => Icon::CamPip,
        CameraLayoutKind::Split => Icon::CamSplit,
        CameraLayoutKind::Fullscreen => Icon::CamFull,
        CameraLayoutKind::Hide => Icon::Window,
    };
    if r.width() > 70.0 {
        p.text(r.center() - vec2(0.0, 10.0), egui::Align2::CENTER_CENTER, "Layout", egui::FontId::proportional(12.0), Color32::from_white_alpha(120));
        let label_rect = Rect::from_center_size(r.center() + vec2(0.0, 9.0), vec2(r.width(), 18.0));
        icons::paint_with_text(p, label_rect, Some(icon), 14.0, seg.kind.label(), egui::FontId::proportional(13.5), Color32::WHITE);
    } else if r.width() > 24.0 {
        icons::paint(p, Rect::from_center_size(r.center(), vec2(14.0, 14.0)), icon, Color32::WHITE);
    }
}

fn paint_zoom(p: &egui::Painter, r: Rect, seg: &ZoomSegment, selected: bool) {
    let rad = 10.0f32.min(r.width() / 2.0);
    gradient_bar(p, r, rad, &ZOOM_STOPS, ZOOM_EDGE, 0.55, ZOOM_BORDER, 1.0);
    if selected {
        p.rect_stroke(r.expand(2.0), CornerRadius::same(rad as u8 + 2), Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
    }
    if r.width() > 70.0 {
        let mode = match seg.focus {
            ZoomFocus::Auto => "Auto",
            ZoomFocus::Manual { .. } => "Manual",
        };
        let scale = if (seg.scale - seg.scale.round()).abs() < 0.05 { format!("{:.0}×", seg.scale) } else { format!("{:.1}×", seg.scale) };
        let loupe = seg.kind == crate::edit::ZoomKind::Loupe;
        p.text(r.center() - vec2(0.0, 10.0), egui::Align2::CENTER_CENTER, if loupe { "Glass Loupe" } else { "Zoom" }, egui::FontId::proportional(12.0), Color32::from_white_alpha(150));
        // "[zoom icon] 2x   [mouse icon] Auto"
        let font = egui::FontId::proportional(13.5);
        let a = p.layout_no_wrap(scale, font.clone(), Color32::WHITE);
        let b = p.layout_no_wrap(mode.to_owned(), font, Color32::WHITE);
        let icon = 14.0;
        let total = icon + 3.0 + a.size().x + 12.0 + icon + 3.0 + b.size().x;
        let mut x = r.center().x - total / 2.0;
        let y = r.center().y + 9.0;
        icons::paint(p, Rect::from_center_size(pos2(x + icon / 2.0, y), vec2(icon, icon)), if loupe { Icon::Loupe } else { Icon::Zoom }, Color32::WHITE);
        x += icon + 3.0;
        let aw = a.size();
        p.galley(pos2(x, y - aw.y / 2.0), a, Color32::WHITE);
        x += aw.x + 12.0;
        icons::paint(p, Rect::from_center_size(pos2(x + icon / 2.0, y), vec2(icon, icon)), Icon::Mouse, Color32::WHITE);
        x += icon + 3.0;
        let bh = b.size().y;
        p.galley(pos2(x, y - bh / 2.0), b, Color32::WHITE);
    } else if r.width() > 30.0 {
        p.text(r.center(), egui::Align2::CENTER_CENTER, format!("{:.1}×", seg.scale), egui::FontId::proportional(12.0), Color32::WHITE);
    }
}

/// Colours sampled from Screen Studio's v4 timeline (top to bottom).
const ZOOM_STOPS: [(f32, Color32); 6] = [
    (0.0, Color32::from_rgb(0x3a, 0x21, 0xe2)),
    (0.27, Color32::from_rgb(0x59, 0x42, 0xec)),
    (0.53, Color32::from_rgb(0x6a, 0x57, 0xee)),
    (0.74, Color32::from_rgb(0x71, 0x5e, 0xee)),
    (0.88, Color32::from_rgb(0x6a, 0x57, 0xea)),
    (1.0, Color32::from_rgb(0x59, 0x44, 0xe4)),
];
const ZOOM_EDGE: Color32 = Color32::from_rgb(0x3a, 0x21, 0xe2);
const ZOOM_BORDER: Color32 = Color32::from_rgb(0x8b, 0x7e, 0xe7);

/// Pink mask bars (sampled from Screen Studio's mask screenshots).
const MASK_STOPS: [(f32, Color32); 4] = [
    (0.0, Color32::from_rgb(0xa1, 0x2c, 0x62)),
    (0.45, Color32::from_rgb(0xbd, 0x3e, 0x7b)),
    (0.8, Color32::from_rgb(0xc6, 0x45, 0x83)),
    (1.0, Color32::from_rgb(0xb5, 0x3a, 0x76)),
];
const MASK_EDGE: Color32 = Color32::from_rgb(0x8f, 0x24, 0x56);
const MASK_BORDER: Color32 = Color32::from_rgb(0xe0, 0x86, 0xb2);

const CLIP_STOPS: [(f32, Color32); 5] = [
    (0.0, Color32::from_rgb(0x8e, 0x65, 0x24)),
    (0.3, Color32::from_rgb(0xa4, 0x75, 0x2a)),
    (0.7, Color32::from_rgb(0xbc, 0x85, 0x31)),
    (0.85, Color32::from_rgb(0xbd, 0x86, 0x31)),
    (1.0, Color32::from_rgb(0xbe, 0x8a, 0x37)),
];
const CLIP_EDGE: Color32 = Color32::from_rgb(0x8e, 0x65, 0x24);
const CLIP_BORDER: Color32 = Color32::from_rgb(0xc2, 0x8e, 0x3c);

fn gradient_at(stops: &[(f32, Color32)], t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if t <= b {
            return ca.lerp_to_gamma(cb, (t - a) / (b - a).max(1e-6));
        }
    }
    stops[stops.len() - 1].1
}

/// Rounded bar with a vertical gradient, darkened left/right edges and a
/// thin light border, matching the look of Screen Studio's timeline items.
fn gradient_bar(
    p: &egui::Painter,
    r: Rect,
    radius: f32,
    stops: &[(f32, Color32)],
    edge: Color32,
    edge_amount: f32,
    border: Color32,
    alpha: f32,
) {
    let rad = radius.min(r.height() / 2.0).min(r.width() / 2.0).max(0.0);
    let edge_w = 10.0f32.min(r.width() / 3.0);
    // Horizontal sample positions: dense near the edges for the shading.
    let mut xs: Vec<f32> = vec![0.0, 1.5, 3.5, 6.0, edge_w];
    xs.retain(|x| *x <= r.width() / 2.0);
    let mut cols: Vec<f32> = xs.iter().map(|x| r.left() + x).collect();
    cols.extend(xs.iter().rev().map(|x| r.right() - x));
    cols.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    let color = |x: f32, y: f32| {
        let base = gradient_at(stops, (y - r.top()) / r.height());
        let d = (x - r.left()).min(r.right() - x).max(0.0);
        let shade = edge_amount * (1.0 - (d / edge_w).clamp(0.0, 1.0)).powi(2);
        base.lerp_to_gamma(edge, shade).gamma_multiply(alpha)
    };
    // Half-extent of the row at height y after cutting the rounded corners.
    let inset = |y: f32| {
        let dy = if y < r.top() + rad {
            r.top() + rad - y
        } else if y > r.bottom() - rad {
            y - (r.bottom() - rad)
        } else {
            0.0
        };
        rad - (rad * rad - dy * dy).max(0.0).sqrt()
    };
    let rows = (r.height().ceil() as usize).clamp(2, 96);
    let mut mesh = egui::Mesh::default();
    for j in 0..rows {
        let (y0, y1) = (r.top() + r.height() * j as f32 / rows as f32, r.top() + r.height() * (j + 1) as f32 / rows as f32);
        let yi = if y0 < r.center().y { y1 } else { y0 }; // tighter corner cut
        let ins = inset(yi);
        let (lx, rx) = (r.left() + ins, r.right() - ins);
        let mut row: Vec<f32> = vec![lx];
        row.extend(cols.iter().copied().filter(|x| *x > lx && *x < rx));
        row.push(rx);
        for w in row.windows(2) {
            let base = mesh.vertices.len() as u32;
            for &(x, y) in &[(w[0], y0), (w[1], y0), (w[0], y1), (w[1], y1)] {
                mesh.colored_vertex(pos2(x, y), color(x, y));
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 3, base + 2);
        }
    }
    p.add(mesh);
    // The anti-aliased border also hides the mesh's stair-stepped corners.
    p.rect_stroke(r, CornerRadius::same(rad as u8), Stroke::new(1.0, border.gamma_multiply(alpha)), StrokeKind::Inside);
}

/// Exponential approach to `target` (ease-out in both directions), stored per
/// id across frames. Keeps repainting until settled.
fn smooth(ui: &Ui, id: egui::Id, target: f32) -> f32 {
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    let v = ui.data_mut(|d| {
        let v = d.get_temp_mut_or(id, target);
        *v += (target - *v) * (1.0 - (-dt * 14.0).exp());
        if (target - *v).abs() < 0.002 {
            *v = target;
        }
        *v
    });
    if v != target {
        ui.ctx().request_repaint();
    }
    v
}

fn paint_mask(p: &egui::Painter, r: Rect, m: &Mask, selected: bool) {
    let rad = 10.0f32.min(r.width() / 2.0);
    let alpha = if m.disabled { 0.45 } else { 1.0 };
    gradient_bar(p, r, rad, &MASK_STOPS, MASK_EDGE, 0.5, MASK_BORDER, alpha);
    if selected {
        p.rect_stroke(r.expand(2.0), CornerRadius::same(rad as u8 + 2), Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
    }
    let label = match m.kind {
        MaskKind::Blur => "Sensitive data".to_string(),
        MaskKind::Highlight => format!("Highlight {:.0}%", m.opacity * 100.0),
    };
    if r.width() > 80.0 {
        p.text(r.center() - vec2(0.0, 10.0), egui::Align2::CENTER_CENTER, "Mask", egui::FontId::proportional(12.0), Color32::from_white_alpha(150));
        icons::paint_with_text(p, Rect::from_center_size(r.center() + vec2(0.0, 9.0), vec2(r.width(), 18.0)), Some(Icon::Mask), 14.0, &label, egui::FontId::proportional(13.5), Color32::WHITE.gamma_multiply(alpha.max(0.7)));
    } else if r.width() > 30.0 {
        icons::paint(p, Rect::from_center_size(r.center(), vec2(14.0, 14.0)), Icon::Mask, Color32::WHITE);
    }
}

/// Audio waveform on the clip bar: a filled shape slightly lighter than the
/// clip (a warm white wash, measured as #DCAB50 over #C8890D in Screen
/// Studio), rising from the bottom of the bar.
const WAVE: Color32 = Color32::from_rgba_premultiplied(51, 48, 43, 51);

fn paint_waveform(p: &egui::Painter, clip: Rect, geo: &Geo, env: &[f32], t0: f64, t1: f64) {
    let rate = super::audio::ENVELOPE_RATE;
    // Sits on the bar's bottom border, inset from the rounded ends.
    let inner = Rect::from_min_max(pos2(clip.left() + 4.0, clip.top() + 3.0), pos2(clip.right() - 4.0, clip.bottom() - 1.0));
    let max_h = inner.height() * 0.78;
    // Half-point columns for a finer outline on HiDPI screens.
    let step = 0.5f32;
    let cols = (inner.width() / step).max(1.0) as usize;
    let span = geo.duration / geo.width as f64 * step as f64;
    let sample = |t: f64| -> f32 {
        if span > 1.0 / rate {
            // Zoomed out: peak over the time this column covers.
            let (a, b) = (((t - span / 2.0) * rate).max(0.0) as usize, ((t + span / 2.0) * rate).ceil() as usize);
            env.get(a.min(env.len())..b.min(env.len()).max(a.min(env.len()))).map(|s| s.iter().cloned().fold(0f32, f32::max)).unwrap_or(0.0)
        } else {
            // Zoomed in: interpolate between envelope points.
            let f = (t * rate).max(0.0);
            let i = f.floor() as usize;
            let (a, b) = (env.get(i).copied().unwrap_or(0.0), env.get(i + 1).copied().unwrap_or(0.0));
            a + (b - a) * (f - f.floor()) as f32
        }
    };
    let raw: Vec<f32> = (0..=cols).map(|c| sample(geo.t(inner.left() + c as f32 * step).clamp(t0, t1))).collect();
    // Gaussian blur across columns so the outline is rounded, not stepped.
    const K: [f32; 9] = [0.03, 0.06, 0.12, 0.18, 0.22, 0.18, 0.12, 0.06, 0.03];
    let vals: Vec<f32> = (0..raw.len())
        .map(|i| {
            K.iter().enumerate().map(|(k, w)| raw[(i as isize + k as isize - 4).clamp(0, raw.len() as isize - 1) as usize] * w).sum()
        })
        .collect();
    let mut mesh = egui::Mesh::default();
    let mut outline = Vec::with_capacity(vals.len());
    for (c, v) in vals.iter().enumerate() {
        let x = inner.left() + c as f32 * step;
        let top = inner.bottom() - v * max_h;
        outline.push(pos2(x, top));
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(x, top), WAVE);
        mesh.colored_vertex(pos2(x, inner.bottom()), WAVE);
        if c > 0 {
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i + 1, i);
        }
    }
    p.add(mesh);
    // Anti-aliased edge along the top of the fill.
    p.add(egui::Shape::line(outline, Stroke::new(1.0, WAVE)));
}
