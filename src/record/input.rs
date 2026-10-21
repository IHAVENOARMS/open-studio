//! Click and key logging straight from evdev. Requires read access to
//! /dev/input/event* (membership in the `input` group).

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
