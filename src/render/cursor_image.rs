//! High-resolution cursors drawn from signed distance fields, in the style of
//! macOS: black shapes with a white outline, and a white hand with a black
//! outline. All share one unit scale so they appear at consistent sizes.
//! Premultiplied RGBA.

/// Canvas size of every vector cursor.
pub const CANVAS: u32 = 192;
/// Pixels per cursor unit (an arrow is ~20 units tall).
pub const PX_PER_UNIT: f32 = 6.16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Vector {
    Arrow,
    IBeam,
    Hand,
    OpenHand,
    ClosedHand,
    Crosshair,
    Move,
    ResizeEw,
    ResizeNs,
    ResizeNwse,
    ResizeNesw,
    NotAllowed,
}

impl Vector {
    /// Vector cursor for a canonical cursor name, if we draw one.
    pub fn for_name(name: &str) -> Option<Self> {
        Some(match name {
            "default" => Self::Arrow,
            "text" => Self::IBeam,
            "pointer" => Self::Hand,
            "grab" => Self::OpenHand,
            "grabbing" => Self::ClosedHand,
            "crosshair" | "cell" => Self::Crosshair,
            "move" => Self::Move,
            "ew-resize" => Self::ResizeEw,
            "ns-resize" => Self::ResizeNs,
            "nwse-resize" => Self::ResizeNwse,
            "nesw-resize" => Self::ResizeNesw,
            "not-allowed" => Self::NotAllowed,
            _ => return None,
        })
    }

    /// Hotspot position on the canvas, in pixels.
    pub fn hotspot(self) -> (f32, f32) {
        match self {
            Self::Arrow => (44.0, 34.0),
            Self::Hand => (82.0, 30.0),
            _ => (CANVAS as f32 / 2.0, CANVAS as f32 / 2.0),
        }
    }
}

type P = (f32, f32);

fn dot(a: P, b: P) -> f32 {
    a.0 * b.0 + a.1 * b.1
}
fn sub(a: P, b: P) -> P {
    (a.0 - b.0, a.1 - b.1)
}

fn sd_polygon(p: P, v: &[P]) -> f32 {
    let mut d = dot(sub(p, v[0]), sub(p, v[0]));
    let mut s = 1.0f32;
    let n = v.len();
    let mut j = n - 1;
    for i in 0..n {
        let e = sub(v[j], v[i]);
        let w = sub(p, v[i]);
        let t = (dot(w, e) / dot(e, e)).clamp(0.0, 1.0);
        let b = (w.0 - e.0 * t, w.1 - e.1 * t);
        d = d.min(dot(b, b));
        let c = [p.1 >= v[i].1, p.1 < v[j].1, e.0 * w.1 > e.1 * w.0];
        if (c[0] && c[1] && c[2]) || (!c[0] && !c[1] && !c[2]) {
            s = -s;
        }
        j = i;
    }
    s * d.sqrt()
}

fn sd_capsule(p: P, a: P, b: P, r: f32) -> f32 {
    let pa = sub(p, a);
    let ba = sub(b, a);
    let h = (dot(pa, ba) / dot(ba, ba).max(1e-6)).clamp(0.0, 1.0);
    let d = (pa.0 - ba.0 * h, pa.1 - ba.1 * h);
    dot(d, d).sqrt() - r
}

fn sd_round_box(p: P, c: P, half: P, r: f32) -> f32 {
    let q = ((p.0 - c.0).abs() - half.0 + r, (p.1 - c.1).abs() - half.1 + r);
    let outside = (q.0.max(0.0), q.1.max(0.0));
    dot(outside, outside).sqrt() + q.0.max(q.1).min(0.0) - r
}

fn rotate(p: P, deg: f32) -> P {
    let (s, c) = deg.to_radians().sin_cos();
    (p.0 * c + p.1 * s, -p.0 * s + p.1 * c)
}

const ARROW: [P; 7] = [(0.0, 0.0), (0.0, 17.2), (4.1, 13.4), (6.9, 19.6), (9.6, 18.4), (6.9, 12.3), (12.4, 12.3)];

/// Double-headed arrow along x, centred on the origin.
fn double_arrow(p: P) -> f32 {
    let shaft = sd_capsule(p, (-6.0, 0.0), (6.0, 0.0), 0.8);
    let left = sd_polygon(p, &[(-9.6, 0.0), (-5.4, -3.6), (-5.4, 3.6)]);
    let right = sd_polygon(p, &[(9.6, 0.0), (5.4, 3.6), (5.4, -3.6)]);
    shaft.min(left).min(right)
}

struct Style {
    /// Signed distance of the main shape (units).
    shape: Box<dyn Fn(P) -> f32>,
    /// Optional dark detail lines drawn on top of the fill (hand creases).
    detail: Option<Box<dyn Fn(P) -> f32>>,
    fill: f32,
    outline: f32,
    outline_width: f32,
}

fn style(v: Vector) -> Style {
    let black = |shape: Box<dyn Fn(P) -> f32>, w: f32| Style { shape, detail: None, fill: 0.0, outline: 1.0, outline_width: w };
    let palm = |p: P| sd_round_box(p, (4.6, 13.4), (5.4, 4.3), 2.6).min(sd_round_box(p, (4.6, 18.4), (4.3, 1.8), 0.9));
    match v {
        Vector::Arrow => black(Box::new(|p| sd_polygon(p, &ARROW)), 1.35),
        Vector::IBeam => black(
            Box::new(|p| {
                let bar = sd_capsule(p, (0.0, -7.6), (0.0, 7.6), 0.55);
                let serif = |a: P, b: P| sd_capsule(p, a, b, 0.55);
                bar.min(serif((-2.5, -9.0), (0.0, -7.7)))
                    .min(serif((2.5, -9.0), (0.0, -7.7)))
                    .min(serif((-2.5, 9.0), (0.0, 7.7)))
                    .min(serif((2.5, 9.0), (0.0, 7.7)))
            }),
            1.0,
        ),
        Vector::Hand => Style {
            shape: Box::new(move |p| {
                let f = |a: P, b: P, r: f32| sd_capsule(p, a, b, r);
                f((0.0, 1.6), (0.0, 9.5), 1.6)
                    .min(f((3.2, 6.6), (3.2, 10.5), 1.5))
                    .min(f((6.1, 7.4), (6.1, 11.0), 1.4))
                    .min(f((8.7, 8.6), (8.7, 11.8), 1.25))
                    .min(f((-0.7, 12.0), (-3.4, 9.0), 1.4))
                    .min(palm(p))
            }),
            detail: Some(Box::new(|p| {
                let l = |a: P, b: P| sd_capsule(p, a, b, 0.22);
                l((1.6, 9.0), (1.6, 11.6)).min(l((4.65, 9.8), (4.65, 11.8))).min(l((7.4, 10.4), (7.4, 12.2)))
            })),
            fill: 1.0,
            outline: 0.0,
            outline_width: 0.85,
        },
        Vector::OpenHand => Style {
            shape: Box::new(move |p| {
                let p = (p.0 + 4.6, p.1 + 9.0);
                let f = |a: P, b: P, r: f32| sd_capsule(p, a, b, r);
                f((0.0, 2.6), (0.0, 10.0), 1.5)
                    .min(f((3.1, 1.6), (3.1, 10.0), 1.5))
                    .min(f((6.1, 2.4), (6.1, 10.5), 1.4))
                    .min(f((8.8, 4.6), (8.8, 11.0), 1.25))
                    .min(f((-0.8, 12.6), (-3.9, 9.2), 1.45))
                    .min(palm(p))
            }),
            detail: None,
            fill: 1.0,
            outline: 0.0,
            outline_width: 0.85,
        },
        Vector::ClosedHand => Style {
            shape: Box::new(move |p| {
                let p = (p.0 + 4.6, p.1 + 12.0);
                let f = |a: P, b: P, r: f32| sd_capsule(p, a, b, r);
                f((0.2, 8.6), (0.2, 9.8), 1.5)
                    .min(f((3.2, 8.2), (3.2, 9.8), 1.5))
                    .min(f((6.1, 8.5), (6.1, 10.0), 1.4))
                    .min(f((8.7, 9.2), (8.7, 10.5), 1.25))
                    .min(f((-0.6, 12.4), (-2.2, 10.8), 1.4))
                    .min(palm(p))
            }),
            detail: Some(Box::new(|p| {
                let p = (p.0 + 4.6, p.1 + 12.0);
                let l = |a: P, b: P| sd_capsule(p, a, b, 0.22);
                l((1.7, 8.9), (1.7, 10.6)).min(l((4.65, 8.9), (4.65, 10.6))).min(l((7.4, 9.3), (7.4, 10.8)))
            })),
            fill: 1.0,
            outline: 0.0,
            outline_width: 0.85,
        },
        Vector::Crosshair => black(
            Box::new(|p| sd_capsule(p, (-8.0, 0.0), (8.0, 0.0), 0.45).min(sd_capsule(p, (0.0, -8.0), (0.0, 8.0), 0.45))),
            1.0,
        ),
        Vector::Move => black(Box::new(|p| double_arrow(p).min(double_arrow(rotate(p, 90.0)))), 1.2),
        Vector::ResizeEw => black(Box::new(double_arrow), 1.2),
        Vector::ResizeNs => black(Box::new(|p| double_arrow(rotate(p, 90.0))), 1.2),
        Vector::ResizeNwse => black(Box::new(|p| double_arrow(rotate(p, 45.0))), 1.2),
        Vector::ResizeNesw => black(Box::new(|p| double_arrow(rotate(p, -45.0))), 1.2),
        Vector::NotAllowed => black(
            Box::new(|p| {
                let ring = (dot(p, p).sqrt() - 6.2).abs() - 1.0;
                ring.min(sd_capsule(p, (-4.4, -4.4), (4.4, 4.4), 1.0))
            }),
            1.1,
        ),
    }
}

pub fn rasterize(v: Vector) -> Vec<u8> {
    let s = style(v);
    let n = CANVAS;
    let unit = 1.0 / PX_PER_UNIT;
    let (hx, hy) = v.hotspot();
    let mut out = vec![0u8; (n * n * 4) as usize];
    let aa = unit * 0.75;
    for y in 0..n {
        for x in 0..n {
            let p = ((x as f32 + 0.5 - hx) * unit, (y as f32 + 0.5 - hy) * unit);
            let d = (s.shape)(p);
            let fill_a = (0.5 - d / aa).clamp(0.0, 1.0);
            let outline_a = (0.5 - (d - s.outline_width) / aa).clamp(0.0, 1.0);
            let ds = (s.shape)((p.0, p.1 - 0.7)) - s.outline_width;
            let shadow_a = 0.32 * (1.0 - (ds / 2.4).clamp(0.0, 1.0)).powi(2);
            let mut c = [0.0f32; 4];
            let over = |c: &mut [f32; 4], v: f32, a: f32| {
                for k in 0..3 {
                    c[k] = v * a + c[k] * (1.0 - a);
                }
                c[3] = a + c[3] * (1.0 - a);
            };
            over(&mut c, 0.0, shadow_a);
            over(&mut c, s.outline, outline_a);
            over(&mut c, s.fill, fill_a);
            if let Some(detail) = &s.detail {
                let dd = detail(p);
                over(&mut c, s.outline, (0.5 - dd / aa).clamp(0.0, 1.0) * fill_a);
            }
            let i = ((y * n + x) * 4) as usize;
            for k in 0..4 {
                out[i + k] = (c[k] * 255.0).round() as u8;
            }
        }
    }
    out
}
