//! Line icons drawn with the painter (no glyph or emoji fonts). Each icon is
//! designed on a 20x20 grid and scaled into the given rect.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Folder,
    Export,
    Aspect,
    Sparkle,
    Image,
    Zoom,
    Mouse,
    Back,
    Record,
    Stop,
    Play,
    Pause,
    SkipBack,
    SkipForward,
    Plus,
    Video,
    Aperture,
    Resolution,
    Film,
    Clipboard,
    File,
    Trash,
    Scissors,
    Mask,
    Display,
    Window,
    Phone,
    CameraOff,
    MicOff,
    SystemAudio,
    Gear,
    ChevronDown,
    Close,
    Restart,
    StopCircle,
    PauseCircle,
    Keyboard,
    Mic,
    // Editor rail.
    RailBackground,
    RailCursor,
    RailCameraLayout,
    RailCameraStyle,
    RailCaptions,
    RailTranscript,
    RailAudio,
    RailShortcuts,
    RailAnimation,
}

struct Pen<'a> {
    p: &'a Painter,
    r: Rect,
    s: Stroke,
    color: Color32,
}

impl Pen<'_> {
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        let k = self.r.width().min(self.r.height()) / 20.0;
        let o = self.r.center() - vec2(10.0 * k, 10.0 * k);
        pos2(o.x + x * k, o.y + y * k)
    }
    fn k(&self) -> f32 {
        self.r.width().min(self.r.height()) / 20.0
    }
    fn line(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        self.p.add(Shape::line(pts, self.s));
    }
    fn closed(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        self.p.add(Shape::closed_line(pts, self.s));
    }
    fn fill(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        self.p.add(Shape::convex_polygon(pts, self.color, Stroke::NONE));
    }
    fn rrect(&self, x0: f32, y0: f32, x1: f32, y1: f32, rad: f32) {
        let r = Rect::from_min_max(self.pt(x0, y0), self.pt(x1, y1));
        self.p.rect_stroke(r, rad * self.k(), self.s, egui::StrokeKind::Middle);
    }
    fn circle(&self, x: f32, y: f32, rad: f32) {
        self.p.circle_stroke(self.pt(x, y), rad * self.k(), self.s);
    }
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let k = rect.width().min(rect.height()) / 20.0;
    let pen = Pen { p, r: rect, s: Stroke::new((1.5 * k).max(1.2), color), color };
    match icon {
        Icon::Folder => pen.closed(&[(2.5, 5.0), (8.0, 5.0), (9.8, 7.0), (17.5, 7.0), (17.5, 15.5), (2.5, 15.5)]),
        Icon::Export => {
            pen.line(&[(7.0, 8.0), (4.5, 8.0), (4.5, 17.0), (15.5, 17.0), (15.5, 8.0), (13.0, 8.0)]);
            pen.line(&[(10.0, 12.0), (10.0, 2.5)]);
            pen.line(&[(6.8, 5.5), (10.0, 2.5), (13.2, 5.5)]);
        }
        Icon::Aspect => {
            pen.rrect(2.5, 5.0, 17.5, 15.0, 2.0);
            pen.line(&[(7.0, 5.0), (7.0, 15.0)]);
        }
        Icon::Sparkle => {
            pen.closed(&[(10.0, 2.5), (11.6, 8.4), (17.5, 10.0), (11.6, 11.6), (10.0, 17.5), (8.4, 11.6), (2.5, 10.0), (8.4, 8.4)]);
        }
        Icon::Image => {
            pen.rrect(2.5, 3.5, 17.5, 16.5, 2.5);
            pen.line(&[(2.8, 14.0), (7.5, 9.5), (11.0, 12.5), (13.5, 10.5), (17.2, 13.5)]);
            pen.circle(13.0, 7.2, 1.4);
        }
        Icon::Zoom => {
            pen.circle(8.5, 8.5, 5.5);
            pen.line(&[(12.5, 12.5), (17.0, 17.0)]);
            pen.line(&[(6.0, 8.5), (11.0, 8.5)]);
            pen.line(&[(8.5, 6.0), (8.5, 11.0)]);
        }
        Icon::Mouse => {
            pen.rrect(5.5, 2.5, 14.5, 17.5, 4.5);
            pen.line(&[(10.0, 5.0), (10.0, 8.0)]);
        }
        Icon::Back => {
            pen.line(&[(16.0, 10.0), (4.0, 10.0)]);
            pen.line(&[(8.5, 5.5), (4.0, 10.0), (8.5, 14.5)]);
        }
        Icon::Record => {
            p.circle_filled(pen.pt(10.0, 10.0), 5.0 * k, color);
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_min_max(pen.pt(5.5, 5.5), pen.pt(14.5, 14.5)), 1.5 * k, color);
        }
        Icon::Play => pen.fill(&[(6.5, 4.0), (6.5, 16.0), (16.0, 10.0)]),
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(pen.pt(5.5, 4.0), pen.pt(8.5, 16.0)), k, color);
            p.rect_filled(Rect::from_min_max(pen.pt(11.5, 4.0), pen.pt(14.5, 16.0)), k, color);
        }
        Icon::SkipBack => {
            pen.line(&[(4.5, 4.0), (4.5, 16.0)]);
            pen.closed(&[(15.5, 4.0), (15.5, 16.0), (7.5, 10.0)]);
        }
        Icon::SkipForward => {
            pen.line(&[(15.5, 4.0), (15.5, 16.0)]);
            pen.closed(&[(4.5, 4.0), (4.5, 16.0), (12.5, 10.0)]);
        }
        Icon::Plus => {
            pen.line(&[(10.0, 4.0), (10.0, 16.0)]);
            pen.line(&[(4.0, 10.0), (16.0, 10.0)]);
        }
        Icon::Video => {
            pen.rrect(2.5, 4.0, 17.5, 16.0, 2.5);
            pen.fill(&[(8.0, 7.5), (8.0, 12.5), (12.5, 10.0)]);
        }
        Icon::Aperture => {
            pen.circle(10.0, 10.0, 7.5);
            for i in 0..6 {
                let a = i as f32 * std::f32::consts::PI / 3.0;
                let (s, c) = a.sin_cos();
                pen.line(&[(10.0 + 7.5 * c, 10.0 + 7.5 * s), (10.0 + 3.2 * (a + 1.2).cos(), 10.0 + 3.2 * (a + 1.2).sin())]);
            }
        }
        Icon::Resolution => {
            // Dashed square.
            let segs = [
                ((3.0, 3.0), (7.0, 3.0)), ((9.0, 3.0), (11.0, 3.0)), ((13.0, 3.0), (17.0, 3.0)),
                ((3.0, 17.0), (7.0, 17.0)), ((9.0, 17.0), (11.0, 17.0)), ((13.0, 17.0), (17.0, 17.0)),
                ((3.0, 3.0), (3.0, 7.0)), ((3.0, 9.0), (3.0, 11.0)), ((3.0, 13.0), (3.0, 17.0)),
                ((17.0, 3.0), (17.0, 7.0)), ((17.0, 9.0), (17.0, 11.0)), ((17.0, 13.0), (17.0, 17.0)),
            ];
            for (a, b) in segs {
                pen.line(&[a, b]);
            }
        }
        Icon::Film => {
            pen.rrect(2.5, 4.0, 17.5, 16.0, 1.5);
            pen.line(&[(6.0, 4.0), (6.0, 16.0)]);
            pen.line(&[(14.0, 4.0), (14.0, 16.0)]);
            pen.line(&[(2.5, 10.0), (6.0, 10.0)]);
            pen.line(&[(14.0, 10.0), (17.5, 10.0)]);
        }
        Icon::Clipboard => {
            pen.rrect(4.5, 4.0, 15.5, 17.5, 2.0);
            pen.rrect(7.5, 2.5, 12.5, 5.5, 1.0);
        }
        Icon::File => {
            pen.closed(&[(5.0, 2.5), (12.0, 2.5), (15.5, 6.0), (15.5, 17.5), (5.0, 17.5)]);
            pen.line(&[(12.0, 2.5), (12.0, 6.0), (15.5, 6.0)]);
        }
        Icon::Trash => {
            pen.line(&[(3.5, 5.5), (16.5, 5.5)]);
            pen.line(&[(8.0, 5.5), (8.0, 3.0), (12.0, 3.0), (12.0, 5.5)]);
            pen.closed(&[(5.0, 5.5), (15.0, 5.5), (14.0, 17.0), (6.0, 17.0)]);
            pen.line(&[(8.5, 8.5), (8.5, 14.0)]);
            pen.line(&[(11.5, 8.5), (11.5, 14.0)]);
        }
        Icon::Scissors => {
            pen.circle(5.5, 14.5, 2.6);
            pen.circle(14.5, 14.5, 2.6);
            pen.line(&[(7.4, 12.6), (14.5, 3.0)]);
            pen.line(&[(12.6, 12.6), (5.5, 3.0)]);
        }
        Icon::Display => {
            pen.rrect(2.5, 4.0, 17.5, 15.0, 2.0);
            pen.line(&[(6.0, 12.0), (14.0, 12.0)]);
        }
        Icon::Window => {
            pen.rrect(2.5, 3.5, 17.5, 16.5, 2.0);
            pen.line(&[(2.5, 7.0), (17.5, 7.0)]);
            for x in [5.0, 7.0, 9.0] {
                p.circle_filled(pen.pt(x, 5.25), 0.7 * k, color);
            }
        }
        Icon::Phone => {
            pen.rrect(6.0, 2.5, 14.0, 17.5, 2.0);
            for (x, y) in [(8.6, 6.0), (10.0, 6.0), (11.4, 6.0), (8.6, 7.6), (10.0, 7.6), (11.4, 7.6), (8.6, 9.2), (10.0, 9.2), (11.4, 9.2)] {
                p.circle_filled(pen.pt(x, y), 0.45 * k, color);
            }
            pen.line(&[(9.0, 15.2), (11.0, 15.2)]);
        }
        Icon::CameraOff => {
            pen.rrect(2.5, 6.0, 13.0, 15.0, 2.0);
            pen.closed(&[(13.0, 9.0), (17.5, 6.5), (17.5, 14.5), (13.0, 12.0)]);
            pen.line(&[(2.5, 3.5), (17.0, 17.5)]);
        }
        Icon::MicOff => {
            pen.rrect(7.5, 2.5, 12.5, 11.5, 2.5);
            pen.line(&[(4.5, 9.0), (4.5, 10.0), (6.0, 13.5), (10.0, 15.0), (14.0, 13.5), (15.5, 10.0), (15.5, 9.0)]);
            pen.line(&[(10.0, 15.0), (10.0, 17.5)]);
            pen.line(&[(3.5, 3.0), (16.5, 17.0)]);
        }
        Icon::SystemAudio => {
            pen.rrect(2.5, 3.5, 17.5, 14.5, 2.0);
            pen.line(&[(7.0, 17.0), (13.0, 17.0)]);
            pen.line(&[(10.5, 6.0), (10.5, 11.0)]);
            pen.line(&[(10.5, 6.0), (13.0, 7.0)]);
            pen.circle(9.3, 11.0, 1.3);
        }
        Icon::Gear => {
            pen.circle(10.0, 10.0, 2.6);
            pen.circle(10.0, 10.0, 6.0);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::PI / 4.0;
                let (s, c) = a.sin_cos();
                pen.line(&[(10.0 + 6.0 * c, 10.0 + 6.0 * s), (10.0 + 7.8 * c, 10.0 + 7.8 * s)]);
            }
        }
        Icon::ChevronDown => pen.line(&[(5.0, 7.5), (10.0, 12.5), (15.0, 7.5)]),
        Icon::Close => {
            pen.line(&[(5.5, 5.5), (14.5, 14.5)]);
            pen.line(&[(14.5, 5.5), (5.5, 14.5)]);
        }
        Icon::Restart => {
            let pts: Vec<(f32, f32)> = (0..=24)
                .map(|i| {
                    let a = -1.9 + i as f32 / 24.0 * 5.2;
                    (10.0 + 6.5 * a.cos(), 10.5 + 6.5 * a.sin())
                })
                .collect();
            pen.line(&pts);
            pen.line(&[(5.6, 2.5), (8.0, 4.6), (5.5, 6.8)]);
        }
        Icon::StopCircle => {
            pen.circle(10.0, 10.0, 7.5);
            p.rect_filled(Rect::from_min_max(pen.pt(7.0, 7.0), pen.pt(13.0, 13.0)), k, color);
        }
        Icon::PauseCircle => {
            pen.circle(10.0, 10.0, 7.5);
            pen.line(&[(8.2, 7.0), (8.2, 13.0)]);
            pen.line(&[(11.8, 7.0), (11.8, 13.0)]);
        }
        Icon::RailBackground => {
            // Dashed rounded square with a filled inner square.
            let segs = [
                ((4.0, 2.5), (8.0, 2.5)), ((12.0, 2.5), (16.0, 2.5)), ((4.0, 17.5), (8.0, 17.5)), ((12.0, 17.5), (16.0, 17.5)),
                ((2.5, 4.0), (2.5, 8.0)), ((2.5, 12.0), (2.5, 16.0)), ((17.5, 4.0), (17.5, 8.0)), ((17.5, 12.0), (17.5, 16.0)),
            ];
            for (a, b) in segs {
                pen.line(&[a, b]);
            }
            p.rect_filled(Rect::from_min_max(pen.pt(6.0, 6.0), pen.pt(14.0, 14.0)), 1.5 * k, color);
        }
        Icon::RailCursor => pen.fill(&[(5.5, 2.5), (5.5, 16.0), (8.8, 12.9), (11.2, 17.6), (13.4, 16.6), (11.0, 12.0), (15.6, 12.0)]),
        Icon::RailCameraLayout => {
            pen.rrect(2.5, 3.5, 17.5, 15.0, 2.0);
            p.circle_filled(pen.pt(13.5, 10.0), 1.6 * k, color);
            pen.fill(&[(10.8, 15.0), (11.2, 12.9), (13.5, 12.2), (15.8, 12.9), (16.2, 15.0)]);
        }
        Icon::RailCameraStyle => {
            for (a, b, c) in [((2.5, 6.5), (2.5, 2.5), (6.5, 2.5)), ((13.5, 2.5), (17.5, 2.5), (17.5, 6.5)), ((17.5, 13.5), (17.5, 17.5), (13.5, 17.5)), ((6.5, 17.5), (2.5, 17.5), (2.5, 13.5))] {
                pen.line(&[a, b, c]);
            }
            p.circle_filled(pen.pt(10.0, 8.2), 2.4 * k, color);
            pen.fill(&[(5.8, 15.0), (6.6, 12.4), (10.0, 11.4), (13.4, 12.4), (14.2, 15.0)]);
        }
        Icon::RailCaptions => {
            pen.closed(&[(3.0, 3.5), (17.0, 3.5), (17.0, 13.5), (9.0, 13.5), (5.5, 17.0), (5.5, 13.5), (3.0, 13.5)]);
            pen.line(&[(6.0, 7.0), (14.0, 7.0)]);
            pen.line(&[(6.0, 10.0), (11.5, 10.0)]);
        }
        Icon::RailTranscript => {
            for x in [3.5, 7.0] {
                p.circle_filled(pen.pt(x, 5.0), 1.5 * k, color);
                pen.line(&[(x + 1.3, 5.2), (x + 0.6, 8.0)]);
            }
            pen.line(&[(11.0, 5.0), (17.5, 5.0)]);
            pen.line(&[(2.5, 11.0), (17.5, 11.0)]);
            pen.line(&[(2.5, 16.0), (13.0, 16.0)]);
        }
        Icon::RailAudio => {
            pen.closed(&[(3.0, 7.5), (6.5, 7.5), (10.5, 4.0), (10.5, 16.0), (6.5, 12.5), (3.0, 12.5)]);
            let arc: Vec<(f32, f32)> = (0..=12).map(|i| {
                let a = -0.9 + i as f32 / 12.0 * 1.8;
                (11.5 + 5.0 * a.cos(), 10.0 + 5.0 * a.sin())
            }).collect();
            pen.line(&arc);
        }
        Icon::RailShortcuts => {
            // Command (⌘) glyph: four loops joined by a square.
            pen.rrect(7.5, 7.5, 12.5, 12.5, 0.2);
            for (cx, cy) in [(5.5, 5.5), (14.5, 5.5), (14.5, 14.5), (5.5, 14.5)] {
                pen.circle(cx, cy, 2.0);
            }
        }
        Icon::RailAnimation => {
            let pts: Vec<(f32, f32)> = (0..=20).map(|i| {
                let t = i as f32 / 20.0;
                let x = 4.0 + 12.0 * t;
                let y = 15.5 - 11.0 * (t * t * (3.0 - 2.0 * t));
                (x - 2.0 * (t * std::f32::consts::PI * 2.0).sin(), y)
            }).collect();
            pen.line(&pts);
            p.circle_filled(pen.pt(4.0, 15.5), 1.8 * k, color);
            p.circle_filled(pen.pt(16.0, 4.5), 1.8 * k, color);
        }
        Icon::Mic => {
            pen.rrect(7.5, 2.5, 12.5, 11.5, 2.5);
            pen.line(&[(4.5, 9.0), (4.5, 10.0), (6.0, 13.5), (10.0, 15.0), (14.0, 13.5), (15.5, 10.0), (15.5, 9.0)]);
            pen.line(&[(10.0, 15.0), (10.0, 17.5)]);
        }
        Icon::Keyboard => {
            pen.rrect(2.0, 5.0, 18.0, 15.0, 2.0);
            for (x, y) in [(5.0, 8.0), (8.0, 8.0), (11.0, 8.0), (14.0, 8.0), (5.0, 10.5), (8.0, 10.5), (11.0, 10.5), (14.0, 10.5)] {
                p.circle_filled(pen.pt(x + 0.5, y), 0.6 * k, color);
            }
            pen.line(&[(6.5, 12.8), (13.5, 12.8)]);
        }
        Icon::Mask => {
            pen.rrect(2.5, 4.0, 17.5, 16.0, 2.5);
            p.rect_filled(Rect::from_min_max(pen.pt(5.0, 7.0), pen.pt(12.0, 13.0)), k, color);
        }
    }
}

/// Lays out `icon` followed by `text` (either may be empty) centred in `rect`.
pub fn paint_with_text(p: &Painter, rect: Rect, icon: Option<Icon>, size: f32, text: &str, font: egui::FontId, color: Color32) {
    let galley = p.layout_no_wrap(text.to_owned(), font, color);
    let gap = if icon.is_some() && !text.is_empty() { 7.0 } else { 0.0 };
    let icon_w = if icon.is_some() { size } else { 0.0 };
    let total = icon_w + gap + galley.size().x;
    let x = rect.center().x - total / 2.0;
    if let Some(i) = icon {
        paint(p, Rect::from_center_size(pos2(x + size / 2.0, rect.center().y), vec2(size, size)), i, color);
    }
    p.galley(pos2(x + icon_w + gap, rect.center().y - galley.size().y / 2.0), galley, color);
}

/// A button with an icon and label. `fill` None draws a frameless button.
pub fn button(ui: &mut egui::Ui, icon: Icon, text: &str, font: egui::FontId, fg: Color32, fill: Option<Color32>, min: egui::Vec2) -> egui::Response {
    let galley_w = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), fg).size().x;
    let icon_size = font.size + 3.0;
    let pad = if fill.is_some() { 14.0 } else { 4.0 };
    let w = (icon_size + if text.is_empty() { 0.0 } else { 7.0 + galley_w } + pad * 2.0).max(min.x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, min.y.max(font.size + 14.0)), egui::Sense::click());
    let hovered = resp.hovered();
    if let Some(f) = fill {
        let f = if hovered { f.gamma_multiply(1.12) } else { f };
        ui.painter().rect_filled(rect, 8.0, f);
    }
    let color = if fill.is_none() && hovered { Color32::WHITE } else { fg };
    paint_with_text(ui.painter(), rect, Some(icon), icon_size, text, font, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
