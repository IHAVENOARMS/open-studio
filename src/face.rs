//! Face tracking for the camera: faces are found in camera.mp4 a few times
//! a second with rustface (SeetaFace), smoothed, and cached in
//! face_track.json. The camera crop then follows the face.

use crate::project::Recording;
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE: &str = "face_track.json";
const MODEL_URL: &str = "https://github.com/atomashpolskiy/rustface/raw/master/model/seeta_fd_frontal_v1.0.bin";
/// Samples per second.
const RATE: f64 = 5.0;

/// Face centre over time in camera coordinates (0..1), camera time.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FaceTrack {
    pub samples: Vec<[f64; 3]>,
}

impl FaceTrack {
    pub fn load(dir: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(dir.join(FILE)).ok()?).ok()
    }

    /// Face centre at camera time `t`, interpolated.
    pub fn at(&self, t: f64) -> Option<(f64, f64)> {
        let s = &self.samples;
        let i = s.partition_point(|p| p[0] <= t);
        match (i.checked_sub(1).and_then(|j| s.get(j)), s.get(i)) {
            (Some(a), Some(b)) => {
                let f = ((t - a[0]) / (b[0] - a[0]).max(1e-6)).clamp(0.0, 1.0);
                Some((a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f))
            }
            (Some(a), None) | (None, Some(a)) => Some((a[1], a[2])),
            (None, None) => None,
        }
    }
}

fn model_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "open-studio")
        .map(|d| d.data_dir().join("models"))
        .unwrap_or_else(|| PathBuf::from(".open-studio-models"))
        .join("seeta_fd_frontal_v1.0.bin")
}

/// Downloads the face model on first use (about 1 MB).
pub(crate) fn ensure_model() -> Result<PathBuf> {
    let p = model_path();
    if p.is_file() {
        return Ok(p);
    }
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("part");
    let ok = std::process::Command::new("curl").args(["-fsSL", "-o"]).arg(&tmp).arg(MODEL_URL).status().context("running curl")?.success();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!("could not download the face model"));
    }
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

/// Finds the face through the camera video and saves face_track.json.
/// Blocking; run it on a worker thread.
pub fn analyze(rec: &Recording, progress: &std::sync::atomic::AtomicU32) -> Result<FaceTrack> {
    let cam = rec.camera.as_ref().ok_or_else(|| anyhow!("no camera"))?;
    let model = ensure_model()?;
    let mut det = rustface::create_detector(model.to_str().unwrap()).map_err(|e| anyhow!("face model: {e}"))?;
    det.set_min_face_size(24);
    det.set_score_thresh(2.0);
    det.set_pyramid_scale_factor(0.8);
    det.set_slide_window_step(4, 4);
    // Small greyscale frames at RATE fps are plenty to follow a face.
    let (w, h) = (320u32, ((320.0 * cam.height as f64 / cam.width as f64).round() as u32) & !1);
    let mut child = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
        .arg(rec.dir.join(&cam.file))
        .args(["-vf", &format!("fps={RATE},scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("running ffmpeg")?;
    let mut out = child.stdout.take().unwrap();
    let mut buf = vec![0u8; (w * h) as usize];
    let total = (rec.duration * RATE).max(1.0);
    let mut raw: Vec<[f64; 3]> = vec![];
    let mut n = 0.0;
    while std::io::Read::read_exact(&mut out, &mut buf).is_ok() {
        let t = n / RATE;
        n += 1.0;
        let img = rustface::ImageData::new(&buf, w, h);
        if let Some(f) = det.detect(&img).into_iter().max_by(|a, b| a.score().total_cmp(&b.score())) {
            let b = f.bbox();
            let cx = (b.x() as f64 + b.width() as f64 / 2.0) / w as f64;
            let cy = (b.y() as f64 + b.height() as f64 / 2.0) / h as f64;
            raw.push([t, cx.clamp(0.0, 1.0), cy.clamp(0.0, 1.0)]);
        }
        progress.store(((n / total).min(1.0) * 1000.0) as u32, std::sync::atomic::Ordering::Relaxed);
    }
    let _ = child.wait();
    // Smooth so the crop glides instead of jittering with each detection.
    let mut samples = Vec::with_capacity(raw.len());
    let (mut x, mut y) = raw.first().map_or((0.5, 0.5), |p| (p[1], p[2]));
    for p in &raw {
        x += (p[1] - x) * 0.35;
        y += (p[2] - y) * 0.35;
        samples.push([p[0], x, y]);
    }
    let track = FaceTrack { samples };
    std::fs::write(rec.dir.join(FILE), serde_json::to_string(&track)?)?;
    Ok(track)
}
