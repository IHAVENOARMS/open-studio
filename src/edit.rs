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
    /// A camera zoom, or a Glass Loupe lens that magnifies a spot in place.
    #[serde(default)]
    pub kind: ZoomKind,
    /// Loupe lens size multiplier.
    #[serde(default = "one")]
    pub size: f64,
}

fn one() -> f64 {
    1.0
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ZoomKind {
    #[default]
    Zoom,
    Loupe,
}

impl ZoomSegment {
    pub fn new(start: f64, end: f64, scale: f64, kind: ZoomKind) -> Self {
        Self { start, end, scale, focus: ZoomFocus::Auto, kind, size: 1.0 }
    }
}

/// A phone drawn around a device recording (Screen Studio's device frames,
/// drawn here rather than photographed).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct DeviceFrame {
    pub enabled: bool,
    pub color: FrameColor,
}

impl Default for DeviceFrame {
    fn default() -> Self {
        Self { enabled: true, color: FrameColor::Black }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameColor {
    Black,
    Silver,
    Titanium,
    Blue,
}

impl FrameColor {
    pub const ALL: [FrameColor; 4] = [Self::Black, Self::Silver, Self::Titanium, Self::Blue];
    pub fn name(self) -> &'static str {
        match self {
            Self::Black => "Black",
            Self::Silver => "Silver",
            Self::Titanium => "Titanium",
            Self::Blue => "Blue",
        }
    }
    /// sRGB body colour.
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Self::Black => [0x1c, 0x1c, 0x1f],
            Self::Silver => [0xd9, 0xdb, 0xde],
            Self::Titanium => [0x8e, 0x88, 0x80],
            Self::Blue => [0x2f, 0x3e, 0x5c],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskKind {
    /// "Sensitive data": the region is blurred.
    Blur,
    /// Everything outside the region is dimmed.
    Highlight,
    /// Hide sensitive data: filled with the colour around it.
    Cover,
    /// Hide faces: an oval blur.
    Face,
}

/// Automatic covers (see `privacy`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct PrivacySettings {
    /// Cover emails, phone and card numbers, keys and `phrases` on screen.
    pub hide_sensitive: bool,
    /// Blur faces on screen.
    pub hide_faces: bool,
    /// Custom phrases to hide too (names, clients).
    pub phrases: Vec<String>,
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
    Tall,
}

impl Aspect {
    pub const ALL: [Aspect; 6] = [Self::Auto, Self::Wide, Self::Vertical, Self::Square, Self::Classic, Self::Tall];
    /// Screen Studio's names: "Wide" with "16:9" beside it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Wide => "Wide",
            Self::Vertical => "Vertical",
            Self::Square => "Square",
            Self::Classic => "Classic",
            Self::Tall => "Tall",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "",
            Self::Wide => "16:9",
            Self::Vertical => "9:16",
            Self::Square => "1:1",
            Self::Classic => "4:3",
            Self::Tall => "3:4",
        }
    }
    pub fn ratio(self, video_ratio: f64) -> f64 {
        match self {
            Self::Auto => video_ratio,
            Self::Wide => 16.0 / 9.0,
            Self::Vertical => 9.0 / 16.0,
            Self::Square => 1.0,
            Self::Classic => 4.0 / 3.0,
            Self::Tall => 3.0 / 4.0,
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
    /// Draw a soft touch circle instead of the pointer (phone recordings).
    pub circle: bool,
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
            circle: false,
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
    /// A mirrored phone's sound (its own track in device recordings).
    pub device_volume: f64,
    pub mute_device: bool,
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
            device_volume: 1.0,
            mute_device: false,
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
    /// The shortcuts track is shown on the timeline.
    pub timeline: bool,
    /// Combinations always shown, even single keys or Shift ones
    /// ("Enter", "Shift+Tab"); names as `shortcuts::key_name` gives them.
    pub custom: Vec<String>,
    pub style: KeycapStyle,
    /// Dark plate behind the row of keys.
    pub backdrop: bool,
    /// Column and row on a 3×3 grid; [1, 2] is bottom centre.
    pub position: [u8; 2],
}

impl Default for ShortcutOverlay {
    fn default() -> Self {
        Self { show: true, size: 1.0, single_keys: false, disabled: vec![], timeline: false, custom: vec![], style: KeycapStyle::Dark, backdrop: true, position: [1, 2] }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeycapStyle {
    Dark,
    Light,
    /// Transparent keys with a white outline.
    Outline,
}

/// Camera placement for a stretch of the video (the Layouts track).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraLayoutKind {
    /// Camera over the screen at its position.
    Default,
    /// Screen and camera side by side (stacked on portrait canvases).
    Split,
    /// Camera fills the frame.
    Fullscreen,
    /// Screen only.
    Hide,
    /// Just the person (background removed), in front of the screen.
    Cutout,
}

impl CameraLayoutKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Split => "Split screen",
            Self::Fullscreen => "Fullscreen",
            Self::Hide => "Hide camera",
            Self::Cutout => "Cutout",
        }
    }
}

/// The project-wide layout ("Default layout" cards).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraDefaultLayout {
    Overlay,
    Split,
    /// Background removed, person in front of the screen.
    Cutout,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraAspect {
    Original,
    Square,
    Portrait,
    Landscape,
}

impl CameraAspect {
    pub const ALL: [CameraAspect; 4] = [Self::Original, Self::Square, Self::Portrait, Self::Landscape];
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "Original",
            Self::Square => "1:1",
            Self::Portrait => "3:4",
            Self::Landscape => "16:9",
        }
    }
    /// Width / height, given the camera's own aspect.
    pub fn ratio(self, source: f64) -> f64 {
        match self {
            Self::Original => source,
            Self::Square => 1.0,
            Self::Portrait => 0.75,
            Self::Landscape => 16.0 / 9.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CameraSettings {
    /// Hide the camera for the whole video.
    pub hidden: bool,
    pub layout: CameraDefaultLayout,
    /// Column and row on the 3x3 position grid (the centre is not used).
    pub position: [u8; 2],
    /// Height as a fraction of the canvas height, when not zoomed in.
    pub size: f64,
    /// Shrink the camera while the screen is zoomed in.
    pub shrink_on_zoom: bool,
    pub zoom_size: f64,
    pub aspect: CameraAspect,
    /// 0 = square corners, 1 = fully round.
    pub roundness: f64,
    pub mirror: bool,
    pub shadow: f64,
    /// Lip-sync correction in seconds; positive shows the camera later.
    pub delay: f64,
    /// Part of the camera picture kept: x, y, w, h (normalized).
    pub crop: [f64; 4],
    /// Height of the cutout (Camera cutout layout) as a fraction of the canvas.
    pub cutout_size: f64,
    /// Keep the face centred by moving the crop (uses face_track.json).
    pub face_tracking: bool,
    /// Hide the camera while nobody is speaking.
    pub hide_during_silence: bool,
    /// Colour grading: a built-in look, its strength, or a .cube LUT file.
    pub look: crate::render::looks::Look,
    pub look_strength: f64,
    pub lut: Option<String>,
}

/// Default camera height (fraction of the canvas) and its height while the
/// screen is zoomed in.
pub const SIZE_DEFAULT: f64 = 0.38;
pub const ZOOM_SIZE_DEFAULT: f64 = 0.30;

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            hidden: false,
            layout: CameraDefaultLayout::Overlay,
            position: [2, 2],
            size: SIZE_DEFAULT,
            shrink_on_zoom: true,
            zoom_size: ZOOM_SIZE_DEFAULT,
            aspect: CameraAspect::Square,
            roundness: 0.45,
            mirror: false,
            shadow: 0.6,
            delay: 0.0,
            crop: [0.0, 0.0, 1.0, 1.0],
            cutout_size: 0.54,
            face_tracking: false,
            hide_during_silence: false,
            look: crate::render::looks::Look::None,
            look_strength: 1.0,
            lut: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct CameraLayoutSegment {
    pub start: f64,
    pub end: f64,
    pub kind: CameraLayoutKind,
    /// Overrides the project position (Default layout only).
    pub position: Option<[u8; 2]>,
    /// Overrides the project size while this layout is active.
    pub custom_size: Option<f64>,
    pub disabled: bool,
}

impl CameraLayoutSegment {
    pub fn new(start: f64, end: f64, kind: CameraLayoutKind) -> Self {
        Self { start, end, kind, position: None, custom_size: None, disabled: false }
    }
}

/// A removed stretch of the recording, in recording time.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub start: f64,
    pub end: f64,
}

/// Settings for a stretch of the clip (a slice, from the timeline's
/// right-click menu): playback speed, audio volume and cursor.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct SliceProps {
    /// Recording time the settings apply to.
    pub start: f64,
    pub end: f64,
    pub speed: f64,
    /// Microphone volume multiplier.
    pub volume: f64,
    pub system_volume: f64,
    /// External device (phone) audio volume multiplier.
    pub device_volume: f64,
    pub hide_cursor: bool,
    /// Follow the real cursor exactly (no smoothing), e.g. for dropdowns.
    pub raw_cursor: bool,
}

impl Default for SliceProps {
    fn default() -> Self {
        Self { start: 0.0, end: 0.0, speed: 1.0, volume: 1.0, system_volume: 1.0, device_volume: 1.0, hide_cursor: false, raw_cursor: false }
    }
}

impl SliceProps {
    /// Same settings, ignoring where they apply.
    pub fn same(&self, o: &SliceProps) -> bool {
        SliceProps { start: 0.0, end: 0.0, ..*self } == SliceProps { start: 0.0, end: 0.0, ..*o }
    }
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
    /// fontconfig pattern for Arabic words, which the Latin fonts lack.
    pub fn arabic_pattern(self) -> &'static str {
        match self {
            Self::Serif => "Noto Naskh Arabic:semibold,Noto Sans Arabic:semibold,serif:semibold:lang=ar",
            _ => "Noto Sans Arabic:semibold,Vazirmatn:semibold,sans-serif:semibold:lang=ar",
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
    /// Points where the clip is split into slices (recording time, sorted).
    pub splits: Vec<f64>,
    /// Per-slice speed, volume and cursor settings, sorted, non-overlapping.
    pub slices: Vec<SliceProps>,
    /// Typing suggestions removed (their start times; see `typing`).
    pub typing_removed: Vec<f64>,
    /// Hide sensitive data and Hide faces.
    pub privacy: PrivacySettings,
    /// Phone frame around device recordings.
    pub device_frame: DeviceFrame,
    /// Part of the screen recording kept (Crop): x, y, w, h as fractions.
    pub crop: [f64; 4],
    pub captions: CaptionSettings,
    pub camera: CameraSettings,
    /// Layout changes over time (the Layouts track), sorted.
    pub camera_layouts: Vec<CameraLayoutSegment>,
    /// 0..1, radial blur while the camera zooms.
    pub zoom_blur: f64,
    /// 0..1, directional blur while the camera pans.
    pub pan_blur: f64,
}

impl EditConfig {
    /// Copies the look from `preset` (background, cursor, camera, captions
    /// style, audio, animations…), keeping what belongs to this recording:
    /// zooms, masks, cuts, trim, crop, the layouts track and transcript fixes.
    pub fn apply_preset(&mut self, preset: &EditConfig) {
        let keep = std::mem::replace(self, preset.clone());
        self.zoom_segments = keep.zoom_segments;
        self.masks = keep.masks;
        self.cuts = keep.cuts;
        self.splits = keep.splits;
        self.slices = keep.slices;
        self.typing_removed = keep.typing_removed;
        self.trim_start = keep.trim_start;
        self.trim_end = keep.trim_end;
        self.crop = keep.crop;
        self.camera_layouts = keep.camera_layouts;
        self.captions.hidden_words = keep.captions.hidden_words;
        self.captions.word_edits = keep.captions.word_edits;
    }

    /// True when applying `preset` would change nothing.
    pub fn matches_preset(&self, preset: &EditConfig) -> bool {
        let mut applied = self.clone();
        applied.apply_preset(preset);
        applied == *self
    }
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
            splits: vec![],
            slices: vec![],
            typing_removed: vec![],
            privacy: PrivacySettings::default(),
            device_frame: DeviceFrame::default(),
            crop: [0.0, 0.0, 1.0, 1.0],
            captions: CaptionSettings::default(),
            camera: CameraSettings::default(),
            camera_layouts: vec![],
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
            if let Ok(mut cfg) = serde_json::from_str::<Self>(&text) {
                // Projects saved with earlier camera defaults get the current ones.
                let c = &mut cfg.camera;
                let old = |s: f64, z: f64| (c.size - s).abs() < 1e-9 && (c.zoom_size - z).abs() < 1e-9;
                if old(0.3, 0.2) || old(0.38, 0.33) {
                    c.size = SIZE_DEFAULT;
                    c.zoom_size = ZOOM_SIZE_DEFAULT;
                }
                return cfg;
            }
        }
        let mut cfg = Self::default();
        let device = rec.source.kind == "device";
        // Start from the look of the last project of the same kind (desktop
        // or phone), when that's on.
        let last = if !cfg!(test) && crate::settings::Settings::load().use_last_project_settings { crate::presets::last(device) } else { None };
        if let Some(last) = &last {
            cfg.apply_preset(last);
        } else if device {
            // Phone recordings: a touch circle instead of a desktop arrow,
            // and rounder corners like a phone screen.
            cfg.cursor.circle = true;
            cfg.cursor.click_visual = ClickVisual::Circle;
            cfg.radius = 48.0;
        }
        // "Create zooms automatically" (Settings), on unless turned off.
        let auto_zooms = cfg!(test) || crate::settings::Settings::load().auto_zooms;
        if auto_zooms {
            cfg.zoom_segments = crate::engine::auto_zoom_segments(rec, cfg.default_zoom);
        }
        // The phone's sound plays through the speakers, so system audio has
        // it too: keep only the clean device track in the mix.
        if device && rec.audio.iter().any(|a| a.kind == "device") {
            cfg.audio.mute_system = true;
        }
        // Keep these starting settings with the recording: opening it again
        // before it's saved mustn't rebuild them from whatever project was
        // edited since.
        if !cfg!(test) && dir.join(crate::project::RECORDING_FILE).exists() {
            let _ = cfg.save(dir);
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
        for (a, b, speed) in self.segments(duration) {
            if t < a - 1e-6 {
                return None;
            }
            if t <= b + 1e-6 {
                return Some(acc + (t - a).max(0.0) / speed);
            }
            acc += (b - a) / speed;
        }
        None
    }

    /// The clip's slices: kept ranges split at the split points.
    pub fn slice_ranges(&self, duration: f64) -> Vec<(f64, f64)> {
        let mut out = vec![];
        for (a, b) in self.kept_ranges(duration) {
            let mut t = a;
            for &s in self.splits.iter().filter(|&&s| s > a + 1e-3 && s < b - 1e-3) {
                out.push((t, s));
                t = s;
            }
            out.push((t, b));
        }
        out
    }

    /// Kept stretches with one speed each: (start, end, speed).
    pub fn segments(&self, duration: f64) -> Vec<(f64, f64, f64)> {
        let mut out = vec![];
        for (a, b) in self.kept_ranges(duration) {
            let mut edges: Vec<f64> = self.slices.iter().flat_map(|p| [p.start, p.end]).filter(|&x| x > a + 1e-4 && x < b - 1e-4).collect();
            edges.sort_by(f64::total_cmp);
            let mut t = a;
            for e in edges.into_iter().chain([b]) {
                if e - t > 1e-4 {
                    out.push((t, e, self.slice_props((t + e) * 0.5).speed.clamp(0.25, 16.0)));
                }
                t = e;
            }
        }
        out
    }

    /// Settings in force at recording time `t`.
    pub fn slice_props(&self, t: f64) -> SliceProps {
        self.slices.iter().copied().find(|p| t >= p.start && t < p.end).unwrap_or_default()
    }

    /// Changes the settings for `start..end`, starting from those at its middle.
    pub fn set_slice_props(&mut self, start: f64, end: f64, f: impl FnOnce(&mut SliceProps)) {
        let mut p = self.slice_props((start + end) * 0.5);
        f(&mut p);
        p.start = start;
        p.end = end;
        let mut out: Vec<SliceProps> = vec![];
        for q in self.slices.drain(..) {
            if q.end <= start || q.start >= end {
                out.push(q);
                continue;
            }
            if q.start < start {
                out.push(SliceProps { end: start, ..q });
            }
            if q.end > end {
                out.push(SliceProps { start: end, ..q });
            }
        }
        if !p.same(&SliceProps::default()) {
            out.push(p);
        }
        out.sort_by(|a, b| a.start.total_cmp(&b.start));
        self.slices = out;
    }

    /// Splits the clip at `t` (ignored outside the kept video or on a split).
    pub fn split_at(&mut self, t: f64, duration: f64) -> bool {
        let inside = self.kept_ranges(duration).iter().any(|&(a, b)| t > a + 0.05 && t < b - 0.05);
        if !inside || self.splits.iter().any(|s| (s - t).abs() < 0.05) {
            return false;
        }
        self.splits.push(t);
        self.splits.sort_by(f64::total_cmp);
        true
    }

    /// Joins the slices meeting at `t`; the merged slice takes the settings
    /// of the one at `keep`.
    pub fn merge_at(&mut self, t: f64, keep: f64, duration: f64) {
        let slices = self.slice_ranges(duration);
        let (a, b) = (
            slices.iter().find(|s| (s.1 - t).abs() < 1e-3).map_or(t, |s| s.0),
            slices.iter().find(|s| (s.0 - t).abs() < 1e-3).map_or(t, |s| s.1),
        );
        self.splits.retain(|s| (s - t).abs() > 1e-3);
        let props = self.slice_props(keep);
        self.set_slice_props(a, b, |p| *p = props);
    }

    /// Undoes every trim, cut and split, and clears slice settings.
    pub fn reset_slices(&mut self) {
        self.cuts.clear();
        self.splits.clear();
        self.slices.clear();
        self.trim_start = 0.0;
        self.trim_end = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_carry_the_look_not_the_timeline() {
        let mut look = EditConfig::default();
        look.padding = 0.2;
        look.camera.size = 0.5;
        look.zoom_segments.clear();
        let mut project = EditConfig::default();
        project.cuts.push(Cut { start: 1.0, end: 2.0 });
        project.crop = [0.1, 0.1, 0.8, 0.8];
        assert!(!project.matches_preset(&look));
        project.apply_preset(&look);
        assert_eq!(project.padding, 0.2);
        assert_eq!(project.camera.size, 0.5);
        // The recording's own edits stay.
        assert_eq!(project.cuts.len(), 1);
        assert_eq!(project.crop, [0.1, 0.1, 0.8, 0.8]);
        assert!(project.matches_preset(&look));
    }

    #[test]
    fn slices_split_merge_and_speed() {
        let mut e = EditConfig::default();
        let d = 10.0;
        assert!(e.split_at(4.0, d));
        assert!(!e.split_at(4.01, d), "too close to an existing split");
        e.add_cut(7.0, 8.0);
        assert_eq!(e.slice_ranges(d), vec![(0.0, 4.0), (4.0, 7.0), (8.0, 10.0)]);
        // 2x on the middle slice: 3 s become 1.5 s.
        e.set_slice_props(4.0, 7.0, |p| p.speed = 2.0);
        assert_eq!(e.segments(d), vec![(0.0, 4.0, 1.0), (4.0, 7.0, 2.0), (8.0, 10.0, 1.0)]);
        assert_eq!(e.output_time(6.0, d), Some(5.0));
        assert_eq!(e.output_time(9.0, d), Some(6.5));
        assert_eq!(e.output_time(7.5, d), None);
        // Merging keeps the settings of the slice it was asked from.
        e.merge_at(4.0, 2.0, d);
        assert!(e.splits.is_empty());
        assert_eq!(e.slice_props(5.0).speed, 1.0);
        e.set_slice_props(0.0, 7.0, |p| p.hide_cursor = true);
        assert!(e.slice_props(3.0).hide_cursor && !e.slice_props(9.0).hide_cursor);
        e.reset_slices();
        assert!(e.cuts.is_empty() && e.slices.is_empty() && e.slice_ranges(d) == vec![(0.0, 10.0)]);
    }
}
