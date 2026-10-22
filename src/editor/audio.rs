//! Audio in the editor: preview playback (one GStreamer pipeline per track,
//! kept in step with the video clock) and the waveform envelope shown on the
//! clip track.

use crate::project::Recording;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Envelope resolution: values per second.
pub const ENVELOPE_RATE: f64 = 100.0;

struct Track {
    pipeline: gst::Pipeline,
    volume: gst::Element,
    offset: f64,
    gain: f64,
    kind: &'static str,
}

/// Preview playback of the export mix (without the microphone filters).
pub struct AudioPreview {
    tracks: Vec<Track>,
    /// Settings the tracks were built from; rebuilt when these change.
    key: String,
}

impl AudioPreview {
    pub fn new(rec: &Recording, edit: &crate::edit::EditConfig) -> Self {
        let _ = gst::init();
        let clicks = crate::clicks::render_track(rec, edit);
        let mix = crate::export::audio_tracks(rec, edit, clicks.as_deref());
        let tracks = mix
            .iter()
            .filter_map(|t| {
                let desc = format!(
                    // scaletempo keeps voices at their pitch on sped-up slices.
                    "filesrc location=\"{}\" ! decodebin ! audioconvert ! scaletempo ! audioconvert ! audioresample ! volume name=v volume={} ! autoaudiosink",
                    t.path.display(),
                    t.gain
                );
                let pipeline = gst::parse::launch(&desc).ok()?.downcast::<gst::Pipeline>().ok()?;
                let volume = pipeline.by_name("v")?;
                pipeline.set_state(gst::State::Paused).ok()?;
                Some(Track { pipeline, volume, offset: t.offset, gain: t.gain, kind: t.kind })
            })
            .collect();
        Self { tracks, key: Self::key_for(edit) }
    }

    /// What, besides volume, changes the set of tracks.
    pub fn key_for(edit: &crate::edit::EditConfig) -> String {
        let a = &edit.audio;
        format!("{:?}{}{}{:?}{}{}{:?}{}", a.music, a.mute_music, a.mute_microphone, a.mic_mode, a.mute_system, a.mute_device, edit.cursor.click_sound, edit.cursor.hidden)
    }

    pub fn is_current(&self, edit: &crate::edit::EditConfig) -> bool {
        self.key == Self::key_for(edit)
    }

    /// Starts playing from video time `t` with the settings of the slice there.
    pub fn play(&self, t: f64, slice: &crate::edit::SliceProps) {
        for tr in &self.tracks {
            let pos = (t - tr.offset).max(0.0);
            let _ = tr.pipeline.seek(
                slice.speed,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                gst::ClockTime::from_nseconds((pos * 1e9) as u64),
                gst::SeekType::None,
                gst::ClockTime::NONE,
            );
            let k = match tr.kind {
                "microphone" => slice.volume,
                "system" => slice.system_volume,
                "device" => slice.device_volume,
                _ => 1.0,
            };
            tr.volume.set_property("volume", tr.gain * k);
            let _ = tr.pipeline.set_state(gst::State::Playing);
        }
    }

    pub fn pause(&self) {
        for tr in &self.tracks {
            let _ = tr.pipeline.set_state(gst::State::Paused);
        }
    }

    pub fn set_muted(&self, mute: bool) {
        for tr in &self.tracks {
            let _ = tr.volume.set_property("mute", mute);
        }
    }
}

impl Drop for AudioPreview {
    fn drop(&mut self) {
        for tr in &self.tracks {
            let _ = tr.pipeline.set_state(gst::State::Null);
        }
    }
}

/// Peak envelope of all tracks mixed, on the video timeline, normalised to
/// 0..1 (`ENVELOPE_RATE` values per second).
pub fn envelope(rec: &Recording) -> Vec<f32> {
    let n = (rec.duration * ENVELOPE_RATE).ceil() as usize + 1;
    let mut env = vec![0f32; n];
    for t in &rec.audio {
        let Some(samples) = read_wav_mono(&rec.dir.join(&t.file)) else { continue };
        let per = (crate::record::audio::RATE as f64 / ENVELOPE_RATE) as usize;
        for (i, chunk) in samples.chunks(per).enumerate() {
            let peak = chunk.iter().fold(0f32, |m, s| m.max(s.abs()));
            let vt = t.offset + i as f64 / ENVELOPE_RATE;
            let k = (vt * ENVELOPE_RATE).round();
            if k >= 0.0 && (k as usize) < n {
                env[k as usize] = env[k as usize].max(peak);
            }
        }
    }
    // Perceptual scaling: quiet speech still shows.
    let max = env.iter().cloned().fold(1e-4f32, f32::max);
    env.iter().map(|v| (v / max).powf(0.6)).collect()
}

/// 48 kHz s16 stereo WAV (as recorded) as mono f32.
fn read_wav_mono(path: &Path) -> Option<Vec<f32>> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut bytes = vec![];
    f.read_to_end(&mut bytes).ok()?;
    let data = bytes.get(44..)?;
    Some(
        data.chunks_exact(4)
            .map(|c| {
                let l = i16::from_le_bytes([c[0], c[1]]) as f32;
                let r = i16::from_le_bytes([c[2], c[3]]) as f32;
                (l + r) / 65536.0
            })
            .collect(),
    )
}

/// Computes the envelope on a worker thread.
pub fn envelope_async(rec: &Recording, ctx: egui::Context) -> Arc<Mutex<Option<Vec<f32>>>> {
    let slot = Arc::new(Mutex::new(None));
    if rec.audio.is_empty() {
        return slot;
    }
    let (s, r) = (slot.clone(), rec.clone());
    std::thread::spawn(move || {
        *s.lock().unwrap() = Some(envelope(&r));
        ctx.request_repaint();
    });
    slot
}
