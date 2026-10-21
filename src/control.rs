//! A tiny control socket so a running recording can be stopped from a
//! keybinding (`open-studio stop`), e.g. `bind = SUPER SHIFT, R, exec, open-studio stop`.

use crate::util::{control_socket, runtime_dir};
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ControlServer {
    pub stop_requested: Arc<AtomicBool>,
}

impl ControlServer {
    pub fn start() -> Result<Self> {
        std::fs::create_dir_all(runtime_dir())?;
        let path = control_socket();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).context("binding control socket")?;
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        std::thread::spawn(move || {
            for mut conn in listener.incoming().flatten() {
                let mut buf = String::new();
                let _ = conn.read_to_string(&mut buf);
                if buf.trim() == "stop" {
                    f.store(true, Ordering::Relaxed);
                    let _ = conn.write_all(b"ok");
                }
            }
        });
        Ok(Self { stop_requested: flag })
    }

    pub fn take_stop(&self) -> bool {
        self.stop_requested.swap(false, Ordering::Relaxed)
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(control_socket());
    }
}

pub fn send_stop() -> Result<()> {
    let mut s = UnixStream::connect(control_socket()).context("no recording in progress")?;
    s.write_all(b"stop")?;
    s.shutdown(std::net::Shutdown::Write)?;
    Ok(())
}

/// Tray icon shown while recording; clicking it stops the recording (the
/// equivalent of Screen Studio's menu bar stop button).
struct RecordingTray {
    stop: Arc<AtomicBool>,
}

impl ksni::Tray for RecordingTray {
    fn id(&self) -> String {
        "open-studio".into()
    }
    fn title(&self) -> String {
        "Open Studio: recording (click to stop)".into()
    }
    fn icon_name(&self) -> String {
        "media-record".into()
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.stop.store(true, Ordering::Relaxed);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            ksni::menu::StandardItem {
                label: "Stop recording".into(),
                icon_name: "media-playback-stop".into(),
                activate: Box::new(|t: &mut Self| t.stop.store(true, Ordering::Relaxed)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub struct TrayHandle(ksni::blocking::Handle<RecordingTray>);

impl Drop for TrayHandle {
    fn drop(&mut self) {
        self.0.shutdown().wait();
    }
}

impl ControlServer {
    /// Shows the recording tray icon; it shares the stop flag with the socket.
    pub fn tray(&self) -> Option<TrayHandle> {
        use ksni::blocking::TrayMethods;
        RecordingTray { stop: self.stop_requested.clone() }.spawn().ok().map(TrayHandle)
    }
}
