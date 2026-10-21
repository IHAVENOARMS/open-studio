use std::path::PathBuf;

/// CLOCK_MONOTONIC in nanoseconds. GStreamer's system clock and our input
/// pollers all stamp against this so video frames and events line up.
pub fn monotonic_ns() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64
}

pub fn recordings_dir() -> PathBuf {
    let base = directories::UserDirs::new()
        .and_then(|d| d.video_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join("Videos"));
    base.join("Open Studio")
}

pub fn config_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "open-studio")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".open-studio"))
}

pub fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("open-studio")
}

pub fn control_socket() -> PathBuf {
    runtime_dir().join("control.sock")
}

/// Human timestamp for folder names, e.g. "2026-10-21 16.30.12", from local time.
pub fn local_timestamp() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!(
        "{:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

pub fn format_time(t: f64) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u64;
    let s = t - m as f64 * 60.0;
    format!("{m}:{s:04.1}")
}

/// Path for display, with the home directory shortened to `~`.
pub fn tilde(p: &std::path::Path) -> String {
    match std::env::home_dir().and_then(|h| p.strip_prefix(&h).ok().map(|r| r.to_path_buf())) {
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}
