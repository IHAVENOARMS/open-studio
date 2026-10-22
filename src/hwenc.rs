//! Hardware H.264 encoding for recording and export: VAAPI (AMD, Intel and
//! open-source drivers) or NVENC (NVIDIA). Each candidate is checked once per
//! run by encoding a tiny clip the same way real encodes do, so drivers that
//! are missing, broken or busy fall back to x264 instead of failing a
//! recording.
//!
//! GPU encoders only do 4:2:0 colour, so they trade the x264 recorder's 4:4:4
//! sharpness on zoomed-in coloured text for far less CPU use.

use std::process::{Command, Stdio};
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq)]
pub enum Gpu {
    /// A DRM render node such as /dev/dri/renderD128.
    Vaapi(String),
    Nvenc,
}

impl Gpu {
    pub fn label(&self) -> &'static str {
        match self {
            Gpu::Vaapi(_) => "VAAPI",
            Gpu::Nvenc => "NVENC",
        }
    }

    /// ffmpeg arguments that go before `-i`.
    pub fn input_args(&self) -> Vec<String> {
        match self {
            Gpu::Vaapi(node) => vec!["-init_hw_device".into(), format!("vaapi=gpu:{node}"), "-filter_hw_device".into(), "gpu".into()],
            Gpu::Nvenc => vec![],
        }
    }

    /// ffmpeg arguments after the input: converts RGB(A) frames to BT.709
    /// limited-range 4:2:0 (on the GPU for VAAPI) and encodes at constant
    /// quantizer `qp` with a keyframe every `gop` frames.
    pub fn output_args(&self, qp: u32, gop: u32) -> Vec<String> {
        let mut a: Vec<String> = match self {
            Gpu::Vaapi(_) => vec![
                "-vf".into(),
                "hwupload,scale_vaapi=format=nv12:out_color_matrix=bt709:out_range=tv".into(),
                "-c:v".into(),
                "h264_vaapi".into(),
                "-rc_mode".into(),
                "CQP".into(),
                "-qp".into(),
                qp.to_string(),
            ],
            Gpu::Nvenc => vec![
                "-vf".into(),
                "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p".into(),
                "-c:v".into(),
                "h264_nvenc".into(),
                "-preset".into(),
                "p4".into(),
                "-rc".into(),
                "constqp".into(),
                "-qp".into(),
                qp.to_string(),
            ],
        };
        a.extend(["-g".into(), gop.to_string()]);
        a.extend(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"].map(String::from));
        a
    }
}

static GPU: OnceLock<Option<Gpu>> = OnceLock::new();

/// The result of `detect` if it has finished, without waiting.
pub fn detected() -> Option<Option<&'static Gpu>> {
    GPU.get().map(Option::as_ref)
}

/// Runs `detect` on a background thread so it's ready when needed.
pub fn detect_in_background() {
    std::thread::spawn(|| {
        detect();
    });
}

/// The first working hardware encoder, or None (no GPU encoder, ffmpeg built
/// without one, or `OPEN_STUDIO_NO_GPU_ENCODE` set). Blocking on first call
/// (well under a second); later calls are free.
pub fn detect() -> Option<&'static Gpu> {
    GPU.get_or_init(|| {
        if std::env::var_os("OPEN_STUDIO_NO_GPU_ENCODE").is_some() {
            return None;
        }
        let mut nodes: Vec<String> = std::fs::read_dir("/dev/dri")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str().filter(|n| n.starts_with("renderD")).map(|n| format!("/dev/dri/{n}")))
            .collect();
        nodes.sort();
        let mut candidates: Vec<Gpu> = nodes.into_iter().map(Gpu::Vaapi).collect();
        candidates.push(Gpu::Nvenc);
        candidates.into_iter().find(works)
    })
    .as_ref()
}

/// Encodes a few frames of a test pattern from raw RGBA, like the recorder
/// and exporter do.
fn works(gpu: &Gpu) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "quiet", "-nostdin"])
        .args(gpu.input_args())
        .args(["-f", "lavfi", "-i", "testsrc2=s=256x144:r=30:d=0.2,format=rgba"])
        .args(gpu.output_args(20, 30))
        .args(["-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    #[test]
    fn detection_is_stable() {
        // Whatever this machine has, asking twice gives the same answer.
        assert_eq!(super::detect(), super::detect());
    }
}
