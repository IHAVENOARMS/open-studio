//! User edits, persisted as project.json next to the recording.

use crate::project::{PROJECT_FILE, Recording};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum ZoomFocus {
    /// Camera follows the cursor.
    Auto,
    /// Fixed point, normalized video coordinates.
    Manual { x: f64, y: f64 },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ZoomSegment {
    pub start: f64,
    pub end: f64,
    pub scale: f64,
    pub focus: ZoomFocus,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationSpeed {
    Slow,
    Mellow,
    Quick,
    Rapid,
}

impl AnimationSpeed {
    pub const ALL: [AnimationSpeed; 4] = [Self::Slow, Self::Mellow, Self::Quick, Self::Rapid];
    pub fn label(self) -> &'static str {
        match self {
            Self::Slow => "Slow",
            Self::Mellow => "Mellow",
            Self::Quick => "Quick",
            Self::Rapid => "Rapid",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Background {
    /// Wallpaper id from `assets/wallpapers.json`.
    Wallpaper(String),
    /// Index into `render::background::PRESETS`.
    Gradient(usize),
    Color([f32; 3]),
    /// Absolute path to a user-chosen image.
    Image(String),
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aspect {
    Auto,
    Wide,
    Vertical,
    Square,
    Classic,
}

impl Aspect {
    pub const ALL: [Aspect; 5] = [Self::Auto, Self::Wide, Self::Vertical, Self::Square, Self::Classic];
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Wide => "16:9",
            Self::Vertical => "9:16",
            Self::Square => "1:1",
            Self::Classic => "4:3",
        }
    }
    pub fn ratio(self, video_ratio: f64) -> f64 {
        match self {
            Self::Auto => video_ratio,
            Self::Wide => 16.0 / 9.0,
            Self::Vertical => 9.0 / 16.0,
            Self::Square => 1.0,
            Self::Classic => 4.0 / 3.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CursorStyle {
    /// Multiplier on the natural cursor size.
    pub size: f64,
    pub smooth: bool,
    pub speed: AnimationSpeed,
    pub hide_when_idle: bool,
    pub click_effect: bool,
    /// Ignore recorded cursor shapes and always draw the arrow.
    pub always_default: bool,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self { size: 1.5, smooth: true, speed: AnimationSpeed::Mellow, hide_when_idle: false, click_effect: true, always_default: false }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct EditConfig {
    pub zoom_segments: Vec<ZoomSegment>,
    pub zoom_speed: AnimationSpeed,
    pub default_zoom: f64,
    pub background: Background,
    /// 0..1, blur applied to wallpaper and image backgrounds.
    pub background_blur: f64,
    /// Fraction of the shorter canvas side left around the video.
    pub padding: f64,
    /// Corner radius in output pixels at 1080p.
    pub radius: f64,
    /// 0..1
    pub shadow: f64,
    pub cursor: CursorStyle,
    pub aspect: Aspect,
    pub trim_start: f64,
    pub trim_end: Option<f64>,
    pub motion_blur: bool,
    /// 0..1, radial blur while the camera zooms.
    pub zoom_blur: f64,
    /// 0..1, directional blur while the camera pans.
    pub pan_blur: f64,
}

impl Default for EditConfig {
    fn default() -> Self {
        Self {
            zoom_segments: vec![],
            zoom_speed: AnimationSpeed::Mellow,
            default_zoom: 2.0,
            background: default_background(),
            background_blur: 0.0,
            padding: 0.08,
            radius: 12.0,
            shadow: 0.6,
            cursor: CursorStyle::default(),
            aspect: Aspect::Auto,
            trim_start: 0.0,
            trim_end: None,
            motion_blur: true,
            zoom_blur: 0.5,
            pan_blur: 0.25,
        }
    }
}

/// First installed wallpaper, falling back to a built-in gradient.
fn default_background() -> Background {
    crate::wallpapers::manifest()
        .iter()
        .find(|w| crate::wallpapers::installed(&w.id))
        .map(|w| Background::Wallpaper(w.id.clone()))
        .unwrap_or(Background::Gradient(0))
}

impl EditConfig {
    /// Loads project.json, or builds a fresh config with automatic zooms.
    pub fn load_or_create(dir: &Path, rec: &Recording) -> Self {
        if let Ok(text) = std::fs::read_to_string(dir.join(PROJECT_FILE)) {
            if let Ok(cfg) = serde_json::from_str(&text) {
                return cfg;
            }
        }
        let mut cfg = Self::default();
        cfg.zoom_segments = crate::engine::auto_zoom_segments(rec, cfg.default_zoom);
        cfg
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::write(dir.join(PROJECT_FILE), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn trim_range(&self, duration: f64) -> (f64, f64) {
        let end = self.trim_end.unwrap_or(duration).min(duration);
        (self.trim_start.clamp(0.0, end), end)
    }
}
