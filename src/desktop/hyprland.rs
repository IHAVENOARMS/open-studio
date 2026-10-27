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
    pub name: String,
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

/// Runs a dispatcher. Hyprland 0.56+ takes Lua (`hl.dsp...`); older versions
/// take the classic `dispatch name args` form, used as a fallback.
pub fn dispatch(lua: &str, legacy: &str) -> Result<()> {
    let reply = request(&format!("dispatch {lua}"))?;
    if reply.trim() == "ok" {
        return Ok(());
    }
    let reply = request(&format!("dispatch {legacy}"))?;
    if reply.trim() == "ok" { Ok(()) } else { Err(anyhow!("hyprctl dispatch failed: {}", reply.trim())) }
}

/// Keeps layer surfaces with this namespace out of screen capture (so
/// speaker notes aren't recorded). Hyprland 0.56+ (Lua config); older
/// versions ignore it. True when Hyprland took the rule.
pub fn hide_layer_from_capture(namespace: &str) -> bool {
    let lua = format!("hl.layer_rule({{ name = \"{namespace}\", match = {{ namespace = \"^{namespace}$\" }}, no_screen_share = true }})");
    request(&format!("eval {lua}")).is_ok_and(|r| r.trim() == "ok")
}

/// Moves the window owned by `pid` to a workspace without following it.
pub fn move_window_to_workspace(pid: u32, workspace: &str, follow: bool) -> Result<()> {
    let lua = format!("hl.dsp.window.move({{workspace=\"{workspace}\", follow={follow}, window=\"pid:{pid}\"}})");
    let legacy = if follow {
        format!("movetoworkspace {workspace},pid:{pid}")
    } else {
        format!("movetoworkspacesilent {workspace},pid:{pid}")
    };
    dispatch(&lua, &legacy)
}

#[derive(Deserialize, Clone, Debug)]
pub struct Client {
    pub address: String,
    pub class: String,
    pub title: String,
    pub at: [f64; 2],
    pub size: [f64; 2],
    pub workspace: ClientWorkspace,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub mapped: bool,
    #[serde(rename = "focusHistoryID", default)]
    pub focus_history: i64,
    #[serde(default)]
    pub pid: i64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ClientWorkspace {
    pub id: i64,
}

pub fn clients() -> Result<Vec<Client>> {
    Ok(serde_json::from_str(&request("j/clients")?)?)
}

#[derive(Deserialize, Clone, Debug)]
pub struct MonitorInfo {
    pub name: String,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: ClientWorkspace,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "refreshRate", default)]
    pub refresh_rate: f64,
}

/// Monitors with their active workspace (for finding visible windows).
pub fn monitor_infos() -> Result<Vec<MonitorInfo>> {
    Ok(serde_json::from_str(&request("j/monitors")?)?)
}
