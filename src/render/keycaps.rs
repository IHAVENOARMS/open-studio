//! Keycap overlay for keyboard shortcuts, drawn on the CPU like Screen
//! Studio's v4 Mac keycaps: one dark key per pressed key, modifiers with a
//! symbol top-left and their name bottom-left, other keys centred.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use std::sync::OnceLock;

pub struct Image {
    /// Premultiplied RGBA.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        for pattern in ["SF Pro Text:semibold", "Inter:semibold", "sans-serif:semibold"] {
            let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
            let path = String::from_utf8_lossy(&out.stdout).to_string();
            if let Ok(data) = std::fs::read(&path) {
                if let Ok(f) = FontVec::try_from_vec(data) {
                    return Some(f);
                }
            }
        }
        None
    })
    .as_ref()
}

/// (symbol, name) for modifier keys.
fn modifier(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "Ctrl" => ("\u{2303}", "control"),
        "Alt" => ("\u{2325}", "alt"),
        "Shift" => ("\u{21E7}", "shift"),
        "Super" => ("\u{2318}", "super"),
        _ => return None,
    })
}

struct Canvas {
    w: usize,
    h: usize,
    /// Premultiplied linear-ish RGBA in 0..1.
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn blend(&mut self, x: usize, y: usize, c: [f32; 3], a: f32) {
        if x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let p = &mut self.px[y * self.w + x];
        for i in 0..3 {
            p[i] = c[i] * a + p[i] * (1.0 - a);
        }
        p[3] = a + p[3] * (1.0 - a);
    }

    /// Rounded rectangle with a vertical colour ramp, antialiased.
    fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, top: [f32; 4], bottom: [f32; 4]) {
        let (x0, y0) = (x.floor().max(0.0) as usize, y.floor().max(0.0) as usize);
        let (x1, y1) = (((x + w).ceil() as usize).min(self.w), ((y + h).ceil() as usize).min(self.h));
        let (cx, cy, hw, hh) = (x + w * 0.5, y + h * 0.5, w * 0.5, h * 0.5);
        for py in y0..y1 {
            let t = ((py as f32 + 0.5 - y) / h).clamp(0.0, 1.0);
            let c: [f32; 4] = std::array::from_fn(|i| top[i] + (bottom[i] - top[i]) * t);
            for px in x0..x1 {
                let qx = (px as f32 + 0.5 - cx).abs() - hw + r;
                let qy = (py as f32 + 0.5 - cy).abs() - hh + r;
                let d = (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - r;
                let cover = (0.5 - d).clamp(0.0, 1.0);
                self.blend(px, py, [c[0], c[1], c[2]], cover * c[3]);
            }
        }
    }

    fn text(&mut self, font: &FontVec, s: &str, size: f32, x: f32, baseline: f32, color: [f32; 3]) {
        let sf = font.as_scaled(PxScale::from(size));
        let mut pen = x;
        let mut prev = None;
        for ch in s.chars() {
            let id = font.glyph_id(ch);
            if let Some(p) = prev {
                pen += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(pen, baseline));
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| {
                    let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px >= 0 && py >= 0 {
                        self.blend(px as usize, py as usize, color, cov.min(1.0));
                    }
                });
            }
            pen += sf.h_advance(id);
            prev = Some(id);
        }
    }
}

fn text_width(font: &FontVec, s: &str, size: f32) -> f32 {
    let sf = font.as_scaled(PxScale::from(size));
    s.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum()
}

fn has_glyph(font: &FontVec, s: &str) -> bool {
    s.chars().all(|c| font.glyph_id(c).0 != 0)
}

/// Renders the keys for one shortcut, each `key_h` pixels tall.
pub fn render(keys: &[&str], key_h: u32) -> Image {
    let h = key_h as f32;
    let font = font();
    let gap = (h * 0.09).round();
    let pad = (h * 0.07).round();
    let shadow = (h * 0.06).round();
    // Key widths: modifiers are wide keys, single characters square.
    let widths: Vec<f32> = keys
        .iter()
        .map(|k| {
            if modifier(k).is_some() {
                h * 1.3
            } else if k.chars().count() <= 1 {
                h
            } else {
                let tw = font.map_or(0.0, |f| text_width(f, k, h * 0.26));
                (tw + h * 0.5).max(h)
            }
        })
        .collect();
    let total_w = widths.iter().sum::<f32>() + gap * (keys.len().saturating_sub(1)) as f32;
    let (w, ih) = ((total_w + 2.0 * pad).ceil() as usize, (h + 2.0 * pad + shadow).ceil() as usize);
    let mut cv = Canvas { w, h: ih, px: vec![[0.0; 4]; w * ih] };

    // Backdrop behind the row of keys.
    cv.rrect(0.0, 0.0, w as f32, ih as f32, h * 0.24, [0.04, 0.04, 0.045, 0.55], [0.03, 0.03, 0.035, 0.6]);

    let mut x = pad;
    for (k, kw) in keys.iter().zip(&widths) {
        let r = h * 0.17;
        // Drop shadow, rim and face.
        cv.rrect(x, pad + shadow, *kw, h, r, [0.0, 0.0, 0.0, 0.5], [0.0, 0.0, 0.0, 0.5]);
        cv.rrect(x, pad, *kw, h, r, [0.32, 0.32, 0.33, 1.0], [0.15, 0.15, 0.16, 1.0]);
        let rim = (h * 0.018).max(1.0);
        cv.rrect(x + rim, pad + rim, kw - 2.0 * rim, h - 2.0 * rim, r - rim, [0.175, 0.175, 0.18, 1.0], [0.135, 0.135, 0.14, 1.0]);
        if let Some(f) = font {
            let white = [1.0, 1.0, 1.0];
            if let Some((sym, name)) = modifier(k) {
                let left = x + h * 0.14;
                if has_glyph(f, sym) {
                    cv.text(f, sym, h * 0.3, left, pad + h * 0.4, white);
                }
                cv.text(f, name, h * 0.24, left, pad + h * 0.84, white);
            } else {
                let size = if k.chars().count() <= 1 { h * 0.42 } else { h * 0.26 };
                let tw = text_width(f, k, size);
                let sf = f.as_scaled(PxScale::from(size));
                let baseline = pad + h * 0.5 + (sf.ascent() + sf.descent()) * 0.5 - sf.descent() * 0.15;
                cv.text(f, k, size, x + (kw - tw) * 0.5, baseline, white);
            }
        }
        x += kw + gap;
    }

    // Encode premultiplied sRGB bytes (the texture view decodes sRGB).
    let rgba = cv
        .px
        .iter()
        .flat_map(|p| {
            let a = p[3].clamp(0.0, 1.0);
            let enc = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            [enc(p[0]), enc(p[1]), enc(p[2]), enc(a)]
        })
        .collect();
    Image { rgba, width: w as u32, height: ih as u32 }
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_shortcut() {
        let img = super::render(&["Ctrl", "Shift", "M"], 96);
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        assert!(img.width > 300 && img.height > 96);
        // Somewhere on the M key there is white text.
        assert!(img.rgba.chunks(4).any(|p| p[0] > 240 && p[3] > 240));
        if let Ok(out) = std::env::var("KEYCAPS_PNG") {
            image::RgbaImage::from_raw(img.width, img.height, img.rgba).unwrap().save(out).unwrap();
        }
    }
}
