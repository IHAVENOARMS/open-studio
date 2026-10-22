//! PipeWire -> GStreamer -> ffmpeg. GStreamer handles format negotiation and
//! turns the damage-driven PipeWire stream into constant frame rate 4:4:4 YUV;
//! ffmpeg does the encoding so we can use whatever encoders it was built with.
//!
//! Quality matters more than usual here: the editor zooms in 2x or more, so
//! chroma subsampling and block artifacts that are invisible at 1:1 become
//! obvious. On the CPU we keep full chroma (4:4:4) and use x264 `veryfast`
//! (which, unlike `ultrafast`, keeps the deblocking filter) at a low CRF.
//! With a GPU encoder (`hwenc`), frames stay RGB until the GPU converts them
//! to 4:2:0, which costs some colour detail but almost no CPU.

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
    /// Frames pushed by the direct capture backend, timestamped in
    /// recording time (see `RecClock`).
    Direct(super::direct::FrameInfo),
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
    /// Buffer timestamps are already recording time (direct capture).
    pts_is_rec_time: bool,
    gpu: Option<&'static crate::hwenc::Gpu>,
}

pub struct Capture {
    pipeline: gst::Pipeline,
    appsrc: Option<gst_app::AppSrc>,
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
    pub fn start(source: CaptureSource, out: &Path, fps: u32, gpu: Option<&'static crate::hwenc::Gpu>) -> Result<Self> {
        gst::init()?;
        let direct = matches!(source, CaptureSource::Direct(_));
        let src = match source {
            CaptureSource::PipeWire { fd, node_id } => format!(
                "pipewiresrc fd={fd} path={node_id} do-timestamp=true keepalive-time=100 always-copy=true"
            ),
            CaptureSource::Direct(info) => format!(
                "appsrc name=src is-live=true format=time do-timestamp=false \
                 caps=video/x-raw,format={},width={},height={},framerate=0/1 ! queue max-size-buffers=16",
                info.gst_format, info.width, info.height
            ),
            CaptureSource::Test { width, height } => format!(
                "videotestsrc is-live=true pattern=ball ! video/x-raw,width={width},height={height},framerate=60/1"
            ),
        };
        // For the GPU, any packed RGB format passes through untouched.
        let format = if gpu.is_some() { "format={BGRx,BGRA,RGBx,RGBA}" } else { "format=Y444,colorimetry=bt709" };
        let desc = format!(
            "{src} ! videorate ! video/x-raw,framerate={fps}/1 ! videoconvert n-threads=0 \
             ! video/x-raw,{format} ! appsink name=sink sync=false max-buffers=16"
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

        let writer = Arc::new(Mutex::new(Writer { pts_is_rec_time: direct, gpu, ..Default::default() }));
        let appsrc = pipeline.by_name("src").and_then(|e| e.downcast::<gst_app::AppSrc>().ok());
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
        Ok(Self { pipeline, appsrc, writer, fps })
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

    /// Pusher for the direct backend's sink.
    pub fn pusher(&self) -> Option<impl Fn(&[u8], i64) + Send + 'static> {
        let src = self.appsrc.clone()?;
        Some(move |data: &[u8], pts_ns: i64| {
            let mut buf = gst::Buffer::from_mut_slice(data.to_vec());
            buf.get_mut().unwrap().set_pts(gst::ClockTime::from_nseconds(pts_ns.max(0) as u64));
            let _ = src.push_buffer(buf);
        })
    }

    pub fn stop(self) -> Result<CaptureResult> {
        if let Some(src) = &self.appsrc {
            let _ = src.end_of_stream();
        }
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

fn spawn_encoder(out: &PathBuf, w: u32, h: u32, fps: u32, format: gst_video::VideoFormat, gpu: Option<&crate::hwenc::Gpu>) -> Result<Child> {
    use gst_video::VideoFormat as F;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    // Short GOP keeps seeking in the editor cheap.
    if let Some(gpu) = gpu {
        let pix_fmt = match format {
            F::Bgrx => "bgr0",
            F::Bgra => "bgra",
            F::Rgbx => "rgb0",
            _ => "rgba",
        };
        cmd.args(gpu.input_args())
            .args(["-f", "rawvideo", "-pix_fmt", pix_fmt, "-s", &format!("{w}x{h}"), "-r", &fps.to_string(), "-i", "-"])
            .args(gpu.output_args(16, fps));
    } else {
        cmd.args(["-f", "rawvideo", "-pix_fmt", "yuv444p"])
            .args(["-color_range", "tv", "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"])
            .args(["-s", &format!("{w}x{h}"), "-r", &fps.to_string(), "-i", "-"])
            .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "14", "-g", &fps.to_string()])
            .args(["-pix_fmt", "yuv444p"]);
    }
    let child = cmd
        .args(["-movflags", "+faststart"])
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
        let mut child = spawn_encoder(out, width, height, fps, info.format(), w.gpu)?;
        w.stdin = child.stdin.take();
        w.child = Some(child);
        w.size = (width, height);
        let pts = buffer.pts().map(|t| t.nseconds() as i64);
        w.first_frame_ns = Some(match (base, pts) {
            (_, Some(p)) if w.pts_is_rec_time => p,
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
    // Y444 has three full-size planes, packed RGB one; neither is subsampled.
    for plane in 0..info.n_planes() {
        let row_bytes = width as usize * info.format_info().pixel_stride()[plane as usize] as usize;
        let stride = frame.plane_stride()[plane as usize] as usize;
        let data = frame.plane_data(plane).map_err(|_| anyhow!("plane data"))?;
        for row in 0..height as usize {
            let start = row * stride;
            w.scratch.extend_from_slice(&data[start..start + row_bytes]);
        }
    }
    let Writer { stdin, scratch, .. } = w;
    stdin.as_mut().unwrap().write_all(scratch).context("writing to encoder")?;
    w.frames += 1;
    Ok(())
}
