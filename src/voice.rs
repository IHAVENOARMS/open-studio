//! When someone is speaking, from the microphone track's loudness. Used by
//! the camera's "Hide during silence".

use crate::project::Recording;
use std::io::Read;

/// Silences shorter than this don't hide the camera.
const MIN_SILENCE: f64 = 1.5;

/// Spans of speech in video time, merged across short pauses.
pub fn speech_spans(rec: &Recording) -> Vec<(f64, f64)> {
    let Some(track) = rec.audio.iter().find(|a| a.kind == "microphone") else { return vec![] };
    let Some((rate, channels, samples)) = read_wav(&rec.dir.join(&track.file)) else { return vec![] };
    // RMS per 50 ms window.
    let win = (rate as usize / 20) * channels;
    let rms: Vec<f32> = samples
        .chunks(win.max(1))
        .map(|c| (c.iter().map(|s| (*s as f32 / 32768.0).powi(2)).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    if rms.is_empty() {
        return vec![];
    }
    // Speech is well above the noise floor (the quiet 20th percentile).
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let floor = sorted[sorted.len() / 5];
    let threshold = (floor * 4.0).max(0.012);
    let mut spans: Vec<(f64, f64)> = vec![];
    for (i, v) in rms.iter().enumerate() {
        if *v < threshold {
            continue;
        }
        let (a, b) = (i as f64 * 0.05 + track.offset, (i + 1) as f64 * 0.05 + track.offset);
        match spans.last_mut() {
            Some(last) if a - last.1 < MIN_SILENCE => last.1 = b,
            _ => spans.push((a, b)),
        }
    }
    // Ignore clicks and bumps shorter than 0.15 s.
    spans.retain(|(a, b)| b - a >= 0.15);
    spans
}

/// Visibility 0..1 at `t`: 1 while speaking, fading out over 0.3 s after
/// speech and back in just before it.
pub fn visibility(spans: &[(f64, f64)], t: f64) -> f64 {
    const FADE: f64 = 0.3;
    let d = spans
        .iter()
        .map(|(a, b)| if t < *a { a - t } else if t > *b { t - b } else { 0.0 })
        .fold(f64::INFINITY, f64::min);
    let f = (1.0 - d / FADE).clamp(0.0, 1.0);
    f * f * (3.0 - 2.0 * f)
}

/// 16-bit PCM WAV: (sample rate, channels, interleaved samples).
fn read_wav(path: &std::path::Path) -> Option<(u32, usize, Vec<i16>)> {
    let mut data = vec![];
    std::fs::File::open(path).ok()?.read_to_end(&mut data).ok()?;
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }
    let (mut rate, mut channels, mut bits) = (0u32, 0usize, 0u16);
    let mut i = 12;
    while i + 8 <= data.len() {
        let id = &data[i..i + 4];
        let len = u32::from_le_bytes(data[i + 4..i + 8].try_into().ok()?) as usize;
        let body = &data[i + 8..(i + 8 + len).min(data.len())];
        if id == b"fmt " && body.len() >= 16 {
            channels = u16::from_le_bytes([body[2], body[3]]) as usize;
            rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            if bits != 16 || channels == 0 {
                return None;
            }
            let samples = body.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
            return Some((rate, channels, samples));
        }
        i += 8 + len + (len & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn visibility_fades_around_speech() {
        let spans = [(1.0, 2.0), (5.0, 6.0)];
        assert_eq!(super::visibility(&spans, 1.5), 1.0);
        assert_eq!(super::visibility(&spans, 3.5), 0.0);
        let v = super::visibility(&spans, 2.15);
        assert!(v > 0.0 && v < 1.0);
    }
}
