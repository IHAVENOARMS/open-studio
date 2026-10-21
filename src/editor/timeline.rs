//! Timeline: ruler, amber clip track (with trim handles) and the purple zoom
//! track where zoom segments are added, moved, resized and selected.

use crate::edit::{EditConfig, ZoomFocus, ZoomSegment};
use crate::theme;
use crate::util::format_time;
use egui::{Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

const EDGE: f32 = 7.0;
const MIN_SEG: f64 = 0.3;

#[derive(Clone, Copy, Debug)]
enum Drag {
    Scrub,
    Move { idx: usize, grab: f64 },
    ResizeStart(usize),
    ResizeEnd(usize),
    TrimStart,
    TrimEnd,
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
    Segment(usize, Drag),
    TrimStart,
    TrimEnd,
    EmptyZoom,
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
    // Previous
    let (r, col) = button(ui, pos2(c.x - 64.0, c.y), "prev");
    let s = Stroke::new(1.8, col);
    let o = pos2(c.x - 64.0, c.y);
    p.line_segment([o + vec2(-7.0, -8.0), o + vec2(-7.0, 8.0)], s);
    p.add(egui::Shape::convex_polygon(vec![o + vec2(7.0, -8.0), o + vec2(7.0, 8.0), o + vec2(-4.0, 0.0)], Color32::TRANSPARENT, s));
    if r.clicked() {
        out.seek = Some(0.0);
    }
    // Play / pause in a ring.
    let (r, col) = button(ui, c, "play");
    p.circle_stroke(c, 14.0, Stroke::new(1.8, col));
    if playing {
        p.rect_filled(Rect::from_center_size(c + vec2(-3.5, 0.0), vec2(3.0, 11.0)), 1.0, col);
        p.rect_filled(Rect::from_center_size(c + vec2(3.5, 0.0), vec2(3.0, 11.0)), 1.0, col);
    } else {
        p.add(egui::Shape::convex_polygon(vec![c + vec2(-4.0, -7.0), c + vec2(-4.0, 7.0), c + vec2(7.0, 0.0)], col, Stroke::NONE));
    }
    if r.clicked() {
        out.toggle_play = true;
    }
    // Next
    let (r, col) = button(ui, pos2(c.x + 64.0, c.y), "next");
    let s = Stroke::new(1.8, col);
    let o = pos2(c.x + 64.0, c.y);
    p.line_segment([o + vec2(7.0, -8.0), o + vec2(7.0, 8.0)], s);
    p.add(egui::Shape::convex_polygon(vec![o + vec2(-7.0, -8.0), o + vec2(-7.0, 8.0), o + vec2(4.0, 0.0)], Color32::TRANSPARENT, s));
    if r.clicked() {
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
    selected: &mut Option<usize>,
) -> Output {
    let mut out = Output::default();
    let full = ui.available_rect_before_wrap();
    let geo = Geo { left: full.left() + 10.0, width: full.width() - 20.0, duration: duration.max(0.01) };
    let ruler = Rect::from_min_size(full.min, vec2(full.width(), 30.0));
    let clip = Rect::from_min_max(pos2(full.left(), ruler.bottom() + 6.0), pos2(full.right(), ruler.bottom() + 66.0));
    let zoom = Rect::from_min_max(pos2(full.left(), clip.bottom() + 10.0), pos2(full.right(), clip.bottom() + 70.0));
    let area = Rect::from_min_max(full.min, pos2(full.right(), zoom.bottom()));
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
    p.rect_filled(Rect::from_x_y_ranges(geo.x(0.0)..=geo.x(duration), clip.y_range()), CornerRadius::same(8), Color32::from_rgb(0x3a, 0x2c, 0x12));
    let clip_r = Rect::from_x_y_ranges(geo.x(trim_s)..=geo.x(trim_e), clip.y_range());
    p.rect_filled(clip_r, CornerRadius::same(8), theme::CLIP_A);
    p.rect_stroke(clip_r, CornerRadius::same(8), Stroke::new(1.0, theme::CLIP_B), StrokeKind::Inside);
    if clip_r.width() > 60.0 {
        p.text(clip_r.center() - vec2(0.0, 10.0), egui::Align2::CENTER_CENTER, "Clip", egui::FontId::proportional(12.5), Color32::from_rgb(0xe8, 0xc6, 0x86));
        p.text(clip_r.center() + vec2(0.0, 9.0), egui::Align2::CENTER_CENTER, format!("{:.0}s  ·  1×", trim_e - trim_s), egui::FontId::proportional(13.5), Color32::WHITE);
    }

    // Zoom track.
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
        if zoom.contains(pos) {
            for (i, s) in edit.zoom_segments.iter().enumerate() {
                let (x0, x1) = (geo.x(s.start), geo.x(s.end));
                if pos.x >= x0 - 2.0 && pos.x <= x1 + 2.0 {
                    let d = if (pos.x - x0).abs() < EDGE {
                        Drag::ResizeStart(i)
                    } else if (pos.x - x1).abs() < EDGE {
                        Drag::ResizeEnd(i)
                    } else {
                        Drag::Move { idx: i, grab: geo.t(pos.x) - s.start }
                    };
                    return Hit::Segment(i, d);
                }
            }
            return Hit::EmptyZoom;
        }
        Hit::Other
    };

    for (i, s) in edit.zoom_segments.iter().enumerate() {
        let r = Rect::from_x_y_ranges(geo.x(s.start)..=geo.x(s.end), zoom.y_range());
        paint_zoom(&p, r, s, *selected == Some(i));
    }
    if edit.zoom_segments.is_empty() && hover.is_none_or(|h| !zoom.contains(h)) {
        p.text(zoom.center(), egui::Align2::CENTER_CENTER, "Click or drag to add zoom on cursor", egui::FontId::proportional(13.0), Color32::from_rgb(0x4a, 0x4c, 0x52));
    }

    // Hover feedback.
    if state.drag.is_none() {
        if let Some(h) = hover {
            match hit(h, edit) {
                Hit::EmptyZoom => {
                    if let Some((s, e)) = free_span(edit, geo.t(h.x), duration) {
                        let r = Rect::from_x_y_ranges(geo.x(s)..=geo.x(e), zoom.y_range());
                        p.rect_filled(r, CornerRadius::same(8), theme::ZOOM_A.gamma_multiply(0.35));
                        p.text(r.center(), egui::Align2::CENTER_CENTER, "+  Zoom", egui::FontId::proportional(13.0), Color32::from_white_alpha(180));
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    }
                }
                Hit::Segment(_, Drag::Move { .. }) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
                Hit::Segment(..) | Hit::TrimStart | Hit::TrimEnd => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
                Hit::Other => {}
            }
        }
    }

    // Interaction.
    let press = ui.input(|i| i.pointer.press_origin());
    if resp.drag_started() {
        if let Some(pos) = press {
            state.drag = match hit(pos, edit) {
                Hit::Segment(i, d) => {
                    *selected = Some(i);
                    Some(d)
                }
                Hit::TrimStart => Some(Drag::TrimStart),
                Hit::TrimEnd => Some(Drag::TrimEnd),
                Hit::EmptyZoom => None,
                Hit::Other => Some(Drag::Scrub),
            };
        }
    }
    if let (Some(drag), Some(pos)) = (state.drag, resp.interact_pointer_pos()) {
        let t = geo.t(pos.x);
        match drag {
            Drag::Scrub => out.seek = Some(t),
            Drag::Move { idx, grab } => {
                let len = edit.zoom_segments[idx].end - edit.zoom_segments[idx].start;
                let (lo, hi) = neighbours(edit, idx, duration);
                let start = (t - grab).clamp(lo, (hi - len).max(lo));
                let s = &mut edit.zoom_segments[idx];
                s.start = start;
                s.end = start + len;
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
            Drag::ResizeStart(idx) => {
                let (lo, _) = neighbours(edit, idx, duration);
                let s = &mut edit.zoom_segments[idx];
                s.start = t.clamp(lo, s.end - MIN_SEG);
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            Drag::ResizeEnd(idx) => {
                let (_, hi) = neighbours(edit, idx, duration);
                let s = &mut edit.zoom_segments[idx];
                s.end = t.clamp(s.start + MIN_SEG, hi);
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
                Hit::Segment(i, _) => *selected = Some(i),
                Hit::EmptyZoom => {
                    if let Some((s, e)) = free_span(edit, geo.t(pos.x), duration) {
                        edit.zoom_segments.push(ZoomSegment { start: s, end: e, scale: edit.default_zoom, focus: ZoomFocus::Auto });
                        edit.zoom_segments.sort_by(|a, b| a.start.total_cmp(&b.start));
                        *selected = edit.zoom_segments.iter().position(|z| z.start == s);
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
    p.line_segment([pos2(x, ruler.top() + 4.0), pos2(x, zoom.bottom())], Stroke::new(1.5, theme::PLAYHEAD));
    p.circle_filled(pos2(x, ruler.top() + 4.0), 6.0, theme::PLAYHEAD);
    out
}

/// Allowed range for segment `idx` without overlapping its neighbours.
fn neighbours(edit: &EditConfig, idx: usize, duration: f64) -> (f64, f64) {
    let lo = if idx > 0 { edit.zoom_segments[idx - 1].end } else { 0.0 };
    let hi = edit.zoom_segments.get(idx + 1).map(|s| s.start).unwrap_or(duration);
    (lo, hi)
}

/// Where a new 2s zoom starting at `t` fits, shrunk to the gap available.
fn free_span(edit: &EditConfig, t: f64, duration: f64) -> Option<(f64, f64)> {
    let next = edit.zoom_segments.iter().map(|s| s.start).filter(|s| *s > t).fold(duration, f64::min);
    let prev = edit.zoom_segments.iter().map(|s| s.end).filter(|e| *e <= t).fold(0.0, f64::max);
    let start = t.max(prev);
    let end = (start + 2.0).min(next);
    (end - start >= MIN_SEG).then_some((start, end))
}

fn paint_zoom(p: &egui::Painter, r: Rect, seg: &ZoomSegment, selected: bool) {
    let rad = 8.0f32.min(r.width() / 2.0);
    // Rounded caps in the end colours, horizontal gradient mesh in between.
    p.rect_filled(r, CornerRadius::same(rad as u8), theme::ZOOM_A);
    p.rect_filled(Rect::from_x_y_ranges((r.right() - 2.0 * rad)..=r.right(), r.y_range()), CornerRadius::same(rad as u8), theme::ZOOM_B);
    if r.width() > 2.0 * rad {
        let mut mesh = egui::Mesh::default();
        let (x0, x1) = (r.left() + rad, r.right() - rad);
        let f0 = (rad / r.width()).clamp(0.0, 1.0);
        let a = theme::ZOOM_A.lerp_to_gamma(theme::ZOOM_B, f0);
        let b = theme::ZOOM_A.lerp_to_gamma(theme::ZOOM_B, 1.0 - f0);
        mesh.colored_vertex(pos2(x0, r.top()), a);
        mesh.colored_vertex(pos2(x1, r.top()), b);
        mesh.colored_vertex(pos2(x0, r.bottom()), a);
        mesh.colored_vertex(pos2(x1, r.bottom()), b);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        p.add(mesh);
    }
    // Lighter grab handles at both ends.
    if r.width() > 24.0 {
        let hw = 6.0;
        let handle = Color32::from_white_alpha(38);
        p.rect_filled(Rect::from_x_y_ranges(r.left()..=r.left() + hw, r.y_range()), CornerRadius { nw: rad as u8, sw: rad as u8, ne: 0, se: 0 }, handle);
        p.rect_filled(Rect::from_x_y_ranges(r.right() - hw..=r.right(), r.y_range()), CornerRadius { ne: rad as u8, se: rad as u8, nw: 0, sw: 0 }, handle);
    }
    if selected {
        p.rect_stroke(r.expand(2.0), CornerRadius::same(rad as u8 + 2), Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
    }
    if r.width() > 70.0 {
        let mode = match seg.focus {
            ZoomFocus::Auto => "Auto",
            ZoomFocus::Manual { .. } => "Manual",
        };
        let scale = if (seg.scale - seg.scale.round()).abs() < 0.05 { format!("{:.0}×", seg.scale) } else { format!("{:.1}×", seg.scale) };
        p.text(r.center() - vec2(0.0, 10.0), egui::Align2::CENTER_CENTER, "Zoom", egui::FontId::proportional(12.0), Color32::from_white_alpha(150));
        p.text(r.center() + vec2(0.0, 9.0), egui::Align2::CENTER_CENTER, format!("🔍 {scale}   🖱 {mode}"), egui::FontId::proportional(13.5), Color32::WHITE);
    } else if r.width() > 30.0 {
        p.text(r.center(), egui::Align2::CENTER_CENTER, format!("{:.1}×", seg.scale), egui::FontId::proportional(12.0), Color32::WHITE);
    }
}
