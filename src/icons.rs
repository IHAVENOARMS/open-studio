//! Line icons drawn with the painter (no glyph or emoji fonts). Each icon is
//! designed on a 20x20 grid and scaled into the given rect.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Folder,
    Export,
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
    Loupe,
    ZoomAdd,
    Undo,
    Redo,
    AspectLayout,
    Crop,
    Quality,
    ZoomOutLens,
    ZoomInLens,
    CamCutout,
    CamPip,
    CamSplit,
    CamFull,
    CamOnly,
    CamHidden,
    MaskHighlight,
    MaskBlur,
    EyeSlash,
    Mask,
    Display,
    Window,
    Phone,
    CameraOff,
    CameraOn,
    MicOff,
    SystemAudio,
    Gear,
    ChevronDown,
    Close,
    Restart,
    StopCircle,
    PauseCircle,
    Keyboard,
    Flag,
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
    // Transcript editor.
    FillerWords,
    TightenPauses,
    TranscriptLines,
    // Top bar and command menu.
    Command,
    Presets,
    PresetGear,
    ExportFilled,
    ArrowRight,
    ExtractFile,
    ClipboardVideo,
    // Playback bar and clip labels.
    WaveformZoom,
    Speedometer,
    FoldCuts,
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
        Icon::Command => {
            // The Linux Super key symbol (❖) inside a circle.
            pen.circle(10.0, 10.0, 8.2);
            let (h, d) = (1.75, 2.05);
            for (dx, dy) in [(0.0, -d), (d, 0.0), (0.0, d), (-d, 0.0)] {
                let (x, y) = (10.0 + dx, 10.0 + dy);
                pen.fill(&[(x, y - h), (x + h, y), (x, y + h), (x - h, y)]);
            }
        }
        Icon::Presets => {
            // A wand with seven rays around its tip (the eighth is the wand).
            let (cx, cy) = (8.2, 8.2);
            for deg in [-90.0f32, -45.0, 0.0, 90.0, 135.0, 180.0, -135.0] {
                let (s, c) = deg.to_radians().sin_cos();
                pen.line(&[(cx + c * 3.6, cy + s * 3.6), (cx + c * 6.0, cy + s * 6.0)]);
            }
            let d = std::f32::consts::FRAC_1_SQRT_2 * 1.05;
            pen.closed(&[(cx + 1.2 - d, cy + 1.2 + d), (cx + 1.2 + d, cy + 1.2 - d), (17.2 + d, 17.2 - d), (17.2 - d, 17.2 + d)]);
        }
        Icon::PresetGear => {
            // Toothed ring with a three-spoke hub.
            let teeth = 18;
            let pts: Vec<(f32, f32)> = (0..teeth * 2)
                .map(|i| {
                    let a = i as f32 / (teeth * 2) as f32 * std::f32::consts::TAU;
                    let r = if i % 2 == 0 { 8.0 } else { 6.6 };
                    (10.0 + a.cos() * r, 10.0 + a.sin() * r)
                })
                .collect();
            pen.closed(&pts);
            pen.circle(10.0, 10.0, 5.2);
            pen.circle(10.0, 10.0, 1.3);
            for deg in [0.0f32, 120.0, 240.0] {
                let (s, c) = deg.to_radians().sin_cos();
                pen.line(&[(10.0 + c * 1.3, 10.0 + s * 1.3), (10.0 + c * 5.2, 10.0 + s * 5.2)]);
            }
        }
        Icon::ExportFilled => {
            pen.fill(&[(4.5, 8.0), (15.5, 8.0), (15.5, 17.0), (4.5, 17.0)]);
            p.line_segment([pen.pt(10.0, 12.5), pen.pt(10.0, 2.5)], Stroke::new(1.8 * k, color));
            pen.line(&[(6.8, 5.5), (10.0, 2.5), (13.2, 5.5)]);
            p.line_segment([pen.pt(10.0, 8.0), pen.pt(10.0, 12.5)], Stroke::new(1.8 * k, Color32::BLACK));
        }
        Icon::ArrowRight => {
            pen.line(&[(3.5, 10.0), (16.0, 10.0)]);
            pen.line(&[(11.5, 5.5), (16.0, 10.0), (11.5, 14.5)]);
        }
        Icon::ExtractFile => {
            pen.line(&[(7.0, 7.0), (4.5, 7.0), (4.5, 17.0), (15.5, 17.0), (15.5, 7.0), (13.0, 7.0)]);
            pen.line(&[(10.0, 2.5), (10.0, 12.5)]);
            pen.line(&[(6.8, 9.5), (10.0, 12.7), (13.2, 9.5)]);
        }
        Icon::ClipboardVideo => {
            // Speech bubble holding a camera: "send as a message".
            pen.closed(&[(5.0, 3.5), (15.0, 3.5), (17.0, 5.5), (17.0, 12.5), (15.0, 14.5), (8.5, 14.5), (5.0, 17.5), (5.0, 14.5), (3.0, 12.5), (3.0, 5.5)]);
            pen.fill(&[(6.3, 6.8), (11.3, 6.8), (11.3, 11.2), (6.3, 11.2)]);
            pen.fill(&[(11.6, 9.0), (14.2, 7.2), (14.2, 10.8)]);
        }
        Icon::WaveformZoom => {
            // Waveform bars with a small magnifier at the bottom right.
            for (x, h) in [(2.6, 2.2), (5.0, 5.2), (7.4, 7.2), (9.8, 4.6), (12.2, 3.0)] {
                pen.line(&[(x, 9.0 - h), (x, 9.0 + h)]);
            }
            let small = Pen { s: Stroke::new(pen.s.width * 0.9, color), ..pen };
            small.circle(14.4, 13.6, 2.4);
            small.line(&[(16.1, 15.3), (18.2, 17.4)]);
        }
        Icon::FoldCuts => {
            // Two arrows closing in on a seam.
            pen.line(&[(10.0, 3.5), (10.0, 16.5)]);
            pen.fill(&[(2.5, 6.8), (7.6, 10.0), (2.5, 13.2)]);
            pen.fill(&[(17.5, 6.8), (12.4, 10.0), (17.5, 13.2)]);
        }
        Icon::Speedometer => {
            pen.circle(10.0, 10.0, 7.5);
            pen.line(&[(10.0, 10.0), (13.4, 6.6)]);
            p.circle_filled(pen.pt(10.0, 10.0), 1.3 * k, color);
        }
        Icon::FillerWords => {
            // Waveform bars with a minus badge.
            for (x, h) in [(2.8, 2.0), (5.4, 5.0), (8.0, 7.0), (10.6, 3.5)] {
                pen.line(&[(x, 10.0 - h), (x, 10.0 + h)]);
            }
            p.circle_filled(pen.pt(14.8, 13.8), 3.9 * k, color);
            p.line_segment([pen.pt(13.0, 13.8), pen.pt(16.6, 13.8)], Stroke::new(1.4 * k, Color32::BLACK));
        }
        Icon::TightenPauses => {
            // A frame squeezed from both sides.
            pen.rrect(5.6, 4.5, 14.4, 15.5, 1.6);
            pen.line(&[(8.4, 4.5), (8.4, 15.5)]);
            pen.line(&[(11.6, 4.5), (11.6, 15.5)]);
            pen.fill(&[(1.2, 7.4), (1.2, 12.6), (3.8, 10.0)]);
            pen.fill(&[(18.8, 7.4), (18.8, 12.6), (16.2, 10.0)]);
        }
        Icon::TranscriptLines => {
            p.circle_filled(pen.pt(4.6, 5.2), 2.1 * k, color);
            pen.line(&[(8.6, 5.2), (17.0, 5.2)]);
            pen.line(&[(3.0, 10.2), (17.0, 10.2)]);
            pen.line(&[(3.0, 14.8), (17.0, 14.8)]);
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
        Icon::CameraOn => {
            pen.rrect(2.5, 6.0, 13.0, 15.0, 2.0);
            pen.closed(&[(13.0, 9.0), (17.5, 6.5), (17.5, 14.5), (13.0, 12.0)]);
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
        // Rail icons, redrawn from Screen Studio's v4 rail (landing page
        // video at 2x): heavier 1.6-unit strokes, filled details.
        Icon::RailBackground => {
            // Dashed rounded square (corner pieces plus two dashes a side)
            // around a filled square.
            let pen = Pen { s: Stroke::new(1.6 * k, color), ..pen };
            let (x0, y0, x1, y1, r) = (3.55, 3.55, 16.45, 16.45, 2.0);
            let arc = |cx: f32, cy: f32, a0: f32| -> Vec<(f32, f32)> {
                (0..=6).map(|i| {
                    let a = a0 + std::f32::consts::FRAC_PI_2 * i as f32 / 6.0;
                    (cx + r * a.cos(), cy + r * a.sin())
                }).collect()
            };
            use std::f32::consts::PI;
            let arm = 0.6;
            for (cx, cy, a0, start, end) in [
                (x0 + r, y0 + r, PI, (x0, y0 + r + arm), (x0 + r + arm, y0)),
                (x1 - r, y0 + r, -PI / 2.0, (x1 - r - arm, y0), (x1, y0 + r + arm)),
                (x1 - r, y1 - r, 0.0, (x1, y1 - r - arm), (x1 - r - arm, y1)),
                (x0 + r, y1 - r, PI / 2.0, (x0 + r + arm, y1), (x0, y1 - r - arm)),
            ] {
                let mut pts = vec![start];
                pts.extend(arc(cx, cy, a0));
                pts.push(end);
                pen.line(&pts);
            }
            let (g, d) = (1.5, 1.6);
            let s0 = x0 + r + arm + g;
            for (a, b) in [(s0, s0 + d), (s0 + d + g, s0 + 2.0 * d + g)] {
                pen.line(&[(a, y0), (b, y0)]);
                pen.line(&[(a, y1), (b, y1)]);
                pen.line(&[(x0, a), (x0, b)]);
                pen.line(&[(x1, a), (x1, b)]);
            }
            p.rect_filled(Rect::from_min_max(pen.pt(6.05, 6.05), pen.pt(13.95, 13.95)), 0.9 * k, color);
        }
        Icon::RailCursor => {
            // Solid arrow: head and tail as two convex parts.
            pen.fill(&[(5.6, 2.8), (5.8, 14.4), (9.0, 11.9), (11.2, 11.5), (14.8, 11.1)]);
            pen.fill(&[(8.1, 12.2), (10.5, 17.4), (13.1, 16.4), (10.9, 11.4)]);
            p.circle_filled(pen.pt(11.8, 16.9), 1.35 * k, color);
        }
        Icon::RailCameraLayout => {
            // Screen outline with a person over its bottom-right corner,
            // cut out from the outline.
            let pen = Pen { s: Stroke::new(1.6 * k, color), ..pen };
            pen.rrect(0.9, 4.2, 16.3, 16.2, 2.1);
            let bg = crate::theme::BG;
            p.circle_filled(pen.pt(15.0, 9.3), 3.4 * k, bg);
            p.rect_filled(Rect::from_min_max(pen.pt(9.8, 11.6), pen.pt(20.0, 17.6)), 4.0 * k, bg);
            p.circle_filled(pen.pt(15.0, 9.3), 2.6 * k, color);
            dome(p, &pen, 10.6, 19.4, 12.4, 16.4, color);
        }
        Icon::RailCameraStyle => {
            // Viewfinder corners around a person.
            let pen = Pen { s: Stroke::new(1.6 * k, color), ..pen };
            let (x0, y0, x1, y1, a) = (3.2, 3.3, 16.8, 16.7, 3.2);
            for (p0, c, p1) in [
                ((x0, y0 + a), (x0, y0), (x0 + a, y0)),
                ((x1 - a, y0), (x1, y0), (x1, y0 + a)),
                ((x1, y1 - a), (x1, y1), (x1 - a, y1)),
                ((x0 + a, y1), (x0, y1), (x0, y1 - a)),
            ] {
                // Rounded corner: approach and leave the corner with a small curve.
                let lerp = |a: (f32, f32), b: (f32, f32), t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                let (q0, q1) = (lerp(c, p0, 0.45), lerp(c, p1, 0.45));
                let mut pts = vec![p0, q0];
                for i in 1..6 {
                    let t = i as f32 / 6.0;
                    let m = lerp(lerp(q0, c, t), lerp(c, q1, t), t);
                    pts.push(m);
                }
                pts.extend([q1, p1]);
                pen.line(&pts);
            }
            p.circle_filled(pen.pt(10.0, 7.6), 2.3 * k, color);
            dome(p, &pen, 6.0, 14.0, 10.9, 14.3, color);
        }
        Icon::RailCaptions => {
            // Speech bubble with a tail at the lower left and two dashed lines.
            let pen = Pen { s: Stroke::new(1.6 * k, color), ..pen };
            pen.rrect(2.7, 3.1, 17.3, 14.2, 2.5);
            let bg = crate::theme::BG;
            p.rect_filled(Rect::from_min_max(pen.pt(6.3, 13.2), pen.pt(9.3, 15.0)), 0.0, bg);
            pen.line(&[(5.8, 14.2), (6.5, 17.1), (10.0, 14.2)]);
            let thin = Pen { s: Stroke::new(1.0 * k, color), ..pen };
            for (y, segs) in [(8.9, [(5.9, 9.6), (10.9, 14.2)]), (11.0, [(5.9, 7.5), (8.8, 14.2)])] {
                for (a, b) in segs {
                    thin.line(&[(a, y), (b, y)]);
                }
            }
        }
        Icon::RailTranscript => {
            // Filled "66" quote, two short lines beside it, two full lines below.
            let pen = Pen { s: Stroke::new(1.6 * k, color), ..pen };
            let thin = Pen { s: Stroke::new(1.25 * k, color), ..pen };
            for cx in [4.9, 8.6] {
                p.circle_filled(pen.pt(cx, 6.4), 1.65 * k, color);
                thin.line(&[(cx - 1.2, 6.0), (cx - 1.1, 4.6), (cx - 0.3, 3.4), (cx + 1.0, 2.9)]);
            }
            for (y, x0) in [(4.9, 12.4), (8.3, 12.4), (11.8, 3.4), (15.2, 3.4)] {
                pen.line(&[(x0, y), (16.8, y)]);
            }
        }
        Icon::RailAudio => {
            let pen = Pen { s: Stroke::new(1.6 * k, color), ..pen };
            pen.closed(&[(3.6, 7.2), (7.0, 7.2), (11.6, 3.9), (11.6, 16.1), (7.0, 12.8), (3.6, 12.8)]);
            let arc: Vec<(f32, f32)> = (0..=10).map(|i| {
                let a = -0.7 + i as f32 / 10.0 * 1.4;
                (12.6 + 3.3 * a.cos(), 10.0 + 3.3 * a.sin())
            }).collect();
            pen.line(&arc);
        }
        Icon::RailShortcuts => {
            // The Linux Super key symbol (❖): four diamonds around the centre.
            let (h, d) = (3.1, 3.55);
            for (dx, dy) in [(0.0, -d), (d, 0.0), (0.0, d), (-d, 0.0)] {
                let (x, y) = (10.0 + dx, 10.0 + dy);
                pen.fill(&[(x, y - h), (x + h, y), (x, y + h), (x - h, y)]);
            }
        }
        Icon::RailAnimation => {
            // S-curve between two filled end dots.
            let pen = Pen { s: Stroke::new(1.35 * k, color), ..pen };
            let (p0, p1, p2, p3) = ((5.9f32, 4.6f32), (5.6f32, 11.8f32), (14.4f32, 8.2f32), (14.1f32, 15.4f32));
            let pts: Vec<(f32, f32)> = (0..=24).map(|i| {
                let t = i as f32 / 24.0;
                let u = 1.0 - t;
                (
                    u * u * u * p0.0 + 3.0 * u * u * t * p1.0 + 3.0 * u * t * t * p2.0 + t * t * t * p3.0,
                    u * u * u * p0.1 + 3.0 * u * u * t * p1.1 + 3.0 * u * t * t * p2.1 + t * t * t * p3.1,
                )
            }).collect();
            pen.line(&pts);
            p.circle_filled(pen.pt(p0.0, p0.1), 2.5 * k, color);
            p.circle_filled(pen.pt(p3.0, p3.1), 2.5 * k, color);
        }
        Icon::Mic => {
            pen.rrect(7.5, 2.5, 12.5, 11.5, 2.5);
            pen.line(&[(4.5, 9.0), (4.5, 10.0), (6.0, 13.5), (10.0, 15.0), (14.0, 13.5), (15.5, 10.0), (15.5, 9.0)]);
            pen.line(&[(10.0, 15.0), (10.0, 17.5)]);
        }
        Icon::Flag => {
            // Pole and a waving flag (Screen Studio's recording flag).
            pen.line(&[(5.0, 2.5), (5.0, 18.5)]);
            pen.closed(&[(5.0, 3.5), (8.5, 2.6), (11.5, 4.2), (15.5, 3.4), (15.5, 11.0), (11.5, 11.8), (8.5, 10.2), (5.0, 11.1)]);
        }
        Icon::Keyboard => {
            pen.rrect(2.0, 5.0, 18.0, 15.0, 2.0);
            for (x, y) in [(5.0, 8.0), (8.0, 8.0), (11.0, 8.0), (14.0, 8.0), (5.0, 10.5), (8.0, 10.5), (11.0, 10.5), (14.0, 10.5)] {
                p.circle_filled(pen.pt(x + 0.5, y), 0.6 * k, color);
            }
            pen.line(&[(6.5, 12.8), (13.5, 12.8)]);
        }
        // Left tool pill icons, redrawn from Screen Studio's v4 toolbar.
        Icon::ZoomAdd | Icon::Loupe => {
            let pen = Pen { s: Stroke::new(1.45 * k, color), ..pen };
            pen.circle(9.1, 8.8, 4.95);
            pen.line(&[(12.9, 12.6), (15.9, 15.6)]);
            if icon == Icon::ZoomAdd {
                // Diagonal double arrow inside the lens.
                let thin = Pen { s: Stroke::new(1.25 * k, color), ..pen };
                let (a, b) = ((7.0, 6.7), (11.2, 10.9));
                thin.line(&[a, b]);
                thin.line(&[(a.0 + 1.7, a.1), a, (a.0, a.1 + 1.7)]);
                thin.line(&[(b.0 - 1.7, b.1), b, (b.0, b.1 - 1.7)]);
            } else {
                // Four-point star: two thin diamonds and a small centre.
                let (cx, cy, r, w) = (9.1, 8.8, 2.7, 0.75);
                for (dx, dy) in [(w, r), (r, w)] {
                    p.add(egui::Shape::convex_polygon(
                        vec![pen.pt(cx, cy - dy), pen.pt(cx + dx, cy), pen.pt(cx, cy + dy), pen.pt(cx - dx, cy)],
                        color,
                        Stroke::NONE,
                    ));
                }
                p.circle_filled(pen.pt(cx, cy), 1.0 * k, color);
            }
        }
        Icon::Undo | Icon::Redo => {
            // A hooked arrow: back (undo) or forward (redo).
            let flip = |x: f32| if icon == Icon::Redo { 20.0 - x } else { x };
            let mut pts = vec![(flip(4.5), 8.0)];
            pts.extend((0..=12).map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 12.0;
                (flip(11.0 + 4.6 * a.cos()), 12.6 + 4.6 * a.sin())
            }));
            pts.push((flip(8.0), 17.2));
            pen.line(&pts);
            pen.line(&[(flip(7.6), 4.8), (flip(4.5), 8.0), (flip(7.6), 11.2)]);
        }
        // Preview toolbar icons (Screen Studio v4).
        Icon::AspectLayout => {
            pen.rrect(2.5, 4.0, 17.5, 16.0, 2.2);
            pen.line(&[(2.5, 8.2), (17.5, 8.2)]);
            pen.line(&[(11.6, 8.2), (11.6, 16.0)]);
        }
        Icon::Crop => {
            pen.line(&[(5.5, 2.5), (5.5, 14.5), (17.5, 14.5)]);
            pen.line(&[(2.5, 5.5), (14.5, 5.5), (14.5, 17.5)]);
        }
        Icon::Quality => {
            // Seven rings: one in the middle, six around it.
            let r = 1.75;
            pen.circle(10.0, 10.0, r);
            for i in 0..6 {
                let a = std::f32::consts::PI / 3.0 * i as f32;
                pen.circle(10.0 + 4.3 * a.cos(), 10.0 + 4.3 * a.sin(), r);
            }
        }
        Icon::ZoomOutLens | Icon::ZoomInLens => {
            pen.circle(8.6, 8.6, 5.6);
            pen.line(&[(12.7, 12.7), (16.8, 16.8)]);
            pen.line(&[(6.2, 8.6), (11.0, 8.6)]);
            if icon == Icon::ZoomInLens {
                pen.line(&[(8.6, 6.2), (8.6, 11.0)]);
            }
        }
        Icon::CamPip => {
            // Screen outline with a camera disc on its lower-left corner; the
            // outline stops short of the disc.
            let pen = Pen { s: Stroke::new(1.4 * k, color), ..pen };
            let (x0, y0, x1, y1, r) = (4.2, 3.6, 19.0, 15.2, 1.8);
            let mut pts = vec![(7.9, y1)];
            pts.extend(corner_arc(x1 - r, y1 - r, r, 0.5, 0.0));
            pts.extend(corner_arc(x1 - r, y0 + r, r, 0.0, -0.5));
            pts.extend(corner_arc(x0 + r, y0 + r, r, -0.5, -1.0));
            pts.push((x0, 9.2));
            pen.line(&pts);
            let (cx, cy) = (4.4, 13.0);
            p.circle_filled(pen.pt(cx, cy), 3.25 * k, color);
            let dark = Color32::from_rgb(0x1b, 0x1d, 0x22);
            p.circle_filled(pen.pt(cx, cy - 0.9), 1.0 * k, dark);
            dome(p, &pen, cx - 1.7, cx + 1.7, cy + 0.5, cy + 2.0, dark);
        }
        Icon::CamSplit => {
            let pen = Pen { s: Stroke::new(1.4 * k, color), ..pen };
            pen.rrect(2.2, 3.7, 17.8, 16.1, 1.8);
            pen.line(&[(10.0, 3.7), (10.0, 16.1)]);
        }
        Icon::CamCutout => {
            // Person in front of a dotted background (background removed).
            let (hx, hy, hr) = (9.6, 8.5, 2.5);
            for row in 0..6 {
                for col in 0..8 {
                    let (x, y) = (2.5 + col as f32 * 2.14, 3.9 + row as f32 * 2.4);
                    let near_head = (x - hx).hypot(y - hy) < hr + 1.4;
                    let near_body = y > 11.0 && (x - hx).abs() < 5.2 + (y - 11.0) * 0.1;
                    if !near_head && !near_body {
                        p.circle_filled(pen.pt(x, y), 0.48 * k, color);
                    }
                }
            }
            p.circle_filled(pen.pt(hx, hy), hr * k, color);
            dome(p, &pen, 5.0, 14.2, 12.3, 16.3, color);
        }
        Icon::CamFull => {
            // Screen-sized frame filled by a person (camera fullscreen).
            let pen = Pen { s: Stroke::new(1.4 * k, color), ..pen };
            pen.rrect(2.3, 3.8, 17.6, 16.1, 1.8);
            p.circle_filled(pen.pt(10.0, 8.5), 2.35 * k, color);
            dome(p, &pen, 5.9, 14.1, 12.4, 16.1, color);
        }
        Icon::CamHidden => {
            // Outlined person crossed out, the outline broken around the slash.
            let pen = Pen { s: Stroke::new(1.4 * k, color), ..pen };
            let (a, b) = ((4.6, 3.7), (16.8, 16.4));
            let head: Vec<(f32, f32)> = (0..=28).map(|i| {
                let t = std::f32::consts::TAU * i as f32 / 28.0;
                (11.0 + 2.15 * t.cos(), 6.1 + 2.15 * t.sin())
            }).collect();
            // Shoulders: top arc, then the base drawn as many points so the
            // gap around the slash cuts it cleanly.
            let mut body: Vec<(f32, f32)> = (0..=24).map(|i| {
                let t = std::f32::consts::PI * (1.0 + i as f32 / 24.0);
                (10.0 + 5.8 * t.cos(), 16.4 + 4.8 * t.sin())
            }).collect();
            body.extend((0..=20).map(|i| (15.8 - 11.6 * i as f32 / 20.0, 16.4)));
            for path in [head, body] {
                for part in split_near_line(&path, a, b, 0.75) {
                    pen.line(&part);
                }
            }
            pen.line(&[a, b]);
        }
        Icon::MaskBlur => {
            // Sensitive data: frame with a slashed circle.
            let pen = Pen { s: Stroke::new(1.4 * k, color), ..pen };
            pen.rrect(2.1, 3.7, 17.9, 16.2, 1.8);
            pen.circle(10.0, 9.95, 3.2);
            let d = 3.2 * std::f32::consts::FRAC_1_SQRT_2;
            pen.line(&[(10.0 - d, 9.95 + d), (10.0 + d, 9.95 - d)]);
        }
        Icon::MaskHighlight => {
            // Highlight: hatched frame with a clear circle in the middle.
            let pen = Pen { s: Stroke::new(1.4 * k, color), ..pen };
            let (x0, y0, x1, y1) = (2.1, 3.7, 17.9, 16.2);
            pen.rrect(x0, y0, x1, y1, 1.8);
            let (cx, cy, r) = (10.0, 9.95, 3.3);
            pen.circle(cx, cy, r);
            // Lines x + y = c, clipped to the frame and kept outside the circle.
            for c in [9.0f32, 13.6, 18.2, 22.8, 27.4] {
                let pts: Vec<(f32, f32)> = (0..=60)
                    .map(|i| {
                        let x = x0 + (x1 - x0) * i as f32 / 60.0;
                        (x, c - x)
                    })
                    .filter(|&(x, y)| y > y0 + 0.2 && y < y1 - 0.2 && x > x0 + 0.2 && x < x1 - 0.2)
                    .collect();
                let mut part = vec![];
                for (x, y) in pts {
                    if (x - cx).hypot(y - cy) < r + 0.9 {
                        if part.len() > 1 {
                            pen.line(&part);
                        }
                        part.clear();
                    } else {
                        part.push((x, y));
                    }
                }
                if part.len() > 1 {
                    pen.line(&part);
                }
            }
        }
        Icon::Mask => {
            pen.rrect(2.5, 4.0, 17.5, 16.0, 2.5);
            p.rect_filled(Rect::from_min_max(pen.pt(5.0, 7.0), pen.pt(12.0, 13.0)), k, color);
        }
        Icon::CamOnly => {
            pen.rrect(3.5, 3.5, 16.5, 16.5, 2.5);
            p.circle_filled(pen.pt(10.0, 8.5), 2.4 * k, color);
            pen.fill(&[(5.8, 16.5), (6.6, 13.2), (10.0, 12.0), (13.4, 13.2), (14.2, 16.5)]);
        }
        Icon::EyeSlash => {
            // Eye outline with a pupil, struck through.
            let top: Vec<Pos2> = (0..=12).map(|i| {
                let t = i as f32 / 12.0;
                pen.pt(2.5 + 15.0 * t, 10.0 - 5.5 * (std::f32::consts::PI * t).sin())
            }).collect();
            let bottom: Vec<Pos2> = (0..=12).map(|i| {
                let t = i as f32 / 12.0;
                pen.pt(2.5 + 15.0 * t, 10.0 + 5.5 * (std::f32::consts::PI * t).sin())
            }).collect();
            p.add(egui::Shape::line(top, pen.s));
            p.add(egui::Shape::line(bottom, pen.s));
            pen.circle(10.0, 10.0, 2.4);
            pen.line(&[(3.5, 3.0), (16.5, 17.0)]);
        }
    }
}

/// Quarter-ish arc around (cx, cy) from a0 to a1 (in turns of 2π).
fn corner_arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> Vec<(f32, f32)> {
    (0..=6)
        .map(|i| {
            let a = std::f32::consts::PI * (a0 + (a1 - a0) * i as f32 / 6.0);
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// Splits a polyline where it passes within `gap` of the segment a-b.
fn split_near_line(path: &[(f32, f32)], a: (f32, f32), b: (f32, f32), gap: f32) -> Vec<Vec<(f32, f32)>> {
    let dist = |p: (f32, f32)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
    };
    let mut out = vec![];
    let mut cur: Vec<(f32, f32)> = vec![];
    for &p in path {
        if dist(p) < gap {
            if cur.len() > 1 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        } else {
            cur.push(p);
        }
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    out
}

/// Filled half-ellipse (a person's shoulders) from x0..x1, top to bottom.
fn dome(p: &Painter, pen: &Pen, x0: f32, x1: f32, top: f32, bottom: f32, color: Color32) {
    let (cx, rx, ry) = ((x0 + x1) / 2.0, (x1 - x0) / 2.0, bottom - top);
    let pts: Vec<egui::Pos2> = (0..=20)
        .map(|i| {
            let a = std::f32::consts::PI * (1.0 + i as f32 / 20.0);
            pen.pt(cx + rx * a.cos(), bottom + ry * a.sin())
        })
        .collect();
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
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
