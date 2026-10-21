//! Recording clock: CLOCK_MONOTONIC with paused intervals cut out, so video
//! frames and input events recorded after a pause line up.

use crate::util::monotonic_ns;
use std::sync::Mutex;

#[derive(Default)]
struct State {
    paused_since: Option<i64>,
    /// Completed pauses (start, end) in monotonic ns.
    pauses: Vec<(i64, i64)>,
}

pub struct RecClock {
    start: i64,
    state: Mutex<State>,
}

impl RecClock {
    pub fn new() -> Self {
        Self { start: monotonic_ns(), state: Mutex::new(State::default()) }
    }

    pub fn pause(&self) {
        let mut s = self.state.lock().unwrap();
        if s.paused_since.is_none() {
            s.paused_since = Some(monotonic_ns());
        }
    }

    pub fn resume(&self) {
        let mut s = self.state.lock().unwrap();
        if let Some(p) = s.paused_since.take() {
            s.pauses.push((p, monotonic_ns()));
        }
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().unwrap().paused_since.is_some()
    }

    /// Recording time (ns since start, pauses removed) for a monotonic
    /// timestamp; None if it falls inside a pause.
    pub fn map(&self, mono: i64) -> Option<i64> {
        let s = self.state.lock().unwrap();
        if s.paused_since.is_some_and(|p| mono >= p) {
            return None;
        }
        let mut cut = 0;
        for &(a, b) in &s.pauses {
            if mono >= b {
                cut += b - a;
            } else if mono >= a {
                return None;
            }
        }
        Some(mono - self.start - cut)
    }

    pub fn now(&self) -> Option<i64> {
        self.map(monotonic_ns())
    }

    /// Recording time elapsed so far (frozen while paused).
    pub fn elapsed_secs(&self) -> f64 {
        let s = self.state.lock().unwrap();
        let now = s.paused_since.unwrap_or_else(monotonic_ns);
        let cut: i64 = s.pauses.iter().map(|(a, b)| b - a).sum();
        (now - self.start - cut).max(0) as f64 / 1e9
    }
}
