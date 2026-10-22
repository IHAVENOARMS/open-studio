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
    /// Connected over the network (wireless debugging or `adb tcpip`).
    pub wireless: bool,
    /// False while the phone still has to allow debugging from this computer.
    pub ready: bool,
}

impl Device {
    pub fn label(&self) -> String {
        match (self.ready, self.wireless) {
            (false, _) => format!("{} (allow debugging on the phone)", self.model),
            (true, true) => format!("{} (Wi-Fi)", self.model),
            (true, false) => self.model.clone(),
        }
    }
}

fn is_network_serial(serial: &str) -> bool {
    serial.contains(':') || serial.contains("._adb-tls-connect.")
}

/// Devices adb can talk to (USB, wireless debugging, or `adb tcpip`).
pub fn list() -> Vec<Device> {
    let Some(adb) = adb() else { return vec![] };
    let Ok(out) = Command::new(&adb).args(["devices", "-l"]).stderr(Stdio::null()).output() else { return vec![] };
    let mut out: Vec<Device> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next()?;
            if state != "device" && state != "unauthorized" {
                return None; // offline
            }
            let ready = state == "device";
            let model = parts.find_map(|p| p.strip_prefix("model:")).map(|m| m.replace('_', " ")).unwrap_or_else(|| serial.clone());
            let model = if ready { friendly_name(&adb, &serial).unwrap_or(model) } else { model };
            Some(Device { wireless: is_network_serial(&serial), serial, model, ready })
        })
        .collect();
    // A phone on USB and Wi-Fi at once shows up twice; keep one of each name.
    out.sort_by_key(|d| d.wireless);
    let mut seen = std::collections::HashSet::new();
    out.retain(|d| seen.insert(d.model.clone()));
    out
}

/// Finds phones on the network and connects to them, then lists all devices.
/// Candidates are wireless addresses used before (`known`) and phones
/// advertising Wireless debugging over mDNS. Blocking (a few seconds at most);
/// run it off the UI thread.
pub fn discover(known: &[String]) -> Vec<Device> {
    let Some(adb) = adb() else { return vec![] };
    let connected: Vec<String> = list().into_iter().map(|d| d.serial).collect();
    let mut targets: Vec<String> = known.iter().filter(|k| !connected.contains(k)).cloned().collect();
    for addr in mdns_wireless_debugging() {
        if !connected.contains(&addr) && !targets.contains(&addr) {
            targets.push(addr);
        }
    }
    let handles: Vec<_> = targets
        .into_iter()
        .map(|t| {
            let adb = adb.clone();
            std::thread::spawn(move || {
                let Ok(mut child) = Command::new(&adb).args(["connect", &t]).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return };
                // An unreachable address can hang for a long time; give up quickly.
                for _ in 0..30 {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                let _ = child.kill();
                let _ = child.wait();
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
    list()
}

/// "ip:port" of phones advertising Wireless debugging (_adb-tls-connect) on
/// the local network, via avahi (Arch's adb is built without mDNS).
fn mdns_wireless_debugging() -> Vec<String> {
    let Ok(mut child) = Command::new("avahi-browse")
        .args(["-rpt", "_adb-tls-connect._tcp"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return vec![];
    };
    for _ in 0..30 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    let mut text = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut out, &mut text);
    }
    let _ = child.wait();
    // "=;iface;proto;name;type;domain;host;address;port;txt"
    let mut out = vec![];
    for l in text.lines().filter(|l| l.starts_with('=')) {
        let f: Vec<&str> = l.split(';').collect();
        if f.len() > 8 && f[2] == "IPv4" {
            let addr = format!("{}:{}", f[7], f[8]);
            if !out.contains(&addr) {
                out.push(addr);
            }
        }
    }
    out
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

/// Name scrcpy's audio stream gets (SDL takes it from the environment), so
/// recordings can capture the phone's sound on its own while it keeps
/// playing on the speakers as usual.
pub const DEVICE_AUDIO_STREAM: &str = "Open Studio device audio";

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
        // Name the audio stream (SDL3 and SDL2 read these) so it can be
        // recorded by itself.
        cmd.env("SDL_APP_NAME", DEVICE_AUDIO_STREAM).env("SDL_AUDIO_DEVICE_APP_NAME", DEVICE_AUDIO_STREAM);
        let child = cmd.spawn().context("starting scrcpy")?;
        Ok(Self { device: device.clone(), options: opts.clone(), child })
    }

    /// Where to record the phone's sound from (see `audio::AudioCapture`).
    pub fn audio_source(&self) -> Option<String> {
        self.options.audio.then(|| format!("{}{DEVICE_AUDIO_STREAM}", super::audio::STREAM_PREFIX))
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
