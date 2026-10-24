//! Cameras for the recording bar: webcams (V4L2) and Android phones, whose
//! cameras are streamed over adb with scrcpy's server. A `CameraFeed` keeps
//! the latest frame for the live preview and hands every frame to the
//! recorder while recording.

use anyhow::{Context, Result, anyhow, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video::prelude::*;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    V4l2 { path: String, name: String },
    /// A phone camera: adb serial and "back" or "front".
    Phone { serial: String, name: String, facing: String },
}

impl Source {
    /// Stable id kept in the settings.
    pub fn id(&self) -> String {
        match self {
            Source::V4l2 { path, .. } => format!("v4l2:{path}"),
            Source::Phone { serial, facing, .. } => format!("phone:{serial}:{facing}"),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Source::V4l2 { name, .. } => name.clone(),
            Source::Phone { name, facing, .. } => format!("{name} ({facing} camera)"),
        }
    }
}

/// Colour cameras, then each connected phone's back and front cameras.
pub fn list() -> Vec<Source> {
    let mut out = webcams();
    if super::device::scrcpy_server().is_some() {
        for d in super::device::list().into_iter().filter(|d| d.ready) {
            for facing in ["back", "front"] {
                out.push(Source::Phone { serial: d.serial.clone(), name: d.model.clone(), facing: facing.into() });
            }
        }
    }
    out
}

fn webcams() -> Vec<Source> {
    if gst::init().is_err() {
        return vec![];
    }
    let mon = gst::DeviceMonitor::new();
    mon.add_filter(Some("Video/Source"), None);
    if mon.start().is_err() {
        return vec![];
    }
    let mut out: Vec<Source> = vec![];
    for dev in mon.devices() {
        let Some(props) = dev.properties() else { continue };
        let path = props.get::<String>("api.v4l2.path").or_else(|_| props.get::<String>("device.path"));
        let Ok(path) = path else { continue };
        // Skip infrared and other greyscale-only nodes.
        let caps = dev.caps().map(|c| c.to_string()).unwrap_or_default();
        let colour = ["image/jpeg", "YUY2", "NV12", "I420", "RGB", "BGR", "UYVY"].iter().any(|f| caps.contains(f));
        if !colour || out.iter().any(|s| matches!(s, Source::V4l2 { path: p, .. } if *p == path)) {
            continue;
        }
        let name = dev.display_name().trim_end_matches(" (V4L2)").to_string();
        out.push(Source::V4l2 { path, name });
    }
    mon.stop();
    out
}

pub struct CamFrame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA.
    pub rgba: Vec<u8>,
    /// Increments per frame, for the preview to notice new ones.
    pub seq: u64,
}

type Sink = Box<dyn FnMut(&CamFrame) + Send>;

pub struct CameraFeed {
    pub source: Source,
    pipeline: gst::Pipeline,
    latest: Arc<Mutex<Option<Arc<CamFrame>>>>,
    sink: Arc<Mutex<Option<Sink>>>,
    error: Arc<Mutex<Option<String>>>,
    phone: Option<PhoneStream>,
}

/// Largest camera picture (width, height) for a Maximum camera resolution
/// setting, landscape 16:9 at most.
pub fn max_size(max_height: u32) -> (u32, u32) {
    let h = match max_height {
        0..=720 => 720,
        721..=1080 => 1080,
        _ => 2160,
    };
    (h * 16 / 9, h)
}

impl CameraFeed {
    /// Opens a camera, recording at most `max_height` lines (Maximum camera
    /// resolution: 720, 1080 or 2160).
    pub fn open(source: &Source, max_height: u32) -> Result<Self> {
        gst::init()?;
        let tail = "videoconvert n-threads=0 ! video/x-raw,format=RGBA ! appsink name=sink sync=false max-buffers=2 drop=true";
        let (w, h) = max_size(max_height);
        let (desc, phone) = match source {
            // Largest mode within the limit; MJPEG or raw, whichever the camera offers.
            Source::V4l2 { path, .. } => (
                format!("v4l2src device={path} ! video/x-raw,width=[1,{w}],height=[1,{h}];image/jpeg,width=[1,{w}],height=[1,{h}] ! decodebin ! {tail}"),
                None,
            ),
            Source::Phone { serial, facing, .. } => (
                format!("appsrc name=src is-live=true do-timestamp=true format=time caps=video/x-h264,stream-format=byte-stream ! h264parse ! avdec_h264 ! {tail}"),
                Some((serial.clone(), facing.clone())),
            ),
        };
        let pipeline = gst::parse::launch(&desc)?.downcast::<gst::Pipeline>().map_err(|_| anyhow!("not a pipeline"))?;
        let latest: Arc<Mutex<Option<Arc<CamFrame>>>> = Arc::default();
        let sink: Arc<Mutex<Option<Sink>>> = Arc::default();
        let error: Arc<Mutex<Option<String>>> = Arc::default();
        let appsink = pipeline.by_name("sink").unwrap().downcast::<gst_app::AppSink>().map_err(|_| anyhow!("appsink"))?;
        let (l, s) = (latest.clone(), sink.clone());
        let mut seq = 0u64;
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let Some(frame) = to_frame(&sample, seq) else { return Ok(gst::FlowSuccess::Ok) };
                    seq += 1;
                    if let Some(f) = s.lock().unwrap().as_mut() {
                        f(&frame);
                    }
                    *l.lock().unwrap() = Some(Arc::new(frame));
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        pipeline.set_state(gst::State::Playing).context("starting camera")?;
        let phone = match phone {
            Some((serial, facing)) => {
                let src = pipeline.by_name("src").unwrap().downcast::<gst_app::AppSrc>().map_err(|_| anyhow!("appsrc"))?;
                Some(PhoneStream::start(&serial, &facing, w, src, error.clone())?)
            }
            None => None,
        };
        Ok(Self { source: source.clone(), pipeline, latest, sink, error, phone })
    }

    pub fn latest(&self) -> Option<Arc<CamFrame>> {
        self.latest.lock().unwrap().clone()
    }

    /// Receives every new frame (on a streaming thread) until cleared.
    pub fn set_sink(&self, f: Option<Sink>) {
        *self.sink.lock().unwrap() = f;
    }

    pub fn error(&self) -> Option<String> {
        if let Some(e) = self.error.lock().unwrap().clone() {
            return Some(e);
        }
        let bus = self.pipeline.bus()?;
        while let Some(msg) = bus.pop() {
            if let gst::MessageView::Error(err) = msg.view() {
                return Some(err.error().to_string());
            }
        }
        None
    }
}

impl Drop for CameraFeed {
    fn drop(&mut self) {
        self.set_sink(None);
        drop(self.phone.take());
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn to_frame(sample: &gst::Sample, seq: u64) -> Option<CamFrame> {
    let info = gstreamer_video::VideoInfo::from_caps(sample.caps()?).ok()?;
    let buffer = sample.buffer()?;
    let frame = gstreamer_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).ok()?;
    let (w, h) = (info.width(), info.height());
    let stride = frame.plane_stride()[0] as usize;
    let data = frame.plane_data(0).ok()?;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h as usize {
        rgba.extend_from_slice(&data[row * stride..row * stride + w as usize * 4]);
    }
    Some(CamFrame { width: w, height: h, rgba, seq })
}

/// scrcpy's server running on the phone in camera mode, sending raw H.264
/// through an adb port forward.
struct PhoneStream {
    adb: std::path::PathBuf,
    serial: String,
    port: u16,
    server: Child,
    stop: Arc<AtomicBool>,
}

impl PhoneStream {
    /// `max_side`: the picture's long side at most this many pixels.
    fn start(serial: &str, facing: &str, max_side: u32, src: gst_app::AppSrc, error: Arc<Mutex<Option<String>>>) -> Result<Self> {
        let adb = super::device::adb().ok_or_else(|| anyhow!("adb is not installed"))?;
        let jar = super::device::scrcpy_server().ok_or_else(|| anyhow!("scrcpy is not installed"))?;
        let version = super::device::scrcpy_version().unwrap_or_else(|| "4.1".into());
        let run = |args: &[&str]| -> Result<String> {
            let out = Command::new(&adb).arg("-s").arg(serial).args(args).stdin(Stdio::null()).output()?;
            if !out.status.success() {
                bail!("adb {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
            }
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        };
        const REMOTE: &str = "/data/local/tmp/open-studio-scrcpy.jar";
        run(&["push", &jar.to_string_lossy(), REMOTE])?;
        let scid = format!("{:08x}", (std::process::id() ^ crate::util::monotonic_ns() as u32) & 0x7fff_ffff);
        let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        run(&["forward", &format!("tcp:{port}"), &format!("localabstract:scrcpy_{scid}")])?;
        let server = Command::new(&adb)
            .args(["-s", serial, "shell", &format!("CLASSPATH={REMOTE}"), "app_process", "/", "com.genymobile.scrcpy.Server", &version])
            .args([
                format!("scid={scid}"),
                "log_level=warn".into(),
                "video_source=camera".into(),
                format!("camera_facing={facing}"),
                format!("max_size={max_side}"),
                "camera_fps=30".into(),
                "audio=false".into(),
                "control=false".into(),
                "tunnel_forward=true".into(),
                "raw_stream=true".into(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("starting the phone camera")?;
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        std::thread::spawn(move || {
            // The server needs a moment to start listening.
            let mut conn = None;
            for _ in 0..50 {
                if st.load(Ordering::Relaxed) {
                    return;
                }
                if let Ok(mut c) = std::net::TcpStream::connect(("127.0.0.1", port)) {
                    // adb accepts the connection before the server is ready and
                    // then closes it; a first byte means it's really streaming.
                    let mut first = [0u8; 1];
                    if matches!(c.read(&mut first), Ok(1)) {
                        let _ = src.push_buffer(gst::Buffer::from_mut_slice(first.to_vec()));
                        conn = Some(c);
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            let Some(mut c) = conn else {
                *error.lock().unwrap() = Some("the phone camera did not start (Android 12 or later is needed)".into());
                return;
            };
            let mut buf = vec![0u8; 1 << 16];
            while !st.load(Ordering::Relaxed) {
                match c.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let _ = src.push_buffer(gst::Buffer::from_mut_slice(buf[..n].to_vec()));
                    }
                }
            }
            if !st.load(Ordering::Relaxed) {
                *error.lock().unwrap() = Some("the phone camera stopped".into());
            }
        });
        Ok(Self { adb, serial: serial.to_string(), port, server, stop })
    }
}

impl Drop for PhoneStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = Command::new(&self.adb)
            .args(["-s", &self.serial, "forward", "--remove", &format!("tcp:{}", self.port)])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod size_tests {
    #[test]
    fn maximum_camera_resolution() {
        assert_eq!(super::max_size(720), (1280, 720));
        assert_eq!(super::max_size(1080), (1920, 1080));
        assert_eq!(super::max_size(2160), (3840, 2160));
        // Anything else rounds to the nearest offered step.
        assert_eq!(super::max_size(0), (1280, 720));
        assert_eq!(super::max_size(5000), (3840, 2160));
    }
}
