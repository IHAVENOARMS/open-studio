//! Putting things on the system clipboard: wl-copy on Wayland, xclip or
//! xsel on X11. Both keep serving the clipboard from a background process
//! after we hand the data over.

use std::io::Write;
use std::process::{Command, Stdio};

/// Clipboard tools to try for `mime` data, best first, given which display
/// servers are reachable.
fn tools(wayland: bool, x11: bool, mime: &str) -> Vec<Vec<String>> {
    let mut out = vec![];
    if wayland {
        out.push(vec!["wl-copy".into(), "--type".into(), mime.into()]);
    }
    if x11 {
        out.push(vec!["xclip".into(), "-selection".into(), "clipboard".into(), "-t".into(), mime.into()]);
        // xsel can't label the data, so it's only good for text.
        if mime.starts_with("text/plain") {
            out.push(vec!["xsel".into(), "--clipboard".into(), "--input".into()]);
        }
    }
    out
}

/// Copies `data` of type `mime` ("image/png", "text/uri-list").
pub fn copy(mime: &str, data: &[u8]) -> Result<(), String> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    let candidates = tools(wayland, x11, mime);
    if candidates.is_empty() {
        return Err("no display to copy to".into());
    }
    let mut missing = vec![];
    for argv in candidates {
        let child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            missing.push(argv[0].clone());
            continue;
        };
        let wrote = child.stdin.take().unwrap().write_all(data);
        let status = child.wait().map_err(|e| e.to_string())?;
        return match (wrote, status.success()) {
            (Ok(()), true) => Ok(()),
            _ => Err(format!("{} failed", argv[0])),
        };
    }
    Err(format!("install {} to copy", missing.join(" or ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_a_tool_for_the_display_server() {
        let names = |w, x, m| tools(w, x, m).into_iter().map(|a| a[0].clone()).collect::<Vec<_>>();
        assert_eq!(names(true, true, "image/png"), ["wl-copy", "xclip"]);
        assert_eq!(names(false, true, "image/png"), ["xclip"]);
        assert_eq!(names(false, true, "text/plain;charset=utf-8"), ["xclip", "xsel"]);
        assert!(names(false, false, "image/png").is_empty());
        assert!(tools(true, false, "image/png")[0].contains(&"image/png".to_string()));
    }

    /// Copies a PNG and reads it back. Opt-in (touches the real clipboard):
    /// `cargo test -- --ignored clipboard_round_trip`.
    #[test]
    #[ignore]
    fn clipboard_round_trip() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 20, 30, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let png = png.into_inner();
        copy("image/png", &png).unwrap();
        let back = Command::new("wl-paste").args(["--type", "image/png"]).output().unwrap().stdout;
        assert_eq!(back, png);
    }
}
