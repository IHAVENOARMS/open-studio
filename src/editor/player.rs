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
    Play { from: f64, at: Instant, speed: f64 },
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

    /// Plays from `from` (video time) as of `at`, `speed` video seconds per second.
    pub fn play(&self, from: f64, at: Instant, speed: f64) {
        let _ = self.tx.send(Cmd::Play { from, at, speed });
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
    let mut playing: Option<(f64, Instant, f64)> = None;
    // Consecutive decode failures while playing; a few are retried (the
    // decoder is reopened) before playback stops.
    let mut failures = 0;
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
        // Only the newest command matters: each one replaces the playhead.
        // Working through a backlog (a "play from here" per frame while
        // scrubbing during playback, each reopening the decoder) left the
        // picture seconds behind, frozen until a pause flushed it.
        let mut cmd = cmd;
        if cmd.is_some() {
            while let Ok(next) = rx.try_recv() {
                cmd = Some(next);
            }
        }
        if std::env::var_os("OPEN_STUDIO_DEBUG_PLAYER").is_some() {
            match &cmd {
                Some(Cmd::Seek(t)) => eprintln!("{} seek {t:.2}", st.video.display()),
                Some(Cmd::Play { from, .. }) => eprintln!("{} play {from:.2}", st.video.display()),
                Some(Cmd::Pause) => eprintln!("{} pause", st.video.display()),
                None => {}
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
            Some(Cmd::Play { from, at, speed }) => playing = Some((from, at, speed)),
            Some(Cmd::Pause) => playing = None,
            None => {}
        }
        if let Some((from, at, speed)) = playing {
            let t = from + at.elapsed().as_secs_f64() * speed;
            let want = (t * fps).floor() as i64;
            if want > st.index || st.decoder.is_none() {
                if st.advance_to(want) {
                    publish(&st);
                    failures = 0;
                } else {
                    failures += 1;
                    if std::env::var_os("OPEN_STUDIO_DEBUG_PLAYER").is_some() {
                        eprintln!("{} decode failed at frame {want} ({failures})", st.video.display());
                    }
                    if failures >= 3 {
                        playing = None;
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            }
            // Sleep until the next frame is due.
            let next = ((st.index + 1) as f64 / fps - from) / speed;
            let wait = next - at.elapsed().as_secs_f64();
            if wait > 0.0 {
                std::thread::sleep(std::time::Duration::from_secs_f64(wait.min(0.02)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays, pauses mid-way (pause + seek, as the editor does) and plays
    /// again, many times; after every resume frames must keep coming.
    /// Opt-in (decodes video): `cargo test -- --ignored pause_resume`.
    #[test]
    #[ignore]
    fn pause_resume_keeps_frames_coming() {
        let dir = std::env::temp_dir().join("open-studio-player-test");
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("cam.mp4");
        if !video.exists() {
            assert!(std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=40", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
                .arg(&video)
                .status()
                .unwrap()
                .success());
        }
        let p = Player::new(video, (640, 360), 30, egui::Context::default());
        let wait = |ms| std::thread::sleep(std::time::Duration::from_millis(ms));
        let mut t = 2.0;
        p.seek(t);
        wait(300);
        for round in 0..12 {
            let start = Instant::now();
            p.play(t, start, 1.0);
            wait(400 + 37 * round);
            let before = p.latest().map(|f| f.index);
            wait(300);
            let after = p.latest().map(|f| f.index);
            assert!(after > before, "round {round}: frames stopped at {before:?}");
            // Scrubbing while playing: a "play from here" every frame.
            if round == 6 {
                for k in 0..60 {
                    let at = Instant::now();
                    p.play(5.0 + (k % 7) as f64 * 3.1, at, 1.0);
                    wait(16);
                }
                t = 30.0;
                let at = Instant::now();
                p.play(t, at, 1.0);
                wait(500);
                let f = p.latest().unwrap().index as f64 / 30.0;
                assert!((f - (t + 0.5)).abs() < 0.35, "after a scrub the camera is at {f:.2}s, expected about {:.2}s", t + 0.5);
                t += at.elapsed().as_secs_f64();
                p.pause();
                p.seek(t);
                wait(100);
                continue;
            }
            // Pause where the playhead is, like the editor.
            t += start.elapsed().as_secs_f64();
            p.pause();
            p.seek(t);
            wait(if round % 2 == 0 { 20 } else { 250 });
        }
    }
}
