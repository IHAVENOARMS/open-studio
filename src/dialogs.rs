//! Desktop file dialogs through the xdg-desktop-portal FileChooser, run on a
//! worker thread. Each call returns a receiver that yields the result once.

use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

fn run<F>(ctx: egui::Context, job: F) -> Receiver<Vec<PathBuf>>
where
    F: AsyncFnOnce() -> ashpd::Result<SelectedFiles> + Send + 'static,
{
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let files = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()
            .and_then(|rt| match rt.block_on(job()) {
                Ok(f) => Some(f),
                Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => None,
                Err(e) => {
                    eprintln!("open-studio: file dialog failed: {e}");
                    None
                }
            })
            .map(|f| f.uris().iter().filter_map(|u| uri_to_path(u.as_str())).collect())
            .unwrap_or_default();
        let _ = tx.send(files);
        ctx.request_repaint();
    });
    rx
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    uri.strip_prefix("file://").map(|p| PathBuf::from(percent_decode(p)))
}

pub fn images_filter() -> FileFilter {
    FileFilter::new("Images").mimetype("image/jpeg").mimetype("image/png").mimetype("image/webp")
}

/// Choose one or more existing files.
pub fn open_files(ctx: egui::Context, title: &'static str, filter: Option<FileFilter>, multiple: bool) -> Receiver<Vec<PathBuf>> {
    run(ctx, async move || {
        let mut req = SelectedFiles::open_file().title(title).modal(true).multiple(multiple);
        if let Some(f) = filter {
            req = req.filter(f);
        }
        req.send().await?.response()
    })
}

/// Choose a folder.
pub fn open_folder(ctx: egui::Context, title: &'static str, start: Option<PathBuf>) -> Receiver<Vec<PathBuf>> {
    run(ctx, async move || {
        let req = SelectedFiles::open_file().title(title).modal(true).directory(true);
        let req = match &start {
            Some(dir) => req.current_folder(dir)?,
            None => req,
        };
        req.send().await?.response()
    })
}

/// Choose where to save a file.
pub fn save_file(ctx: egui::Context, title: &'static str, folder: &Path, name: &str) -> Receiver<Vec<PathBuf>> {
    let (folder, name) = (folder.to_path_buf(), name.to_string());
    run(ctx, async move || {
        SelectedFiles::save_file()
            .title(title)
            .modal(true)
            .current_name(name.as_str())
            .current_folder(&folder)?
            .send()
            .await?
            .response()
    })
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
