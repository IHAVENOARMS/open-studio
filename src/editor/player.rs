//! Background decoder that feeds preview frames. The UI owns the clock; the
//! player just keeps the latest frame for the requested time available.

use crate::decode::Decoder;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Preview frames are decoded at most this wide. Full resolution up to 4K so
/// zoomed-in previews stay sharp.
pub const PREVIEW_MAX_WIDTH: u32 = 3840;

pub struct Frame {
    pub index: i64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

enum Cmd {
    Seek(f64),
    Play { from: f64, at: Instant },
    Pause,
}

pub struct Player {
    tx: Sender<Cmd>,
    latest: Arc<Mutex<Option<Arc<Frame>>>>,
}

impl Player {
    pub fn new(video: PathBuf, size: (u32, u32), fps: u32, repaint: egui::Context) -> Self {
        let (tx, rx) = channel();
        let latest = Arc::new(Mutex::new(None));
        let l = latest.clone();
        std::thread::spawn(move || run(video, size, fps as f64, rx, l, repaint));
        Self { tx, latest }
    }

    pub fn seek(&self, t: f64) {
        let _ = self.tx.send(Cmd::Seek(t));
    }

    pub fn play(&self, from: f64, at: Instant) {
        let _ = self.tx.send(Cmd::Play { from, at });
    }

    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }

    pub fn latest(&self) -> Option<Arc<Frame>> {
        self.latest.lock().unwrap().clone()
    }
}

struct State {
    video: PathBuf,
    size: (u32, u32),
    fps: f64,
    decoder: Option<Decoder>,
    /// Index of the last frame read from `decoder`.
    index: i64,
    buf: Vec<u8>,
}

impl State {
    /// Reads forward to frame `want`, reopening the decoder for backwards or
    /// long jumps. Returns false when the stream ended first.
    fn advance_to(&mut self, want: i64) -> bool {
        let reopen = match &self.decoder {
            None => true,
            Some(_) => want < self.index || want - self.index > (self.fps as i64) * 2,
        };
        if reopen {
            match Decoder::open(&self.video, want as f64 / self.fps, self.size, Some(PREVIEW_MAX_WIDTH)) {
                Ok(d) => {
                    self.decoder = Some(d);
                    self.index = want - 1;
                }
                Err(_) => return false,
            }
        }
        let d = self.decoder.as_mut().unwrap();
        while self.index < want {
            match d.read(&mut self.buf) {
                Ok(true) => self.index += 1,
                _ => {
                    self.decoder = None;
                    return false;
                }
            }
        }
        true
    }

    fn frame(&self) -> Arc<Frame> {
        let d = self.decoder.as_ref().unwrap();
        Arc::new(Frame { index: self.index, width: d.width, height: d.height, rgba: self.buf.clone() })
    }
}

fn run(video: PathBuf, size: (u32, u32), fps: f64, rx: Receiver<Cmd>, latest: Arc<Mutex<Option<Arc<Frame>>>>, ctx: egui::Context) {
    let mut st = State { video, size, fps, decoder: None, index: -1, buf: Vec::new() };
    let mut playing: Option<(f64, Instant)> = None;
    let publish = |st: &State| {
        *latest.lock().unwrap() = Some(st.frame());
        ctx.request_repaint();
    };
    loop {
        // While paused block for a command; while playing just poll.
        let cmd = if playing.is_some() {
            rx.try_recv().ok()
        } else {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            }
        };
        // Collapse a burst of seeks (scrubbing) into the last one.
        let mut cmd = cmd;
        if matches!(cmd, Some(Cmd::Seek(_))) {
            while let Ok(next) = rx.try_recv() {
                cmd = Some(next);
            }
        }
        match cmd {
            Some(Cmd::Seek(t)) => {
                playing = None;
                let want = (t * fps).floor().max(0.0) as i64;
                if st.advance_to(want) {
                    publish(&st);
                }
            }
            Some(Cmd::Play { from, at }) => playing = Some((from, at)),
            Some(Cmd::Pause) => playing = None,
            None => {}
        }
        if let Some((from, at)) = playing {
            let t = from + at.elapsed().as_secs_f64();
            let want = (t * fps).floor() as i64;
            if want > st.index || st.decoder.is_none() {
                if st.advance_to(want) {
                    publish(&st);
                } else {
                    playing = None;
                }
            }
            // Sleep until the next frame is due.
            let next = (st.index + 1) as f64 / fps - from;
            let wait = next - at.elapsed().as_secs_f64();
            if wait > 0.0 {
                std::thread::sleep(std::time::Duration::from_secs_f64(wait.min(0.02)));
            }
        }
    }
}
