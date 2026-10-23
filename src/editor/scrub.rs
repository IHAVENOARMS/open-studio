//! The Scrubber (Screen Studio's waveform-and-magnifier button under the
//! preview): while it's on, dragging the playhead plays the sound under it,
//! forwards or backwards, faster or slower with the drag. Used to find an
//! exact spot by ear.
//!
//! Granular, so speech stays intelligible: short overlapping snippets
//! (Hann-windowed, 50% overlap) are taken from wherever the playhead is and
//! played at their normal pitch; a faster drag skips further between them.
//! Playing exactly what the drag passed over, squeezed into each frame,
//! garbled speech with every jitter of the mouse.

use crate::edit::EditConfig;
use crate::project::Recording;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Mix sample rate: plenty for speech, and light to hold in memory.
const RATE: u32 = 24_000;

pub struct Scrubber {
    /// Mono mix on the video timeline, filled by a background decode.
    pcm: Arc<Mutex<Option<Arc<Vec<i16>>>>>,
    /// Settings the mix was made from; remade when they change.
    key: String,
    src: Option<(gst::Pipeline, gst_app::AppSrc)>,
    /// Playhead and wall time of the previous drag step.
    last: Option<(f64, Instant)>,
    grains: Grains,
}

impl Default for Scrubber {
    fn default() -> Self {
        Self { pcm: Arc::default(), key: String::new(), src: None, last: None, grains: Grains::default() }
    }
}

/// Snippet length: about a syllable's worth, short enough to follow the drag.
const GRAIN: usize = (RATE as usize * 60) / 1000;
const HOP: usize = GRAIN / 2;

/// Overlap-add state carried between drag steps.
#[derive(Default)]
struct Grains {
    /// Sound still to come from snippets already started.
    tail: Vec<f32>,
    /// Output samples owed but not yet a whole hop.
    owed: usize,
    /// Smoothed drag speed (video seconds per second), against mouse jitter.
    speed: f64,
}

impl Grains {
    /// Output for a drag from `from` to `to` over `n` samples of real time.
    fn render(&mut self, pcm: &[i16], from: f64, to: f64, n: usize) -> Vec<i16> {
        if self.tail.len() < GRAIN {
            self.tail.resize(GRAIN, 0.0);
        }
        let dt = n as f64 / RATE as f64;
        self.speed = 0.7 * self.speed + 0.3 * (to - from) / dt.max(1e-3);
        let total = self.owed + n;
        let hops = total / HOP;
        self.owed = total % HOP;
        let mut out = Vec::with_capacity(hops * HOP);
        for k in 0..hops {
            // A held-still playhead goes quiet instead of looping a snippet.
            if self.speed.abs() > 0.08 {
                let at = from + (to - from) * ((k + 1) * HOP) as f64 / (hops * HOP).max(1) as f64;
                let start = at * RATE as f64;
                let dir = if self.speed < 0.0 { -1.0 } else { 1.0 };
                for i in 0..GRAIN {
                    let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / GRAIN as f64).cos();
                    let pos = start + dir * i as f64;
                    let s = if pos < 0.0 { 0 } else { *pcm.get(pos as usize).unwrap_or(&0) };
                    self.tail[i] += (w * s as f64) as f32;
                }
            }
            out.extend(self.tail[..HOP].iter().map(|v| v.clamp(-32768.0, 32767.0) as i16));
            self.tail.drain(..HOP);
            self.tail.resize(GRAIN, 0.0);
        }
        out
    }
}

fn mix_key(edit: &EditConfig) -> String {
    let a = &edit.audio;
    format!("{}|{}|{}|{}", super::audio::AudioPreview::key_for(edit), a.microphone_volume, a.system_volume, a.device_volume)
}

impl Scrubber {
    /// Makes the mix in the background if it's missing or out of date.
    pub fn prepare(&mut self, rec: &Recording, edit: &EditConfig) {
        let key = mix_key(edit);
        if key == self.key {
            return;
        }
        self.key = key;
        *self.pcm.lock().unwrap() = None;
        // Microphone, system and phone audio at their volumes; music and
        // click sounds aren't what you scrub for.
        let tracks: Vec<_> = crate::export::audio_tracks(rec, edit, None).into_iter().filter(|t| !t.looped).collect();
        let len = (rec.duration * RATE as f64) as usize + RATE as usize;
        let out = self.pcm.clone();
        std::thread::spawn(move || {
            let mut mix = vec![0f32; len];
            for t in &tracks {
                let Ok(mut child) = Command::new("ffmpeg")
                    .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
                    .arg(&t.path)
                    .args(["-ac", "1", "-ar", &RATE.to_string(), "-f", "s16le", "-"])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                else {
                    continue;
                };
                let mut bytes = vec![];
                let _ = child.stdout.take().unwrap().read_to_end(&mut bytes);
                let _ = child.wait();
                let start = (t.offset * RATE as f64).round() as i64;
                for (i, s) in bytes.chunks_exact(2).enumerate() {
                    let j = start + i as i64;
                    if j >= 0 && (j as usize) < mix.len() {
                        mix[j as usize] += i16::from_le_bytes([s[0], s[1]]) as f32 * t.gain as f32;
                    }
                }
            }
            let pcm: Vec<i16> = mix.iter().map(|v| v.clamp(-32768.0, 32767.0) as i16).collect();
            *out.lock().unwrap() = Some(Arc::new(pcm));
        });
    }

    fn output(&mut self) -> Option<&gst_app::AppSrc> {
        if self.src.is_none() {
            let _ = gst::init();
            let desc = format!(
                "appsrc name=src is-live=true format=time do-timestamp=true \
                 caps=audio/x-raw,format=S16LE,rate={RATE},channels=1,layout=interleaved \
                 ! audioconvert ! audioresample ! autoaudiosink sync=false"
            );
            let pipeline = gst::parse::launch(&desc).ok()?.downcast::<gst::Pipeline>().ok()?;
            let src = pipeline.by_name("src")?.downcast::<gst_app::AppSrc>().ok()?;
            pipeline.set_state(gst::State::Playing).ok()?;
            self.src = Some((pipeline, src));
        }
        self.src.as_ref().map(|(_, s)| s)
    }

    /// The playhead was dragged to `t`: plays what it passed over since the
    /// previous step, squeezed or stretched into the time that took.
    pub fn scrub(&mut self, t: f64) {
        let now = Instant::now();
        let prev = self.last.replace((t, now));
        let Some(pcm) = self.pcm.lock().unwrap().clone() else { return };
        let Some((from, at)) = prev else { return };
        let dt = now.duration_since(at).as_secs_f64().clamp(0.004, 0.06);
        let samples = self.grains.render(&pcm, from, t, (dt * RATE as f64) as usize);
        if samples.is_empty() {
            return;
        }
        let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        if let Some(src) = self.output() {
            let _ = src.push_buffer(gst::Buffer::from_mut_slice(bytes));
        }
    }

    /// The drag ended (or the Scrubber is off).
    pub fn end(&mut self) {
        self.last = None;
        self.grains = Grains::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_follow_the_drag_at_normal_pitch() {
        // A ramp: sample value = position in hundredths of a second.
        let pcm: Vec<i16> = (0..RATE * 4).map(|i| (i / (RATE / 100)) as i16).collect();
        let mut g = Grains::default();
        // Drag forwards at 1x for a while: steady output from where the
        // playhead is, rising like the recording.
        let mut t = 1.0;
        let mut out = vec![];
        for _ in 0..30 {
            out.extend(g.render(&pcm, t, t + 0.016, (0.016 * RATE as f64) as usize));
            t += 0.016;
        }
        let late = &out[out.len() - HOP..];
        let mean = late.iter().map(|v| *v as f64).sum::<f64>() / late.len() as f64;
        assert!((mean / 100.0 - t).abs() < 0.15, "playing {:.2}s, playhead at {t:.2}s", mean / 100.0);
        // Holding still: it goes quiet.
        for _ in 0..30 {
            out = g.render(&pcm, t, t, (0.016 * RATE as f64) as usize);
        }
        assert!(out.iter().all(|v| *v == 0));
    }
}

impl Drop for Scrubber {
    fn drop(&mut self) {
        if let Some((p, _)) = &self.src {
            let _ = p.set_state(gst::State::Null);
        }
    }
}
