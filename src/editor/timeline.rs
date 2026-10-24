//! Timeline: ruler, amber clip track (with trim handles), the purple zoom
//! track and the pink mask track. Items on the two lower tracks are added,
//! moved, resized and selected the same way.

use crate::edit::{CameraLayoutKind, CameraLayoutSegment, EditConfig, Mask, MaskKind, ZoomFocus, ZoomSegment};
use crate::icons::{self, Icon};
use crate::theme;
use crate::typing::{self, Typing};
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
    /// A slice of the clip (index into `EditConfig::slice_ranges`).
    Clip,
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
        Track::Clip => vec![],
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
        Track::Clip => {}
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
        Track::Clip => 0,
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

pub struct State {
    drag: Option<Drag>,
    /// Scissors tool: a click on the clip splits it there.
    pub split_mode: bool,
    /// Timeline magnification; 1 fits the whole recording.
    pub zoom: f32,
    /// Recording seconds scrolled off the left edge while zoomed in.
    scroll: f64,
    /// Slice the clip's right-click menu is for.
    menu_slice: Option<(f64, f64)>,
    /// Audio waveform on the clip track.
    pub waveform: bool,
    /// Cuts fold away so the clips sit side by side (Screen Studio's view).
    pub fold_cuts: bool,
    /// Scrubber: dragging the playhead plays the sound under it.
    pub scrubber: bool,
    /// Typing suggestion whose "Speed up typing" popover is open.
    typing_open: Option<Typing>,
    /// Speed on that popover's slider.
    typing_speed: f64,
}

impl Default for State {
    fn default() -> Self {
        let s = crate::settings::Settings::load();
        Self {
            drag: None,
            split_mode: false,
            zoom: 1.0,
            scroll: 0.0,
            menu_slice: None,
            waveform: s.timeline_waveform,
            fold_cuts: s.timeline_fold_cuts,
            scrubber: s.scrubber,
            typing_open: None,
            typing_speed: crate::typing::DEFAULT_SPEED,
        }
    }
}

#[derive(Default)]
pub struct Output {
    pub seek: Option<f64>,
    pub toggle_play: bool,
    /// Message for the bottom toast ("Removed 2.0s — Ctrl+Z to undo").
    pub toast: Option<String>,
    /// The playhead is being dragged (for the Scrubber).
    pub scrubbing: bool,
    /// Playback speed picked from the speed button.
    pub rate: Option<f64>,
}

/// Maps recording time to x. With folded cuts the timeline shows "display
/// time": recording time minus the cuts before it, so a cut takes no width.
struct Geo {
    left: f32,
    width: f32,
    /// Display duration (the recording minus folded cuts).
    duration: f64,
    /// Folded cuts, recording time, sorted.
    folds: Vec<(f64, f64)>,
}

/// Seconds as Screen Studio labels cut lengths: "0.4s", "1.0s", "2s".
fn short_secs(s: f64) -> String {
    if s < 1.95 { format!("{s:.1}s") } else { format!("{s:.0}s") }
}

/// Yellow teardrop pin above the clip at `x`, with the removed length and
/// scissors. Returns the click response.
fn cut_pin(ui: &Ui, p: &egui::Painter, x: f32, clip_top: f32, label: &str, id: egui::Id) -> egui::Response {
    let c = pos2(x, clip_top - 15.0);
    let r = ui.interact(Rect::from_center_size(c, vec2(26.0, 30.0)), id, Sense::click());
    let fill = if r.hovered() { Color32::from_rgb(0xff, 0xcf, 0x4d) } else { Color32::from_rgb(0xf4, 0xb5, 0x2a) };
    let edge = Color32::from_rgb(0xc8, 0x88, 0x10);
    let tip = pos2(x, clip_top - 0.5);
    p.add(egui::Shape::convex_polygon(vec![c + vec2(-7.6, 5.5), tip, c + vec2(7.6, 5.5)], fill, Stroke::NONE));
    p.circle(c, 11.5, fill, Stroke::new(1.0, edge));
    p.add(egui::Shape::convex_polygon(vec![c + vec2(-7.0, 6.2), tip + vec2(0.0, -1.2), c + vec2(7.0, 6.2)], fill, Stroke::NONE));
    let ink = Color32::from_rgb(0x3d, 0x28, 0x04);
    p.text(c + vec2(0.0, -4.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(9.5), ink);
    icons::paint(p, Rect::from_center_size(c + vec2(0.0, 4.5), vec2(11.0, 11.0)), Icon::Scissors, ink);
    r.on_hover_cursor(CursorIcon::PointingHand)
}

/// Display time of recording time `t`: cuts before it removed, a time
/// inside a cut lands on its seam.
fn fold(folds: &[(f64, f64)], t: f64) -> f64 {
    let mut d = t;
    for &(a, b) in folds {
        if t >= b {
            d -= b - a;
        } else if t > a {
            d -= t - a;
        }
    }
    d
}

impl Geo {
    fn x(&self, t: f64) -> f32 {
        self.xd(fold(&self.folds, t))
    }
    /// x of a display time.
    fn xd(&self, d: f64) -> f32 {
        self.left + (d / self.duration) as f32 * self.width
    }
    fn t(&self, x: f32) -> f64 {
        let mut t = (((x - self.left) / self.width) as f64 * self.duration).clamp(0.0, self.duration);
        for &(a, b) in &self.folds {
            if t >= a {
                t += b - a;
            }
        }
        t
    }
}

enum Hit {
    Item(Selection, Drag),
    TrimStart,
    TrimEnd,
    Empty(Track),
    Other,
}

/// Transport controls under the preview: the scissors (split tool) on the
/// left, time and play controls in the middle, and the waveform toggle with
/// the timeline zoom on the right.
pub fn playback_bar(ui: &mut Ui, state: &mut State, time: f64, duration: f64, playing: bool, rate: f64) -> Output {
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

    // Playback speed: shows the J/L shuttle speed, and picks one.
    let label = super::shuttle::label(if playing { rate } else { 1.0 });
    let g = p.layout_no_wrap(label, egui::FontId::proportional(12.5), theme::TEXT);
    let pill = Rect::from_min_size(pos2(c.x + 196.0, c.y - 12.0), vec2(g.size().x + 16.0, 24.0));
    let resp = ui
        .interact(pill, ui.scope_id().with("speed"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Playback speed  (J / K / L)");
    let shuttling = playing && rate != 1.0;
    let fill = if shuttling {
        Color32::from_rgba_unmultiplied(0x5b, 0x45, 0xf0, 90)
    } else if resp.hovered() {
        Color32::from_white_alpha(22)
    } else {
        Color32::from_white_alpha(10)
    };
    p.rect_filled(pill, CornerRadius::same(7), fill);
    p.galley(pill.center() - g.size() / 2.0, g, if shuttling { Color32::WHITE } else { theme::TEXT });
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(140.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        for r in super::shuttle::MENU {
            if super::menu_item(ui, &super::shuttle::label(r), "", playing && rate == r).clicked() {
                out.rate = Some(r);
                ui.close();
            }
        }
    });

    // Scissors: the split tool.
    let sc = pos2(rect.left() + 20.0, c.y);
    if state.split_mode {
        p.rect_filled(Rect::from_center_size(sc, vec2(34.0, 30.0)), CornerRadius::same(8), Color32::from_rgba_unmultiplied(0x5b, 0x45, 0xf0, 90));
    }
    let r = icon_button(ui, sc, "split", Icon::Scissors, 22.0).on_hover_text(if state.split_mode {
        "Split tool on: click the clip to split it  (C splits at the playhead)"
    } else {
        "Split tool: click the clip to split it, or hold Alt  (C splits at the playhead)"
    });
    if r.clicked() {
        state.split_mode = !state.split_mode;
    }

    // Right: waveform toggle | zoom level and slider.
    let slider = Rect::from_min_max(pos2(rect.right() - 118.0, c.y - 9.0), pos2(rect.right() - 6.0, c.y + 9.0));
    let label_x = slider.left() - 12.0;
    let zoom_label = if state.zoom < 1.05 { "1×".to_string() } else { format!("{:.1}×", state.zoom) };
    p.text(pos2(label_x, c.y), egui::Align2::RIGHT_CENTER, &zoom_label, egui::FontId::proportional(14.0), theme::TEXT);
    let sep_x = label_x - 34.0;
    p.line_segment([pos2(sep_x, c.y - 11.0), pos2(sep_x, c.y + 11.0)], Stroke::new(1.0, Color32::from_white_alpha(24)));
    // Scrubber: highlighted while on, as in Screen Studio.
    let scrub_c = pos2(sep_x - 24.0, c.y);
    if state.scrubber {
        p.rect_filled(Rect::from_center_size(scrub_c, vec2(34.0, 30.0)), CornerRadius::same(8), Color32::from_rgba_unmultiplied(0x5b, 0x45, 0xf0, 70));
    }
    let wave = ui
        .interact(Rect::from_center_size(scrub_c, vec2(32.0, 32.0)), ui.scope_id().with("scrubber"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(if state.scrubber {
            "Scrubber on: drag the playhead to hear the sound under it"
        } else {
            "Scrubber: hear the sound while dragging the playhead, to find an exact spot"
        });
    let col = if state.scrubber { theme::KNOB } else if wave.hovered() { Color32::WHITE } else { theme::TEXT };
    icons::paint(&p, Rect::from_center_size(scrub_c, vec2(20.0, 20.0)), Icon::WaveformZoom, col);
    let fold_c = pos2(sep_x - 56.0, c.y);
    let fold = icon_button(ui, fold_c, "fold-cuts", Icon::FoldCuts, 20.0)
        .on_hover_text(if state.fold_cuts { "Show cut parts on the timeline" } else { "Fold cut parts away on the timeline" });
    if !state.fold_cuts {
        p.rect_filled(Rect::from_center_size(fold_c, vec2(22.0, 22.0)), 0.0, theme::BG.gamma_multiply(0.55));
    }
    if fold.clicked() {
        state.fold_cuts = !state.fold_cuts;
        let mut s = crate::settings::Settings::load();
        s.timeline_fold_cuts = state.fold_cuts;
        s.save();
    }
    if wave.clicked() {
        state.scrubber = !state.scrubber;
        let mut s = crate::settings::Settings::load();
        s.scrubber = state.scrubber;
        s.save();
    }
    // Zoom slider: log scale from fitting everything to about 3 s across.
    let max_zoom = (duration / 3.0).max(1.0) as f32;
    let resp = ui.interact(slider, ui.scope_id().with("tl-zoom"), Sense::click_and_drag()).on_hover_text("Zoom the timeline");
    let frac = if max_zoom > 1.0 { state.zoom.ln() / max_zoom.ln() } else { 0.0 };
    if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let f = ((pos.x - slider.left() - 9.0) / (slider.width() - 18.0)).clamp(0.0, 1.0);
        state.zoom = max_zoom.powf(f).max(1.0);
    }
    let track_y = c.y;
    p.line_segment([pos2(slider.left() + 9.0, track_y), pos2(slider.right() - 9.0, track_y)], Stroke::new(2.0, Color32::from_rgb(0x2b, 0x2d, 0x32)));
    let kx = slider.left() + 9.0 + frac.clamp(0.0, 1.0) * (slider.width() - 18.0);
    let knob = if resp.hovered() || resp.dragged() { Color32::from_rgb(0x7a, 0x7b, 0x80) } else { Color32::from_rgb(0x5d, 0x5e, 0x63) };
    p.circle_filled(pos2(kx, track_y), 9.0, knob);
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
    playing: bool,
    selected: &mut Option<Selection>,
    waveform: Option<&[f32]>,
    has_camera: bool,
    has_system: bool,
    has_device: bool,
    shortcuts: &[crate::shortcuts::Shortcut],
    flags: &[f64],
    typing: &[Typing],
) -> Output {
    let mut out = Output::default();
    let full = ui.available_rect_before_wrap();
    // The waveform is always shown, as in Screen Studio.
    let _ = state.waveform;
    // Zoom and scroll: Ctrl+wheel zooms around the pointer, the wheel scrolls.
    let folds: Vec<(f64, f64)> = if state.fold_cuts {
        edit.cuts.iter().map(|c| (c.start.max(0.0), c.end.min(duration))).filter(|(a, b)| b > a).collect()
    } else {
        vec![]
    };
    let dur = (duration - folds.iter().map(|(a, b)| b - a).sum::<f64>()).max(0.01);
    let base_w = full.width() - 20.0;
    let max_zoom = (dur / 3.0).max(1.0) as f32;
    if ui.rect_contains_pointer(full) {
        let (delta, ctrl, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.command, i.pointer.hover_pos()));
        if ctrl && delta.y != 0.0 {
            let px = pointer.map_or(full.center().x, |p| p.x);
            let width = base_w * state.zoom;
            let t_at = state.scroll + ((px - full.left() - 10.0) / width) as f64 * dur;
            state.zoom = (state.zoom * (delta.y / 200.0).exp()).clamp(1.0, max_zoom);
            state.scroll = t_at - ((px - full.left() - 10.0) / (base_w * state.zoom)) as f64 * dur;
        } else if state.zoom > 1.0 && (delta.x != 0.0 || delta.y != 0.0) {
            let d = if delta.x != 0.0 { delta.x } else { delta.y };
            state.scroll -= (d / (base_w * state.zoom)) as f64 * dur;
        }
    }
    state.zoom = state.zoom.clamp(1.0, max_zoom);
    let width = base_w * state.zoom;
    let visible = dur / state.zoom as f64;
    // Keep the playhead in view while playing.
    let shown = fold(&folds, time);
    if playing && (shown < state.scroll || shown > state.scroll + visible * 0.95) {
        state.scroll = shown - visible * 0.1;
    }
    state.scroll = state.scroll.clamp(0.0, (dur - visible).max(0.0));
    let geo = Geo { left: full.left() + 10.0 - (state.scroll / dur) as f32 * width, width, duration: dur, folds };
    let ruler = Rect::from_min_size(full.min, vec2(full.width(), 30.0));
    let clip = Rect::from_min_max(pos2(full.left(), ruler.bottom() + 6.0), pos2(full.right(), ruler.bottom() + 66.0));
    let zoom = Rect::from_min_max(pos2(full.left(), clip.bottom() + 10.0), pos2(full.right(), clip.bottom() + 70.0));
    let mask = Rect::from_min_max(pos2(full.left(), zoom.bottom() + 10.0), pos2(full.right(), zoom.bottom() + 70.0));
    let cam = Rect::from_min_max(pos2(full.left(), mask.bottom() + 10.0), pos2(full.right(), mask.bottom() + 70.0));
    let track_rect = |t: Track| match t {
        Track::Zoom => zoom,
        Track::Mask => mask,
        Track::Camera => cam,
        Track::Clip => clip,
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
    // Ticks count display time (with folded cuts, the time without them).
    let mut t = 0.0;
    while t <= geo.duration + 1e-6 {
        let x = geo.xd(t);
        if t > 0.0 {
            p.text(pos2(x, ruler.top() + 10.0), egui::Align2::CENTER_CENTER, format_time(t).trim_end_matches(".0"), egui::FontId::proportional(12.5), theme::TEXT_DIM);
        }
        p.circle_filled(pos2(x, ruler.bottom() - 6.0), 1.5, theme::TEXT_DIM);
        let half = geo.xd(t + step / 2.0);
        if t + step / 2.0 <= geo.duration {
            p.circle_filled(pos2(half, ruler.bottom() - 6.0), 1.5, Color32::from_rgb(0x4a, 0x4c, 0x52));
        }
        t += step;
    }

    // Recording flags: a flag on the ruler at each marked moment; click to
    // go there.
    for (n, &t) in flags.iter().enumerate() {
        let x = geo.x(t);
        if x < full.left() - 8.0 || x > full.right() + 8.0 {
            continue;
        }
        let icon = Rect::from_min_size(pos2(x - 2.5, ruler.top() + 1.0), vec2(16.0, 16.0));
        let r = ui
            .interact(icon.expand(3.0), ui.scope_id().with(("flag", n)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("Recording flag at {}", format_time_precise(t)));
        let col = if r.hovered() { Color32::WHITE } else { FLAG };
        p.line_segment([pos2(x, ruler.top() + 14.0), pos2(x, ruler.bottom())], Stroke::new(1.2, col.gamma_multiply(0.6)));
        icons::paint(&p, icon, Icon::Flag, col);
        if r.clicked() {
            out.seek = Some(t);
        }
    }

    // Clip track (trimmed region bright, the rest dimmed).
    let full_clip = Rect::from_x_y_ranges(geo.x(0.0)..=geo.x(duration), clip.y_range());
    gradient_bar(&p, full_clip, 10.0, &CLIP_STOPS, CLIP_EDGE, 0.5, CLIP_BORDER, 0.3);
    // Slices: the trim range split at cuts and split points.
    let kept = edit.slice_ranges(duration);
    for &(a, b) in &kept {
        let r = Rect::from_x_y_ranges(geo.x(a)..=geo.x(b), clip.y_range());
        // Neighbouring slices sit 2px apart so a cut or split reads as a gap.
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
            // "Clip", then "3s ⏱ 1×" (output length and speed), and "Hide"
            // when the slice hides the cursor.
            let props = edit.slice_props((a + b) * 0.5);
            p.text(r.center() - vec2(0.0, 10.0), egui::Align2::CENTER_CENTER, "Clip", egui::FontId::proportional(12.5), Color32::from_rgb(0xe8, 0xc6, 0x86));
            let font = egui::FontId::proportional(13.5);
            let speed = if (props.speed - props.speed.round()).abs() < 0.05 { format!("{:.0}×", props.speed) } else { format!("{:.1}×", props.speed) };
            let g_len = p.layout_no_wrap(short_secs((b - a) / props.speed).replace(".0s", "s"), font.clone(), Color32::WHITE);
            let g_speed = p.layout_no_wrap(speed, font.clone(), Color32::WHITE);
            let g_hide = props.hide_cursor.then(|| p.layout_no_wrap("Hide".into(), font.clone(), Color32::WHITE));
            let total = g_len.size().x + 8.0 + 15.0 + 3.0 + g_speed.size().x + g_hide.as_ref().map_or(0.0, |g| 8.0 + 13.0 + g.size().x);
            if total < r.width() - 12.0 {
                let y = r.center().y + 9.0;
                let mut x = r.center().x - total / 2.0;
                let h = g_len.size().y;
                p.galley(pos2(x, y - h / 2.0), g_len.clone(), Color32::WHITE);
                x += g_len.size().x + 8.0;
                icons::paint(&p, Rect::from_center_size(pos2(x + 7.5, y), vec2(14.0, 14.0)), Icon::Speedometer, Color32::WHITE);
                x += 18.0;
                p.galley(pos2(x, y - h / 2.0), g_speed.clone(), Color32::WHITE);
                x += g_speed.size().x + 8.0;
                if let Some(g) = g_hide {
                    icons::paint(&p, Rect::from_center_size(pos2(x + 6.0, y), vec2(13.0, 13.0)), Icon::RailCursor, Color32::WHITE);
                    p.galley(pos2(x + 13.0, y - h / 2.0), g, Color32::WHITE);
                }
            }
        }
    }
    // The selected slice gets a light outline.
    if let Some(Selection { track: Track::Clip, index }) = *selected
        && let Some(&(a, b)) = kept.get(index)
    {
        let r = Rect::from_x_y_ranges(geo.x(a)..=geo.x(b), clip.y_range());
        let r = if kept.len() > 1 { r.shrink2(vec2(1.0, 0.0)) } else { r };
        p.rect_stroke(r, CornerRadius::same(10), Stroke::new(2.0, Color32::from_rgb(0xff, 0xe7, 0xa8)), StrokeKind::Inside);
    }
    // Pins over removed parts: cuts and trimmed ends. Click to bring them back.
    for c in edit.cuts.clone() {
        if c.end <= trim_s || c.start >= trim_e {
            continue;
        }
        let x = geo.x((c.start + c.end) * 0.5);
        let r = cut_pin(ui, &p, x, clip.top(), &short_secs(c.end - c.start), ui.scope_id().with(("cut", (c.start * 1000.0) as i64)));
        if r.on_hover_text("Click to restore this part").clicked() {
            edit.restore(c.start, c.end);
        }
    }
    if trim_s > 0.05 && cut_pin(ui, &p, geo.x(trim_s), clip.top(), &short_secs(trim_s), ui.scope_id().with("trim-start")).on_hover_text("Click to undo the trim").clicked() {
        edit.trim_start = 0.0;
    }
    if duration - trim_e > 0.05
        && cut_pin(ui, &p, geo.x(trim_e), clip.top(), &short_secs(duration - trim_e), ui.scope_id().with("trim-end")).on_hover_text("Click to undo the trim").clicked()
    {
        edit.trim_end = None;
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
                Track::Clip => {}
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
                            Track::Camera | Track::Clip => (&LAYOUT_STOPS, LAYOUT_EDGE, LAYOUT_BORDER, "Layout"),
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

    // Scissors: with the split tool (or Alt held) a click on the clip splits it.
    let splitting = state.split_mode || ui.input(|i| i.modifiers.alt);
    if splitting && let Some(h) = hover.filter(|h| clip.contains(*h)) {
        p.line_segment([pos2(h.x, clip.top() - 2.0), pos2(h.x, clip.bottom() + 2.0)], Stroke::new(2.0, Color32::WHITE));
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    }
    // Right-click on a slice opens its menu.
    if resp.secondary_clicked() {
        state.menu_slice = resp
            .interact_pointer_pos()
            .filter(|p| clip.contains(*p))
            .and_then(|p| {
                let t = geo.t(p.x);
                kept.iter().copied().find(|&(a, b)| t >= a && t <= b)
            });
    }
    if let Some(slice) = state.menu_slice {
        resp.context_menu(|ui| slice_menu(ui, edit, slice, time, duration, has_system, has_device, &mut out));
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
            Drag::Scrub => {
                out.seek = Some(t);
                out.scrubbing = true;
            }
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
        if let Some(pos) = resp.interact_pointer_pos().filter(|p| splitting && clip.contains(*p)) {
            edit.split_at(geo.t(pos.x), duration);
        } else if let Some(pos) = resp.interact_pointer_pos() {
            match hit(pos, edit) {
                Hit::Item(sel, _) => *selected = Some(sel),
                Hit::Empty(track) => {
                    let len = if track == Track::Zoom { 2.0 } else { 3.0 };
                    if let Some((s, e)) = free_span_on(edit, track, geo.t(pos.x), len, duration) {
                        *selected = Some(add_item(edit, track, s, e));
                    }
                }
                _ => {
                    let t = geo.t(pos.x);
                    out.seek = Some(t);
                    // A click on the clip selects that slice (Slice editor).
                    *selected = clip
                        .contains(pos)
                        .then(|| kept.iter().position(|&(a, b)| t >= a && t <= b))
                        .flatten()
                        .map(|index| Selection { track: Track::Clip, index });
                }
            }
        }
    }

    // Typing suggestions: amber pills just above the clip; a click opens
    // "Speed up typing".
    let suggestions = typing::suggestions(typing, edit, duration);
    for t in &suggestions {
        let r = Rect::from_x_y_ranges(geo.x(t.start)..=geo.x(t.end), (clip.top() - 21.0)..=(clip.top() - 3.0)).shrink2(vec2(1.0, 0.0));
        if r.width() < 6.0 || r.right() < full.left() || r.left() > full.right() {
            continue;
        }
        let resp = ui
            .interact(r, ui.scope_id().with(("typing", (t.start * 1000.0) as i64)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Typing: click to speed it up");
        let open = state.typing_open == Some(*t);
        let fill = if open || resp.hovered() { TYPING_FILL_HOVER } else { TYPING_FILL };
        p.rect_filled(r, CornerRadius::same(5), fill);
        p.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, TYPING_EDGE), StrokeKind::Inside);
        let font = egui::FontId::proportional(11.5);
        let g = p.layout_no_wrap("Typing".into(), font, TYPING_TEXT);
        let content = 14.0 + 5.0 + g.size().x;
        let clipped = p.with_clip_rect(r.shrink(1.0));
        if r.width() > content + 12.0 {
            let x0 = r.center().x - content / 2.0;
            icons::paint(&clipped, Rect::from_center_size(pos2(x0 + 7.0, r.center().y), vec2(14.0, 14.0)), Icon::Keyboard, TYPING_TEXT);
            clipped.galley(pos2(x0 + 19.0, r.center().y - g.size().y / 2.0), g, TYPING_TEXT);
        } else if r.width() > 20.0 {
            icons::paint(&clipped, Rect::from_center_size(r.center(), vec2(14.0, 14.0)), Icon::Keyboard, TYPING_TEXT);
        }
        if resp.clicked() {
            if open {
                state.typing_open = None;
            } else {
                state.typing_open = Some(*t);
                state.typing_speed = typing::DEFAULT_SPEED;
            }
        }
        if open {
            typing_popover(ui, state, edit, &suggestions, duration, r);
        }
    }
    if state.typing_open.is_some_and(|o| !suggestions.contains(&o)) {
        state.typing_open = None;
    }

    // Shortcuts track (toggled from the Keyboard shortcuts tab).
    let mut line_bottom = bottom;
    if !shortcuts.is_empty() {
        let row = Rect::from_min_max(pos2(full.left(), bottom + 10.0), pos2(full.right(), bottom + 44.0));
        ui.allocate_rect(row, Sense::hover());
        shortcut_track(ui, row, &geo, edit, shortcuts, duration);
        line_bottom = row.bottom();
    }

    // Playhead.
    let x = geo.x(time);
    let p = ui.painter().with_clip_rect(Rect::from_x_y_ranges(full.x_range(), (ruler.top() - 4.0)..=(line_bottom + 2.0)));
    p.line_segment([pos2(x, ruler.top() + 4.0), pos2(x, line_bottom)], Stroke::new(1.5, theme::PLAYHEAD));
    p.circle_filled(pos2(x, ruler.top() + 4.0), 6.0, theme::PLAYHEAD);
    out
}

/// "Speed up typing", over the pill `pill`: a speed slider and Apply
/// suggestion (Enter), Apply all suggestions (Ctrl+Enter), Remove suggestion.
fn typing_popover(ui: &Ui, state: &mut State, edit: &mut EditConfig, all: &[Typing], duration: f64, pill: Rect) {
    let Some(t) = state.typing_open else { return };
    let ctx = ui.ctx().clone();
    let w = 360.0;
    let (enter, all_keys, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    let mut apply_one = enter;
    let mut apply_all = all_keys;
    let mut remove = false;
    let area = egui::Area::new(ui.scope_id().with("typing-popover"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(pill.center().x.clamp(w / 2.0 + 8.0, ctx.content_rect().right() - w / 2.0 - 8.0), pill.top() - 6.0))
        .show(&ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(0x1f, 0x1f, 0x21))
                .stroke(Stroke::new(1.0, Color32::from_white_alpha(28)))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(18))
                .shadow(egui::epaint::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(140) })
                .show(ui, |ui| {
                    ui.set_width(w - 36.0);
                    ui.horizontal(|ui| {
                        let (ir, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                        icons::paint(ui.painter(), ir, Icon::Keyboard, theme::TEXT);
                        ui.label(egui::RichText::new("Speed up typing").size(15.0).color(theme::TEXT));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("{:.1}×", state.typing_speed)).size(13.0).color(theme::TEXT_DIM));
                        });
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("You are typing during this part of the recording. Speed it up using options below.")
                            .size(13.0)
                            .color(theme::TEXT_DIM),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let mut speed = state.typing_speed;
                        ui.scope(|ui| {
                            ui.set_width(ui.available_width() - 76.0);
                            super::widgets::slider(ui, &mut speed, typing::MIN_SPEED..=typing::MAX_SPEED, None);
                        });
                        state.typing_speed = (speed * 10.0).round() / 10.0;
                        ui.add_space(8.0);
                        if popover_button(ui, "Reset", "", 68.0, false).clicked() {
                            state.typing_speed = typing::DEFAULT_SPEED;
                        }
                    });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let half = (ui.available_width() - 8.0) / 2.0;
                        apply_all |= popover_button(ui, "Apply all suggestions", "Ctrl+↵", half, false).clicked();
                        apply_one |= popover_button(ui, "Apply suggestion", "↵", half, true).clicked();
                    });
                    ui.add_space(8.0);
                    let full = ui.available_width();
                    remove = popover_button(ui, "Remove suggestion", "", full, false).clicked();
                });
        });
    // A click anywhere else closes it (the pill's own click toggles it).
    let outside = ctx.input(|i| i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !pill.contains(p)));
    if apply_all {
        for s in all {
            typing::apply(edit, *s, state.typing_speed, duration);
        }
    } else if apply_one {
        typing::apply(edit, t, state.typing_speed, duration);
    } else if remove {
        typing::remove(edit, t);
    }
    if apply_all || apply_one || remove || esc || outside {
        state.typing_open = None;
    }
}

/// A popover button: `label` and a dim key hint; `primary` is filled purple,
/// the others dark with lavender or white text.
fn popover_button(ui: &mut Ui, label: &str, hint: &str, w: f32, primary: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(w, 38.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let p = ui.painter();
    let fill = match (primary, resp.hovered()) {
        (true, false) => theme::ACCENT,
        (true, true) => Color32::from_rgb(0x5a, 0x42, 0xf5),
        (false, false) => Color32::from_rgb(0x26, 0x26, 0x29),
        (false, true) => Color32::from_rgb(0x30, 0x30, 0x34),
    };
    p.rect_filled(r, CornerRadius::same(7), fill);
    if !primary {
        p.rect_stroke(r, CornerRadius::same(7), Stroke::new(1.0, Color32::from_white_alpha(18)), StrokeKind::Inside);
    }
    // Secondary buttons with a key hint read lavender, like Screen Studio's
    // "Apply all suggestions".
    let col = if primary || hint.is_empty() { Color32::WHITE } else { theme::KNOB };
    let font = egui::FontId::proportional(13.5);
    let g = p.layout_no_wrap(label.to_string(), font.clone(), col);
    let h = p.layout_no_wrap(hint.to_string(), egui::FontId::proportional(12.0), col.gamma_multiply(0.85));
    let gap = if hint.is_empty() { 0.0 } else { 8.0 };
    let total = g.size().x + gap + h.size().x;
    let x = r.center().x - total / 2.0;
    p.galley(pos2(x, r.center().y - g.size().y / 2.0), g.clone(), col);
    p.galley(pos2(x + g.size().x + gap, r.center().y - h.size().y / 2.0), h, col);
    resp
}

/// The clip's right-click menu for slice `a..b`, as in Screen Studio.
#[allow(clippy::too_many_arguments)]
fn slice_menu(ui: &mut Ui, edit: &mut EditConfig, (a, b): (f64, f64), time: f64, duration: f64, has_system: bool, has_device: bool, out: &mut Output) {
    ui.set_min_width(260.0);
    let props = edit.slice_props((a + b) * 0.5);
    let pct = |v: f64| format!("{:.0}%", v * 100.0);
    ui.menu_button("Set speed", |ui| {
        for s in [1.0, 1.2, 1.4, 1.6, 1.8, 2.0] {
            if ui.selectable_label((props.speed - s).abs() < 1e-6, format!("{s}×")).clicked() {
                edit.set_slice_props(a, b, |p| p.speed = s);
                ui.close();
            }
        }
    });
    let volumes = [0.0, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0];
    ui.menu_button("Set volume", |ui| {
        for v in volumes {
            if ui.selectable_label((props.volume - v).abs() < 1e-6, pct(v)).clicked() {
                edit.set_slice_props(a, b, |p| p.volume = v);
                ui.close();
            }
        }
    });
    if has_system {
        ui.menu_button("Set system audio volume", |ui| {
            for v in volumes {
                if ui.selectable_label((props.system_volume - v).abs() < 1e-6, pct(v)).clicked() {
                    edit.set_slice_props(a, b, |p| p.system_volume = v);
                    ui.close();
                }
            }
        });
    }
    if has_device {
        ui.menu_button("Set external device audio volume", |ui| {
            for v in volumes {
                if ui.selectable_label((props.device_volume - v).abs() < 1e-6, pct(v)).clicked() {
                    edit.set_slice_props(a, b, |p| p.device_volume = v);
                    ui.close();
                }
            }
        });
    }
    ui.separator();
    if ui.add(egui::Button::new("Remove").shortcut_text("Del")).clicked() {
        edit.add_cut(a, b);
        out.toast = Some(format!("Removed {} — Ctrl+Z to undo", short_secs(b - a)));
        ui.close();
    }
    if ui.add(egui::Button::new("Split at current time").shortcut_text("C")).clicked() {
        edit.split_at(time, duration);
        ui.close();
    }
    ui.separator();
    let (ts, te) = edit.trim_range(duration);
    let trimmed = ((a - ts).abs() < 1e-3 && ts > 0.0) || ((b - te).abs() < 1e-3 && te < duration);
    if trimmed && ui.button("Reset trim").clicked() {
        if (a - ts).abs() < 1e-3 {
            edit.trim_start = 0.0;
        }
        if (b - te).abs() < 1e-3 {
            edit.trim_end = None;
        }
        ui.close();
    }
    if edit.splits.iter().any(|s| (s - b).abs() < 1e-3) && ui.button("Merge with next").clicked() {
        edit.merge_at(b, (a + b) * 0.5, duration);
        ui.close();
    }
    if edit.splits.iter().any(|s| (s - a).abs() < 1e-3) && ui.button("Merge with previous").clicked() {
        edit.merge_at(a, (a + b) * 0.5, duration);
        ui.close();
    }
    ui.separator();
    let smooth_label = if props.raw_cursor { "Enable smooth mouse movement" } else { "Disable smooth mouse movement" };
    if ui.button(smooth_label).clicked() {
        edit.set_slice_props(a, b, |p| p.raw_cursor = !p.raw_cursor);
        ui.close();
    }
    if ui.button(if props.hide_cursor { "Show mouse cursor" } else { "Hide mouse cursor" }).clicked() {
        edit.set_slice_props(a, b, |p| p.hide_cursor = !p.hide_cursor);
        ui.close();
    }
    ui.separator();
    if ui.button("Reset trims and cuts for all slices").clicked() {
        edit.reset_slices();
        ui.close();
    }
}

/// One pill per detected shortcut, as long as it shows in the video. Click
/// to hide or show it; right-click for all shortcuts with the same keys.
fn shortcut_track(ui: &mut Ui, row: Rect, geo: &Geo, edit: &mut EditConfig, list: &[crate::shortcuts::Shortcut], duration: f64) {
    let font = egui::FontId::proportional(12.0);
    for (i, sc) in list.iter().enumerate() {
        let end = list.get(i + 1).map_or(sc.t + crate::engine::SHORTCUT_SHOW, |n| n.t.min(sc.t + crate::engine::SHORTCUT_SHOW)).min(duration);
        let (x0, x1) = (geo.x(sc.t), geo.x(end).max(geo.x(sc.t) + 22.0));
        let r = Rect::from_x_y_ranges(x0..=x1 - 2.0, row.y_range());
        let off = edit.shortcuts.disabled.contains(&sc.id);
        let resp = ui.interact(r, ui.scope_id().with(("shortcut", sc.id)), Sense::click());
        let fill = match (off, resp.hovered()) {
            (true, _) => Color32::from_rgb(0x22, 0x24, 0x29),
            (false, true) => Color32::from_rgb(0x3a, 0x3d, 0x46),
            (false, false) => Color32::from_rgb(0x2e, 0x31, 0x39),
        };
        let p = ui.painter_at(r);
        p.rect_filled(r, CornerRadius::same(8), fill);
        p.rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0, Color32::from_white_alpha(if off { 10 } else { 24 })), StrokeKind::Inside);
        let label = sc.label();
        let col = if off { Color32::from_rgb(0x5c, 0x5e, 0x66) } else { Color32::WHITE };
        let g = p.layout_no_wrap(label.clone(), font.clone(), col);
        let tx = (r.center().x - g.size().x / 2.0).max(r.left() + 6.0);
        let gw = g.size().x;
        p.galley(pos2(tx, r.center().y - g.size().y / 2.0), g, col);
        if off {
            p.line_segment([pos2(tx, r.center().y), pos2(tx + gw, r.center().y)], Stroke::new(1.0, col));
        }
        let resp = resp.on_hover_text(if off { "Hidden. Click to show it in the video." } else { "Click to hide it from the video." });
        if resp.clicked() {
            let d = &mut edit.shortcuts.disabled;
            if off {
                d.retain(|x| *x != sc.id);
            } else {
                d.push(sc.id);
            }
        }
        resp.context_menu(|ui| {
            let same: Vec<usize> = list.iter().filter(|o| o.keys == sc.keys).map(|o| o.id).collect();
            let d = &mut edit.shortcuts.disabled;
            if ui.button(format!("Disable all \"{label}\" shortcuts")).clicked() {
                for id in &same {
                    if !d.contains(id) {
                        d.push(*id);
                    }
                }
                ui.close();
            }
            if ui.button(format!("Enable all \"{label}\" shortcuts")).clicked() {
                d.retain(|x| !same.contains(x));
                ui.close();
            }
        });
    }
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
        CameraLayoutKind::Cutout => Icon::CamCutout,
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

/// Typing suggestion pills.
const TYPING_FILL: Color32 = Color32::from_rgb(0x4a, 0x35, 0x0e);
const TYPING_FILL_HOVER: Color32 = Color32::from_rgb(0x63, 0x47, 0x12);
const TYPING_EDGE: Color32 = Color32::from_rgb(0xd0, 0x92, 0x1e);
const TYPING_TEXT: Color32 = Color32::from_rgb(0xf3, 0xe6, 0xc8);
/// Recording flags on the ruler.
const FLAG: Color32 = Color32::from_rgb(0xf5, 0xc4, 0x51);
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

#[cfg(test)]
mod fold_tests {
    use super::*;

    #[test]
    fn folded_cuts_take_no_width() {
        // 10 s recording, cuts at 2-3 and 6-8: 7 s on screen, 70 px.
        let geo = Geo { left: 0.0, width: 70.0, duration: 7.0, folds: vec![(2.0, 3.0), (6.0, 8.0)] };
        assert_eq!(geo.x(2.0), geo.x(3.0), "a cut is a seam");
        assert_eq!(geo.x(2.5), geo.x(3.0));
        assert!((geo.x(5.0) - 40.0).abs() < 1e-4);
        assert!((geo.x(9.0) - 60.0).abs() < 1e-4);
        // Back from x: kept times round-trip, a seam maps past its cut.
        for t in [0.5, 1.9, 3.5, 5.9, 8.5, 10.0] {
            assert!((geo.t(geo.x(t)) - t).abs() < 1e-3, "{t}");
        }
        assert!((geo.t(geo.x(2.0)) - 3.0).abs() < 1e-3);
    }
}

#[cfg(test)]
mod typing_tests {
    use super::*;
    use crate::project::Key;

    /// Clicking a Typing pill opens "Speed up typing"; Enter applies it to
    /// that stretch, Ctrl+Enter to all of them.
    #[test]
    fn typing_pill_popover_applies() {
        let dir = std::env::temp_dir().join("open-studio-test-config");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        let duration = 30.0;
        let typed = |from: f64| (0..20).map(move |i| Key { t: from + i as f64 * 0.15, code: 30, down: true });
        let found = typing::detect(&typed(2.0).chain(typed(10.0)).chain(typed(20.0)).collect::<Vec<_>>(), duration);
        assert_eq!(found.len(), 3);
        let ctx = egui::Context::default();
        let mut state = State::default();
        state.fold_cuts = false;
        let mut edit = EditConfig::default();
        let mut selected = None;
        let size = vec2(1200.0, 400.0);
        let mut time = 0.0;
        let mut frame = |state: &mut State, edit: &mut EditConfig, events: Vec<egui::Event>| {
            time += 0.05;
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(time), events, ..Default::default() };
            let mut full = None;
            let mut out = ctx.run_ui(raw, |ui| {
                full = Some(ui.available_rect_before_wrap());
                show(ui, state, duration, edit, 0.0, false, &mut selected, None, false, false, false, &[], &[], &found);
            });
            out.textures_delta.clear();
            full.unwrap()
        };
        let full = frame(&mut state, &mut edit, vec![]);
        // Pill of the first stretch: just above the clip track.
        let t = found[0];
        let x = |s: f64| full.left() + 10.0 + (s / duration) as f32 * (full.width() - 20.0);
        let pill = pos2(x((t.start + t.end) / 2.0), full.top() + 36.0 - 12.0);
        let m = egui::Modifiers::default();
        let click = |pos| {
            vec![
                vec![egui::Event::PointerMoved(pos)],
                vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: m }],
                vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: m }],
            ]
        };
        for ev in click(pill) {
            frame(&mut state, &mut edit, ev);
        }
        assert_eq!(state.typing_open, Some(t), "popover didn't open");
        let key = |key, modifiers| vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }];
        frame(&mut state, &mut edit, key(egui::Key::Enter, m));
        assert_eq!(state.typing_open, None);
        assert_eq!(edit.slice_props((t.start + t.end) / 2.0).speed, typing::DEFAULT_SPEED);
        assert_eq!(typing::suggestions(&found, &edit, duration).len(), 2);
        // The rest at once.
        frame(&mut state, &mut edit, vec![]);
        let u = found[1];
        for ev in click(pos2(x((u.start + u.end) / 2.0), pill.y)) {
            frame(&mut state, &mut edit, ev);
        }
        assert_eq!(state.typing_open, Some(u));
        frame(&mut state, &mut edit, key(egui::Key::Enter, egui::Modifiers::COMMAND));
        assert!(typing::suggestions(&found, &edit, duration).is_empty());
        assert!(found.iter().all(|f| edit.slice_props((f.start + f.end) / 2.0).speed == typing::DEFAULT_SPEED));
    }
}
