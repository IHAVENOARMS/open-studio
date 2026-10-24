//! Audio capture (microphone or system audio) through PipeWire's Pulse
//! interface, written to 48 kHz stereo 16-bit WAV. Timestamps share the
//! monotonic clock with video, and paused stretches are dropped.

use super::clock::RecClock;
use anyhow::{Context, Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use serde::Deserialize;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

#[derive(Clone, Debug)]
pub struct Device {
    /// Pulse source name.
    pub name: String,
    pub description: String,
    /// A sink monitor (system audio) rather than an input.
    pub monitor: bool,
}

#[derive(Deserialize)]
struct PaSource {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    monitor_of_sink: Option<String>,
}

/// Input devices (microphones) and output monitors.
pub fn list_devices() -> Vec<Device> {
    let Ok(out) = std::process::Command::new("pactl").args(["-f", "json", "list", "sources"]).output() else { return vec![] };
    let list: Vec<PaSource> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    list.into_iter()
        .map(|s| Device {
            monitor: s.monitor_of_sink.as_deref().is_some_and(|m| m != "n/a") || s.name.ends_with(".monitor"),
            description: if s.description.is_empty() { s.name.clone() } else { s.description },
            name: s.name,
        })
        .collect()
}

/// Pulse name for "whatever is playing": the default sink's monitor.
pub const SYSTEM_AUDIO: &str = "@DEFAULT_MONITOR@";
pub const DEFAULT_MIC: &str = "@DEFAULT_SOURCE@";
/// Device names starting with this record one app's playback stream (by
/// PipeWire node name) instead of a Pulse source.
pub const STREAM_PREFIX: &str = "stream:";

/// Live input level of a microphone, for the bar under the microphone
/// icon (Screen Studio's sign that the microphone is picking up sound).
pub struct MicMeter {
    pipeline: gst::Pipeline,
    /// Peak level 0..1000 (see `meter_fraction`).
    level: Arc<std::sync::atomic::AtomicU32>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

/// Bar length (0..1) for a peak level in dB: silence below -60 dB, full at
/// 0 dB.
pub fn meter_fraction(db: f64) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    ((db + 60.0) / 60.0).clamp(0.0, 1.0) as f32
}

impl MicMeter {
    pub fn start(device: &str) -> Option<Self> {
        Self::start_from(&format!("pulsesrc device=\"{device}\" client-name=\"Open Studio\""))
    }

    /// Meters what `src` (a GStreamer source description) produces.
    fn start_from(src: &str) -> Option<Self> {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        gst::init().ok()?;
        let desc = format!("{src} ! level interval=50000000 post-messages=true ! fakesink sync=false");
        let pipeline = gst::parse::launch(&desc).ok()?.downcast::<gst::Pipeline>().ok()?;
        let bus = pipeline.bus()?;
        pipeline.set_state(gst::State::Playing).ok()?;
        let level = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (l, st) = (level.clone(), stop.clone());
        std::thread::spawn(move || {
            while !st.load(Ordering::Relaxed) {
                let Some(msg) = bus.timed_pop_filtered(gst::ClockTime::from_mseconds(100), &[gst::MessageType::Element, gst::MessageType::Error]) else {
                    continue;
                };
                if let gst::MessageView::Error(_) = msg.view() {
                    l.store(0, Ordering::Relaxed);
                    return;
                }
                let Some(s) = msg.structure().filter(|s| s.name() == "level") else { continue };
                let Ok(peaks) = s.get::<gst::glib::ValueArray>("peak") else { continue };
                let db = peaks.iter().filter_map(|v| v.get::<f64>().ok()).fold(f64::NEG_INFINITY, f64::max);
                l.store((meter_fraction(db) * 1000.0) as u32, Ordering::Relaxed);
            }
        });
        Some(Self { pipeline, level, stop })
    }

    /// Current level, 0..1.
    pub fn level(&self) -> f32 {
        self.level.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0
    }
}

impl Drop for MicMeter {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// True when a playback stream with this node name exists right now.
fn stream_exists(name: &str) -> bool {
    std::process::Command::new("pactl")
        .args(["list", "sink-inputs"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == format!("node.name = \"{name}\"")))
}

struct Wav {
    file: std::io::BufWriter<std::fs::File>,
    frames: u64,
    /// Recording-domain time of the first written sample.
    first_ns: Option<i64>,
    error: Option<String>,
}

pub struct AudioCapture {
    pipeline: gst::Pipeline,
    wav: Arc<Mutex<Wav>>,
    path: PathBuf,
    _monitor: Option<MonitorLevel>,
}

/// While recording system audio, the speakers' monitor at full volume.
/// A monitor turned down (seen at 8%, -66 dB) records music as near
/// silence; the previous level is put back when the recording ends.
struct MonitorLevel {
    source: String,
    /// Raw per-channel volumes to restore.
    previous: Vec<String>,
}

impl MonitorLevel {
    fn raise() -> Option<Self> {
        let pactl = |args: &[&str]| -> Option<String> {
            let o = std::process::Command::new("pactl").args(args).output().ok()?;
            o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        let source = format!("{}.monitor", pactl(&["get-default-sink"])?);
        // "Volume: front-left: 5140 /   8% / -66.33 dB,   front-right: 5140 / ..."
        let text = pactl(&["get-source-volume", &source])?;
        let previous: Vec<String> = text.split(',').filter_map(|ch| ch.split(':').nth(ch.split(':').count() - 1)?.split('/').next().map(|v| v.trim().to_string())).filter(|v| v.parse::<u32>().is_ok()).collect();
        let full = 65536;
        if previous.is_empty() || previous.iter().all(|v| v.parse::<u32>().unwrap_or(full) >= full) {
            return None;
        }
        pactl(&["set-source-volume", &source, "100%"])?;
        eprintln!("open-studio: raised {source} from {} to 100% while recording system audio", previous.join("/"));
        Some(Self { source, previous })
    }
}

impl Drop for MonitorLevel {
    fn drop(&mut self) {
        let mut args = vec!["set-source-volume".to_string(), self.source.clone()];
        args.extend(self.previous.iter().cloned());
        let _ = std::process::Command::new("pactl").args(&args).status();
    }
}

pub struct AudioResult {
    /// First sample time in the same domain the recorder uses for video.
    pub first_ns: i64,
    pub frames: u64,
}

fn write_header(f: &mut impl Write, frames: u64) -> std::io::Result<()> {
    let data = frames * CHANNELS as u64 * 2;
    f.write_all(b"RIFF")?;
    f.write_all(&((36 + data).min(u32::MAX as u64) as u32).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&CHANNELS.to_le_bytes())?;
    f.write_all(&RATE.to_le_bytes())?;
    f.write_all(&(RATE * CHANNELS as u32 * 2).to_le_bytes())?;
    f.write_all(&(CHANNELS * 2).to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&(data.min(u32::MAX as u64) as u32).to_le_bytes())
}

impl AudioCapture {
    /// `use_clock`: map timestamps through the pausable recording clock
    /// (direct capture); otherwise raw monotonic time (portal capture).
    pub fn start(device: &str, out: &Path, clock: Arc<RecClock>, use_clock: bool) -> Result<Self> {
        gst::init()?;
        let src = match device.strip_prefix(STREAM_PREFIX) {
            // Tap one app's playback (PipeWire). Without the stream PipeWire
            // would connect the default source, the microphone, instead.
            Some(name) => {
                if !stream_exists(name) {
                    anyhow::bail!("its audio isn't playing (no \"{name}\" stream)");
                }
                format!("pipewiresrc target-object=\"{name}\" do-timestamp=true client-name=\"Open Studio\"")
            }
            None => format!("pulsesrc device=\"{device}\" do-timestamp=true client-name=\"Open Studio\""),
        };
        let monitor = if device == SYSTEM_AUDIO { MonitorLevel::raise() } else { None };
        let desc = format!(
            "{src} ! audioconvert ! audioresample \
             ! audio/x-raw,format=S16LE,rate={RATE},channels={CHANNELS},layout=interleaved ! appsink name=sink sync=false"
        );
        let pipeline = gst::parse::launch(&desc)?.downcast::<gst::Pipeline>().map_err(|_| anyhow!("not a pipeline"))?;
        let sysclock = gst::SystemClock::obtain();
        sysclock.set_property("clock-type", gst::ClockType::Monotonic);
        pipeline.use_clock(Some(&sysclock));
        let mut file = std::io::BufWriter::new(std::fs::File::create(out).with_context(|| format!("creating {}", out.display()))?);
        write_header(&mut file, 0)?;
        let wav = Arc::new(Mutex::new(Wav { file, frames: 0, first_ns: None, error: None }));
        let sink = pipeline.by_name("sink").unwrap().downcast::<gst_app::AppSink>().map_err(|_| anyhow!("appsink"))?;
        let (w, pw) = (wav.clone(), pipeline.downgrade());
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let Some(buf) = sample.buffer() else { return Ok(gst::FlowSuccess::Ok) };
                    let base = pw.upgrade().and_then(|p| p.base_time()).map(|t| t.nseconds() as i64).unwrap_or(0);
                    let mono = base + buf.pts().map(|t| t.nseconds() as i64).unwrap_or(0);
                    let t = if use_clock { clock.map(mono) } else { Some(mono) };
                    let Some(t) = t else { return Ok(gst::FlowSuccess::Ok) }; // paused
                    let Ok(map) = buf.map_readable() else { return Ok(gst::FlowSuccess::Ok) };
                    let mut w = w.lock().unwrap();
                    w.first_ns.get_or_insert(t);
                    if let Err(e) = w.file.write_all(&map) {
                        w.error = Some(e.to_string());
                        return Err(gst::FlowError::Error);
                    }
                    w.frames += map.len() as u64 / (CHANNELS as u64 * 2);
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        pipeline.set_state(gst::State::Playing).context("starting audio capture")?;
        Ok(Self { pipeline, wav, path: out.to_path_buf(), _monitor: monitor })
    }

    pub fn stop(self) -> Result<Option<AudioResult>> {
        self.pipeline.send_event(gst::event::Eos::new());
        if let Some(bus) = self.pipeline.bus() {
            bus.timed_pop_filtered(gst::ClockTime::from_mseconds(500), &[gst::MessageType::Eos, gst::MessageType::Error]);
        }
        let _ = self.pipeline.set_state(gst::State::Null);
        let mut w = self.wav.lock().unwrap();
        w.file.flush()?;
        let frames = w.frames;
        let f = w.file.get_mut();
        f.seek(SeekFrom::Start(0))?;
        write_header(f, frames)?;
        f.flush()?;
        if let Some(e) = &w.error {
            return Err(anyhow!("audio capture failed: {e}"));
        }
        let _ = &self.path;
        Ok(w.first_ns.map(|first_ns| AudioResult { first_ns, frames }))
    }
}

#[cfg(test)]
mod meter_tests {
    use super::*;

    #[test]
    fn meter_follows_the_level() {
        assert_eq!(meter_fraction(f64::NEG_INFINITY), 0.0);
        assert_eq!(meter_fraction(-80.0), 0.0);
        assert_eq!(meter_fraction(-30.0), 0.5);
        assert_eq!(meter_fraction(3.0), 1.0);
        // A half-scale tone reads about -6 dB; silence reads nothing.
        let tone = MicMeter::start_from("audiotestsrc is-live=true wave=sine volume=0.5").unwrap();
        let quiet = MicMeter::start_from("audiotestsrc is-live=true wave=silence").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert!((tone.level() - 0.9).abs() < 0.05, "tone level {}", tone.level());
        assert_eq!(quiet.level(), 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records a single app's playback by node name. Needs PipeWire and
    /// pw-play, so it's opt-in: `cargo test -- --ignored stream_tap`.
    #[test]
    #[ignore]
    fn stream_tap_records_only_that_app() {
        let dir = std::env::temp_dir().join("open-studio-tap-test");
        std::fs::create_dir_all(&dir).unwrap();
        let tone = dir.join("tone.wav");
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:d=4", "-af", "volume=0.01"])
            .arg(&tone)
            .status()
            .unwrap()
            .success());
        let name = "Open Studio tap test";
        let mut play = std::process::Command::new("pw-play")
            .args(["-P", &format!("{{ node.name = \"{name}\" }}")])
            .arg(&tone)
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(800));
        let out = dir.join("tap.wav");
        let cap = AudioCapture::start(&format!("{STREAM_PREFIX}{name}"), &out, Arc::new(RecClock::new()), false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let res = cap.stop().unwrap().unwrap();
        let _ = play.wait();
        assert!(res.frames > RATE as u64, "frames {}", res.frames);
        let data = std::fs::read(&out).unwrap();
        let peak = data[44..].chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs()).max().unwrap();
        // 0.01 of a 1/8-amplitude sine is about 41 of 32767.
        assert!(peak > 20 && peak < 200, "peak {peak}");
        // A missing stream is refused rather than recording the microphone.
        assert!(AudioCapture::start(&format!("{STREAM_PREFIX}no such stream"), &dir.join("x.wav"), Arc::new(RecClock::new()), false).is_err());
    }
}

#[cfg(test)]
mod system_tests {
    use super::*;

    /// System audio is recorded at full level even when the monitor is
    /// turned down, and the monitor's level is put back afterwards.
    /// Opt-in (needs a running sound server): `cargo test -- --ignored system_audio`.
    #[test]
    #[ignore]
    fn system_audio_full_level_and_restored() {
        let monitor = || {
            let sink = std::process::Command::new("pactl").arg("get-default-sink").output().unwrap();
            let src = format!("{}.monitor", String::from_utf8_lossy(&sink.stdout).trim());
            let v = std::process::Command::new("pactl").args(["get-source-volume", &src]).output().unwrap();
            String::from_utf8_lossy(&v.stdout).lines().next().unwrap_or("").to_string()
        };
        let before = monitor();
        let dir = std::env::temp_dir().join("open-studio-tap-test");
        std::fs::create_dir_all(&dir).unwrap();
        let tone = dir.join("tone.wav");
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:d=4", "-af", "volume=0.01"])
            .arg(&tone)
            .status()
            .unwrap()
            .success());
        let mut play = std::process::Command::new("pw-play").arg(&tone).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(600));
        let out = dir.join("system.wav");
        let cap = AudioCapture::start(SYSTEM_AUDIO, &out, Arc::new(RecClock::new()), false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        cap.stop().unwrap();
        let _ = play.wait();
        assert_eq!(monitor(), before, "monitor level restored");
        let data = std::fs::read(&out).unwrap();
        let peak = data[44..].chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs()).max().unwrap();
        assert!(peak > 20, "peak {peak}");
    }
}
