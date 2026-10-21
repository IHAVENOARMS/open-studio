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
    /// Remove the cursor completely.
    pub hidden: bool,
    /// Near the end, move the cursor back to where it started.
    pub loop_position: bool,
    /// Seconds before the end at which the loop starts.
    pub loop_duration: f64,
    pub click_visual: ClickVisual,
    pub click_sound: ClickSound,
    pub click_volume: f64,
    /// Tilt while moving, 0..1.
    pub rotate: f64,
    /// Freeze the cursor this many seconds before the end (0 = off).
    pub stop_at_end: f64,
    /// Filter out small involuntary movements.
    pub remove_shakes: bool,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self { size: 1.5, smooth: true, speed: AnimationSpeed::Mellow, hide_when_idle: false, click_effect: true,
            always_default: false,
            theme: None,
            hidden: false,
            loop_position: false,
            loop_duration: 1.0,
            click_visual: ClickVisual::None,
            click_sound: ClickSound::None,
            click_volume: 0.6,
            rotate: 0.0,
            stop_at_end: 0.0,
            remove_shakes: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicMode {
    Standard,
    NoiseReduction,
    Podcast,
    Loud,
    Off,
}

impl MicMode {
    pub const ALL: [MicMode; 5] = [Self::Standard, Self::NoiseReduction, Self::Podcast, Self::Loud, Self::Off];
    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::NoiseReduction => "Noise reduction",
            Self::Podcast => "Podcast",
            Self::Loud => "Loud",
            Self::Off => "Off",
        }
    }
    pub fn help(self) -> &'static str {
        match self {
            Self::Standard => "Your microphone as recorded, with gentle level balancing.",
            Self::NoiseReduction => "Removes steady background noise like fans and hum.",
            Self::Podcast => "Noise reduction, compression and EQ for a close, broadcast sound.",
            Self::Loud => "Boosts quiet microphones and evens out the volume.",
            Self::Off => "Microphone audio is not included.",
        }
    }
    /// ffmpeg filter chain applied to the microphone track.
    pub fn filter(self) -> &'static str {
        match self {
            Self::Standard => "dynaudnorm=f=250:g=15:m=6",
            Self::NoiseReduction => "afftdn=nr=18:nf=-38,dynaudnorm=f=250:g=15:m=6",
            Self::Podcast => "afftdn=nr=16:nf=-40,highpass=f=80,acompressor=threshold=-20dB:ratio=3:attack=8:release=120,equalizer=f=3500:t=q:w=1.2:g=3,alimiter=limit=0.95",
            Self::Loud => "dynaudnorm=f=150:g=9:m=20,volume=1.6,alimiter=limit=0.95",
            Self::Off => "volume=0",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AudioSettings {
    /// 0..2 gain per track kind.
    pub microphone_volume: f64,
    pub system_volume: f64,
    /// Mutes everything.
    pub mute: bool,
    pub mic_mode: MicMode,
    pub mute_microphone: bool,
    pub mute_system: bool,
    /// Background music file (absolute path), looped to the video length.
    pub music: Option<String>,
    pub music_volume: f64,
    pub mute_music: bool,
    /// Kept for older projects; superseded by `mic_mode`.
    pub noise_reduction: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            microphone_volume: 1.0,
            system_volume: 1.0,
            mute: false,
            mic_mode: MicMode::Standard,
            mute_microphone: false,
            mute_system: false,
            music: None,
            music_volume: 0.35,
            mute_music: false,
            noise_reduction: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowStyle {
    Classic,
    Directional,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Inset {
    pub enabled: bool,
    /// Frame width as a fraction of the screen height.
    pub size: f64,
    pub color: [f32; 3],
    pub opacity: f64,
    /// Where the extra space goes: -1..1 per axis (0 = centred).
    pub offset: [f64; 2],
}

impl Default for Inset {
    fn default() -> Self {
        Self { enabled: false, size: 0.03, color: [0.0, 0.0, 0.0], opacity: 1.0, offset: [0.0, 0.0] }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickVisual {
    None,
    Circle,
    Ripple,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickSound {
    None,
    MagicMouse,
    MxMaster,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenAnimation {
    /// Settles quickly, easy to follow and read.
    Focused,
    /// Longer, softer camera moves.
    Smooth,
}

impl ScreenAnimation {
    pub fn curve(self) -> ZoomCurve {
        match self {
            Self::Focused => ZoomCurve::Bezier { x1: 0.23, y1: 1.0, x2: 0.32, y2: 1.0, duration: 0.55 },
            Self::Smooth => ZoomCurve::SMOOTH,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ShortcutOverlay {
    pub show: bool,
    /// Label size multiplier.
    pub size: f64,
    /// Also show single-key presses (not only combinations).
    pub single_keys: bool,
    /// Indices into the recording's detected shortcuts that are hidden.
    pub disabled: Vec<usize>,
}

impl Default for ShortcutOverlay {
    fn default() -> Self {
        Self { show: true, size: 1.0, single_keys: false, disabled: vec![] }
    }
}

/// A removed stretch of the recording, in recording time.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub start: f64,
    pub end: f64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionFont {
    Sans,
    Rounded,
    Serif,
    Mono,
}

impl CaptionFont {
    pub const ALL: [CaptionFont; 4] = [Self::Sans, Self::Rounded, Self::Serif, Self::Mono];
    pub fn label(self) -> &'static str {
        match self {
            Self::Sans => "Default",
            Self::Rounded => "Rounded",
            Self::Serif => "Serif",
            Self::Mono => "Mono",
        }
    }
    /// fontconfig pattern.
    pub fn pattern(self) -> &'static str {
        match self {
            Self::Sans => "SF Pro Display:semibold,Inter:semibold,sans-serif:semibold",
            Self::Rounded => "SF Pro Rounded:semibold,Nunito:semibold,sans-serif:semibold",
            Self::Serif => "New York:semibold,Noto Serif:semibold,serif:semibold",
            Self::Mono => "SF Mono:semibold,JetBrains Mono:semibold,monospace:semibold",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionPosition {
    Bottom,
    Top,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionDisplay {
    LineByLine,
    WordByWord,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionAnimation {
    Appear,
    Fade,
    Slide,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CaptionSettings {
    pub show: bool,
    /// Size multiplier.
    pub size: f64,
    pub font: CaptionFont,
    /// Colour of the words already spoken (sRGB 0..1); upcoming words are dimmer.
    pub color: [f32; 3],
    pub position: CaptionPosition,
    pub display: CaptionDisplay,
    pub animation: CaptionAnimation,
    /// Words removed from the captions only (the video keeps them).
    pub hidden_words: Vec<usize>,
    /// Corrected spellings by word index.
    pub word_edits: std::collections::BTreeMap<usize, String>,
    /// Transcription settings.
    pub model: String,
    pub language: String,
    pub prompt: String,
    /// Transcript editor: keep cut words visible, struck through.
    pub show_deleted: bool,
}

impl Default for CaptionSettings {
    fn default() -> Self {
        Self {
            show: true,
            size: 1.0,
            font: CaptionFont::Sans,
            color: [1.0, 1.0, 1.0],
            position: CaptionPosition::Bottom,
            display: CaptionDisplay::LineByLine,
            animation: CaptionAnimation::Fade,
            hidden_words: vec![],
            word_edits: Default::default(),
            model: "base".into(),
            language: String::new(),
            prompt: String::new(),
            show_deleted: true,
        }
    }
}

impl CaptionSettings {
    pub fn word_text<'a>(&'a self, i: usize, original: &'a str) -> &'a str {
        self.word_edits.get(&i).map(|s| s.as_str()).unwrap_or(original)
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
    pub audio: AudioSettings,
    pub inset: Inset,
    pub shadow_style: ShadowStyle,
    /// Directional shadow: angle in degrees (0 = light from the top), distance
    /// and softness 0..1.
    pub shadow_angle: f64,
    pub shadow_distance: f64,
    pub shadow_softness: f64,
    pub screen_animation: ScreenAnimation,
    /// Zoom changes happen instantly, without animation.
    pub instant_animation: bool,
    pub shortcuts: ShortcutOverlay,
    /// Removed stretches, sorted and non-overlapping.
    pub cuts: Vec<Cut>,
    pub captions: CaptionSettings,
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
            audio: AudioSettings::default(),
            inset: Inset::default(),
            shadow_style: ShadowStyle::Classic,
            shadow_angle: 0.0,
            shadow_distance: 0.4,
            shadow_softness: 0.6,
            screen_animation: ScreenAnimation::Smooth,
            instant_animation: false,
            shortcuts: ShortcutOverlay::default(),
            cuts: vec![],
            captions: CaptionSettings::default(),
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
        if rec.source.kind == "device" {
            // Phone recordings: taps as circles instead of a desktop arrow,
            // and rounder corners like a phone screen.
            cfg.cursor.hidden = true;
            cfg.cursor.click_visual = ClickVisual::Circle;
            cfg.radius = 48.0;
        }
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

    /// Parts of the recording that make it into the video: the trim range
    /// minus the cuts.
    pub fn kept_ranges(&self, duration: f64) -> Vec<(f64, f64)> {
        let (s, e) = self.trim_range(duration);
        let mut out = vec![];
        let mut t = s;
        for c in &self.cuts {
            if c.end <= t || c.start >= e {
                continue;
            }
            if c.start > t {
                out.push((t, c.start));
            }
            t = t.max(c.end);
        }
        if t < e {
            out.push((t, e));
        }
        out.retain(|(a, b)| b - a > 1e-4);
        out
    }

    /// Edited-video time of recording time `t`, or None when it is cut or trimmed.
    pub fn output_time(&self, t: f64, duration: f64) -> Option<f64> {
        let mut acc = 0.0;
        for (a, b) in self.kept_ranges(duration) {
            if t < a - 1e-6 {
                return None;
            }
            if t <= b + 1e-6 {
                return Some(acc + (t - a).max(0.0));
            }
            acc += b - a;
        }
        None
    }

    /// The cut containing `t`, if any.
    pub fn cut_at(&self, t: f64) -> Option<Cut> {
        self.cuts.iter().copied().find(|c| t >= c.start && t < c.end)
    }

    /// True when most of `start..end` is cut out.
    pub fn is_cut(&self, start: f64, end: f64) -> bool {
        self.cut_at((start + end) * 0.5).is_some()
    }

    /// Adds a cut, merging overlaps.
    pub fn add_cut(&mut self, start: f64, end: f64) {
        if end - start < 1e-3 {
            return;
        }
        self.cuts.push(Cut { start, end });
        self.cuts.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut merged: Vec<Cut> = vec![];
        for c in self.cuts.drain(..) {
            match merged.last_mut() {
                Some(m) if c.start <= m.end + 1e-3 => m.end = m.end.max(c.end),
                _ => merged.push(c),
            }
        }
        self.cuts = merged;
    }

    /// Removes cuts overlapping `start..end` (restores that part).
    pub fn restore(&mut self, start: f64, end: f64) {
        let mut out = vec![];
        for c in self.cuts.drain(..) {
            if c.end <= start || c.start >= end {
                out.push(c);
                continue;
            }
            if c.start < start {
                out.push(Cut { start: c.start, end: start });
            }
            if c.end > end {
                out.push(Cut { start: end, end: c.end });
            }
        }
        self.cuts = out;
    }
}
