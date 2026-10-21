//! On-disk project format. A project is a directory holding the raw capture
//! (`screen.mp4`), what was recorded alongside it (`recording.json`) and the
//! user's edits (`project.json`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const VIDEO_FILE: &str = "screen.mp4";
pub const RECORDING_FILE: &str = "recording.json";
pub const PROJECT_FILE: &str = "project.json";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Recording {
    pub version: u32,
    pub video: String,
    /// Captured video size in pixels.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration: f64,
    /// True when the compositor drew the cursor into the video; we then skip
    /// rendering our own.
    pub cursor_embedded: bool,
    pub source: Source,
    pub events: Events,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Source {
    pub kind: String,
    pub name: Option<String>,
    /// Captured area in compositor logical coordinates (x, y, w, h).
    pub logical_rect: Option<[f64; 4]>,
}

/// All times are seconds relative to the first video frame, positions are in
/// video pixels.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Events {
    pub moves: Vec<[f64; 3]>,
    pub clicks: Vec<Click>,
    pub keys: Vec<Key>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Click {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub button: u16,
    pub down: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Key {
    pub t: f64,
    pub code: u16,
    pub down: bool,
}

impl Recording {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(RECORDING_FILE);
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::write(dir.join(RECORDING_FILE), serde_json::to_string(self)?)?;
        Ok(())
    }

    /// Cursor position at time `t`, linearly interpolated between samples.
    pub fn cursor_at(&self, t: f64) -> Option<(f64, f64)> {
        let m = &self.events.moves;
        if m.is_empty() {
            return None;
        }
        let i = m.partition_point(|s| s[0] <= t);
        if i == 0 {
            return Some((m[0][1], m[0][2]));
        }
        if i >= m.len() {
            let l = m[m.len() - 1];
            return Some((l[1], l[2]));
        }
        let (a, b) = (m[i - 1], m[i]);
        let f = ((t - a[0]) / (b[0] - a[0]).max(1e-9)).clamp(0.0, 1.0);
        Some((a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f))
    }
}

pub struct ProjectPaths {
    pub dir: PathBuf,
}

impl ProjectPaths {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn video(&self) -> PathBuf {
        self.dir.join(VIDEO_FILE)
    }
    pub fn name(&self) -> String {
        self.dir
            .file_name()
            .map(|n| n.to_string_lossy().trim_end_matches(".openstudio").to_string())
            .unwrap_or_else(|| "Untitled".into())
    }
}
