//! Wallpaper library. The curated list (with licences and credits) ships in
//! `assets/wallpapers.json`; the images themselves are downloaded on demand
//! into the user's data directory, resized to at most 4K, with thumbnails.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Deserialize, Clone, Debug)]
pub struct Wallpaper {
    pub id: String,
    pub category: String,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub license: String,
}

pub fn manifest() -> &'static [Wallpaper] {
    static LIST: OnceLock<Vec<Wallpaper>> = OnceLock::new();
    LIST.get_or_init(|| serde_json::from_str(include_str!("../assets/wallpapers.json")).unwrap_or_default())
}

pub fn get(id: &str) -> Option<&'static Wallpaper> {
    manifest().iter().find(|w| w.id == id)
}

/// Categories in manifest order.
pub fn categories() -> Vec<&'static str> {
    let mut out: Vec<&str> = Vec::new();
    for w in manifest() {
        if !out.contains(&w.category.as_str()) {
            out.push(&w.category);
        }
    }
    out
}

pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "open-studio")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".open-studio"))
        .join("wallpapers")
}

pub fn image_path(id: &str) -> PathBuf {
    data_dir().join(format!("{id}.jpg"))
}

pub fn thumb_path(id: &str) -> PathBuf {
    data_dir().join("thumbs").join(format!("{id}.jpg"))
}

pub fn installed(id: &str) -> bool {
    image_path(id).exists() && thumb_path(id).exists()
}

pub fn missing() -> Vec<&'static Wallpaper> {
    manifest().iter().filter(|w| !installed(&w.id)).collect()
}

fn run(cmd: &mut Command) -> Result<()> {
    let out = cmd.output().with_context(|| format!("running {:?}", cmd.get_program()))?;
    if !out.status.success() {
        bail!("{:?} failed: {}", cmd.get_program(), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Downloads one wallpaper and produces the 4K image and its thumbnail.
pub fn fetch(w: &Wallpaper) -> Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(dir.join("thumbs"))?;
    let tmp = dir.join(format!(".{}.download", w.id));
    run(Command::new("curl")
        .args(["-fsSL", "--retry", "2", "--max-time", "120", "-A", "OpenStudio/0.1 (open-source screen recorder; wallpaper download)", "-o"])
        .arg(&tmp)
        .arg(&w.url))
    .with_context(|| format!("downloading {}", w.title))?;
    let convert = |out: &Path, filter: &str, q: &str| {
        run(Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(&tmp)
            .args(["-frames:v", "1", "-vf", filter, "-q:v", q, "-pix_fmt", "yuvj444p"])
            .arg(out))
    };
    let res = convert(&image_path(&w.id), "scale='min(3840,iw)':-2:flags=lanczos", "2")
        .and_then(|_| convert(&thumb_path(&w.id), "scale=320:-2:flags=lanczos", "3"));
    let _ = std::fs::remove_file(&tmp);
    res
}

/// Downloads every missing wallpaper with a few parallel workers. `done` is
/// incremented per finished item (successful or not); errors are returned.
pub fn fetch_missing(done: &AtomicUsize) -> Vec<String> {
    let todo = missing();
    let next = AtomicUsize::new(0);
    let errors = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..3 {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(w) = todo.get(i) else { break };
                    if let Err(e) = fetch(w) {
                        errors.lock().unwrap().push(format!("{}: {e:#}", w.id));
                    }
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    errors.into_inner().unwrap()
}

/// File behind a background reference: a wallpaper id or an absolute path.
pub fn resolve(reference: &str) -> PathBuf {
    if reference.starts_with('/') { PathBuf::from(reference) } else { image_path(reference) }
}
