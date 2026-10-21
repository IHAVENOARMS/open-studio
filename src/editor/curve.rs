//! Animation curve editor for zoom transitions: spring or cubic bezier,
//! drawn as a graph with a looping preview of the motion.

use super::widgets;
use crate::edit::{AnimationSpeed, EditConfig, ZoomCurve};
use crate::theme;
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};

const CURVE_A: Color32 = Color32::from_rgb(0x5b, 0x45, 0xf0);
const CURVE_B: Color32 = theme::KNOB;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Spring,
    Bezier,
}

const BEZIER_PRESETS: [(&str, [f64; 4]); 5] = [
    ("Ease out", [0.16, 1.0, 0.3, 1.0]),
    ("Ease in-out", [0.65, 0.0, 0.35, 1.0]),
    ("Expo out", [0.19, 1.0, 0.22, 1.0]),
    ("Back out", [0.34, 1.56, 0.64, 1.0]),
    ("Linear", [0.0, 0.0, 1.0, 1.0]),
];

/// Points (x in 0..1, progress) of the curve plus the time span it covers
/// and the settle time in seconds.
fn curve_points(curve: &ZoomCurve, n: usize) -> (Vec<(f64, f64)>, f64, f64) {
    match *curve {
        ZoomCurve::Bezier { x1, y1, x2, y2, duration } => {
            let pts = (0..=n).map(|i| {
                let x = i as f64 / n as f64;
                (x, crate::edit::cubic_bezier(x1, y1, x2, y2, x))
            });
            (pts.collect(), duration, duration)
        }
        ZoomCurve::Spring { tension, friction, mass } => {
            let window = ZoomCurve::spring_window(tension, friction, mass);
            let dt = 1.0 / 600.0;
            let (mut p, mut v, mut t) = (0.0f64, 0.0f64, 0.0f64);
            let mut pts = vec![(0.0, 0.0)];
            let mut settle = 0.0;
            for i in 1..=n {
                let t_end = window * i as f64 / n as f64;
                while t < t_end {
                    let a = (tension * (1.0 - p) - friction * v) / mass;
                    v += a * dt;
                    p += v * dt;
                    t += dt;
                }
                if (p - 1.0).abs() > 0.01 {
                    settle = t_end;
                }
                pts.push((i as f64 / n as f64, p));
            }
            (pts, window, settle)
        }
    }
}

pub fn show(ui: &mut Ui, edit: &mut EditConfig) {
    let mut curve = edit.zoom_curve.unwrap_or(ZoomCurve::from_speed(edit.zoom_speed));
    let mut kind = if matches!(curve, ZoomCurve::Spring { .. }) { Kind::Spring } else { Kind::Bezier };
    let mut changed = false;

    if widgets::tabs(ui, &mut kind, &[(Kind::Spring, "Spring"), (Kind::Bezier, "Curve")]) {
        curve = match kind {
            Kind::Spring => ZoomCurve::from_speed(edit.zoom_speed),
            Kind::Bezier => ZoomCurve::EASE_OUT,
        };
        changed = true;
    }
    ui.add_space(2.0);

    // Presets.
    match kind {
        Kind::Spring => {
            let mut speed = if edit.zoom_curve.is_none() { Some(edit.zoom_speed) } else { None };
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for s in AnimationSpeed::ALL {
                    if widgets::chip(ui, s.label(), speed == Some(s)).clicked() {
                        speed = Some(s);
                        edit.zoom_speed = s;
                        curve = ZoomCurve::from_speed(s);
                        edit.zoom_curve = None;
                    }
                }
                if edit.zoom_curve.is_some() {
                    widgets::chip(ui, "Custom", true);
                }
            });
        }
        Kind::Bezier => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (name, p) in BEZIER_PRESETS {
                    let current = matches!(curve, ZoomCurve::Bezier { x1, y1, x2, y2, .. }
                        if (x1, y1, x2, y2) == (p[0], p[1], p[2], p[3]));
                    if widgets::chip(ui, name, current).clicked() {
                        let duration = match curve {
                            ZoomCurve::Bezier { duration, .. } => duration,
                            _ => 0.9,
                        };
                        curve = ZoomCurve::Bezier { x1: p[0], y1: p[1], x2: p[2], y2: p[3], duration };
                        changed = true;
                    }
                }
            });
        }
    }
    ui.add_space(4.0);
    if graph(ui, &mut curve) {
        changed = true;
    }
    ui.add_space(4.0);

    // Parameters.
    match &mut curve {
        ZoomCurve::Spring { tension, friction, mass } => {
            for (label, v, range, default) in [
                ("Tension", tension, 20.0..=900.0, 200.0),
                ("Friction", friction, 4.0..=140.0, 40.0),
                ("Mass", mass, 0.5..=8.0, 2.25),
            ] {
                theme::section(ui, &format!("{label}  ·  {v:.1}"));
                if widgets::slider(ui, v, range, Some(default)) {
                    changed = true;
                }
            }
        }
        ZoomCurve::Bezier { duration, .. } => {
            theme::section(ui, &format!("Duration  ·  {:.2}s", duration));
            if widgets::slider(ui, duration, 0.15..=2.5, Some(0.9)) {
                changed = true;
            }
        }
    }
    if changed {
        // A spring that matches a preset is stored as the preset.
        let preset = AnimationSpeed::ALL.into_iter().find(|s| ZoomCurve::from_speed(*s) == curve);
        match preset {
            Some(s) => {
                edit.zoom_speed = s;
                edit.zoom_curve = None;
            }
            None => edit.zoom_curve = Some(curve),
        }
    }
}

/// Draws the curve card; bezier control points are draggable. Returns true
/// when the curve was edited.
fn graph(ui: &mut Ui, curve: &mut ZoomCurve) -> bool {
    let w = ui.available_width();
    let (card, _) = ui.allocate_exact_size(vec2(w, 228.0), Sense::hover());
    let p = ui.painter_at(card.expand(2.0));
    p.rect_filled(card, CornerRadius::same(12), Color32::from_rgb(0x10, 0x11, 0x15));
    p.rect_stroke(card, CornerRadius::same(12), Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);

    let (pts, span, settle) = curve_points(curve, 120);
    // Vertical range: 0..1 plus any overshoot or control points.
    let mut lo = -0.08f64;
    let mut hi = 1.08f64;
    for (_, y) in &pts {
        lo = lo.min(*y - 0.05);
        hi = hi.max(*y + 0.05);
    }
    if let ZoomCurve::Bezier { y1, y2, .. } = *curve {
        lo = lo.min(y1.min(y2) - 0.05);
        hi = hi.max(y1.max(y2) + 0.05);
    }
    let plot = Rect::from_min_max(card.min + vec2(20.0, 54.0), card.max - vec2(20.0, 30.0));
    let to = |x: f64, y: f64| pos2(plot.left() + x as f32 * plot.width(), plot.bottom() - ((y - lo) / (hi - lo)) as f32 * plot.height());
    let from = |q: Pos2| (((q.x - plot.left()) / plot.width()) as f64, lo + ((plot.bottom() - q.y) / plot.height()) as f64 * (hi - lo));

    // Grid.
    let grid = Color32::from_white_alpha(9);
    for i in 1..4 {
        let x = plot.left() + plot.width() * i as f32 / 4.0;
        p.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, grid));
    }
    for y in [0.0, 1.0] {
        let a = to(0.0, y);
        let b = to(1.0, y);
        p.extend(Shape::dashed_line(&[a, b], Stroke::new(1.0, Color32::from_white_alpha(28)), 4.0, 4.0));
    }

    // Soft fill under the curve, then the curve with a horizontal gradient.
    let base_y = to(0.0, 0.0).y;
    let mut mesh = egui::Mesh::default();
    for (i, (x, y)) in pts.iter().enumerate() {
        let top = to(*x, *y);
        mesh.colored_vertex(top, CURVE_A.gamma_multiply(0.22));
        mesh.colored_vertex(pos2(top.x, base_y), CURVE_A.gamma_multiply(0.0));
        if i > 0 {
            let k = (i * 2) as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    p.add(mesh);
    for w in pts.windows(2) {
        let col = CURVE_A.lerp_to_gamma(CURVE_B, w[0].0 as f32);
        p.line_segment([to(w[0].0, w[0].1), to(w[1].0, w[1].1)], Stroke::new(2.5, col));
    }

    let mut edited = false;
    // Bezier control points.
    if let ZoomCurve::Bezier { x1, y1, x2, y2, .. } = curve {
        let handles = [((0.0, 0.0), (x1, y1), "p1"), ((1.0, 1.0), (x2, y2), "p2")];
        for (anchor, (hx, hy), id) in handles {
            let a = to(anchor.0, anchor.1);
            let h = to(*hx, *hy);
            p.line_segment([a, h], Stroke::new(1.0, Color32::from_white_alpha(70)));
            p.circle_filled(a, 3.0, Color32::from_white_alpha(120));
            let resp = ui.interact(Rect::from_center_size(h, vec2(20.0, 20.0)), ui.scope_id().with(("curve-handle", id)), Sense::drag());
            let hot = resp.hovered() || resp.dragged();
            if hot {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            if resp.dragged() {
                if let Some(pp) = resp.interact_pointer_pos() {
                    let (nx, ny) = from(pp);
                    *hx = (nx.clamp(0.0, 1.0) * 100.0).round() / 100.0;
                    *hy = (ny.clamp(-0.8, 1.8) * 100.0).round() / 100.0;
                    edited = true;
                }
            }
            p.circle_filled(h, if hot { 8.0 } else { 7.0 }, CURVE_B);
            p.circle_stroke(h, if hot { 8.0 } else { 7.0 }, Stroke::new(if hot { 2.0 } else { 1.5 }, Color32::WHITE));
        }
    }

    // Looping preview: a dot riding the curve and a tiny frame zooming in.
    let t = ui.input(|i| i.time);
    let cycle = span + 0.8;
    let phase = (t % cycle) / span;
    let x = phase.min(1.0);
    let i = ((x * (pts.len() - 1) as f64).round() as usize).min(pts.len() - 1);
    let (px, py) = pts[i];
    let dot = to(px, py);
    p.line_segment([pos2(dot.x, plot.top()), pos2(dot.x, plot.bottom())], Stroke::new(1.0, Color32::from_white_alpha(20)));
    p.circle_filled(dot, 5.0, Color32::WHITE);
    p.circle_filled(dot, 9.0, CURVE_B.gamma_multiply(0.25));
    let frame = Rect::from_min_size(pos2(card.right() - 74.0, card.top() + 10.0), vec2(56.0, 34.0));
    p.rect_filled(frame, CornerRadius::same(5), Color32::from_rgb(0x1b, 0x1d, 0x22));
    let zoom = 1.0 + py.clamp(-0.5, 1.8) as f32;
    let inner = Rect::from_center_size(frame.center(), frame.size() * 0.5 * zoom).intersect(frame.shrink(2.0));
    p.rect_filled(inner, CornerRadius::same(3), CURVE_A.lerp_to_gamma(CURVE_B, py.clamp(0.0, 1.0) as f32));
    p.rect_stroke(frame, CornerRadius::same(5), Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);

    // Captions.
    // Caption sits in the bottom row between the time labels, clear of the
    // preview thumbnail.
    let mono = egui::FontId::monospace(11.0);
    let room = plot.width() - 90.0;
    let caption = match *curve {
        ZoomCurve::Bezier { x1, y1, x2, y2, .. } => {
            let full = format!("cubic-bezier({x1:.2}, {y1:.2}, {x2:.2}, {y2:.2})");
            // Drop the function name when the sidebar is narrow.
            if p.layout_no_wrap(full.clone(), mono.clone(), theme::TEXT_DIM).size().x <= room {
                full
            } else {
                format!("{x1:.2}, {y1:.2}, {x2:.2}, {y2:.2}")
            }
        }
        ZoomCurve::Spring { .. } => format!("Settles in {settle:.2}s"),
    };
    let caption = if p.layout_no_wrap(caption.clone(), mono.clone(), theme::TEXT_DIM).size().x <= room { caption } else { String::new() };
    p.text(pos2(plot.center().x, card.bottom() - 14.0), egui::Align2::CENTER_CENTER, caption, mono, theme::TEXT_DIM);
    let small = egui::FontId::proportional(11.0);
    p.text(pos2(plot.left(), card.bottom() - 14.0), egui::Align2::LEFT_CENTER, "0s", small.clone(), theme::TEXT_DIM);
    p.text(pos2(plot.right(), card.bottom() - 14.0), egui::Align2::RIGHT_CENTER, format!("{span:.2}s"), small, theme::TEXT_DIM);
    ui.ctx().request_repaint();
    edited
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ctx: &egui::Context, edit: &mut EditConfig, t: f64, events: Vec<egui::Event>) -> egui::CursorIcon {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(380.0, 700.0))),
            time: Some(t),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| show(ui, edit)).platform_output.cursor_icon
    }

    #[test]
    fn dragging_a_bezier_handle_edits_the_curve() {
        let ctx = egui::Context::default();
        let mut edit = EditConfig { zoom_curve: Some(ZoomCurve::EASE_OUT), ..Default::default() };
        let _ = run(&ctx, &mut edit, 0.0, vec![]);
        // Find the first handle by scanning for a grab cursor.
        let mut handle = None;
        'scan: for y in (100..400).step_by(3) {
            for x in (0..380).step_by(3) {
                let pos = pos2(x as f32, y as f32);
                if run(&ctx, &mut edit, 0.1, vec![egui::Event::PointerMoved(pos)]) == egui::CursorIcon::Grab {
                    handle = Some(pos);
                    break 'scan;
                }
            }
        }
        let h = handle.expect("no draggable handle found");
        let m = egui::Modifiers::default();
        let _ = run(&ctx, &mut edit, 0.2, vec![egui::Event::PointerButton { pos: h, button: egui::PointerButton::Primary, pressed: true, modifiers: m }]);
        for i in 1..=6 {
            let _ = run(&ctx, &mut edit, 0.2 + i as f64 * 0.02, vec![egui::Event::PointerMoved(h + vec2(10.0 * i as f32, 15.0 * i as f32))]);
        }
        let _ = run(&ctx, &mut edit, 0.4, vec![egui::Event::PointerButton { pos: h + vec2(60.0, 90.0), button: egui::PointerButton::Primary, pressed: false, modifiers: m }]);
        assert_ne!(edit.zoom_curve, Some(ZoomCurve::EASE_OUT), "curve unchanged after drag");
        assert!(matches!(edit.zoom_curve, Some(ZoomCurve::Bezier { .. })));
    }
}
