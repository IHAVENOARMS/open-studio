mod app;
mod control;
mod cursor_themes;
mod editor;
mod settings;
mod theme;
mod decode;
mod dialogs;
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
mod xcursor;

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
    /// Print cursor image changes for a few seconds (development aid).
    #[command(hide = true)]
    DebugCursor {
        #[arg(long, default_value_t = 5.0)]
        seconds: f64,
        #[arg(long)]
        dump: Option<PathBuf>,
        /// Visit these global points ("x,y x,y ...") with real pointer motion.
        #[arg(long)]
        visit: Option<String>,
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
        /// mp4 or gif
        #[arg(long, default_value = "mp4")]
        format: String,
        /// studio, social, web or web-low
        #[arg(long, default_value = "social")]
        quality: String,
        /// Put the exported file on the clipboard.
        #[arg(long)]
        clipboard: bool,
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
        Some(Cmd::Export { project, out, height, fps, format, quality, clipboard }) => {
            let format = match format.as_str() {
                "gif" => settings::Format::Gif,
                "mp4" => settings::Format::Mp4,
                f => anyhow::bail!("unknown format {f}"),
            };
            let quality = match quality.as_str() {
                "studio" => settings::Quality::Studio,
                "social" => settings::Quality::Social,
                "web" => settings::Quality::Web,
                "web-low" => settings::Quality::WebLow,
                q => anyhow::bail!("unknown quality {q}"),
            };
            let rec = project::Recording::load(&project)?;
            let edit = edit::EditConfig::load_or_create(&project, &rec);
            let out = out.unwrap_or_else(|| project.join("export.mp4"));
            let (device, queue) = gpu::headless()?;
            let mut comp = render::Compositor::new(device, queue);
            let progress = export::Progress::default();
            let started = std::time::Instant::now();
            export::export(&mut comp, &project, &rec, &edit, &export::ExportOptions {
                    out: out.clone(),
                    height,
                    fps,
                    format,
                    quality,
                }, &progress)?;
            eprintln!("open-studio: exported in {:.1}s", started.elapsed().as_secs_f64());
            if clipboard {
                editor::copy_file_to_clipboard(&out).map_err(|e| anyhow::anyhow!(e))?;
            }
            println!("{}", out.display());
        }
        Some(Cmd::Edit { project }) => app::run(Some(project))?,
        Some(Cmd::DebugCursor { seconds, dump, visit }) => {
            let stop = std::sync::Arc::new(AtomicBool::new(false));
            let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            record::cursor_shape::spawn(stop.clone(), sink.clone())?;
            if let Some(points) = visit {
                visit_points(&points)?;
            } else {
                std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
            }
            stop.store(true, Ordering::Relaxed);
            for (i, c) in sink.lock().unwrap().iter().enumerate() {
                match &c.shape {
                    record::cursor_shape::Shape::Custom { width, height, rgba, hotspot, .. } => {
                        println!("{i}: t={} custom {width}x{height} hotspot={hotspot:?}", c.t_ns);
                        if let Some(d) = &dump {
                            std::fs::create_dir_all(d)?;
                            image::save_buffer(d.join(format!("{i}.png")), rgba, *width, *height, image::ExtendedColorType::Rgba8)?;
                        }
                    }
                    s => println!("{i}: t={} {s:?}", c.t_ns),
                }
            }
        }
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

/// Development aid: warps the pointer to each point, then nudges it through a
/// virtual uinput mouse so applications see real motion and update their
/// cursor. Restores the original position.
fn visit_points(points: &str) -> Result<()> {
    use evdev::{AttributeSet, EventType, InputEvent, RelativeAxisCode, uinput::VirtualDevice};
    let mut axes = AttributeSet::<RelativeAxisCode>::new();
    axes.insert(RelativeAxisCode::REL_X);
    axes.insert(RelativeAxisCode::REL_Y);
    let mut keys = AttributeSet::<evdev::KeyCode>::new();
    keys.insert(evdev::KeyCode::BTN_LEFT);
    let mut dev = VirtualDevice::builder()?.name("open-studio test pointer").with_relative_axes(&axes)?.with_keys(&keys)?.build()?;
    std::thread::sleep(std::time::Duration::from_millis(400));
    let orig = record::hyprland::cursor_pos()?;
    let warp = |x: f64, y: f64| {
        record::hyprland::dispatch(&format!("hl.dsp.cursor.move({{x={x},y={y}}})"), &format!("movecursor {x} {y}"))
    };
    for p in points.split_whitespace() {
        // A trailing "!" clicks at the point.
        let (p, click) = match p.strip_suffix('!') {
            Some(p) => (p, true),
            None => (p, false),
        };
        let Some((x, y)) = p.split_once(',') else { continue };
        let (x, y): (f64, f64) = (x.parse()?, y.parse()?);
        warp(x - 2.0, y)?;
        std::thread::sleep(std::time::Duration::from_millis(60));
        for _ in 0..2 {
            dev.emit(&[InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, 1)])?;
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        if click {
            for v in [1, 0] {
                dev.emit(&[InputEvent::new(EventType::KEY.0, evdev::KeyCode::BTN_LEFT.0, v)])?;
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(900));
    }
    warp(orig.0, orig.1)?;
    std::thread::sleep(std::time::Duration::from_millis(300));
    Ok(())
}
