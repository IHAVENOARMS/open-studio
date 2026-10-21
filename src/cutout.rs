//! Camera background removal ("Camera cutout"). Every camera frame goes
//! through MODNet (portrait matting, Apache-2.0) with ONNX Runtime; the
//! mattes are saved as a small greyscale video, camera_matte.mp4, with the
//! camera's timing, which the compositor uses as the camera's alpha.

use crate::project::Recording;
use anyhow::{Context, Result, anyhow};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const FILE: &str = "camera_matte.mp4";
const MODEL_URL: &str = "https://huggingface.co/Xenova/modnet/resolve/main/onnx/model.onnx";

fn model_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "open-studio")
        .map(|d| d.data_dir().join("models"))
        .unwrap_or_else(|| PathBuf::from(".open-studio-models"))
        .join("modnet.onnx")
}

/// Downloads the model on first use (about 26 MB).
fn ensure_model() -> Result<PathBuf> {
    let p = model_path();
    if p.is_file() {
        return Ok(p);
    }
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("part");
    let ok = Command::new("curl").args(["-fsSL", "-o"]).arg(&tmp).arg(MODEL_URL).status().context("running curl")?.success();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!("could not download the background removal model"));
    }
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

/// Matte size: about 512 wide, both sides multiples of 32 (MODNet's stride).
pub fn matte_size(w: u32, h: u32) -> (u32, u32) {
    let mw = 512u32;
    let mh = ((mw as f64 * h as f64 / w as f64 / 32.0).round() as u32).max(1) * 32;
    (mw, mh)
}

pub struct Progress {
    /// 0..=1000
    pub permille: AtomicU32,
    pub cancel: AtomicBool,
}

impl Default for Progress {
    fn default() -> Self {
        Self { permille: AtomicU32::new(0), cancel: AtomicBool::new(false) }
    }
}

/// Makes camera_matte.mp4 for the recording's camera and records it in
/// recording.json. Blocking (a couple of minutes for a few minutes of
/// video); run it on a worker thread.
pub fn process(rec: &Recording, progress: &Progress) -> Result<String> {
    let cam = rec.camera.as_ref().ok_or_else(|| anyhow!("this recording has no camera"))?;
    let model = ensure_model()?;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut session = ort::session::Session::builder()
        .and_then(|b| b.with_intra_threads(threads))
        .and_then(|b| b.commit_from_file(&model))
        .map_err(|e| anyhow!("loading the model: {e}"))?;
    let (w, h) = matte_size(cam.width, cam.height);
    let src = rec.dir.join(&cam.file);
    let mut decoder = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
        .arg(&src)
        .args(["-vf", &format!("scale={w}:{h}:flags=area"), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdout(Stdio::piped())
        .spawn()
        .context("running ffmpeg")?;
    let tmp = rec.dir.join("camera_matte.part.mp4");
    let mut encoder = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "gray"])
        .args(["-s", &format!("{w}x{h}"), "-r", &cam.fps.to_string(), "-i", "-"])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "12", "-pix_fmt", "gray", "-g", &cam.fps.to_string()])
        .arg(&tmp)
        .stdin(Stdio::piped())
        .spawn()
        .context("running ffmpeg")?;
    let mut input = decoder.stdout.take().unwrap();
    let mut output = encoder.stdin.take().unwrap();
    let total = (rec.duration * cam.fps as f64).max(1.0);
    let (wu, hu) = (w as usize, h as usize);
    let mut rgb = vec![0u8; wu * hu * 3];
    let mut tensor = vec![0f32; 3 * wu * hu];
    let mut gray = vec![0u8; wu * hu];
    let mut n = 0.0;
    while input.read_exact(&mut rgb).is_ok() {
        if progress.cancel.load(Ordering::Relaxed) {
            let _ = decoder.kill();
            drop(output);
            let _ = encoder.kill();
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow!("cancelled"));
        }
        // NCHW, normalised to -1..1.
        for i in 0..wu * hu {
            for c in 0..3 {
                tensor[c * wu * hu + i] = rgb[i * 3 + c] as f32 / 127.5 - 1.0;
            }
        }
        let value = ort::value::Tensor::from_array(([1usize, 3, hu, wu], tensor.clone())).map_err(|e| anyhow!("{e}"))?;
        let out = session.run(ort::inputs![value]).map_err(|e| anyhow!("running the model: {e}"))?;
        let (_, m) = out[0].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
        for (g, v) in gray.iter_mut().zip(m) {
            *g = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        output.write_all(&gray).context("writing the matte")?;
        n += 1.0;
        progress.permille.store(((n / total).min(1.0) * 1000.0) as u32, Ordering::Relaxed);
    }
    let _ = decoder.wait();
    drop(output);
    if !encoder.wait()?.success() {
        return Err(anyhow!("encoding the matte failed"));
    }
    std::fs::rename(&tmp, rec.dir.join(FILE))?;
    // Remember it in recording.json.
    let mut saved = Recording::load(&rec.dir)?;
    if let Some(c) = &mut saved.camera {
        c.matte = Some(FILE.into());
    }
    saved.save(&rec.dir)?;
    progress.permille.store(1000, Ordering::Relaxed);
    Ok(FILE.into())
}
