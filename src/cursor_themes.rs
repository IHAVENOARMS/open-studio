//! Cursor themes for rendering: themes the user imported (from an archive or
//! a folder) plus XCursor themes installed on the system.

use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    /// Theme directory (contains `cursors/`).
    pub dir: PathBuf,
    pub imported: bool,
}

pub fn imported_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "open-studio")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".open-studio"))
        .join("cursors")
}

fn has_cursors(dir: &Path) -> bool {
    let c = dir.join("cursors");
    c.is_dir() && (c.join("default").exists() || c.join("left_ptr").exists())
}

fn display_name(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("index.theme"))
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("Name=").map(|n| n.trim().to_string())))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
}

fn themes_in(root: &Path, imported: bool) -> Vec<Theme> {
    let mut out = vec![];
    let Ok(entries) = std::fs::read_dir(root) else { return out };
    for e in entries.flatten() {
        let dir = e.path();
        if has_cursors(&dir) {
            out.push(Theme { name: display_name(&dir), dir, imported });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Imported themes first, then system themes (deduplicated by name).
pub fn list() -> Vec<Theme> {
    let mut out = themes_in(&imported_dir(), true);
    let home = std::env::home_dir().unwrap_or_default();
    let mut roots = vec![home.join(".local/share/icons"), home.join(".icons")];
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    roots.extend(data.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("icons")));
    for r in roots {
        for t in themes_in(&r, false) {
            if !out.iter().any(|o| o.name == t.name) {
                out.push(t);
            }
        }
    }
    out
}

/// Finds a theme directory at or below `root` (archives often wrap the
/// theme in one or two extra folders).
fn find_root(root: &Path, depth: u32) -> Option<PathBuf> {
    if has_cursors(root) {
        return Some(root.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.into_iter().find_map(|d| find_root(&d, depth - 1))
}

fn is_hyprcursor(root: &Path, depth: u32) -> bool {
    if root.join("manifest.hl").exists() || root.join("manifest.toml").exists() {
        return true;
    }
    depth > 0
        && std::fs::read_dir(root)
            .map(|es| es.flatten().any(|e| e.path().is_dir() && is_hyprcursor(&e.path(), depth - 1)))
            .unwrap_or(false)
}

/// Imports an XCursor theme from a folder or an archive (anything bsdtar
/// reads: .tar.gz, .tar.xz, .tar.bz2, .zip, .7z).
pub fn import(src: &Path) -> Result<Theme> {
    let base = imported_dir();
    std::fs::create_dir_all(&base)?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis();
    let staging = base.join(format!(".import-{stamp}"));
    std::fs::create_dir_all(&staging)?;
    let result = (|| {
        let unpacked = if src.is_dir() {
            src.to_path_buf()
        } else {
            let out = Command::new("bsdtar").arg("-xf").arg(src).arg("-C").arg(&staging).output().context("running bsdtar")?;
            if !out.status.success() {
                bail!("couldn't extract archive: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            staging.clone()
        };
        let Some(root) = find_root(&unpacked, 3) else {
            if is_hyprcursor(&unpacked, 3) {
                bail!("this is a hyprcursor theme; only XCursor themes are supported");
            }
            bail!("no XCursor theme found (expected a folder with a cursors/ directory)");
        };
        let name = display_name(&root);
        let slug: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
        let dest = base.join(&slug);
        if dest.exists() {
            std::fs::remove_dir_all(&dest)?;
        }
        let out = Command::new("cp").arg("-rL").arg(&root).arg(&dest).output()?;
        if !out.status.success() {
            bail!("copying theme failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(Theme { name, dir: dest, imported: true })
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

pub fn remove(theme: &Theme) -> Result<()> {
    if !theme.imported || !theme.dir.starts_with(imported_dir()) {
        return Err(anyhow!("only imported themes can be removed"));
    }
    std::fs::remove_dir_all(&theme.dir)?;
    Ok(())
}
