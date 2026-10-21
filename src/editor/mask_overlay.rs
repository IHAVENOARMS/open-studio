//! On-preview editor for the selected mask: a rectangle that can be moved,
//! with eight resize handles, drawn through the current camera transform.

use crate::engine::{Camera, Layout};
use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2};

pub struct Mapping<'a> {
    pub screen: Rect,
    pub layout: &'a Layout,
    pub cam: Camera,
    /// Screen card placement from the camera layout: offset x, y, scale.
    pub xf: [f64; 3],
}

impl Mapping<'_> {
    /// Normalized video point to preview pixels, through the screen card's
    /// layout placement and the zoom camera (as the shader does).
    pub fn to_screen(&self, u: f64, v: f64) -> Pos2 {
        let l = self.layout;
        let [ox, oy, s] = self.xf;
        let qx = ox + (l.content.x + u * l.content.w) * s;
        let qy = oy + (l.content.y + v * l.content.h) * s;
        let h = self.screen.height() as f64;
        pos2(
            self.screen.min.x + (((qx - self.cam.cx) * self.cam.scale + l.aspect * 0.5) * h) as f32,
            self.screen.min.y + (((qy - self.cam.cy) * self.cam.scale + 0.5) * h) as f32,
        )
    }
    /// Preview pixels back to normalized video coordinates.
    pub fn to_uv(&self, p: Pos2) -> (f64, f64) {
        let l = self.layout;
        let [ox, oy, s] = self.xf;
        let h = self.screen.height() as f64;
        let ux = (p.x - self.screen.min.x) as f64 / h;
        let uy = (p.y - self.screen.min.y) as f64 / h;
        let qx = (self.cam.cx + (ux - l.aspect * 0.5) / self.cam.scale - ox) / s.max(1e-6);
        let qy = (self.cam.cy + (uy - 0.5) / self.cam.scale - oy) / s.max(1e-6);
        ((qx - l.content.x) / l.content.w, (qy - l.content.y) / l.content.h)
    }
}

/// Draws the mask rectangle and applies drags to `r` (normalized x, y, w, h).
pub fn edit(ui: &mut Ui, m: &Mapping, id: egui::Id, r: &mut [f64; 4]) {
    let a = m.to_screen(r[0], r[1]);
    let b = m.to_screen(r[0] + r[2], r[1] + r[3]);
    let rect = Rect::from_two_pos(a, b);
    let painter = ui.painter_at(m.screen);
    let white = Stroke::new(1.5, Color32::WHITE);
    painter.rect_stroke(rect, 2.0, white, egui::StrokeKind::Middle);
    let guide = Stroke::new(1.0, Color32::from_white_alpha(140));
    painter.extend(egui::Shape::dashed_line(&[pos2(rect.left(), rect.center().y), pos2(rect.right(), rect.center().y)], guide, 4.0, 4.0));
    painter.extend(egui::Shape::dashed_line(&[pos2(rect.center().x, rect.top()), pos2(rect.center().x, rect.bottom())], guide, 4.0, 4.0));

    // Body: move.
    let body = ui.interact(rect, id.with("body"), Sense::drag());
    if body.dragged() {
        let (u0, v0) = m.to_uv(rect.min);
        let (u1, v1) = m.to_uv(rect.min + body.drag_delta());
        r[0] = (r[0] + (u1 - u0)).clamp(0.0, 1.0 - r[2]);
        r[1] = (r[1] + (v1 - v0)).clamp(0.0, 1.0 - r[3]);
    }
    if body.hovered() || body.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
    }

    // Handles: (x side, y side) where -1 = min edge, 1 = max edge, 0 = none.
    let handles: [(i8, i8); 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];
    for (k, (hx, hy)) in handles.iter().copied().enumerate() {
        let x = match hx {
            -1 => rect.left(),
            1 => rect.right(),
            _ => rect.center().x,
        };
        let y = match hy {
            -1 => rect.top(),
            1 => rect.bottom(),
            _ => rect.center().y,
        };
        let c = pos2(x, y);
        let resp = ui.interact(Rect::from_center_size(c, Vec2::splat(16.0)), id.with(("handle", k)), Sense::drag());
        let hot = resp.hovered() || resp.dragged();
        painter.circle(c, if hot { 6.5 } else { 5.5 }, Color32::from_black_alpha(90), white);
        if hot {
            ui.ctx().set_cursor_icon(match (hx, hy) {
                (0, _) => egui::CursorIcon::ResizeVertical,
                (_, 0) => egui::CursorIcon::ResizeHorizontal,
                (a, b) if a == b => egui::CursorIcon::ResizeNwSe,
                _ => egui::CursorIcon::ResizeNeSw,
            });
        }
        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let (u, v) = m.to_uv(p);
                let (u, v) = (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
                let (mut x0, mut y0, mut x1, mut y1) = (r[0], r[1], r[0] + r[2], r[1] + r[3]);
                let min = 0.02;
                match hx {
                    -1 => x0 = u.min(x1 - min),
                    1 => x1 = u.max(x0 + min),
                    _ => {}
                }
                match hy {
                    -1 => y0 = v.min(y1 - min),
                    1 => y1 = v.max(y0 + min),
                    _ => {}
                }
                *r = [x0, y0, x1 - x0, y1 - y0];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Rect as CRect;

    #[test]
    fn mapping_round_trips_through_layout_placement() {
        let layout = Layout { aspect: 16.0 / 9.0, card: CRect { x: 0.1, y: 0.1, w: 1.5, h: 0.8 }, content: CRect { x: 0.1, y: 0.1, w: 1.5, h: 0.8 }, video_w: 1920.0, video_h: 1080.0 };
        let cam = Camera { cx: 0.9, cy: 0.45, scale: 1.4 };
        let m = Mapping { screen: Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(1600.0, 900.0)), layout: &layout, cam, xf: [0.05, 0.1, 0.6] };
        let p = m.to_screen(0.3, 0.7);
        let (u, v) = m.to_uv(p);
        assert!((u - 0.3).abs() < 1e-4 && (v - 0.7).abs() < 1e-4, "{u} {v}");
        // Same point as the shader: q_canvas = xf.xy + q * xf.z, then the zoom camera.
        let q = (0.05 + (0.1 + 0.3 * 1.5) * 0.6, 0.1 + (0.1 + 0.7 * 0.8) * 0.6);
        let sx = 10.0 + (((q.0 - 0.9) * 1.4 + 16.0 / 18.0) * 900.0) as f32;
        assert!((p.x - sx).abs() < 0.01);
    }
}
