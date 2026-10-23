//! Keycap overlay for keyboard shortcuts, drawn on the CPU like Screen
//! Studio's v4 Mac keycaps: one dark key per pressed key, modifiers with a
//! symbol top-left and their name bottom-left, other keys centred.

use super::raster::{self, Canvas, Image, has_glyph, text_width};
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

fn font() -> Option<&'static FontVec> {
    raster::font("SF Pro Text:semibold,Inter:semibold,sans-serif:semibold")
}

/// (symbol, name) for modifier keys, as printed on PC keyboards: Ctrl and
/// Alt are plain words (symbol ""), Shift has its arrow, and Super uses the
/// Linux/X11 symbol ❖ (drawn by `Canvas::super_symbol`).
fn modifier(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "Ctrl" => ("", "Ctrl"),
        "Alt" => ("", "Alt"),
        "Shift" => ("\u{21E7}", "shift"),
        "Super" => ("\u{2756}", "super"),
        _ => return None,
    })
}

/// Renders the keys for one shortcut, each `key_h` pixels tall, in `style`,
/// with or without the plate behind them.
pub fn render(keys: &[&str], key_h: u32, style: crate::edit::KeycapStyle, backdrop: bool) -> Image {
    use crate::edit::KeycapStyle;
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
    let mut cv = Canvas::new(w, ih);

    // Backdrop behind the row of keys.
    if backdrop {
        cv.rrect(0.0, 0.0, w as f32, ih as f32, h * 0.24, [0.04, 0.04, 0.045, 0.55], [0.03, 0.03, 0.035, 0.6]);
    }
    let ink = if style == KeycapStyle::Light { [0.1, 0.1, 0.11] } else { [1.0, 1.0, 1.0] };

    let mut x = pad;
    for (k, kw) in keys.iter().zip(&widths) {
        let r = h * 0.17;
        let rim = (h * 0.018).max(1.0);
        match style {
            KeycapStyle::Dark => {
                // Drop shadow, rim and face.
                cv.rrect(x, pad + shadow, *kw, h, r, [0.0, 0.0, 0.0, 0.5], [0.0, 0.0, 0.0, 0.5]);
                cv.rrect(x, pad, *kw, h, r, [0.32, 0.32, 0.33, 1.0], [0.15, 0.15, 0.16, 1.0]);
                cv.rrect(x + rim, pad + rim, kw - 2.0 * rim, h - 2.0 * rim, r - rim, [0.175, 0.175, 0.18, 1.0], [0.135, 0.135, 0.14, 1.0]);
            }
            KeycapStyle::Light => {
                cv.rrect(x, pad + shadow, *kw, h, r, [0.0, 0.0, 0.0, 0.35], [0.0, 0.0, 0.0, 0.35]);
                cv.rrect(x, pad, *kw, h, r, [0.86, 0.86, 0.88, 1.0], [0.7, 0.7, 0.73, 1.0]);
                cv.rrect(x + rim, pad + rim, kw - 2.0 * rim, h - 2.0 * rim, r - rim, [0.99, 0.99, 1.0, 1.0], [0.93, 0.93, 0.95, 1.0]);
            }
            KeycapStyle::Outline => {
                let line = (h * 0.035).max(1.5);
                cv.rrect_ring(x, pad, *kw, h, r, line, [1.0, 1.0, 1.0, 0.95]);
            }
        }
        if let Some(f) = font {
            let white = ink;
            if let Some(("", name)) = modifier(k) {
                // Word-only modifier (Ctrl, Alt): centred like a PC keycap.
                let size = h * 0.32;
                let tw = text_width(f, name, size);
                let sf = f.as_scaled(PxScale::from(size));
                let baseline = pad + h * 0.5 + (sf.ascent() + sf.descent()) * 0.5 - sf.descent() * 0.15;
                cv.text(f, name, size, x + (kw - tw) * 0.5, baseline, white);
            } else if let Some((sym, name)) = modifier(k) {
                let left = x + h * 0.14;
                if *k == "Super" {
                    cv.super_symbol(left + h * 0.13, pad + h * 0.3, h * 0.3, white);
                } else if has_glyph(f, sym) {
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

    cv.into_image()
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_shortcut() {
        let img = super::render(&["Ctrl", "Alt", "Super", "Shift", "M"], 96, crate::edit::KeycapStyle::Dark, true);
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        assert!(img.width > 300 && img.height > 96);
        // Somewhere on the M key there is white text.
        assert!(img.rgba.chunks(4).any(|p| p[0] > 240 && p[3] > 240));
        if let Ok(out) = std::env::var("KEYCAPS_PNG") {
            image::RgbaImage::from_raw(img.width, img.height, img.rgba).unwrap().save(out).unwrap();
        }
    }
}
