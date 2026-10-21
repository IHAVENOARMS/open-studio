//! Rasterises a macOS-style arrow cursor (black fill, white outline, soft
//! shadow) from a polygon signed distance field. Premultiplied RGBA.

pub const SIZE: (u32, u32) = (128, 176);
/// Arrow tip within the image, as a fraction of its size.
pub const HOTSPOT: (f32, f32) = (0.18, 0.12);

// Arrow outline in cursor units (tip at origin, ~24 units tall).
const ARROW: [(f32, f32); 7] = [
    (0.0, 0.0),
    (0.0, 17.2),
    (4.1, 13.4),
    (6.9, 19.6),
    (9.6, 18.4),
    (6.9, 12.3),
    (12.4, 12.3),
];

fn sd_polygon(p: (f32, f32), v: &[(f32, f32)]) -> f32 {
    let dot = |a: (f32, f32), b: (f32, f32)| a.0 * b.0 + a.1 * b.1;
    let sub = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0, a.1 - b.1);
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

pub fn rasterize() -> Vec<u8> {
    let (w, h) = SIZE;
    // Units per pixel: the arrow (~20 units tall) fills ~70% of the height.
    let unit = 20.0 / (h as f32 * 0.70);
    let (hx, hy) = (HOTSPOT.0 * w as f32, HOTSPOT.1 * h as f32);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let p = ((x as f32 + 0.5 - hx) * unit, (y as f32 + 0.5 - hy) * unit);
            let d = sd_polygon(p, &ARROW);
            let aa = unit * 0.75;
            let outline = 1.35;
            let fill_a = (0.5 - d / aa).clamp(0.0, 1.0);
            let white_a = (0.5 - (d - outline) / aa).clamp(0.0, 1.0);
            let ds = sd_polygon((p.0, p.1 - 0.7), &ARROW) - outline;
            let shadow_a = 0.35 * (1.0 - (ds / 2.2).clamp(0.0, 1.0)).powi(2);
            // Composite: shadow, then white outline, then black fill (premultiplied).
            let mut c = [0.0f32; 4];
            let over = |c: &mut [f32; 4], rgb: f32, a: f32| {
                for k in 0..3 {
                    c[k] = rgb * a + c[k] * (1.0 - a);
                }
                c[3] = a + c[3] * (1.0 - a);
            };
            over(&mut c, 0.0, shadow_a);
            over(&mut c, 1.0, white_a);
            over(&mut c, 0.0, fill_a);
            let i = ((y * w + x) * 4) as usize;
            for k in 0..4 {
                // Colour channels are 0 or 1, so sRGB encoding is a no-op except for alpha blending.
                out[i + k] = (c[k] * 255.0).round() as u8;
            }
        }
    }
    out
}
