//! Video decoding through an ffmpeg subprocess emitting raw RGBA.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};

pub struct Decoder {
    child: Child,
    stdout: ChildStdout,
    pub width: u32,
    pub height: u32,
}

/// Output size for decoding `src` with an optional width cap (kept even).
pub fn scaled_size(src: (u32, u32), max_w: Option<u32>) -> (u32, u32) {
    match max_w {
        Some(m) if src.0 > m => {
            let w = m & !1;
            let h = ((src.1 as f64 * w as f64 / src.0 as f64).round() as u32).max(2) & !1;
            (w, h)
        }
        _ => src,
    }
}

impl Decoder {
    /// Starts decoding at `start` seconds. Frames come out at the source rate.
    pub fn open(path: &Path, start: f64, src: (u32, u32), max_w: Option<u32>) -> Result<Self> {
        let (width, height) = scaled_size(src, max_w);
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        if start > 0.0 {
            cmd.args(["-ss", &format!("{start:.4}")]);
        }
        cmd.arg("-i").arg(path);
        if (width, height) != src {
            cmd.args(["-vf", &format!("scale={width}:{height}:flags=area")]);
        }
        cmd.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
        let mut child = cmd.stdout(Stdio::piped()).stdin(Stdio::null()).spawn().context("launching ffmpeg")?;
        let stdout = child.stdout.take().unwrap();
        Ok(Self { child, stdout, width, height })
    }

    pub fn frame_len(&self) -> usize {
        (self.width * self.height * 4) as usize
    }

    /// Reads the next frame into `buf`; false at end of stream.
    pub fn read(&mut self, buf: &mut Vec<u8>) -> Result<bool> {
        buf.resize(self.frame_len(), 0);
        match self.stdout.read_exact(buf) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
