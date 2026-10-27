//! Recording session: screen capture plus cursor/click/key logging, written
//! out as a project directory.

pub mod audio;
pub mod camera;
pub mod capture;
pub mod clock;
pub mod direct;
pub mod cursor_shape;
pub mod device;
pub mod input;
pub mod portal;

use crate::desktop::{self, hyprland};
use crate::project::{Click, CustomCursor, Events, Key, Recording, ShapeEvent, Source, VIDEO_FILE};
use cursor_shape::{Shape, ShapeChange};
use crate::util::monotonic_ns;
use anyhow::{Context, Result};
use capture::{Capture, CaptureSource};
use clock::RecClock;
use input::InputEvent;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct RecordOptions {
    pub out_dir: Option<PathBuf>,
    pub fps: u32,
    /// Let the compositor draw the real cursor instead of rendering our own.
    pub show_system_cursor: bool,
    /// Reuse the previous portal selection without showing the picker.
    pub reuse_selection: bool,
    /// Record a synthetic test pattern instead of the screen.
    pub test_source: bool,
    /// Capture directly (no portal picker) when set.
    pub direct: Option<DirectRequest>,
    /// Microphone (Pulse source name) to record, if any.
    pub microphone: Option<String>,
    /// Record what the computer plays.
    pub system_audio: bool,
    /// Camera to record alongside (already open for the live preview).
    pub camera: Option<Arc<camera::CameraFeed>>,
    /// Encode on the GPU when one can (falls back to x264).
    pub gpu_encoding: bool,
    /// Source with a mirrored phone's sound, recorded as its own track.
    pub device_audio: Option<String>,
    /// Global shortcut that marks a recording flag ("Ctrl+Alt+F").
    pub flag_shortcut: Option<String>,
}

/// Camera frames being encoded to camera.mp4 during a recording.
type CameraRec = Arc<Mutex<Option<Result<Capture, String>>>>;
/// Recording-clock time of the first and last camera frame.
type CameraSpan = Arc<Mutex<Option<(i64, i64)>>>;

/// Camera frames rate in camera.mp4.
const CAMERA_FPS: u32 = 30;

/// What to record when bypassing the portal (chosen in our own picker).
#[derive(Clone, Debug)]
pub struct DirectRequest {
    pub target: direct::Target,
    /// "monitor", "window" or "area".
    pub kind: String,
    pub name: Option<String>,
    /// Captured area in compositor logical coordinates.
    pub logical_rect: [f64; 4],
    /// Hyprland address of the captured window: it is followed while
    /// recording, so a window moved mid-recording keeps the cursor aligned.
    pub window: Option<String>,
}

impl Default for RecordOptions {
    fn default() -> Self {
        let s = crate::settings::Settings::load();
        Self {
            out_dir: None,
            fps: 60,
            show_system_cursor: false,
            reuse_selection: false,
            test_source: false,
            direct: None,
            microphone: None,
            system_audio: false,
            camera: None,
            gpu_encoding: s.gpu_encoding,
            device_audio: None,
            flag_shortcut: Some(s.flag_shortcut),
        }
    }
}

pub struct Recorder {
    dir: PathBuf,
    capture: Capture,
    stop: Arc<AtomicBool>,
    moves: Arc<Mutex<Vec<(i64, f64, f64)>>>,
    /// Where the captured window's top-left was, over time (window capture).
    window_track: Arc<Mutex<Vec<(i64, [f64; 2])>>>,
    shapes: Arc<Mutex<Vec<ShapeChange>>>,
    /// Recording flags, monotonic ns.
    flags: Arc<Mutex<Vec<i64>>>,
    input_rx: Receiver<InputEvent>,
    source: Source,
    show_system_cursor: bool,
    started: std::time::Instant,
    clock: Arc<RecClock>,
    audio: Vec<(String, audio::AudioCapture)>,
    /// Timestamps run on `clock` (direct capture) rather than raw monotonic.
    paused_clock: bool,
    direct_error: Arc<Mutex<Option<String>>>,
    camera: Option<(Arc<camera::CameraFeed>, CameraRec, CameraSpan)>,
    _portal: Option<(tokio::runtime::Runtime, portal::PortalStream)>,
}

impl Recorder {
    pub fn start(opts: RecordOptions) -> Result<Self> {
        let dir = match &opts.out_dir {
            Some(d) => d.clone(),
            None => crate::util::unsaved_dir().join(format!("Recording {}", crate::util::local_timestamp())),
        };
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        let mut source = Source::default();
        let clock = Arc::new(RecClock::new());
        let stop = Arc::new(AtomicBool::new(false));
        let direct_error = Arc::new(Mutex::new(None));
        let mut direct_capture = None;
        let (capture_source, portal) = if let Some(req) = &opts.direct {
            let d = direct::Direct::open(&req.target, opts.show_system_cursor)?;
            source.kind = req.kind.clone();
            source.name = req.name.clone();
            source.logical_rect = Some(req.logical_rect);
            let info = d.info;
            direct_capture = Some(d);
            (CaptureSource::Direct(info), None)
        } else if opts.test_source {
            source.kind = "test".into();
            source.logical_rect = Some([0.0, 0.0, 1280.0, 720.0]);
            (CaptureSource::Test { width: 1280, height: 720 }, None)
        } else {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(1).build()?;
            let stream = rt.block_on(portal::open(opts.show_system_cursor, opts.reuse_selection))?;
            source.kind = stream.kind.clone();
            if let (Some(p), Some(s)) = (stream.position, stream.size) {
                source.logical_rect = Some([p.0 as f64, p.1 as f64, s.0 as f64, s.1 as f64]);
            }
            let cs = CaptureSource::PipeWire { fd: stream.fd.as_raw_fd(), node_id: stream.node_id };
            (cs, Some((rt, stream)))
        };

        let gpu = if opts.gpu_encoding { crate::hwenc::detect() } else { None };
        let capture = Capture::start(capture_source, &dir.join(VIDEO_FILE), opts.fps, gpu)?;
        let paused_clock = direct_capture.is_some();
        if let Some(d) = direct_capture {
            let push = capture.pusher().expect("direct capture has an appsrc");
            let (st, cl, err) = (stop.clone(), clock.clone(), direct_error.clone());
            std::thread::spawn(move || {
                if let Err(e) = d.run(st, cl, Box::new(move |data, t| push(data, t))) {
                    *err.lock().unwrap() = Some(format!("{e:#}"));
                }
            });
        }
        let moves = Arc::new(Mutex::new(Vec::new()));
        let window_track = Arc::new(Mutex::new(Vec::new()));
        if let Some(addr) = opts.direct.as_ref().and_then(|r| r.window.clone()).filter(|_| desktop::caps().window_geometry()) {
            spawn_window_tracker(stop.clone(), addr, window_track.clone());
        }
        if opts.test_source {
            spawn_fake_cursor(stop.clone(), moves.clone());
        } else if desktop::caps().pointer_poll() {
            spawn_cursor_poller(stop.clone(), moves.clone());
        } else {
            eprintln!("open-studio: this desktop doesn't report the pointer position, cursor tracking disabled");
        }
        let shapes = Arc::new(Mutex::new(Vec::new()));
        if !opts.test_source && !opts.show_system_cursor && desktop::caps().pointer_poll() && desktop::caps().wlr_screencopy {
            if let Err(e) = cursor_shape::spawn(stop.clone(), shapes.clone()) {
                eprintln!("open-studio: cursor shapes won't be tracked: {e:#}");
            }
        }
        let mut audio_caps = vec![];
        if !opts.test_source {
            let mut sources = vec![];
            if let Some(m) = &opts.microphone {
                sources.push(("microphone", m.clone()));
            }
            if opts.system_audio {
                sources.push(("system", audio::SYSTEM_AUDIO.to_string()));
            }
            if let Some(d) = &opts.device_audio {
                sources.push(("device", d.clone()));
            }
            for (kind, dev) in sources {
                match audio::AudioCapture::start(&dev, &dir.join(format!("{kind}.wav")), clock.clone(), paused_clock) {
                    Ok(c) => audio_caps.push((kind.to_string(), c)),
                    Err(e) => eprintln!("open-studio: {kind} audio unavailable: {e:#}"),
                }
            }
        }
        // Camera: frames from the open feed, stamped with the recording
        // clock (dropped while paused), encoded as they arrive.
        let camera = opts.camera.clone().map(|feed| {
            let rec: CameraRec = Arc::default();
            let span: CameraSpan = Arc::default();
            let (r, sp, cl, path) = (rec.clone(), span.clone(), clock.clone(), dir.join(CAMERA_FILE));
            feed.set_sink(Some(Box::new(move |f: &camera::CamFrame| {
                let Some(t) = cl.now() else { return };
                {
                    let mut s = sp.lock().unwrap();
                    *s = Some(s.map_or((t, t), |(a, _)| (a, t)));
                }
                let mut slot = r.lock().unwrap();
                if slot.is_none() {
                    let info = direct::FrameInfo { width: f.width, height: f.height, gst_format: "RGBA" };
                    *slot = Some(Capture::start(CaptureSource::Direct(info), &path, CAMERA_FPS, gpu).map_err(|e| format!("{e:#}")));
                }
                if let Some(Ok(cap)) = slot.as_ref() {
                    if let Some(push) = cap.pusher() {
                        push(&f.rgba, t);
                    }
                }
            })));
            (feed, rec, span)
        });
        let (tx, input_rx) = channel();
        if !opts.test_source && input::spawn(stop.clone(), tx) == 0 {
            eprintln!("open-studio: {}", input::access_problem().unwrap_or_else(|| "no mouse or keyboard found; clicks won't be tracked".into()));
        }
        // Recording flags from the global shortcut.
        let flags: Arc<Mutex<Vec<i64>>> = Arc::default();
        if let Some(sc) = opts.flag_shortcut.as_deref().and_then(input::Shortcut::parse).filter(|_| !opts.test_source) {
            let hit = Arc::new(AtomicBool::new(false));
            input::watch_shortcut(sc, hit.clone(), stop.clone());
            let (flags, stop) = (flags.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if hit.swap(false, Ordering::Relaxed) {
                        flags.lock().unwrap().push(crate::util::monotonic_ns());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            });
        }
        Ok(Self {
            dir,
            capture,
            stop,
            moves,
            window_track,
            shapes,
            flags,
            input_rx,
            source,
            show_system_cursor: opts.show_system_cursor,
            started: std::time::Instant::now(),
            clock,
            audio: audio_caps,
            paused_clock,
            direct_error,
            camera,
            _portal: portal,
        })
    }

    /// Seconds recorded so far (excluding pauses).
    pub fn elapsed(&self) -> f64 {
        if self.paused_clock { self.clock.elapsed_secs() } else { self.started.elapsed().as_secs_f64() }
    }

    /// Pausing is only available with direct capture.
    pub fn can_pause(&self) -> bool {
        self.paused_clock
    }

    pub fn pause(&self) {
        self.clock.pause();
    }

    pub fn resume(&self) {
        self.clock.resume();
    }

    pub fn is_paused(&self) -> bool {
        self.clock.is_paused()
    }

    /// Where a recorded window's top-left is now (window capture), global
    /// logical pixels.
    pub fn window_origin(&self) -> Option<[f64; 2]> {
        self.window_track.lock().unwrap().last().map(|p| p.1)
    }

    /// Marks this moment with a recording flag (shown on the timeline).
    /// Flags made while paused are dropped, as nothing is recorded then.
    pub fn flag(&self) {
        self.flags.lock().unwrap().push(crate::util::monotonic_ns());
    }

    pub fn flag_count(&self) -> usize {
        self.flags.lock().unwrap().len()
    }

    pub fn error(&self) -> Option<String> {
        self.direct_error.lock().unwrap().clone().or_else(|| self.capture.error())
    }

    /// Stops and throws the recording away.
    pub fn discard(self) {
        let dir = self.dir.clone();
        let _ = self.stop();
        let _ = std::fs::remove_dir_all(dir);
    }

    pub fn stop(self) -> Result<PathBuf> {
        self.stop.store(true, Ordering::Relaxed);
        let fps = self.capture.fps;
        let camera_result = self.camera.as_ref().map(|(feed, rec, span)| {
            feed.set_sink(None);
            let res = match rec.lock().unwrap().take() {
                Some(Ok(cap)) => cap.stop().map_err(|e| format!("{e:#}")),
                Some(Err(e)) => Err(e),
                None => Err("the camera sent no frames".into()),
            };
            // If the encoded length doesn't match the time the frames
            // covered, stretch it so the camera stays in sync with the screen.
            if let (Ok(c), Some((a, b))) = (&res, *span.lock().unwrap()) {
                let real = (b - a) as f64 / 1e9 + 1.0 / CAMERA_FPS as f64;
                let encoded = c.frames as f64 / CAMERA_FPS as f64;
                if (real - encoded).abs() > (0.5f64).max(real * 0.05) {
                    if let Err(e) = retime(&self.dir.join(CAMERA_FILE), real / encoded) {
                        eprintln!("open-studio: camera retime failed: {e:#}");
                    }
                }
            }
            res
        });
        let result = self.capture.stop()?;
        let audio_results: Vec<(String, Option<audio::AudioResult>)> =
            self.audio.into_iter().map(|(k, c)| (k, c.stop().unwrap_or(None))).collect();
        if let Some((rt, stream)) = self._portal {
            let _ = rt.block_on(stream.session.close());
        }
        let t0 = result.first_frame_ns;
        // Event time in seconds from the first frame; None inside pauses.
        let clock = self.clock.clone();
        let paused_clock = self.paused_clock;
        let secs = |ns: i64| -> Option<f64> {
            let t = if paused_clock { clock.map(ns)? } else { ns };
            Some((t - t0) as f64 / 1e9)
        };
        let duration = result.frames as f64 / fps as f64;

        let mut source = self.source;
        if source.logical_rect.is_none() {
            let monitors = desktop::monitors().unwrap_or_default();
            source.logical_rect = guess_monitor_rect(&monitors, result.width, result.height, &self.moves.lock().unwrap());
        }
        // A followed window may have moved: use its position at each moment
        // (the size stays the first one's, like the video).
        let track = self.window_track.lock().unwrap().clone();
        let map = |t: i64, x: f64, y: f64| -> (f64, f64) {
            match source.logical_rect {
                Some(r) => {
                    let i = track.partition_point(|(tt, _)| *tt <= t);
                    let (ox, oy) = if i > 0 { (track[i - 1].1[0], track[i - 1].1[1]) } else { (r[0], r[1]) };
                    ((x - ox) * result.width as f64 / r[2], (y - oy) * result.height as f64 / r[3])
                }
                None => (x, y),
            }
        };

        let raw_moves = self.moves.lock().unwrap().clone();
        let moves: Vec<[f64; 3]> = raw_moves
            .iter()
            .filter_map(|&(t, x, y)| {
                let (vx, vy) = map(t, x, y);
                Some([secs(t)?, vx, vy])
            })
            .collect();
        let mut rec = Recording {
            version: 1,
            video: VIDEO_FILE.into(),
            width: result.width,
            height: result.height,
            fps,
            duration,
            cursor_embedded: self.show_system_cursor,
            source,
            events: Events { moves, clicks: vec![], keys: vec![], shapes: vec![], flags: self.flags.lock().unwrap().iter().filter_map(|&t| secs(t)).collect() },
            custom_cursors: vec![],
            audio: vec![],
            camera: None,
            dir: self.dir.clone(),
            transcript: None,
        };
        match camera_result {
            Some(Ok(c)) => {
                let offset = if paused_clock { (c.first_frame_ns - t0) as f64 / 1e9 } else { 0.0 };
                rec.camera = Some(crate::project::CameraTrack { file: CAMERA_FILE.into(), width: c.width, height: c.height, offset, fps: CAMERA_FPS, matte: None });
            }
            Some(Err(e)) => {
                eprintln!("open-studio: camera not recorded: {e}");
                let _ = std::fs::remove_file(self.dir.join(CAMERA_FILE));
            }
            None => {}
        }
        for (kind, res) in audio_results {
            let file = format!("{kind}.wav");
            match res {
                Some(a) if a.frames > 0 => {
                    let offset = if paused_clock { (a.first_ns - t0) as f64 / 1e9 } else { (a.first_ns - t0) as f64 / 1e9 };
                    rec.audio.push(crate::project::AudioTrack { file, kind, offset });
                }
                _ => {
                    let _ = std::fs::remove_file(self.dir.join(file));
                }
            }
        }
        for change in self.shapes.lock().unwrap().iter() {
            let Some(t) = secs(change.t_ns) else { continue };
            let shape = match &change.shape {
                Shape::Hidden => "hidden".to_string(),
                Shape::Named(n) => n.to_string(),
                Shape::Custom { width, height, rgba, hotspot, scale } => {
                    let idx = rec.custom_cursors.len();
                    let file = format!("cursors/custom-{idx}.png");
                    let _ = std::fs::create_dir_all(self.dir.join("cursors"));
                    if image::save_buffer(self.dir.join(&file), rgba, *width, *height, image::ExtendedColorType::Rgba8).is_err() {
                        continue;
                    }
                    rec.custom_cursors.push(CustomCursor { file, hotspot: [hotspot.0, hotspot.1], scale: *scale });
                    format!("custom:{idx}")
                }
            };
            rec.events.shapes.push(ShapeEvent { t: t.max(0.0), shape });
        }
        for ev in self.input_rx.try_iter() {
            match ev {
                InputEvent::Button { t_ns, code, down } => {
                    let Some(t) = secs(t_ns) else { continue };
                    let (x, y) = rec.cursor_at(t).unwrap_or((0.0, 0.0));
                    rec.events.clicks.push(Click { t, x, y, button: code, down });
                }
                InputEvent::Key { t_ns, code, down } => {
                    let Some(t) = secs(t_ns) else { continue };
                    rec.events.keys.push(Key { t, code, down })
                }
            }
        }
        rec.save(&self.dir)?;
        Ok(self.dir)
    }
}

const CAMERA_FILE: &str = "camera.mp4";

/// Rewrites a video with its timestamps scaled by `k`, at CAMERA_FPS.
fn retime(path: &std::path::Path, k: f64) -> Result<()> {
    let tmp = path.with_extension("retime.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args(["-vf", &format!("setpts=PTS*{k:.6}"), "-r", &CAMERA_FPS.to_string()])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "14", "-pix_fmt", "yuv444p", "-movflags", "+faststart"])
        .arg(&tmp)
        .status()?
        .success();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("ffmpeg failed");
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// When the portal doesn't report where the stream sits in the layout, pick
/// the monitor whose pixel size matches the video, preferring the one the
/// cursor was on at the start.
fn guess_monitor_rect(monitors: &[desktop::Monitor], w: u32, h: u32, moves: &[(i64, f64, f64)]) -> Option<[f64; 4]> {
    let matching: Vec<_> = monitors.iter().filter(|m| m.pixels == (w, h)).collect();
    let first = moves.first().map(|&(_, x, y)| (x, y));
    let pick = match first {
        Some((x, y)) => matching.iter().find(|m| m.contains(x, y)).or(matching.first()),
        None => matching.first(),
    };
    pick.map(|m| m.rect)
}

/// Records where window `addr` is, 30 times a second, while it moves.
fn spawn_window_tracker(stop: Arc<AtomicBool>, addr: String, track: Arc<Mutex<Vec<(i64, [f64; 2])>>>) {
    std::thread::spawn(move || {
        let mut last = [f64::NAN, f64::NAN];
        while !stop.load(Ordering::Relaxed) {
            let t = monotonic_ns();
            if let Some(c) = hyprland::clients().ok().and_then(|cs| cs.into_iter().find(|c| c.address == addr)) {
                if c.at != last {
                    track.lock().unwrap().push((t, c.at));
                    last = c.at;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(33));
        }
    });
}

fn spawn_cursor_poller(stop: Arc<AtomicBool>, moves: Arc<Mutex<Vec<(i64, f64, f64)>>>) {
    std::thread::spawn(move || {
        let period = std::time::Duration::from_micros(1_000_000 / 120);
        let mut last = (f64::NAN, f64::NAN);
        let mut last_push = 0i64;
        while !stop.load(Ordering::Relaxed) {
            let tick = std::time::Instant::now();
            if let Ok(p) = hyprland::cursor_pos() {
                let t = monotonic_ns();
                // Store changes, plus a heartbeat so idle time is represented.
                if p != last || t - last_push > 500_000_000 {
                    moves.lock().unwrap().push((t, p.0, p.1));
                    last = p;
                    last_push = t;
                }
            }
            if let Some(rest) = period.checked_sub(tick.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    });
}

fn spawn_fake_cursor(stop: Arc<AtomicBool>, moves: Arc<Mutex<Vec<(i64, f64, f64)>>>) {
    std::thread::spawn(move || {
        let start = monotonic_ns();
        while !stop.load(Ordering::Relaxed) {
            let t = monotonic_ns();
            let s = (t - start) as f64 / 1e9;
            moves.lock().unwrap().push((t, 640.0 + 400.0 * (s * 0.7).sin(), 360.0 + 220.0 * (s * 1.1).cos()));
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    });
}

/// Direct capture of a monitor, or of an area of it (logical, global coords).
pub fn direct_monitor(name: &str, area: Option<[f64; 4]>) -> Result<DirectRequest> {
    let mon = desktop::monitors()?.into_iter().find(|m| m.name == name).ok_or_else(|| anyhow::anyhow!("no monitor {name}"))?;
    let r = mon.rect;
    match area {
        None => Ok(DirectRequest {
            target: direct::Target::Output { name: name.into(), crop: None },
            kind: "monitor".into(),
            name: Some(name.into()),
            logical_rect: r,
            window: None,
        }),
        Some(a) => {
            // Clip to the monitor, convert to buffer pixels.
            let x0 = a[0].max(r[0]);
            let y0 = a[1].max(r[1]);
            let x1 = (a[0] + a[2]).min(r[0] + r[2]);
            let y1 = (a[1] + a[3]).min(r[1] + r[3]);
            let s = mon.scale();
            let crop = [((x0 - r[0]) * s) as u32, ((y0 - r[1]) * s) as u32, ((x1 - x0) * s) as u32, ((y1 - y0) * s) as u32];
            // Keep the logical rect consistent with the (even-sized) crop.
            let (cw, ch) = ((crop[2] & !1) as f64 / s, (crop[3] & !1) as f64 / s);
            Ok(DirectRequest {
                target: direct::Target::Output { name: name.into(), crop: Some(crop) },
                kind: "area".into(),
                name: Some(name.into()),
                logical_rect: [x0, y0, cw, ch],
                window: None,
            })
        }
    }
}

/// Direct capture of a window by Hyprland client address.
pub fn direct_window(address: &str) -> Result<DirectRequest> {
    let c = hyprland::clients()?.into_iter().find(|c| c.address == address).ok_or_else(|| anyhow::anyhow!("no window {address}"))?;
    Ok(DirectRequest {
        target: direct::Target::Window { app_id: c.class.clone(), title: c.title.clone() },
        kind: "window".into(),
        name: Some(c.title.clone()),
        logical_rect: [c.at[0], c.at[1], c.size[0], c.size[1]],
        window: Some(c.address.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_monitor_is_guessed_by_size_and_cursor() {
        let mon = |name: &str, x: f64| desktop::Monitor { name: name.into(), description: String::new(), rect: [x, 0.0, 1920.0, 1080.0], pixels: (1920, 1080), refresh: 60.0 };
        let hidpi = desktop::Monitor { name: "C".into(), description: String::new(), rect: [3840.0, 0.0, 2560.0, 1440.0], pixels: (3840, 2160), refresh: 60.0 };
        let ms = [mon("A", 0.0), mon("B", 1920.0), hidpi];
        assert_eq!(guess_monitor_rect(&ms, 1920, 1080, &[(0, 2000.0, 10.0)]), Some([1920.0, 0.0, 1920.0, 1080.0]));
        assert_eq!(guess_monitor_rect(&ms, 1920, 1080, &[]), Some([0.0, 0.0, 1920.0, 1080.0]));
        assert_eq!(guess_monitor_rect(&ms, 3840, 2160, &[]), Some([3840.0, 0.0, 2560.0, 1440.0]));
        assert_eq!(guess_monitor_rect(&ms, 800, 600, &[]), None);
        assert_eq!(ms[2].scale(), 1.5);
    }

    /// Flags made while recording land in recording.json at the moments
    /// they were made. Opt-in (records a test pattern):
    /// `cargo test -- --ignored flags_are_saved`.
    #[test]
    #[ignore]
    fn flags_are_saved_at_their_time() {
        let dir = std::env::temp_dir().join("open-studio-flag-test");
        let _ = std::fs::remove_dir_all(&dir);
        let r = Recorder::start(RecordOptions { out_dir: Some(dir.clone()), test_source: true, gpu_encoding: false, ..Default::default() }).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let first = r.elapsed();
        r.flag();
        std::thread::sleep(std::time::Duration::from_millis(700));
        r.flag();
        assert_eq!(r.flag_count(), 2);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let out = r.stop().unwrap();
        let rec = Recording::load(&out).unwrap();
        let f = &rec.events.flags;
        assert_eq!(f.len(), 2);
        assert!((f[0] - first).abs() < 0.3, "first flag at {:.2}s, made at {first:.2}s", f[0]);
        assert!((f[1] - f[0] - 0.7).abs() < 0.1, "flags {f:?} should be 0.7s apart");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
