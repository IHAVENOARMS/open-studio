mod app;
mod bidi;
mod clicks;
mod clipboard;
mod control;
mod cursor_themes;
mod editor;
mod settings;
mod shortcuts;
mod cutout;
mod face;
mod theme;
mod voice;
mod transcript;
mod decode;
mod dialogs;
mod edit;
mod engine;
mod export;
mod gpu;
mod hwenc;
mod icons;
mod presets;
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
        /// Record a microphone ("default" or a Pulse source name).
        #[arg(long)]
        mic: Option<String>,
        /// Record a camera: an id from `--camera list`, e.g. v4l2:/dev/video1 or phone:SERIAL:front.
        #[arg(long)]
        camera: Option<String>,
        /// Record system audio (what the computer plays).
        #[arg(long)]
        system_audio: bool,
    },
    /// Stop the recording in progress.
    Stop,
    /// Mark a recording flag in the recording in progress.
    Flag,
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
    /// Mirror the first Android device, record its window briefly, report (development aid).
    #[command(hide = true)]
    DebugDevice,
    /// Remove the camera background of a project (development aid).
    #[command(hide = true)]
    DebugCutout { project: PathBuf },
    /// Run face tracking on a project's camera video (development aid).
    #[command(hide = true)]
    DebugFace { project: PathBuf },
    /// Open a project in the editor.
    Edit { project: PathBuf },
    /// Transcribe a project's microphone track with Whisper (for captions).
    Transcribe {
        project: PathBuf,
        /// base, small, medium or large-v3-turbo
        #[arg(long, default_value = "base")]
        model: String,
        /// Language code such as "en"; detected when omitted.
        #[arg(long, default_value = "")]
        language: String,
        /// Write captions as SubRip to this file.
        #[arg(long)]
        srt: Option<PathBuf>,
    },
    /// Render a project to MP4 without opening the editor.
    /// Save one frame of the edited video as a PNG image.
    Frame {
        project: PathBuf,
        /// Recording time in seconds.
        #[arg(long, default_value_t = 0.0)]
        time: f64,
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[arg(long, default_value_t = 1080)]
        height: u32,
    },
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
    // Recording and export ask for it later; find the GPU encoder meanwhile.
    hwenc::detect_in_background();
    match cli.cmd {
        Some(Cmd::Record { out, fps, system_cursor, reuse, duration, test, display, area, window, mic, system_audio, camera }) => {
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
                microphone: mic.map(|m| if m == "default" { record::audio::DEFAULT_MIC.to_string() } else { m }),
                system_audio,
                // Headless recordings are saved straight into the projects folder.
                out_dir: out.or_else(|| Some(util::unique_path(&util::projects_dir(), &format!("Recording {}", util::local_timestamp())))),
                fps,
                show_system_cursor: system_cursor,
                reuse_selection: reuse,
                test_source: test,
                gpu_encoding: settings::Settings::load().gpu_encoding,
                device_audio: None,
                flag_shortcut: Some(settings::Settings::load().flag_shortcut),
                camera: match camera.as_deref() {
                    None => None,
                    Some("list") => {
                        for c in record::camera::list() {
                            println!("{}  {}", c.id(), c.label());
                        }
                        return Ok(());
                    }
                    Some(id) => {
                        let src = record::camera::list().into_iter().find(|c| c.id() == id).ok_or_else(|| anyhow::anyhow!("no camera {id}"))?;
                        Some(std::sync::Arc::new(record::camera::CameraFeed::open(&src)?))
                    }
                },
            };
            let dir = record_blocking(opts, duration)?;
            println!("{}", dir.display());
        }
        Some(Cmd::Stop) => control::send_stop()?,
        Some(Cmd::Flag) => control::send_flag()?,
        Some(Cmd::RecordUi) => match recorder_ui::run()? {
            recorder_ui::Outcome::Recorded(dir) => println!("{}", dir.display()),
            recorder_ui::Outcome::Cancelled => std::process::exit(2),
            recorder_ui::Outcome::OpenProjects => std::process::exit(3),
        },
        Some(Cmd::DebugDevice) => {
            let t = std::time::Instant::now();
            let devices = record::device::discover(&settings::Settings::load().known_devices);
            println!("scrcpy: {:?}\ndevices ({:.1}s): {devices:?}", record::device::scrcpy(), t.elapsed().as_secs_f32());
            let d = devices.first().ok_or_else(|| anyhow::anyhow!("no device"))?;
            let mirror = record::device::Mirror::open(d, &settings::Settings::load().scrcpy)?;
            let mut win = None;
            for _ in 0..100 {
                win = mirror.window();
                if win.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let w = win.ok_or_else(|| anyhow::anyhow!("mirror window never appeared"))?;
            let floating = hyprland_floating(&w.address);
            println!("window {} {:?} {:?} floating={floating}", w.class, w.at, w.size);
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let mut req = record::direct_window(&w.address)?;
            req.kind = "device".into();
            let r = record::Recorder::start(record::RecordOptions { direct: Some(req), ..Default::default() })?;
            std::thread::sleep(std::time::Duration::from_secs(3));
            let dir = r.stop()?;
            let rec = project::Recording::load(&dir)?;
            println!("recorded {}x{} {:.2}s kind={}", rec.width, rec.height, rec.duration, rec.source.kind);
            std::fs::remove_dir_all(&dir)?;
        }
        Some(Cmd::DebugCutout { project }) => {
            let rec = project::Recording::load(&project)?;
            let t = std::time::Instant::now();
            cutout::process(&rec, &Default::default())?;
            println!("matte written in {:.1}s", t.elapsed().as_secs_f32());
        }
        Some(Cmd::DebugFace { project }) => {
            let rec = project::Recording::load(&project)?;
            let t = std::time::Instant::now();
            let track = face::analyze(&rec, &Default::default())?;
            println!("{} face samples in {:.1}s", track.samples.len(), t.elapsed().as_secs_f32());
        }
        Some(Cmd::Transcribe { project, model, language, srt }) => {
            let rec = project::Recording::load(&project)?;
            let t = transcript::generate(&rec, &model, &language, "", &Default::default())?;
            t.save(&project)?;
            println!("open-studio: {} words ({})", t.words.len(), t.language);
            if let Some(path) = srt {
                let edit = edit::EditConfig::load_or_create(&project, &rec);
                std::fs::write(&path, transcript::srt(&t, &edit, rec.duration))?;
            }
        }
        Some(Cmd::Frame { project, time, out, height }) => {
            let rec = project::Recording::load(&project)?;
            let edit = edit::EditConfig::load_or_create(&project, &rec);
            let out = out.unwrap_or_else(|| project.join("frame.png"));
            let (device, queue) = gpu::headless()?;
            let mut comp = render::Compositor::new(device, queue);
            export::export_frame(&mut comp, &project, &rec, &edit, time, height, &out)?;
            println!("{}", out.display());
        }
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
        None => start()?,
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
        for _ in 0..control.take_flags() {
            rec.flag();
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

/// Default entry point: the recording bar first (like Screen Studio), then
/// the editor for what was recorded. Without layer-shell support the
/// projects window opens instead.
fn start() -> Result<()> {
    if record::hyprland::available() {
        match recorder_ui::run() {
            Ok(recorder_ui::Outcome::Recorded(dir)) => app::run(Some(dir))?,
            Ok(recorder_ui::Outcome::OpenProjects) => app::run(None)?,
            Ok(recorder_ui::Outcome::Cancelled) => return Ok(()),
            Err(e) => {
                eprintln!("open-studio: recording bar unavailable ({e:#}); opening projects");
                app::run(None)?;
            }
        }
    } else {
        app::run(None)?;
    }
    if app::RELAUNCH_RECORDER.load(Ordering::Relaxed) {
        // A fresh process: the windowing event loop can't be restarted in-process.
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(std::env::current_exe()?).exec();
        anyhow::bail!("relaunching failed: {err}");
    }
    Ok(())
}

fn hyprland_floating(address: &str) -> bool {
    record::hyprland::request("j/clients")
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_array()?.iter().find(|c| c["address"] == address)?.get("floating")?.as_bool())
        .unwrap_or(false)
}
