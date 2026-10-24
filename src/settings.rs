//! App-wide preferences (not per project): export choices and folder.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Mp4,
    Gif,
    /// Apple ProRes in a .mov, for editing in Final Cut, Resolve or Premiere.
    ProRes,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Gif => "gif",
            Self::ProRes => "mov",
        }
    }
}

/// ProRes flavours, as Screen Studio offers them.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProRes {
    Lt,
    Standard,
    Hq,
    P4444,
}

impl ProRes {
    pub const ALL: [ProRes; 4] = [Self::Lt, Self::Standard, Self::Hq, Self::P4444];
    pub fn label(self) -> &'static str {
        match self {
            Self::Lt => "422 LT",
            Self::Standard => "422",
            Self::Hq => "422 HQ",
            Self::P4444 => "4444",
        }
    }
    /// ffmpeg prores_ks profile number and pixel format.
    pub fn ffmpeg(self) -> (u32, &'static str) {
        match self {
            Self::Lt => (1, "yuv422p10le"),
            Self::Standard => (2, "yuv422p10le"),
            Self::Hq => (3, "yuv422p10le"),
            Self::P4444 => (4, "yuv444p10le"),
        }
    }
    /// Apple's published data rate at 1920x1080, 29.97 fps, in Mbit/s.
    pub fn mbps_1080p30(self) -> f64 {
        match self {
            Self::Lt => 102.0,
            Self::Standard => 147.0,
            Self::Hq => 220.0,
            Self::P4444 => 330.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Studio,
    Social,
    Web,
    WebLow,
}

impl Quality {
    pub const ALL: [Quality; 4] = [Self::Studio, Self::Social, Self::Web, Self::WebLow];
    pub fn label(self) -> &'static str {
        match self {
            Self::Studio => "Studio",
            Self::Social => "Social Media",
            Self::Web => "Web",
            Self::WebLow => "Web (Low)",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Studio => "Highest quality, best for further editing. Compression is almost impossible to notice.",
            Self::Social => "Great quality at a reasonable size. Good for posting online.",
            Self::Web => "Smaller file for embedding in websites and docs.",
            Self::WebLow => "Smallest file. Compression is visible on detailed content.",
        }
    }
    /// Constant quantizer for GPU encoders, close to `x264`'s CRF in quality.
    pub fn gpu_qp(self) -> u32 {
        match self {
            Self::Studio => 16,
            Self::Social => 20,
            Self::Web => 25,
            Self::WebLow => 30,
        }
    }
    /// x264 CRF and preset.
    pub fn x264(self) -> (u32, &'static str) {
        match self {
            Self::Studio => (14, "slow"),
            Self::Social => (18, "medium"),
            Self::Web => (23, "medium"),
            Self::WebLow => (28, "fast"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    File,
    Clipboard,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub export_dir: PathBuf,
    /// Where Save puts projects.
    pub projects_dir: PathBuf,
    pub format: Format,
    pub fps: u32,
    /// None means the recording's native height.
    pub height: Option<u32>,
    pub quality: Quality,
    pub destination: Destination,
    /// ProRes flavour when exporting as ProRes.
    pub prores: ProRes,
    /// Global shortcut that stops a recording.
    pub stop_shortcut: String,
    /// Global shortcut that marks a recording flag (Screen Studio: ⌘⌥⌃F).
    pub flag_shortcut: String,
    /// Zooms are made from clicks in every new project.
    pub auto_zooms: bool,
    /// Dim what isn't being recorded while recording.
    pub highlight_recorded_area: bool,
    /// Seconds counted down before recording starts (0 = off).
    pub countdown: u32,
    /// Webcams record at most this tall (720, 1080 or 2160).
    pub max_camera_height: u32,
    /// After recording, show the quick share widget instead of the editor.
    pub quick_share: bool,
    /// Speaker notes: shown, their text, and the prompter's look.
    pub show_notes: bool,
    pub notes: String,
    /// Prompter scroll speed, pixels per second.
    pub prompter_speed: f32,
    pub notes_opacity: f32,
    pub notes_font_size: f32,
    /// Draw the system cursor into the video instead of re-rendering it.
    pub bake_cursor: bool,
    /// Microphone to record (Pulse source name), or None.
    pub microphone: Option<String>,
    /// Record what the computer plays.
    pub system_audio: bool,
    /// Phone mirroring (Device button).
    pub scrcpy: ScrcpyOptions,
    /// Camera to record (see `record::camera::Source::id`), or None.
    pub camera: Option<String>,
    /// Show the live camera bubble above the recording bar.
    pub camera_preview: bool,
    /// Network adb addresses of phones used before, reconnected on demand.
    pub known_devices: Vec<String>,
    /// Editor preview resolution divisor: 1 (Full), 2 (Half), 4 (Quarter).
    pub preview_quality: u32,
    /// Encode recordings and exports on the GPU when it can (see `hwenc`).
    pub gpu_encoding: bool,
    /// New recordings start with the look of the project edited last.
    pub use_last_project_settings: bool,
    /// Audio waveform on the timeline's clip track.
    pub timeline_waveform: bool,
    /// Cut parts take no room on the timeline.
    pub timeline_fold_cuts: bool,
    /// Scrubber: dragging the playhead plays the sound under it.
    pub scrubber: bool,
}

/// scrcpy settings for the Device button.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ScrcpyOptions {
    /// Longest side in pixels; 0 keeps the phone's resolution.
    pub max_size: u32,
    pub video_bitrate_mbps: u32,
    /// 0 lets the phone decide.
    pub max_fps: u32,
    /// "h264", "h265" or "av1".
    pub codec: String,
    /// Play the phone's sound on the computer (recordable as system audio).
    pub audio: bool,
    pub stay_awake: bool,
    pub turn_screen_off: bool,
    pub show_touches: bool,
    /// Mirror only; mouse and keyboard don't reach the phone.
    pub view_only: bool,
}

impl Default for ScrcpyOptions {
    fn default() -> Self {
        Self {
            max_size: 0,
            video_bitrate_mbps: 16,
            max_fps: 60,
            codec: "h264".into(),
            audio: true,
            stay_awake: true,
            turn_screen_off: false,
            show_touches: false,
            view_only: false,
        }
    }
}

impl ScrcpyOptions {
    pub fn args(&self) -> Vec<String> {
        let mut a = vec![format!("--video-bit-rate={}M", self.video_bitrate_mbps), format!("--video-codec={}", self.codec)];
        if self.max_size > 0 {
            a.push(format!("--max-size={}", self.max_size));
        }
        if self.max_fps > 0 {
            a.push(format!("--max-fps={}", self.max_fps));
        }
        for (on, flag) in [
            (!self.audio, "--no-audio"),
            (self.stay_awake, "--stay-awake"),
            (self.turn_screen_off, "--turn-screen-off"),
            (self.show_touches, "--show-touches"),
            (self.view_only, "--no-control"),
        ] {
            if on {
                a.push(flag.into());
            }
        }
        a
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            export_dir: crate::util::recordings_dir(),
            projects_dir: crate::util::recordings_dir(),
            format: Format::Mp4,
            fps: 60,
            height: None,
            quality: Quality::Social,
            destination: Destination::File,
            prores: ProRes::Hq,
            stop_shortcut: "Ctrl+Alt+Enter".into(),
            flag_shortcut: "Ctrl+Alt+F".into(),
            auto_zooms: true,
            highlight_recorded_area: true,
            countdown: 0,
            max_camera_height: 1080,
            quick_share: true,
            show_notes: false,
            notes: String::new(),
            prompter_speed: 40.0,
            notes_opacity: 1.0,
            notes_font_size: 20.0,
            bake_cursor: false,
            microphone: None,
            system_audio: false,
            scrcpy: ScrcpyOptions::default(),
            camera: None,
            camera_preview: true,
            known_devices: vec![],
            preview_quality: 1,
            gpu_encoding: true,
            use_last_project_settings: true,
            timeline_waveform: true,
            timeline_fold_cuts: true,
            scrubber: false,
        }
    }
}

fn path() -> PathBuf {
    crate::util::config_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::util::config_dir());
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), t);
        }
    }
}
