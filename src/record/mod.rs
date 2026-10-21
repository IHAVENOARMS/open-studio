//! Recording session: screen capture plus cursor/click/key logging, written
//! out as a project directory.

pub mod capture;
pub mod hyprland;
pub mod input;
pub mod portal;

use crate::project::{Click, Events, Key, Recording, Source, VIDEO_FILE};
use crate::util::{monotonic_ns, recordings_dir};
use anyhow::{Context, Result};
use capture::{Capture, CaptureSource};
use input::InputEvent;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct RecordOptions {
    pub out_dir: Option<PathBuf>,
    pub fps: u32,
    /// Let the compositor draw the real cursor instead of rendering our own.
    pub show_system_cursor: bool,
    /// Reuse the previous portal selection without showing the picker.
    pub reuse_selection: bool,
    /// Record a synthetic test pattern instead of the screen.
    pub test_source: bool,
}

impl Default for RecordOptions {
    fn default() -> Self {
        Self { out_dir: None, fps: 60, show_system_cursor: false, reuse_selection: false, test_source: false }
    }
}

pub struct Recorder {
    dir: PathBuf,
    capture: Capture,
    stop: Arc<AtomicBool>,
    moves: Arc<Mutex<Vec<(i64, f64, f64)>>>,
    input_rx: Receiver<InputEvent>,
    source: Source,
    show_system_cursor: bool,
    started: std::time::Instant,
    _portal: Option<(tokio::runtime::Runtime, portal::PortalStream)>,
}

impl Recorder {
    pub fn start(opts: RecordOptions) -> Result<Self> {
        let dir = match &opts.out_dir {
            Some(d) => d.clone(),
            None => recordings_dir().join(format!("Recording {}", crate::util::local_timestamp())),
        };
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        let mut source = Source::default();
        let (capture_source, portal) = if opts.test_source {
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

        let capture = Capture::start(capture_source, &dir.join(VIDEO_FILE), opts.fps)?;
        let stop = Arc::new(AtomicBool::new(false));
        let moves = Arc::new(Mutex::new(Vec::new()));
        if opts.test_source {
            spawn_fake_cursor(stop.clone(), moves.clone());
        } else if hyprland::available() {
            spawn_cursor_poller(stop.clone(), moves.clone());
        } else {
            eprintln!("open-studio: not running under Hyprland, cursor tracking disabled");
        }
        let (tx, input_rx) = channel();
        if !opts.test_source && input::spawn(stop.clone(), tx) == 0 {
            eprintln!("open-studio: no readable input devices; clicks won't be tracked (add yourself to the `input` group)");
        }
        Ok(Self {
            dir,
            capture,
            stop,
            moves,
            input_rx,
            source,
            show_system_cursor: opts.show_system_cursor,
            started: std::time::Instant::now(),
            _portal: portal,
        })
    }

    pub fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn error(&self) -> Option<String> {
        self.capture.error()
    }

    pub fn stop(self) -> Result<PathBuf> {
        self.stop.store(true, Ordering::Relaxed);
        let fps = self.capture.fps;
        let result = self.capture.stop()?;
        if let Some((rt, stream)) = self._portal {
            let _ = rt.block_on(stream.session.close());
        }
        let t0 = result.first_frame_ns;
        let secs = |ns: i64| (ns - t0) as f64 / 1e9;
        let duration = result.frames as f64 / fps as f64;

        let mut source = self.source;
        if source.logical_rect.is_none() {
            source.logical_rect = guess_monitor_rect(result.width, result.height, &self.moves.lock().unwrap());
        }
        let map = |x: f64, y: f64| -> (f64, f64) {
            match source.logical_rect {
                Some(r) => (
                    (x - r[0]) * result.width as f64 / r[2],
                    (y - r[1]) * result.height as f64 / r[3],
                ),
                None => (x, y),
            }
        };

        let raw_moves = self.moves.lock().unwrap().clone();
        let moves: Vec<[f64; 3]> = raw_moves
            .iter()
            .map(|&(t, x, y)| {
                let (vx, vy) = map(x, y);
                [secs(t), vx, vy]
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
            events: Events { moves, clicks: vec![], keys: vec![] },
        };
        for ev in self.input_rx.try_iter() {
            match ev {
                InputEvent::Button { t_ns, code, down } => {
                    let t = secs(t_ns);
                    let (x, y) = rec.cursor_at(t).unwrap_or((0.0, 0.0));
                    rec.events.clicks.push(Click { t, x, y, button: code, down });
                }
                InputEvent::Key { t_ns, code, down } => rec.events.keys.push(Key { t: secs(t_ns), code, down }),
            }
        }
        rec.save(&self.dir)?;
        Ok(self.dir)
    }
}

/// When the portal doesn't report where the stream sits in the layout, pick
/// the monitor whose pixel size matches the video, preferring the one the
/// cursor was on at the start.
fn guess_monitor_rect(w: u32, h: u32, moves: &[(i64, f64, f64)]) -> Option<[f64; 4]> {
    let monitors = hyprland::monitors().ok()?;
    let matching: Vec<_> = monitors
        .iter()
        .filter(|m| {
            let (pw, ph) = m.pixel_size();
            (pw as u32, ph as u32) == (w, h)
        })
        .collect();
    let first = moves.first().map(|&(_, x, y)| (x, y));
    let pick = match first {
        Some((x, y)) => matching.iter().find(|m| m.contains(x, y)).or(matching.first()),
        None => matching.first(),
    };
    pick.map(|m| m.logical_rect())
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
