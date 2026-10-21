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
pub enum MaskKind {
    /// "Sensitive data": the region is blurred.
    Blur,
    /// Everything outside the region is dimmed.
    Highlight,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Mask {
    pub start: f64,
    pub end: f64,
    pub kind: MaskKind,
    /// Region in normalized video coordinates: x, y, w, h.
    pub rect: [f64; 4],
    /// Highlight only: how dark the area outside the region gets (0..1).
    pub opacity: f64,
    /// Kept on the timeline but not applied.
    pub disabled: bool,
}

impl Mask {
    pub const DEFAULT_OPACITY: f64 = 0.6;
    pub fn new(start: f64, end: f64) -> Self {
        Self { start, end, kind: MaskKind::Blur, rect: [0.35, 0.4, 0.3, 0.2], opacity: Self::DEFAULT_OPACITY, disabled: false }
    }
}

/// How zoom transitions move.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum ZoomCurve {
    /// Physical spring (Screen Studio's model).
    Spring { tension: f64, friction: f64, mass: f64 },
    /// Cubic bezier easing over a fixed duration (seconds), like CSS
    /// `cubic-bezier(x1, y1, x2, y2)`.
    Bezier { x1: f64, y1: f64, x2: f64, y2: f64, duration: f64 },
}

impl ZoomCurve {
    /// The spring behind a speed preset.
    pub fn from_speed(speed: AnimationSpeed) -> Self {
        let f: f64 = match speed {
            AnimationSpeed::Slow => 0.45,
            AnimationSpeed::Mellow => 1.0,
            AnimationSpeed::Quick => 2.0,
            AnimationSpeed::Rapid => 4.0,
        };
        Self::Spring { tension: 200.0 * f, friction: 40.0 * f.sqrt(), mass: 2.25 }
    }

    /// Default: the iOS sheet curve (as used in Emil Kowalski's Vaul),
    /// a quick start that glides into place.
    pub const SMOOTH: Self = Self::Bezier { x1: 0.32, y1: 0.72, x2: 0.0, y2: 1.0, duration: 0.75 };

    /// Seconds shown on the curve editor's time axis for a spring.
    pub fn spring_window(tension: f64, friction: f64, mass: f64) -> f64 {
        // Long enough to show the spring settle.
        let w = (tension / mass).sqrt();
        let zeta = friction / (2.0 * (tension * mass).sqrt());
        (6.0 / (w * zeta.clamp(0.15, 1.0))).clamp(0.3, 4.0)
    }
}

/// CSS-style cubic bezier: y for a given x (both 0..1), x1/x2 clamped to 0..1.
pub fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
    let (x1, x2) = (x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0));
    let bez = |a: f64, b: f64, t: f64| 3.0 * a * t * (1.0 - t).powi(2) + 3.0 * b * t * t * (1.0 - t) + t.powi(3);
    let x = x.clamp(0.0, 1.0);
    // Solve bez_x(t) = x by bisection (monotonic since x1, x2 in 0..1).
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if bez(x1, x2, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, (lo + hi) / 2.0)
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
    /// XCursor theme directory to draw cursors from; None uses the built-in
    /// vector cursors.
    pub theme: Option<String>,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self { size: 1.5, smooth: true, speed: AnimationSpeed::Mellow, hide_when_idle: false, click_effect: true, always_default: false, theme: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct EditConfig {
    pub zoom_segments: Vec<ZoomSegment>,
    pub masks: Vec<Mask>,
    pub zoom_speed: AnimationSpeed,
    /// Custom curve; None uses the `zoom_speed` preset.
    pub zoom_curve: Option<ZoomCurve>,
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
            masks: vec![],
            zoom_speed: AnimationSpeed::Mellow,
            zoom_curve: Some(ZoomCurve::SMOOTH),
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
