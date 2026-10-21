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
