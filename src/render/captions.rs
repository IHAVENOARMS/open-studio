//! Caption overlay: the current line on a dark rounded box, words already
//! spoken in the caption colour and upcoming ones dimmed (Screen Studio's
//! look).

use super::raster::{self, Canvas, Image, text_width};
use crate::bidi;
use crate::edit::{CaptionDisplay, CaptionSettings};
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

/// Renders `words` with the first `spoken` highlighted, at `font_px` text size.
/// Arabic words use an Arabic font; `rtl` lines run right to left.
pub fn render(words: &[String], rtl: bool, spoken: usize, style: &CaptionSettings, font_px: u32) -> Image {
    let size = font_px as f32;
    let Some(font) = raster::font(style.font.pattern()) else { return Canvas::new(1, 1).into_image() };
    let shown = match style.display {
        CaptionDisplay::LineByLine => words.len(),
        CaptionDisplay::WordByWord => spoken.max(1).min(words.len()),
    };
    let arabic = raster::font(style.font.arabic_pattern());
    // Same em size in both fonts (ab_glyph sizes fonts by line height).
    let em = |f: &FontVec| f.height_unscaled() / f.units_per_em().unwrap_or(1000.0);
    let faces: Vec<(&FontVec, f32)> = words[..shown]
        .iter()
        .map(|w| match arabic {
            Some(a) if bidi::is_rtl(w) => (a, size * em(a) / em(font)),
            _ => (font, size),
        })
        .collect();
    let space = text_width(font, " ", size);
    let widths: Vec<f32> = words[..shown].iter().zip(&faces).map(|(w, (f, s))| text_width(f, w, *s)).collect();
    let text_w = widths.iter().sum::<f32>() + space * shown.saturating_sub(1) as f32;
    let (pad_x, pad_y) = (size * 0.45, size * 0.28);
    let sf = font.as_scaled(PxScale::from(size));
    let line_h = sf.ascent() - sf.descent();
    let (w, h) = ((text_w + 2.0 * pad_x).ceil() as usize, (line_h + 2.0 * pad_y).ceil() as usize);
    let mut cv = Canvas::new(w, h);
    cv.rrect(0.0, 0.0, w as f32, h as f32, size * 0.32, [0.075, 0.05, 0.11, 0.86], [0.065, 0.045, 0.1, 0.86]);
    let baseline = pad_y + sf.ascent();
    let strs: Vec<&str> = words[..shown].iter().map(String::as_str).collect();
    let mut x = pad_x;
    for i in bidi::visual_order(&strs, rtl) {
        let (color, alpha) = if i < spoken { (style.color, 1.0) } else { ([1.0, 1.0, 1.0], 0.5) };
        let (f, s) = faces[i];
        cv.text_alpha(f, strs[i], s, x, baseline, color, alpha);
        x += widths[i] + space;
    }
    cv.into_image()
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_line() {
        let line = std::env::var("CAPTION_WORDS").unwrap_or_else(|_| "Welcome to Open Studio.".into());
        let words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        let rtl = crate::bidi::paragraph_rtl(words.iter().map(String::as_str));
        let img = super::render(&words, rtl, 2, &Default::default(), 48);
        assert!(img.width > 200 && img.height > 48);
        if let Ok(out) = std::env::var("CAPTION_PNG") {
            image::RgbaImage::from_raw(img.width, img.height, img.rgba).unwrap().save(out).unwrap();
        }
    }
}
