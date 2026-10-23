//! The Scrubber (Screen Studio's waveform-and-magnifier button under the
//! preview): while it's on, dragging the playhead plays the sound it passes
//! over, forwards or backwards, faster or slower with the drag, like
//! scrubbing tape. Used to find an exact spot by ear.

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
}

impl Default for Scrubber {
    fn default() -> Self {
        Self { pcm: Arc::default(), key: String::new(), src: None, last: None }
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
        // A pause in the drag starts a fresh stretch (with a fade-in).
        let gap = now.duration_since(at).as_secs_f64();
        let fresh = gap > 0.12;
        let dt = gap.clamp(0.008, 0.06);
        if (t - from).abs() < 1e-4 {
            return;
        }
        let samples = stretch(&pcm, from, t, (dt * RATE as f64) as usize, fresh);
        let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        if let Some(src) = self.output() {
            let _ = src.push_buffer(gst::Buffer::from_mut_slice(bytes));
        }
    }

    /// The drag ended (or the Scrubber is off).
    pub fn end(&mut self) {
        self.last = None;
    }
}

/// `n` samples covering the mix from `from` to `to` seconds (backwards when
/// `to < from`), so the sound speeds up, slows down or reverses with the
/// drag. `fresh` fades the start in to avoid a click.
fn stretch(pcm: &[i16], from: f64, to: f64, n: usize, fresh: bool) -> Vec<i16> {
    let fade = (RATE as usize / 200).min(n / 2).max(1); // 5 ms
    (0..n)
        .map(|i| {
            let pos = ((from + (to - from) * i as f64 / n as f64) * RATE as f64).max(0.0);
            let k = pos.floor() as usize;
            let f = (pos - pos.floor()) as f32;
            let a = *pcm.get(k).unwrap_or(&0) as f32;
            let b = *pcm.get(k + 1).unwrap_or(&0) as f32;
            let mut v = a + (b - a) * f;
            if fresh && i < fade {
                v *= i as f32 / fade as f32;
            }
            v as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn plays_what_the_drag_passes_over() {
        // A ramp: sample value = position in hundredths of a second.
        let pcm: Vec<i16> = (0..super::RATE * 2).map(|i| (i / (super::RATE / 100)) as i16).collect();
        // Dragging 0.50 s -> 0.60 s in 20 ms: reads forwards through 0.5-0.6 s.
        let fwd = super::stretch(&pcm, 0.5, 0.6, 480, false);
        assert_eq!((fwd[0], *fwd.last().unwrap()), (50, 59));
        assert!(fwd.windows(2).all(|w| w[1] >= w[0]));
        // Dragging back reads backwards.
        let back = super::stretch(&pcm, 0.6, 0.5, 480, false);
        assert!(back.windows(2).all(|w| w[1] <= w[0]) && back[0] == 60);
        // A fresh stretch fades in from silence.
        assert_eq!(super::stretch(&pcm, 0.5, 0.6, 480, true)[0], 0);
    }
}

impl Drop for Scrubber {
    fn drop(&mut self) {
        if let Some((p, _)) = &self.src {
            let _ = p.set_state(gst::State::Null);
        }
    }
}
