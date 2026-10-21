//! PipeWire -> GStreamer -> ffmpeg. GStreamer handles format negotiation and
//! turns the damage-driven PipeWire stream into constant frame rate I420;
//! ffmpeg does the encoding so we can use whatever encoders it was built with.

use crate::util::monotonic_ns;
use anyhow::{Context, Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

pub enum CaptureSource {
    PipeWire { fd: i32, node_id: u32 },
    /// Synthetic source used for headless testing.
    Test { width: u32, height: u32 },
}

#[derive(Default)]
struct Writer {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    frames: u64,
    first_frame_ns: Option<i64>,
    size: (u32, u32),
    error: Option<String>,
    scratch: Vec<u8>,
}

pub struct Capture {
    pipeline: gst::Pipeline,
    writer: Arc<Mutex<Writer>>,
    pub fps: u32,
}

pub struct CaptureResult {
    pub frames: u64,
    pub first_frame_ns: i64,
    pub width: u32,
    pub height: u32,
}

impl Capture {
    pub fn start(source: CaptureSource, out: &Path, fps: u32) -> Result<Self> {
        gst::init()?;
        let src = match source {
            CaptureSource::PipeWire { fd, node_id } => format!(
                "pipewiresrc fd={fd} path={node_id} do-timestamp=true keepalive-time=100 always-copy=true"
            ),
            CaptureSource::Test { width, height } => format!(
                "videotestsrc is-live=true pattern=ball ! video/x-raw,width={width},height={height},framerate=60/1"
            ),
        };
        let desc = format!(
            "{src} ! videorate ! video/x-raw,framerate={fps}/1 ! videoconvert n-threads=0 \
             ! video/x-raw,format=I420 ! appsink name=sink sync=false max-buffers=16"
        );
        let pipeline = gst::parse::launch(&desc)
            .context("building GStreamer pipeline")?
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow!("not a pipeline"))?;
        // Pin the clock to CLOCK_MONOTONIC so buffer times compare with our
        // event timestamps.
        let clock = gst::SystemClock::obtain();
        clock.set_property("clock-type", gst::ClockType::Monotonic);
        pipeline.use_clock(Some(&clock));

        let writer = Arc::new(Mutex::new(Writer::default()));
        let sink = pipeline
            .by_name("sink")
            .unwrap()
            .downcast::<gst_app::AppSink>()
            .map_err(|_| anyhow!("appsink"))?;
        let out = out.to_path_buf();
        let w = writer.clone();
        let pipe_weak = pipeline.downgrade();
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let mut w = w.lock().unwrap();
                    let base = pipe_weak.upgrade().and_then(|p| p.base_time()).map(|t| t.nseconds() as i64);
                    match write_sample(&mut w, &sample, &out, fps, base) {
                        Ok(()) => Ok(gst::FlowSuccess::Ok),
                        Err(e) => {
                            w.error = Some(format!("{e:#}"));
                            Err(gst::FlowError::Error)
                        }
                    }
                })
                .build(),
        );
        pipeline.set_state(gst::State::Playing).context("starting capture")?;
        Ok(Self { pipeline, writer, fps })
    }

    /// Error raised by the pipeline or the encoder, if any.
    pub fn error(&self) -> Option<String> {
        if let Some(e) = self.writer.lock().unwrap().error.clone() {
            return Some(e);
        }
        let bus = self.pipeline.bus()?;
        while let Some(msg) = bus.pop() {
            if let gst::MessageView::Error(err) = msg.view() {
                return Some(format!("{} ({:?})", err.error(), err.debug()));
            }
        }
        None
    }

    pub fn stop(self) -> Result<CaptureResult> {
        self.pipeline.send_event(gst::event::Eos::new());
        if let Some(bus) = self.pipeline.bus() {
            bus.timed_pop_filtered(
                gst::ClockTime::from_seconds(3),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            );
        }
        let _ = self.pipeline.set_state(gst::State::Null);
        let mut w = self.writer.lock().unwrap();
        drop(w.stdin.take());
        if let Some(mut child) = w.child.take() {
            let status = child.wait()?;
            if !status.success() {
                return Err(anyhow!("ffmpeg exited with {status}"));
            }
        }
        if let Some(e) = &w.error {
            return Err(anyhow!("capture failed: {e}"));
        }
        let first = w.first_frame_ns.ok_or_else(|| anyhow!("no frames were captured"))?;
        Ok(CaptureResult { frames: w.frames, first_frame_ns: first, width: w.size.0, height: w.size.1 })
    }
}

fn spawn_encoder(out: &PathBuf, w: u32, h: u32, fps: u32) -> Result<Child> {
    // Short GOP keeps seeking in the editor cheap.
    let child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p"])
        .args(["-s", &format!("{w}x{h}"), "-r", &fps.to_string(), "-i", "-"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "16", "-g", &fps.to_string()])
        .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .context("launching ffmpeg (is it installed?)")?;
    Ok(child)
}

fn write_sample(w: &mut Writer, sample: &gst::Sample, out: &PathBuf, fps: u32, base: Option<i64>) -> Result<()> {
    let caps = sample.caps().ok_or_else(|| anyhow!("sample without caps"))?;
    let info = gst_video::VideoInfo::from_caps(caps)?;
    let buffer = sample.buffer().ok_or_else(|| anyhow!("sample without buffer"))?;
    let (width, height) = (info.width(), info.height());
    if w.child.is_none() {
        let mut child = spawn_encoder(out, width, height, fps)?;
        w.stdin = child.stdin.take();
        w.child = Some(child);
        w.size = (width, height);
        let pts = buffer.pts().map(|t| t.nseconds() as i64);
        w.first_frame_ns = Some(match (base, pts) {
            (Some(b), Some(p)) => b + p,
            _ => monotonic_ns(),
        });
    }
    if (width, height) != w.size {
        // Source resized mid-recording (window capture); we keep the first size.
        return Ok(());
    }
    let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
        .map_err(|_| anyhow!("mapping frame"))?;
    // Repack planes tightly; GStreamer pads strides, ffmpeg's rawvideo expects none.
    w.scratch.clear();
    for plane in 0..3u32 {
        let (pw, ph) = if plane == 0 { (width, height) } else { (width.div_ceil(2), height.div_ceil(2)) };
        let stride = frame.plane_stride()[plane as usize] as usize;
        let data = frame.plane_data(plane).map_err(|_| anyhow!("plane data"))?;
        for row in 0..ph as usize {
            let start = row * stride;
            w.scratch.extend_from_slice(&data[start..start + pw as usize]);
        }
    }
    let Writer { stdin, scratch, .. } = w;
    stdin.as_mut().unwrap().write_all(scratch).context("writing to encoder")?;
    w.frames += 1;
    Ok(())
}
