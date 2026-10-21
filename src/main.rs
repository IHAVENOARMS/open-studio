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
mod recorder_ui;
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
        /// Record this monitor directly, without the portal picker (e.g. DP-1).
        #[arg(long)]
        display: Option<String>,
        /// With --display: record only this area, "x,y,w,h" in global logical pixels.
        #[arg(long)]
        area: Option<String>,
        /// Record a window directly by Hyprland address (e.g. 0x55d0...).
        #[arg(long)]
        window: Option<String>,
    },
    /// Stop the recording in progress.
    Stop,
    /// Show the floating recording bar (Display / Window / Area), record,
    /// and print the project directory when done.
    RecordUi,
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
    /// Press a key combination through a virtual uinput keyboard (development aid).
    #[command(hide = true)]
    DebugKeys { combo: String },
    /// Watch for a shortcut for a few seconds and report hits (development aid).
    #[command(hide = true)]
    DebugWatch { combo: String },
    /// Exercise pause/resume, restart and delete on a direct capture (development aid).
    #[command(hide = true)]
    DebugActions { display: String, dir: PathBuf },
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
        Some(Cmd::Record { out, fps, system_cursor, reuse, duration, test, display, area, window }) => {
            let direct = match (display, window) {
                (Some(d), _) => {
                    let area = area
                        .map(|a| -> Result<[f64; 4]> {
                            let v: Vec<f64> = a.split(',').map(|x| x.trim().parse()).collect::<Result<_, _>>()?;
                            anyhow::ensure!(v.len() == 4, "--area needs x,y,w,h");
                            Ok([v[0], v[1], v[2], v[3]])
                        })
                        .transpose()?;
                    Some(record::direct_monitor(&d, area)?)
                }
                (None, Some(w)) => Some(record::direct_window(&w)?),
                _ => None,
            };
            let opts = record::RecordOptions {
                direct,
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
        Some(Cmd::RecordUi) => match recorder_ui::run()? {
            Some(dir) => println!("{}", dir.display()),
            None => std::process::exit(2),
        },
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
        Some(Cmd::DebugActions { display, dir }) => {
            let sleep = |s: f64| std::thread::sleep(std::time::Duration::from_secs_f64(s));
            let opts = |name: &str| record::RecordOptions {
                out_dir: Some(dir.join(name)),
                direct: Some(record::direct_monitor(&display, None).unwrap()),
                ..Default::default()
            };
            // Pause: 1.5s recorded, 2s paused, 1.5s recorded.
            let r = record::Recorder::start(opts("pause"))?;
            sleep(1.5);
            r.pause();
            let at_pause = r.elapsed();
            sleep(2.0);
            let still = r.elapsed();
            r.resume();
            sleep(1.5);
            let p = r.stop()?;
            let rec = project::Recording::load(&p)?;
            println!("pause: elapsed frozen {:.2} -> {:.2}; video duration {:.2}s", at_pause, still, rec.duration);
            // Restart: first take discarded, second kept.
            let a = record::Recorder::start(opts("take1"))?;
            sleep(1.0);
            a.discard();
            let b = record::Recorder::start(opts("take2"))?;
            sleep(1.0);
            let kept = b.stop()?;
            println!("restart: take1 exists={} take2 exists={}", dir.join("take1").exists(), kept.join("screen.mp4").exists());
            // Delete.
            let d = record::Recorder::start(opts("deleted"))?;
            sleep(1.0);
            d.discard();
            println!("delete: dir exists={}", dir.join("deleted").exists());
        }
        Some(Cmd::DebugWatch { combo }) => {
            let sc = record::input::Shortcut::parse(&combo).ok_or_else(|| anyhow::anyhow!("bad combo"))?;
            println!("watching for {sc:?}");
            let hit = std::sync::Arc::new(AtomicBool::new(false));
            let stop = std::sync::Arc::new(AtomicBool::new(false));
            record::input::watch_shortcut(sc, hit.clone(), stop.clone());
            for _ in 0..80 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if hit.swap(false, Ordering::Relaxed) {
                    println!("hit");
                }
            }
            stop.store(true, Ordering::Relaxed);
        }
        Some(Cmd::DebugKeys { combo }) => {
            use evdev::{AttributeSet, EventType, InputEvent, KeyCode, uinput::VirtualDevice};
            let sc = record::input::Shortcut::parse(&combo).ok_or_else(|| anyhow::anyhow!("bad combo"))?;
            let mut keys = AttributeSet::<KeyCode>::new();
            for k in [KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTALT, KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_LEFTMETA, KeyCode::KEY_A, KeyCode::KEY_Z, sc.key] {
                keys.insert(k);
            }
            let mut dev = VirtualDevice::builder()?.name("open-studio test keyboard").with_keys(&keys)?.build()?;
            // Give device watchers (which rescan every 2s) time to see it.
            std::thread::sleep(std::time::Duration::from_millis(2500));
            let mods: Vec<KeyCode> = [(sc.ctrl, KeyCode::KEY_LEFTCTRL), (sc.alt, KeyCode::KEY_LEFTALT), (sc.shift, KeyCode::KEY_LEFTSHIFT), (sc.super_, KeyCode::KEY_LEFTMETA)]
                .into_iter()
                .filter(|(on, _)| *on)
                .map(|(_, k)| k)
                .collect();
            let press = |dev: &mut VirtualDevice, k: KeyCode, v: i32| dev.emit(&[InputEvent::new(EventType::KEY.0, k.0, v)]);
            for k in &mods {
                press(&mut dev, *k, 1)?;
            }
            press(&mut dev, sc.key, 1)?;
            std::thread::sleep(std::time::Duration::from_millis(50));
            press(&mut dev, sc.key, 0)?;
            for k in mods.iter().rev() {
                press(&mut dev, *k, 0)?;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
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
