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
        let desc = format!(
            "pulsesrc device=\"{device}\" do-timestamp=true client-name=\"Open Studio\" ! audioconvert ! audioresample \
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
        Ok(Self { pipeline, wav, path: out.to_path_buf() })
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
