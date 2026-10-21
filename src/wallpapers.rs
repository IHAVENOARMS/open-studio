//! Wallpaper library. The curated list (with licences and credits) ships in
//! `assets/wallpapers.json`; the images themselves are downloaded on demand
//! into the user's data directory, resized to at most 4K, with thumbnails.
//! Wallpapers the user adds live in the same directory, listed in
//! `custom.json`, under the "Custom" category.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{OnceLock, RwLock};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const CUSTOM_CATEGORY: &str = "Custom";

#[derive(Serialize, Deserialize, Clone, Debug)]
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

fn builtin() -> &'static [Wallpaper] {
    static LIST: OnceLock<Vec<Wallpaper>> = OnceLock::new();
    LIST.get_or_init(|| serde_json::from_str(include_str!("../assets/wallpapers.json")).unwrap_or_default())
}

static CUSTOM: RwLock<Option<Vec<Wallpaper>>> = RwLock::new(None);

fn custom_file() -> PathBuf {
    data_dir().join("custom.json")
}

/// Wallpapers the user added, newest first.
pub fn custom() -> Vec<Wallpaper> {
    if let Some(list) = CUSTOM.read().unwrap().as_ref() {
        return list.clone();
    }
    let list: Vec<Wallpaper> =
        std::fs::read_to_string(custom_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    *CUSTOM.write().unwrap() = Some(list.clone());
    list
}

fn save_custom(list: Vec<Wallpaper>) -> Result<()> {
    std::fs::create_dir_all(data_dir())?;
    std::fs::write(custom_file(), serde_json::to_string_pretty(&list)?)?;
    *CUSTOM.write().unwrap() = Some(list);
    Ok(())
}

/// Built-in and custom wallpapers.
pub fn manifest() -> Vec<Wallpaper> {
    let mut all = builtin().to_vec();
    all.extend(custom());
    all
}

pub fn get(id: &str) -> Option<Wallpaper> {
    manifest().into_iter().find(|w| w.id == id)
}

/// Categories in manifest order, with "Custom" last.
pub fn categories() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in builtin() {
        if !out.contains(&w.category) {
            out.push(w.category.clone());
        }
    }
    out.push(CUSTOM_CATEGORY.into());
    out
}

/// Imports a local image as a custom wallpaper.
pub fn add_custom(path: &Path) -> Result<Wallpaper> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis();
    let title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Wallpaper".into());
    let w = Wallpaper {
        id: format!("custom-{stamp}"),
        category: CUSTOM_CATEGORY.into(),
        title,
        url: String::new(),
        author: String::new(),
        license: String::new(),
    };
    convert(path, &w.id)?;
    let mut list = custom();
    list.insert(0, w.clone());
    save_custom(list)?;
    Ok(w)
}

pub fn remove_custom(id: &str) -> Result<()> {
    let list: Vec<Wallpaper> = custom().into_iter().filter(|w| w.id != id).collect();
    let _ = std::fs::remove_file(image_path(id));
    let _ = std::fs::remove_file(thumb_path(id));
    save_custom(list)
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

/// Built-in wallpapers not downloaded yet.
pub fn missing() -> Vec<&'static Wallpaper> {
    builtin().iter().filter(|w| !installed(&w.id)).collect()
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
    let res = convert(&tmp, &w.id);
    let _ = std::fs::remove_file(&tmp);
    res
}

/// Produces the stored 4K JPEG and the thumbnail for wallpaper `id`.
fn convert(src: &Path, id: &str) -> Result<()> {
    std::fs::create_dir_all(data_dir().join("thumbs"))?;
    let one = |out: &Path, filter: &str, q: &str| {
        run(Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(src)
            .args(["-frames:v", "1", "-vf", filter, "-q:v", q, "-pix_fmt", "yuvj444p"])
            .arg(out))
    };
    one(&image_path(id), "scale='min(3840,iw)':-2:flags=lanczos", "2")?;
    one(&thumb_path(id), "scale=320:-2:flags=lanczos", "3")
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
