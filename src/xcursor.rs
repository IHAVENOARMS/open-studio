//! XCursor theme loading: used to recognise which cursor an app is showing
//! and to draw shapes we have no vector version of at high resolution.

use std::path::PathBuf;

pub struct XImage {
    /// Nominal size (e.g. 24, 48).
    pub size: u32,
    pub width: u32,
    pub height: u32,
    pub hotspot: (u32, u32),
    /// Premultiplied RGBA.
    pub rgba: Vec<u8>,
}

/// Canonical cursor names (CSS / cursor-shape-v1 style) and the XCursor file
/// names themes use for them.
pub const NAMES: &[(&str, &[&str])] = &[
    ("default", &["default", "left_ptr", "arrow", "top_left_arrow"]),
    ("text", &["text", "xterm", "ibeam"]),
    ("vertical-text", &["vertical-text"]),
    ("pointer", &["pointer", "hand2", "hand1", "pointing_hand"]),
    ("grab", &["grab", "openhand"]),
    ("grabbing", &["grabbing", "closedhand", "dnd-move"]),
    ("crosshair", &["crosshair", "cross", "tcross"]),
    ("move", &["move", "fleur", "all-scroll", "all-resize"]),
    ("ew-resize", &["ew-resize", "e-resize", "w-resize", "col-resize", "sb_h_double_arrow", "left_side", "right_side"]),
    ("ns-resize", &["ns-resize", "n-resize", "s-resize", "row-resize", "sb_v_double_arrow", "top_side", "bottom_side"]),
    ("nwse-resize", &["nwse-resize", "nw-resize", "se-resize", "top_left_corner", "bottom_right_corner", "bd_double_arrow"]),
    ("nesw-resize", &["nesw-resize", "ne-resize", "sw-resize", "top_right_corner", "bottom_left_corner", "fd_double_arrow"]),
    ("not-allowed", &["not-allowed", "no-drop", "crossed_circle"]),
    ("wait", &["wait", "watch"]),
    ("progress", &["progress", "left_ptr_watch"]),
    ("help", &["help", "question_arrow"]),
    ("context-menu", &["context-menu"]),
    ("copy", &["copy", "dnd-copy"]),
    ("alias", &["alias", "dnd-link"]),
    ("cell", &["cell"]),
    ("zoom-in", &["zoom-in"]),
    ("zoom-out", &["zoom-out"]),
];

pub fn theme_name() -> String {
    std::env::var("XCURSOR_THEME").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| "default".into())
}

pub fn base_size() -> u32 {
    std::env::var("XCURSOR_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(24)
}

fn search_dirs() -> Vec<PathBuf> {
    let home = std::env::home_dir().unwrap_or_default();
    let mut dirs = vec![home.join(".local/share/icons"), home.join(".icons")];
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(data.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("icons")));
    dirs.push("/usr/share/pixmaps".into());
    dirs
}

/// Theme plus everything it inherits, ending with the conventional fallbacks.
fn theme_chain(theme: &str) -> Vec<String> {
    let mut chain = vec![];
    let mut queue = vec![theme.to_string()];
    while let Some(t) = queue.pop() {
        if chain.contains(&t) || chain.len() > 8 {
            continue;
        }
        for d in search_dirs() {
            if let Ok(text) = std::fs::read_to_string(d.join(&t).join("index.theme")) {
                for line in text.lines() {
                    if let Some(rest) = line.trim().strip_prefix("Inherits=") {
                        queue.extend(rest.split(',').map(|s| s.trim().to_string()).rev());
                    }
                }
                break;
            }
        }
        chain.push(t);
    }
    for f in ["Adwaita", "hicolor"] {
        if !chain.iter().any(|c| c == f) {
            chain.push(f.into());
        }
    }
    chain
}

pub fn find(theme: &str, file: &str) -> Option<PathBuf> {
    for t in theme_chain(theme) {
        for d in search_dirs() {
            let p = d.join(&t).join("cursors").join(file);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Every image in an XCursor file (all sizes, all animation frames).
pub fn load(path: &std::path::Path) -> Vec<XImage> {
    let Ok(d) = std::fs::read(path) else { return vec![] };
    let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if d.get(0..4) != Some(b"Xcur") {
        return vec![];
    }
    let n = u32_at(12).unwrap_or(0) as usize;
    let mut out = vec![];
    for i in 0..n.min(1024) {
        let (Some(ty), Some(pos)) = (u32_at(16 + i * 12), u32_at(24 + i * 12)) else { break };
        if ty != 0xfffd0002 {
            continue;
        }
        let p = pos as usize;
        let (Some(size), Some(w), Some(h), Some(xh), Some(yh)) =
            (u32_at(p + 8), u32_at(p + 16), u32_at(p + 20), u32_at(p + 24), u32_at(p + 28))
        else {
            continue;
        };
        let start = p + 36;
        let len = (w * h * 4) as usize;
        let Some(px) = d.get(start..start + len) else { continue };
        let mut rgba = vec![0u8; len];
        for (o, s) in rgba.chunks_exact_mut(4).zip(px.chunks_exact(4)) {
            o.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
        out.push(XImage { size, width: w, height: h, hotspot: (xh, yh), rgba });
    }
    out
}

/// The canonical name's image closest to `size` (first animation frame).
pub fn load_named(canonical: &str, size: u32) -> Option<XImage> {
    let files = NAMES.iter().find(|(c, _)| *c == canonical).map(|(_, f)| *f).unwrap_or(&[]);
    let theme = theme_name();
    for f in files.iter().copied().chain(std::iter::once(canonical)) {
        if let Some(path) = find(&theme, f) {
            let imgs = load(&path);
            let best = imgs.iter().map(|i| i.size).min_by_key(|s| s.abs_diff(size))?;
            return imgs.into_iter().find(|i| i.size == best);
        }
    }
    None
}

/// The canonical name's image closest to `size` from one specific theme
/// directory (no inheritance).
pub fn load_from_dir(dir: &std::path::Path, canonical: &str, size: u32) -> Option<XImage> {
    let files = NAMES.iter().find(|(c, _)| *c == canonical).map(|(_, f)| *f).unwrap_or(&[]);
    for f in files.iter().copied().chain(std::iter::once(canonical)) {
        let path = dir.join("cursors").join(f);
        if path.exists() {
            let imgs = load(&path);
            let best = imgs.iter().map(|i| i.size).min_by_key(|s| s.abs_diff(size))?;
            return imgs.into_iter().find(|i| i.size == best);
        }
    }
    None
}
