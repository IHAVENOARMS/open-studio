//! Small CPU rasterizer for text overlays (keycaps, captions).

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub struct Image {
    /// Premultiplied RGBA.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// First installed font of a comma-separated list of fontconfig patterns;
/// the last pattern accepts whatever fontconfig substitutes.
pub fn font(patterns: &str) -> Option<&'static FontVec> {
    static FONTS: OnceLock<Mutex<HashMap<String, Option<&'static FontVec>>>> = OnceLock::new();
    let mut map = FONTS.get_or_init(Default::default).lock().unwrap();
    *map.entry(patterns.to_string()).or_insert_with(|| {
        let list: Vec<&str> = patterns.split(',').collect();
        for (i, pattern) in list.iter().enumerate() {
            let out = std::process::Command::new("fc-match").args(["-f", "%{family}|%{file}", pattern]).output().ok()?;
            let line = String::from_utf8_lossy(&out.stdout).to_string();
            let (family, file) = line.split_once('|')?;
            let wanted = pattern.split(':').next().unwrap_or("").to_lowercase();
            let generic = matches!(wanted.as_str(), "sans-serif" | "serif" | "monospace");
            if i + 1 < list.len() && !generic && !family.to_lowercase().contains(&wanted) {
                continue;
            }
            if let Ok(f) = std::fs::read(file).map_err(|_| ()).and_then(|d| FontVec::try_from_vec(d).map_err(|_| ())) {
                return Some(&*Box::leak(Box::new(f)));
            }
        }
        None
    })
}

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    /// Premultiplied RGBA in 0..1.
    px: Vec<[f32; 4]>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![[0.0; 4]; w * h] }
    }

    pub fn blend(&mut self, x: usize, y: usize, c: [f32; 3], a: f32) {
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
    #[allow(clippy::too_many_arguments)]
    pub fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, top: [f32; 4], bottom: [f32; 4]) {
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

    /// Filled convex or concave polygon, 4x4 supersampled.
    pub fn polygon(&mut self, pts: &[(f32, f32)], c: [f32; 3], alpha: f32) {
        let (x0, x1) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (y0, y1) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        let inside = |x: f32, y: f32| {
            let mut c = false;
            let mut j = pts.len() - 1;
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[j]);
                if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
                    c = !c;
                }
                j = i;
            }
            c
        };
        for py in y0.floor().max(0.0) as usize..(y1.ceil() as usize).min(self.h) {
            for px in x0.floor().max(0.0) as usize..(x1.ceil() as usize).min(self.w) {
                let mut n = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(px as f32 + (sx as f32 + 0.5) / 4.0, py as f32 + (sy as f32 + 0.5) / 4.0) {
                            n += 1;
                        }
                    }
                }
                if n > 0 {
                    self.blend(px, py, c, alpha * n as f32 / 16.0);
                }
            }
        }
    }

    /// The Super key symbol (❖): four diamonds around a centre, drawn by hand
    /// because few fonts have the glyph.
    pub fn super_symbol(&mut self, cx: f32, cy: f32, size: f32, c: [f32; 3]) {
        let (h, d) = (size * 0.25, size * 0.27);
        for (dx, dy) in [(0.0, -d), (d, 0.0), (0.0, d), (-d, 0.0)] {
            let (x, y) = (cx + dx, cy + dy);
            self.polygon(&[(x, y - h), (x + h, y), (x, y + h), (x - h, y)], c, 1.0);
        }
    }

    pub fn text(&mut self, font: &FontVec, s: &str, size: f32, x: f32, baseline: f32, color: [f32; 3]) {
        self.text_alpha(font, s, size, x, baseline, color, 1.0);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text_alpha(&mut self, font: &FontVec, s: &str, size: f32, x: f32, baseline: f32, color: [f32; 3], alpha: f32) {
        for (id, gx, gy) in shape(font, s, size).0 {
            let g = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(x + gx, baseline + gy));
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| {
                    let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px >= 0 && py >= 0 {
                        self.blend(px as usize, py as usize, color, cov.min(1.0) * alpha);
                    }
                });
            }
        }
    }

    /// Premultiplied sRGB bytes (the texture view decodes sRGB).
    pub fn into_image(self) -> Image {
        let rgba = self
            .px
            .iter()
            .flat_map(|p| {
                let enc = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                [enc(p[0]), enc(p[1]), enc(p[2]), enc(p[3])]
            })
            .collect();
        Image { rgba, width: self.w as u32, height: self.h as u32 }
    }
}

pub fn text_width(font: &FontVec, s: &str, size: f32) -> f32 {
    shape(font, s, size).1
}

/// Shapes `s` with harfrust (Arabic letters joined and set right to left,
/// ligatures, kerning): glyphs in drawing order with their offsets from the
/// pen start (y down), and the total advance.
fn shape(font: &FontVec, s: &str, size: f32) -> (Vec<(GlyphId, f32, f32)>, f32) {
    static DATA: OnceLock<Mutex<HashMap<usize, &'static harfrust::ShaperData>>> = OnceLock::new();
    let sf = font.as_scaled(PxScale::from(size));
    let Ok(font_ref) = harfrust::FontRef::from_index(font.as_slice(), 0) else {
        // Unparseable for the shaper: one glyph per character.
        let mut pen = 0.0;
        let glyphs = s
            .chars()
            .map(|c| {
                let id = font.glyph_id(c);
                let at = pen;
                pen += sf.h_advance(id);
                (id, at, 0.0)
            })
            .collect();
        return (glyphs, pen);
    };
    // Fonts come from `font()` and live forever, so the address is a stable key.
    let data = *DATA
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(font as *const FontVec as usize)
        .or_insert_with(|| Box::leak(Box::new(harfrust::ShaperData::new(&font_ref))));
    let shaper = data.shaper(&font_ref).build();
    let mut buffer = harfrust::UnicodeBuffer::new();
    buffer.push_str(s);
    buffer.guess_segment_properties();
    let out = shaper.shape(buffer, harfrust::ShapeOptions::new());
    let k = sf.h_scale_factor();
    let mut pen = 0.0;
    let glyphs = out
        .glyph_infos()
        .iter()
        .zip(out.glyph_positions())
        .map(|(info, pos)| {
            let g = (GlyphId(info.glyph_id as u16), pen + pos.x_offset as f32 * k, -pos.y_offset as f32 * k);
            pen += pos.x_advance as f32 * k;
            g
        })
        .collect();
    (glyphs, pen)
}

pub fn has_glyph(font: &FontVec, s: &str) -> bool {
    s.chars().all(|c| font.glyph_id(c).0 != 0)
}
