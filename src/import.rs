//! Create project from video, as in Screen Studio: an existing video
//! becomes a project with zooms, backgrounds and the other visual effects.
//! There's no cursor or camera data for it, so those features are off.

use crate::project::{AudioTrack, Events, Recording, Source, RECORDING_FILE, VIDEO_FILE};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
    format: ProbeFormat,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: String,
    #[serde(default)]
    codec_name: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    pix_fmt: String,
    #[serde(default)]
    avg_frame_rate: String,
}

#[derive(Deserialize)]
struct ProbeFormat {
    #[serde(default)]
    duration: String,
}

/// "30000/1001" -> 29.97.
fn rate(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0 && n > 0.0).then_some(n / d)
}

/// Makes a new unsaved project from `video` and returns its folder.
pub fn create_project(video: &Path) -> Result<PathBuf> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(video)
        .output()
        .context("running ffprobe")?;
    if !out.status.success() {
        bail!("not a video ffmpeg can read");
    }
    let probe: Probe = serde_json::from_slice(&out.stdout).context("reading ffprobe output")?;
    let v = probe.streams.iter().find(|s| s.codec_type == "video").ok_or_else(|| anyhow!("the file has no video"))?;
    let has_audio = probe.streams.iter().any(|s| s.codec_type == "audio");
    let fps = rate(&v.avg_frame_rate).unwrap_or(30.0).round().clamp(1.0, 240.0) as u32;
    let stem = video.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Imported video".into());
    let dir = crate::util::unique_path(&crate::util::unsaved_dir(), &stem);
    std::fs::create_dir_all(&dir)?;
    let result = (|| -> Result<Recording> {
        // H.264 in 4:2:0 is kept as it is; anything else is converted so the
        // editor and exporter can seek it quickly.
        let copy = v.codec_name == "h264" && v.pix_fmt == "yuv420p";
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"]).arg(video).args(["-map", "0:v:0", "-an"]);
        if copy {
            cmd.args(["-c:v", "copy"]);
        } else {
            cmd.args(["-c:v", "libx264", "-preset", "medium", "-crf", "16", "-pix_fmt", "yuv420p", "-g", &(fps * 2).to_string()]);
        }
        let ok = cmd.args(["-movflags", "+faststart"]).arg(dir.join(VIDEO_FILE)).status().context("running ffmpeg")?.success();
        if !ok {
            bail!("couldn't copy the video");
        }
        let mut audio = vec![];
        if has_audio {
            let ok = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"])
                .arg(video)
                .args(["-map", "0:a:0", "-ac", "2", "-ar", "48000", "-c:a", "pcm_s16le"])
                .arg(dir.join("system.wav"))
                .status()
                .context("running ffmpeg")?
                .success();
            // Its sound plays as it was: not as a microphone, which would
            // get voice filters.
            if ok {
                audio.push(AudioTrack { file: "system.wav".into(), kind: "system".into(), offset: 0.0 });
            }
        }
        let duration = probe.format.duration.parse::<f64>().ok().filter(|d| *d > 0.0).ok_or_else(|| anyhow!("unknown length"))?;
        Ok(Recording {
            version: 1,
            video: VIDEO_FILE.into(),
            width: v.width,
            height: v.height,
            fps,
            duration,
            // No cursor was recorded: don't draw one.
            cursor_embedded: true,
            source: Source { kind: "import".into(), name: Some(stem.clone()), logical_rect: None },
            events: Events::default(),
            audio,
            camera: None,
            custom_cursors: vec![],
            dir: dir.clone(),
            transcript: None,
        })
    })();
    match result {
        Ok(rec) => {
            rec.save(&dir)?;
            debug_assert!(dir.join(RECORDING_FILE).exists());
            Ok(dir)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rates() {
        assert_eq!(rate("30000/1001").map(|r| (r * 100.0).round()), Some(2997.0));
        assert_eq!(rate("60/1"), Some(60.0));
        assert_eq!(rate("25"), Some(25.0));
        assert_eq!(rate("0/0"), None);
    }

    /// A VP9 WebM with sound becomes a project: H.264 video, the sound as
    /// its own track, the right size, rate and length. Opt-in (encodes
    /// video): `cargo test -- --ignored imports_a_video`.
    #[test]
    #[ignore]
    fn imports_a_video() {
        let tmp = std::env::temp_dir().join("open-studio-import-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.join("data")) };
        let src = tmp.join("demo clip.webm");
        assert!(Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=640x360:r=25:d=2", "-f", "lavfi", "-i", "sine=duration=2"])
            .args(["-c:v", "libvpx-vp9", "-deadline", "realtime", "-c:a", "libopus", "-shortest"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let dir = create_project(&src).unwrap();
        assert!(dir.starts_with(tmp.join("data")), "made in the unsaved folder: {}", dir.display());
        assert_eq!(dir.file_name().unwrap(), "demo clip");
        let rec = Recording::load(&dir).unwrap();
        assert_eq!((rec.width, rec.height, rec.fps), (640, 360, 25));
        assert!((rec.duration - 2.0).abs() < 0.1);
        assert!(rec.cursor_embedded && rec.source.kind == "import");
        assert_eq!(rec.audio.len(), 1);
        assert!(dir.join("system.wav").exists());
        let codec = Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name", "-of", "csv=p=0"])
            .arg(dir.join(VIDEO_FILE))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&codec.stdout).trim(), "h264");
        // Not a video: no project left behind.
        let junk = tmp.join("notes.mp4");
        std::fs::write(&junk, "not a video").unwrap();
        assert!(create_project(&junk).is_err());
        assert!(!crate::util::unsaved_dir().join("notes").exists());
    }
}
