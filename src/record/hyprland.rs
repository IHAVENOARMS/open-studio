//! Minimal Hyprland IPC client. Used to sample the cursor position (the
//! Hyprland portal has no cursor metadata mode) and to list monitors so we can
//! map global coordinates onto the captured output.

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

fn socket_path() -> Option<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let p = PathBuf::from(runtime).join("hypr").join(sig).join(".socket.sock");
    p.exists().then_some(p)
}

pub fn available() -> bool {
    socket_path().is_some()
}

pub fn request(cmd: &str) -> Result<String> {
    let path = socket_path().ok_or_else(|| anyhow!("Hyprland IPC socket not found"))?;
    let mut s = UnixStream::connect(&path).context("connecting to Hyprland IPC")?;
    s.write_all(cmd.as_bytes())?;
    let mut out = String::new();
    s.read_to_string(&mut out)?;
    Ok(out)
}

#[derive(Deserialize)]
struct Pos {
    x: f64,
    y: f64,
}

pub fn cursor_pos() -> Result<(f64, f64)> {
    let p: Pos = serde_json::from_str(&request("j/cursorpos")?)?;
    Ok((p.x, p.y))
}

#[derive(Deserialize, Clone, Debug)]
pub struct Monitor {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    pub transform: u32,
}

impl Monitor {
    /// Mode size in pixels, after rotation.
    pub fn pixel_size(&self) -> (f64, f64) {
        if self.transform % 2 == 1 { (self.height, self.width) } else { (self.width, self.height) }
    }
    pub fn logical_rect(&self) -> [f64; 4] {
        let (w, h) = self.pixel_size();
        [self.x, self.y, w / self.scale, h / self.scale]
    }
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let r = self.logical_rect();
        x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]
    }
}

pub fn monitors() -> Result<Vec<Monitor>> {
    Ok(serde_json::from_str(&request("j/monitors")?)?)
}

#[derive(Deserialize)]
struct Workspace {
    id: i64,
}

pub fn active_workspace() -> Result<i64> {
    let w: Workspace = serde_json::from_str(&request("j/activeworkspace")?)?;
    Ok(w.id)
}
