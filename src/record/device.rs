//! Android phones and tablets, mirrored with scrcpy. Screen Studio records
//! iPhones over USB; here the phone's screen is mirrored into a desktop
//! window with scrcpy and that window is recorded like any other, so pause,
//! microphone and system audio (scrcpy plays the phone's sound) all work,
//! and clicks made on the mirror drive the click effects and auto zoom.

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// scrcpy shipped next to our binary (AppImage: `$APPDIR/usr/bin`), then
/// `$OPEN_STUDIO_SCRCPY`, then `PATH`.
pub fn scrcpy() -> Option<PathBuf> {
    find("scrcpy", "OPEN_STUDIO_SCRCPY")
}

pub fn adb() -> Option<PathBuf> {
    find("adb", "OPEN_STUDIO_ADB")
}

/// scrcpy's phone-side server (used for phone cameras); it must match the
/// installed scrcpy's version.
pub fn scrcpy_server() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SCRCPY_SERVER_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let mut dirs = vec![PathBuf::from("/usr/share/scrcpy"), PathBuf::from("/usr/local/share/scrcpy")];
    if let Some(d) = std::env::var_os("APPDIR") {
        dirs.insert(0, PathBuf::from(d).join("usr/share/scrcpy"));
    }
    adb()?;
    dirs.into_iter().map(|d| d.join("scrcpy-server")).find(|p| p.is_file())
}

/// "4.1" from `scrcpy --version`.
pub fn scrcpy_version() -> Option<String> {
    let out = Command::new(scrcpy()?).arg("--version").output().ok()?;
    String::from_utf8_lossy(&out.stdout).split_whitespace().nth(1).map(str::to_string)
}

fn find(name: &str, env: &str) -> Option<PathBuf> {
    let mut candidates = vec![];
    if let Some(p) = std::env::var_os(env) {
        candidates.push(PathBuf::from(p));
    }
    if let Some(dir) = std::env::var_os("APPDIR") {
        candidates.push(PathBuf::from(dir).join("usr/bin").join(name));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
        candidates.push(dir.join(name));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join(name)));
    }
    candidates.into_iter().find(|p| p.is_file())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub serial: String,
    /// The device name set on the phone, its marketing name, or the adb model.
    pub model: String,
}

/// Devices adb can talk to (USB, or wireless debugging).
pub fn list() -> Vec<Device> {
    let Some(adb) = adb() else { return vec![] };
    let Ok(out) = Command::new(&adb).args(["devices", "-l"]).stderr(Stdio::null()).output() else { return vec![] };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let serial = parts.next()?.to_string();
            if parts.next()? != "device" {
                return None; // offline / unauthorized
            }
            let model = parts.find_map(|p| p.strip_prefix("model:")).map(|m| m.replace('_', " ")).unwrap_or_else(|| serial.clone());
            let model = friendly_name(&adb, &serial).unwrap_or(model);
            Some(Device { serial, model })
        })
        .collect()
}

/// The name the owner gave the device ("Settings > About phone"), or its
/// marketing name.
fn friendly_name(adb: &std::path::Path, serial: &str) -> Option<String> {
    let get = |args: &[&str]| -> Option<String> {
        let out = Command::new(adb).args(["-s", serial, "shell"]).args(args).stderr(Stdio::null()).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty() && s != "null").then_some(s)
    };
    get(&["settings", "get", "global", "device_name"]).or_else(|| get(&["getprop", "ro.product.marketname"]))
}

/// A running scrcpy mirror window. Closed on drop.
pub struct Mirror {
    pub device: Device,
    /// Options it was started with.
    pub options: crate::settings::ScrcpyOptions,
    child: Child,
}

impl Mirror {
    pub fn open(device: &Device, opts: &crate::settings::ScrcpyOptions) -> Result<Self> {
        let Some(scrcpy) = scrcpy() else { bail!("scrcpy is not installed") };
        // Float the mirror so it keeps the phone's shape instead of tiling.
        let rule = "hl.window_rule({ name = 'open-studio-device', match = { class = '^scrcpy$' }, float = true, center = true })";
        let _ = super::hyprland::request(&format!("eval {rule}"));
        let mut cmd = Command::new(scrcpy);
        cmd.args(["--serial", &device.serial, "--window-title", &format!("{} (Open Studio)", device.model)])
            .args(opts.args())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(adb) = adb() {
            cmd.env("ADB", adb);
        }
        let child = cmd.spawn().context("starting scrcpy")?;
        Ok(Self { device: device.clone(), options: opts.clone(), child })
    }

    /// False once scrcpy has quit (phone unplugged, window closed).
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// The mirror's Hyprland window, once it has appeared.
    pub fn window(&self) -> Option<super::hyprland::Client> {
        let pid = self.child.id() as i64;
        super::hyprland::clients().ok()?.into_iter().find(|c| c.pid == pid && c.mapped && !c.hidden)
    }
}

impl Drop for Mirror {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
