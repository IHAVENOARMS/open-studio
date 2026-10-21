//! Small CPU rasterizer for text overlays (keycaps, captions).

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
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

    pub fn text(&mut self, font: &FontVec, s: &str, size: f32, x: f32, baseline: f32, color: [f32; 3]) {
        self.text_alpha(font, s, size, x, baseline, color, 1.0);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text_alpha(&mut self, font: &FontVec, s: &str, size: f32, x: f32, baseline: f32, color: [f32; 3], alpha: f32) {
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
                        self.blend(px as usize, py as usize, color, cov.min(1.0) * alpha);
                    }
                });
            }
            pen += sf.h_advance(id);
            prev = Some(id);
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
    let sf = font.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for c in s.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

pub fn has_glyph(font: &FontVec, s: &str) -> bool {
    s.chars().all(|c| font.glyph_id(c).0 != 0)
}
