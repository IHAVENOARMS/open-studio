//! Click and key logging straight from evdev. Requires read access to
//! /dev/input/event* (usually membership in the group that owns them).

use crate::util::monotonic_ns;
use evdev::{Device, EventSummary, KeyCode};
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug)]
pub enum InputEvent {
    Button { t_ns: i64, code: u16, down: bool },
    Key { t_ns: i64, code: u16, down: bool },
}

/// Starts one reader thread per mouse/keyboard device. Returns how many
/// devices are being watched (0 usually means missing permissions).
pub fn spawn(stop: Arc<AtomicBool>, tx: Sender<InputEvent>) -> usize {
    let mut count = 0;
    for (_path, dev) in evdev::enumerate() {
        let Some(keys) = dev.supported_keys() else { continue };
        let is_mouse = keys.contains(KeyCode::BTN_LEFT);
        let is_keyboard = keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_Z);
        if !is_mouse && !is_keyboard {
            continue;
        }
        count += 1;
        let stop = stop.clone();
        let tx = tx.clone();
        std::thread::spawn(move || read_loop(dev, stop, tx));
    }
    count
}

/// Why clicks, keys and shortcuts can't be read, with the fix, or None when
/// they can. The group owning the devices is looked up, not assumed.
pub fn access_problem() -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let nodes: Vec<_> = std::fs::read_dir("/dev/input")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event")))
        .collect();
    if nodes.is_empty() || nodes.iter().any(|p| std::fs::File::open(p).is_ok()) {
        return None;
    }
    let gid = std::fs::metadata(&nodes[0]).ok()?.gid();
    let (group, member) = group_info(gid)?;
    Some(access_message(&group, member))
}

fn access_message(group: &str, member: bool) -> String {
    if member {
        format!("Clicks, keys and shortcuts can't be recorded until you log out and back in (you were added to the \"{group}\" group).")
    } else {
        format!("Clicks, keys and shortcuts can't be recorded: input devices aren't readable. Add yourself to the \"{group}\" group (sudo usermod -aG {group} $USER), then log out and back in.")
    }
}

/// The group's name, and whether the current user is listed in it.
fn group_info(gid: u32) -> Option<(String, bool)> {
    let mut buf = vec![0 as libc::c_char; 16384];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut out = std::ptr::null_mut();
    let r = unsafe { libc::getgrgid_r(gid, &mut grp, buf.as_mut_ptr(), buf.len(), &mut out) };
    if r != 0 || out.is_null() {
        return None;
    }
    let cstr = |p: *const libc::c_char| unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned();
    let name = cstr(grp.gr_name);
    let user = std::env::var("USER").unwrap_or_default();
    let mut member = false;
    let mut m = grp.gr_mem;
    while !m.is_null() && unsafe { !(*m).is_null() } {
        member |= cstr(unsafe { *m }) == user;
        m = unsafe { m.add(1) };
    }
    Some((name, member))
}

fn read_loop(mut dev: Device, stop: Arc<AtomicBool>, tx: Sender<InputEvent>) {
    let fd = dev.as_raw_fd();
    while !stop.load(Ordering::Relaxed) {
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut pfd, 1, 100) };
        if r <= 0 {
            continue;
        }
        let Ok(events) = dev.fetch_events() else { return };
        let t_ns = monotonic_ns();
        for ev in events {
            if let EventSummary::Key(_, code, value) = ev.destructure() {
                // value: 0 release, 1 press, 2 autorepeat (ignored)
                if value == 2 {
                    continue;
                }
                let down = value == 1;
                let c = code.code();
                let is_button = (0x110..0x118).contains(&c);
                let e = if is_button {
                    InputEvent::Button { t_ns, code: c, down }
                } else {
                    InputEvent::Key { t_ns, code: c, down }
                };
                if tx.send(e).is_err() {
                    return;
                }
            }
        }
    }
}

/// A global key combination, e.g. "Ctrl+Alt+Enter".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_: bool,
    pub key: KeyCode,
}

impl Shortcut {
    pub fn parse(s: &str) -> Option<Self> {
        let mut sc = Shortcut { ctrl: false, alt: false, shift: false, super_: false, key: KeyCode::KEY_ENTER };
        let mut key = None;
        for part in s.split('+').map(|p| p.trim().to_lowercase()) {
            match part.as_str() {
                "ctrl" | "control" => sc.ctrl = true,
                "alt" => sc.alt = true,
                "shift" => sc.shift = true,
                "super" | "meta" | "win" => sc.super_ = true,
                k => key = key_from_name(k),
            }
        }
        sc.key = key?;
        Some(sc)
    }
}

fn key_from_name(k: &str) -> Option<KeyCode> {
    Some(match k {
        "enter" | "return" => KeyCode::KEY_ENTER,
        "space" => KeyCode::KEY_SPACE,
        "escape" | "esc" => KeyCode::KEY_ESC,
        "f9" => KeyCode::KEY_F9,
        "f10" => KeyCode::KEY_F10,
        "f11" => KeyCode::KEY_F11,
        "f12" => KeyCode::KEY_F12,
        "r" => KeyCode::KEY_R,
        "s" => KeyCode::KEY_S,
        "x" => KeyCode::KEY_X,
        "p" => KeyCode::KEY_P,
        "2" => KeyCode::KEY_2,
        "5" => KeyCode::KEY_5,
        "/" | "slash" => KeyCode::KEY_SLASH,
        "." | "period" => KeyCode::KEY_DOT,
        _ => {
            let c = k.chars().next()?;
            if k.len() == 1 && c.is_ascii_alphabetic() {
                let name = format!("KEY_{}", c.to_ascii_uppercase());
                return name.parse().ok();
            }
            return None;
        }
    })
}

/// Watches all keyboards (including ones plugged in later) and sets `hit`
/// whenever `shortcut` is pressed, regardless of which window has focus.
pub fn watch_shortcut(shortcut: Shortcut, hit: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut watched = std::collections::HashSet::new();
        while !stop.load(Ordering::Relaxed) {
            for (path, dev) in evdev::enumerate() {
                if watched.contains(&path) {
                    continue;
                }
                let Some(keys) = dev.supported_keys() else { continue };
                if !(keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_Z)) {
                    continue;
                }
                if std::env::var_os("OPEN_STUDIO_DEBUG_KEYS").is_some() {
                    eprintln!("watching {} ({})", path.display(), dev.name().unwrap_or("?"));
                }
                watched.insert(path);
                let (sc, hit, stop) = (shortcut.clone(), hit.clone(), stop.clone());
                std::thread::spawn(move || watch_device(dev, sc, hit, stop));
            }
            for _ in 0..20 {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    });
}

fn watch_device(mut dev: Device, sc: Shortcut, hit: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    let fd = dev.as_raw_fd();
    let (mut ctrl, mut alt, mut shift, mut sup) = (false, false, false, false);
    while !stop.load(Ordering::Relaxed) {
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut pfd, 1, 100) } <= 0 {
            continue;
        }
        let Ok(events) = dev.fetch_events() else { return };
        for ev in events {
            if let EventSummary::Key(_, code, value) = ev.destructure() {
                let down = value != 0;
                match code {
                    KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => ctrl = down,
                    KeyCode::KEY_LEFTALT | KeyCode::KEY_RIGHTALT => alt = down,
                    KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => shift = down,
                    KeyCode::KEY_LEFTMETA | KeyCode::KEY_RIGHTMETA => sup = down,
                    c if c == sc.key && value == 1 => {
                        if ctrl == sc.ctrl && alt == sc.alt && shift == sc.shift && sup == sc.super_ {
                            hit.store(true, Ordering::Relaxed);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn message_names_the_group() {
        let m = super::access_message("plugdev", false);
        assert!(m.contains("\"plugdev\"") && m.contains("usermod -aG plugdev"));
        assert!(super::access_message("input", true).contains("log out"));
        assert_eq!(super::group_info(0).map(|g| g.0).as_deref(), Some("root"));
    }
}
