//! Mouse click sounds, synthesised (no bundled samples): a soft, low tap
//! ("Magic Mouse") and a sharper two-part mechanical click ("MX Master").

use crate::edit::{ClickSound, EditConfig};
use crate::project::Recording;
use std::io::Write;
use std::path::{Path, PathBuf};

const RATE: u32 = 48_000;

/// Mono samples of one click.
pub fn samples(sound: ClickSound) -> Vec<f32> {
    let n = (RATE as f32 * 0.06) as usize;
    let mut seed = 0x1234_5678u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let mut out = vec![0f32; n];
    let mut lp = 0.0f32;
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / RATE as f32;
        *o = match sound {
            ClickSound::None => 0.0,
            ClickSound::MagicMouse => {
                // Low-passed noise burst plus a short low thump.
                lp += (noise() - lp) * 0.18;
                let env = (-t * 140.0).exp();
                (lp * 1.6 + (2.0 * std::f32::consts::PI * 180.0 * t).sin() * 0.5) * env
            }
            ClickSound::MxMaster => {
                // Press and release transients ~12ms apart, brighter.
                lp += (noise() - lp) * 0.55;
                let a = (-t * 400.0).exp();
                let b = if t > 0.012 { (-(t - 0.012) * 500.0).exp() * 0.6 } else { 0.0 };
                (lp + (2.0 * std::f32::consts::PI * 2400.0 * t).sin() * 0.25) * (a + b)
            }
        } * 0.8;
    }
    out
}

fn write_wav(path: &Path, mono: &[f32]) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data = mono.len() as u32 * 4;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&RATE.to_le_bytes())?;
    f.write_all(&(RATE * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data.to_le_bytes())?;
    for s in mono {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        f.write_all(&v.to_le_bytes())?;
        f.write_all(&v.to_le_bytes())?;
    }
    f.flush()
}

/// Plays one click (sound picker preview).
pub fn preview(sound: ClickSound, volume: f64) {
    let s: Vec<f32> = samples(sound).into_iter().map(|v| v * volume as f32).collect();
    std::thread::spawn(move || {
        let path = std::env::temp_dir().join("open-studio-click-preview.wav");
        if write_wav(&path, &s).is_ok() {
            let _ = std::process::Command::new("pw-play").arg(&path).status();
        }
    });
}

/// Renders a track with a click at every mouse-down, on the video timeline.
/// Returns its path (in the project directory), or None when clicks are off.
pub fn render_track(rec: &Recording, edit: &EditConfig) -> Option<PathBuf> {
    if edit.cursor.click_sound == ClickSound::None || edit.cursor.hidden {
        return None;
    }
    let click = samples(edit.cursor.click_sound);
    let n = (rec.duration * RATE as f64) as usize + click.len();
    let mut track = vec![0f32; n];
    for c in rec.events.clicks.iter().filter(|c| c.down && c.t >= 0.0) {
        let at = (c.t * RATE as f64) as usize;
        for (i, s) in click.iter().enumerate() {
            if let Some(o) = track.get_mut(at + i) {
                *o += s;
            }
        }
    }
    let path = rec.dir.join("clicks.wav");
    write_wav(&path, &track).ok()?;
    Some(path)
}
