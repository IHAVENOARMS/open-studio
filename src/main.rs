mod app;
mod control;
mod editor;
mod theme;
mod decode;
mod edit;
mod engine;
mod export;
mod gpu;
mod icons;
mod project;
mod record;
mod render;
mod util;
mod wallpapers;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Parser)]
#[command(name = "open-studio", about = "Screen recorder with automatic cursor zoom")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Record the screen until Ctrl+C or `open-studio stop`.
    Record {
        /// Project directory to write (default: ~/Videos/Open Studio/Recording <date>)
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 60)]
        fps: u32,
        /// Bake the real system cursor into the video instead of rendering a smooth one.
        #[arg(long)]
        system_cursor: bool,
        /// Reuse the last picked screen without showing the picker.
        #[arg(long)]
        reuse: bool,
        /// Stop automatically after this many seconds.
        #[arg(long)]
        duration: Option<f64>,
        /// Record a synthetic test pattern (no portal).
        #[arg(long, hide = true)]
        test: bool,
    },
    /// Stop the recording in progress.
    Stop,
    /// Download the bundled wallpaper collection (or list it with --list).
    Wallpapers {
        #[arg(long)]
        list: bool,
    },
    /// Open a project in the editor.
    Edit { project: PathBuf },
    /// Render a project to MP4 without opening the editor.
    Export {
        project: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 1080)]
        height: u32,
        #[arg(long, default_value_t = 60)]
        fps: u32,
    },
}

static SIGINT: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    SIGINT.store(true, Ordering::Relaxed);
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::Record { out, fps, system_cursor, reuse, duration, test }) => {
            let opts = record::RecordOptions {
                out_dir: out,
                fps,
                show_system_cursor: system_cursor,
                reuse_selection: reuse,
                test_source: test,
            };
            let dir = record_blocking(opts, duration)?;
            println!("{}", dir.display());
        }
        Some(Cmd::Stop) => control::send_stop()?,
        Some(Cmd::Export { project, out, height, fps }) => {
            let rec = project::Recording::load(&project)?;
            let edit = edit::EditConfig::load_or_create(&project, &rec);
            let out = out.unwrap_or_else(|| project.join("export.mp4"));
            let (device, queue) = gpu::headless()?;
            let mut comp = render::Compositor::new(device, queue);
            let progress = export::Progress::default();
            let started = std::time::Instant::now();
            export::export(&mut comp, &project, &rec, &edit, &export::ExportOptions { out: out.clone(), height, fps }, &progress)?;
            eprintln!("open-studio: exported in {:.1}s", started.elapsed().as_secs_f64());
            println!("{}", out.display());
        }
        Some(Cmd::Edit { project }) => app::run(Some(project))?,
        Some(Cmd::Wallpapers { list }) => {
            if list {
                for w in wallpapers::manifest() {
                    let mark = if wallpapers::installed(&w.id) { "installed" } else { "missing" };
                    println!("{:<10} {:<36} {:<9} {} ({})", w.category, w.id, mark, w.author, w.license);
                }
            } else {
                let total = wallpapers::missing().len();
                let done = std::sync::atomic::AtomicUsize::new(0);
                eprintln!("open-studio: downloading {total} wallpapers to {}", wallpapers::data_dir().display());
                let errors = wallpapers::fetch_missing(&done);
                for e in &errors {
                    eprintln!("  failed: {e}");
                }
                eprintln!("open-studio: {} downloaded, {} failed", total - errors.len(), errors.len());
            }
        }
        None => app::run(None)?,
    }
    Ok(())
}

fn record_blocking(opts: record::RecordOptions, duration: Option<f64>) -> Result<PathBuf> {
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_sigint as *const () as libc::sighandler_t);
    }
    let control = control::ControlServer::start()?;
    let rec = record::Recorder::start(opts)?;
    eprintln!("open-studio: recording… stop with Ctrl+C or `open-studio stop`");
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if let Some(e) = rec.error() {
            anyhow::bail!("recording failed: {e}");
        }
        if SIGINT.load(Ordering::Relaxed) || control.take_stop() {
            break;
        }
        if duration.is_some_and(|d| rec.elapsed() >= d) {
            break;
        }
    }
    rec.stop()
}
