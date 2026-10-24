//! Export several projects at once (Screen Studio 3.x): the chosen projects
//! export one after another into a folder with the export settings; the
//! rest wait for the previous one to finish.

use crate::settings::Settings;
use egui_wgpu::wgpu;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Waiting,
    /// Exporting, 0..1000.
    Exporting(u32),
    Done(PathBuf),
    Failed(String),
    Stopped,
}

pub struct Item {
    pub name: String,
    pub dir: PathBuf,
    pub status: Status,
}

#[derive(Clone)]
pub struct Batch {
    pub items: Arc<Mutex<Vec<Item>>>,
    pub stop: Arc<AtomicBool>,
}

impl Batch {
    pub fn finished(&self) -> bool {
        self.items.lock().unwrap().iter().all(|i| !matches!(i.status, Status::Waiting | Status::Exporting(_)))
    }
}

/// `dir/stem.ext`, or `dir/stem 2.ext`... if that's taken.
pub fn unique_file(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem} {n}.{ext}"));
        n += 1;
    }
    p
}

/// Starts exporting `projects` into `out_dir`, one at a time, on a worker
/// thread.
pub fn start(projects: Vec<PathBuf>, out_dir: PathBuf, settings: Settings, gpu: Option<(wgpu::Device, wgpu::Queue)>, repaint: Option<egui::Context>) -> Batch {
    let items: Vec<Item> = projects
        .into_iter()
        .map(|dir| Item { name: crate::project::ProjectPaths::new(&dir).name(), dir, status: Status::Waiting })
        .collect();
    let batch = Batch { items: Arc::new(Mutex::new(items)), stop: Arc::default() };
    let b = batch.clone();
    std::thread::spawn(move || {
        let mut comp = None;
        let n = b.items.lock().unwrap().len();
        for i in 0..n {
            let set = |s: Status| {
                b.items.lock().unwrap()[i].status = s;
                if let Some(c) = &repaint {
                    c.request_repaint();
                }
            };
            if b.stop.load(Ordering::Relaxed) {
                set(Status::Stopped);
                continue;
            }
            let (dir, name) = {
                let items = b.items.lock().unwrap();
                (items[i].dir.clone(), items[i].name.clone())
            };
            set(Status::Exporting(0));
            let res = (|| -> anyhow::Result<PathBuf> {
                let rec = crate::project::Recording::load(&dir)?;
                let edit = crate::edit::EditConfig::load_or_create(&dir, &rec);
                let (device, queue) = gpu.clone().ok_or_else(|| anyhow::anyhow!("no GPU"))?;
                let comp = comp.get_or_insert_with(|| crate::render::Compositor::new(device, queue));
                let _ = std::fs::create_dir_all(&out_dir);
                let out = unique_file(&out_dir, &name, settings.format.extension());
                let opts = crate::export::ExportOptions {
                    out: out.clone(),
                    height: settings.height.unwrap_or(rec.height.clamp(720, 2160)),
                    fps: settings.fps,
                    format: settings.format,
                    quality: settings.quality,
                    prores: settings.prores,
                };
                // Progress and stopping, passed through to this export.
                let progress = crate::export::Progress::default();
                let running = AtomicBool::new(true);
                std::thread::scope(|s| {
                    // Passes progress up and Stop down while this one exports.
                    s.spawn(|| {
                        while running.load(Ordering::Relaxed) {
                            if b.stop.load(Ordering::Relaxed) {
                                progress.cancel.store(true, Ordering::Relaxed);
                            }
                            b.items.lock().unwrap()[i].status = Status::Exporting(progress.permille.load(Ordering::Relaxed));
                            if let Some(c) = &repaint {
                                c.request_repaint();
                            }
                            std::thread::sleep(std::time::Duration::from_millis(150));
                        }
                    });
                    let r = crate::export::export(comp, &dir, &rec, &edit, &opts, &progress);
                    running.store(false, Ordering::Relaxed);
                    r
                })?;
                Ok(out)
            })();
            set(match res {
                Ok(out) => Status::Done(out),
                Err(_) if b.stop.load(Ordering::Relaxed) => Status::Stopped,
                Err(e) => Status::Failed(format!("{e:#}")),
            });
        }
    });
    batch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names() {
        let d = std::env::temp_dir().join("open-studio-batch-names");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(unique_file(&d, "Demo", "mp4"), d.join("Demo.mp4"));
        std::fs::write(d.join("Demo.mp4"), "").unwrap();
        assert_eq!(unique_file(&d, "Demo", "mp4"), d.join("Demo 2.mp4"));
        std::fs::write(d.join("Demo 2.mp4"), "").unwrap();
        assert_eq!(unique_file(&d, "Demo", "mp4"), d.join("Demo 3.mp4"));
        assert_eq!(unique_file(&d, "Demo", "gif"), d.join("Demo.gif"));
    }

    /// A project that can't be read fails without stopping the others;
    /// after Stop, the rest are marked stopped.
    #[test]
    fn failures_and_stopping() {
        let d = std::env::temp_dir().join("open-studio-batch-run");
        let b = start(vec![d.join("missing a"), d.join("missing b")], d.join("out"), Settings::default(), None, None);
        let t = std::time::Instant::now();
        while !b.finished() {
            assert!(t.elapsed().as_secs() < 10);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let items = b.items.lock().unwrap();
        assert!(items.iter().all(|i| matches!(i.status, Status::Failed(_))), "{:?}", items.iter().map(|i| &i.status).collect::<Vec<_>>());
        drop(items);
        let b = start(vec![d.join("x"), d.join("y")], d.join("out"), Settings::default(), None, None);
        b.stop.store(true, Ordering::Relaxed);
        while !b.finished() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // The first may have failed before Stop landed; the second is stopped.
        assert_eq!(b.items.lock().unwrap()[1].status, Status::Stopped);
    }

    /// Two real projects export one after the other into the folder.
    /// Opt-in (GPU and encoding): `cargo test -- --ignored exports_two`.
    #[test]
    #[ignore]
    fn exports_two_projects() {
        let tmp = std::env::temp_dir().join("open-studio-batch-e2e");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.join("data")) };
        unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.join("config")) };
        let mut dirs = vec![];
        for name in ["one", "two"] {
            let v = tmp.join(format!("{name}.mp4"));
            assert!(std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=1", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
                .arg(&v)
                .status()
                .unwrap()
                .success());
            dirs.push(crate::import::create_project(&v).unwrap());
        }
        let mut settings = Settings::default();
        settings.height = Some(720);
        settings.fps = 30;
        let out = tmp.join("out");
        let b = start(dirs, out.clone(), settings, Some(crate::gpu::headless().unwrap()), None);
        let t = std::time::Instant::now();
        while !b.finished() {
            assert!(t.elapsed().as_secs() < 120);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let items = b.items.lock().unwrap();
        for it in items.iter() {
            assert_eq!(it.status, Status::Done(out.join(format!("{}.mp4", it.name))), "{}", it.name);
        }
        assert!(out.join("one.mp4").exists() && out.join("two.mp4").exists());
    }
}
