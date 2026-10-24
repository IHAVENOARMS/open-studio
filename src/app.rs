//! Top-level window: home screen, recording state and the editor.

use crate::control::ControlServer;
use crate::editor::{Action, Editor, Leaving};
use crate::icons::{self, Icon};
use crate::project::Recording;
use crate::record::{RecordOptions, Recorder, hyprland};
use crate::theme;
use crate::util::format_time;
use egui::{Color32, CornerRadius, RichText, Sense, vec2};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

enum RecPhase {
    Selecting,
    Recording { since: Instant },
    Saving,
}

struct RecordingSession {
    phase: Arc<Mutex<RecPhase>>,
    stop: Arc<AtomicBool>,
    done: Receiver<Result<PathBuf, String>>,
}

enum Mode {
    Home,
    Recording(RecordingSession),
    Editor(Box<Editor>),
}

struct ProjectEntry {
    dir: PathBuf,
    name: String,
    duration: f64,
    modified: std::time::SystemTime,
    unsaved: bool,
}

pub struct App {
    mode: Mode,
    projects: Vec<ProjectEntry>,
    options: RecordOptions,
    hide_while_recording: bool,
    error: Option<String>,
    control: Option<Arc<ControlServer>>,
    /// The unsaved-changes prompt has been answered; let the window close.
    may_close: bool,
    picking_projects_dir: Option<Receiver<Vec<PathBuf>>>,
}

/// Saved projects, plus recordings never saved (left over when the app was
/// closed abruptly), which are listed so they can be saved or deleted.
fn scan_projects() -> Vec<ProjectEntry> {
    let mut out = Vec::new();
    for (root, unsaved) in [(crate::util::projects_dir(), false), (crate::util::unsaved_dir(), true)] {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for e in entries.flatten() {
            let dir = e.path();
            let Ok(rec) = Recording::load(&dir) else { continue };
            let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            let name = crate::project::ProjectPaths::new(&dir).name();
            out.push(ProjectEntry { dir, name, duration: rec.duration, modified, unsaved });
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

impl App {
    pub fn new(cc: &eframe::CreationContext, open: Option<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);
        let mut app = Self {
            mode: Mode::Home,
            projects: scan_projects(),
            options: RecordOptions::default(),
            hide_while_recording: hyprland::available(),
            error: None,
            control: ControlServer::start().ok().map(Arc::new),
            may_close: false,
            picking_projects_dir: None,
        };
        if let (Some(dir), Some(rs)) = (open, cc.wgpu_render_state.as_ref()) {
            app.open_editor(dir, &cc.egui_ctx, rs);
        }
        app
    }

    fn open_editor(&mut self, dir: PathBuf, ctx: &egui::Context, rs: &egui_wgpu::RenderState) {
        match Editor::open(dir, ctx, rs) {
            Ok(ed) => self.mode = Mode::Editor(Box::new(ed)),
            Err(e) => self.error = Some(format!("Couldn't open project: {e:#}")),
        }
    }

    fn start_recording(&mut self, ctx: &egui::Context) {
        let phase = Arc::new(Mutex::new(RecPhase::Selecting));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, done) = channel();
        let (ph, st, opts, hide, ctx2) = (phase.clone(), stop.clone(), self.options.clone(), self.hide_while_recording, ctx.clone());
        let control = self.control.clone();
        if let Some(c) = &control {
            c.take_stop();
            c.take_flags();
        }
        std::thread::spawn(move || {
            // On Hyprland the floating recording bar (direct capture) is used;
            // elsewhere the portal flow.
            let res = if hyprland::available() { supervise_bar(&ctx2) } else { supervise(opts, hide, ph, st, control, &ctx2) };
            let _ = tx.send(res.map_err(|e| format!("{e:#}")));
            ctx2.request_repaint();
        });
        self.mode = Mode::Recording(RecordingSession { phase, stop, done });
    }

    fn home(&mut self, ui: &mut egui::Ui, rs: &egui_wgpu::RenderState) {
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::same(32)))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(560.0);
                    ui.add_space(20.0);
                    ui.label(RichText::new("Open Studio").font(theme::semibold(28.0)));
                    ui.label(RichText::new("Beautiful screen recordings with automatic zoom").color(theme::TEXT_DIM).size(14.0));
                    ui.add_space(24.0);
                    let rec_btn = icons::button(ui, Icon::Record, "Start recording", theme::semibold(15.0), Color32::WHITE, Some(theme::ACCENT), vec2(260.0, 46.0));
                    if rec_btn.clicked() {
                        self.start_recording(&ctx);
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 470.0).max(0.0) / 2.0);
                        ui.checkbox(&mut self.options.reuse_selection, "Reuse last screen");
                        ui.checkbox(&mut self.options.show_system_cursor, "Bake in system cursor");
                        if hyprland::available() {
                            ui.checkbox(&mut self.hide_while_recording, "Hide this window");
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 360.0).max(0.0) / 2.0);
                        let mut s = crate::settings::Settings::load();
                        if ui
                            .checkbox(&mut s.use_last_project_settings, "Use last project settings as default for new recordings")
                            .on_hover_text("New recordings start with the background, cursor, camera and caption settings of the project you edited last.")
                            .changed()
                        {
                            s.save();
                        }
                    });
                    ui.label(
                        RichText::new(format!(
                            "Stop with {} or the recording controls. `open-studio stop` works too.",
                            crate::settings::Settings::load().stop_shortcut
                        ))
                            .color(theme::TEXT_DIM)
                            .size(12.0),
                    );
                    if let Some(e) = &self.error {
                        ui.add_space(8.0);
                        ui.colored_label(theme::RECORD, e);
                    }
                    ui.add_space(28.0);
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Recent projects").font(theme::semibold(14.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(rx) = &self.picking_projects_dir {
                            if let Ok(f) = rx.try_recv() {
                                if let Some(dir) = f.into_iter().next() {
                                    let mut s = crate::settings::Settings::load();
                                    s.projects_dir = dir;
                                    s.save();
                                    self.projects = scan_projects();
                                }
                                self.picking_projects_dir = None;
                            }
                        }
                        if ui.button("Change").on_hover_text("Folder where saved projects go").clicked() && self.picking_projects_dir.is_none() {
                            self.picking_projects_dir = Some(crate::dialogs::open_folder(ctx.clone(), "Projects folder", Some(crate::util::projects_dir())));
                        }
                        ui.label(RichText::new(crate::util::projects_dir().display().to_string()).size(12.0).color(theme::TEXT_DIM));
                    });
                });
                ui.add_space(6.0);
                let mut open = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.projects.is_empty() {
                        ui.label(RichText::new("No recordings yet.").color(theme::TEXT_DIM));
                    }
                    for p in &self.projects {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
                        let fill = if resp.hovered() { theme::SURFACE_HOVER } else { theme::PANEL };
                        ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
                        let g = ui.painter().text(rect.left_center() + vec2(16.0, 0.0), egui::Align2::LEFT_CENTER, &p.name, egui::FontId::proportional(14.0), theme::TEXT);
                        if p.unsaved {
                            ui.painter().text(g.right_center() + vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, "Not saved", egui::FontId::proportional(12.5), theme::TEXT_DIM);
                        }
                        ui.painter().text(rect.right_center() - vec2(16.0, 0.0), egui::Align2::RIGHT_CENTER, format_time(p.duration), egui::FontId::proportional(13.0), theme::TEXT_DIM);
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            open = Some(p.dir.clone());
                        }
                    }
                });
                if let Some(dir) = open {
                    self.open_editor(dir, &ctx, rs);
                }
            });
    }

    fn recording(&mut self, ui: &mut egui::Ui, rs: &egui_wgpu::RenderState) {
        let ctx = ui.ctx().clone();
        let Mode::Recording(session) = &self.mode else { return };
        if let Ok(res) = session.done.try_recv() {
            self.projects = scan_projects();
            match res {
                Ok(dir) => self.open_editor(dir, &ctx, rs),
                Err(e) => {
                    // An empty error means the user cancelled.
                    self.error = (!e.is_empty()).then_some(e);
                    self.mode = Mode::Home;
                }
            }
            return;
        }
        let (text, timer) = match &*session.phase.lock().unwrap() {
            RecPhase::Selecting => ("Choose what to record…".to_string(), None),
            RecPhase::Recording { since } => ("Recording".to_string(), Some(since.elapsed().as_secs_f64())),
            RecPhase::Saving => ("Saving…".to_string(), None),
        };
        let stop = session.stop.clone();
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG)).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 160.0).max(0.0) / 2.0);
                    if timer.is_some() {
                        let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 6.0, theme::RECORD);
                    }
                    ui.label(RichText::new(&text).font(theme::semibold(18.0)));
                });
                if let Some(t) = timer {
                    ui.label(RichText::new(format_time(t)).size(34.0));
                    ui.add_space(12.0);
                    let b = icons::button(ui, Icon::Stop, "Stop recording", theme::semibold(14.0), Color32::WHITE, Some(theme::RECORD), vec2(220.0, 42.0));
                    if b.clicked() {
                        stop.store(true, Ordering::Relaxed);
                    }
                } else {
                    ui.add(egui::Spinner::new().size(28.0));
                }
            });
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Runs the floating recording bar (`open-studio record-ui`) as a child
/// process with this window moved out of the way, and returns the project it
/// recorded. A cancelled recording is reported as an empty error.
fn supervise_bar(ctx: &egui::Context) -> anyhow::Result<PathBuf> {
    let pid = std::process::id();
    let _ = hyprland::move_window_to_workspace(pid, "special:open-studio", false);
    let out = std::process::Command::new(std::env::current_exe()?)
        .arg("record-ui")
        .stdin(std::process::Stdio::null())
        .output();
    if let Ok(ws) = hyprland::active_workspace() {
        let _ = hyprland::move_window_to_workspace(pid, &ws.to_string(), true);
    }
    ctx.request_repaint();
    let out = out?;
    match out.status.code() {
        Some(0) => {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            anyhow::ensure!(!dir.is_empty(), "recorder returned no project");
            Ok(PathBuf::from(dir))
        }
        Some(2) | Some(3) => anyhow::bail!(""),
        _ => anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim()),
    }
}

/// Runs a whole recording on a worker thread: portal selection, capture,
/// waiting for a stop request (UI button, tray, or `open-studio stop`) and
/// writing the project.
fn supervise(
    opts: RecordOptions,
    hide: bool,
    phase: Arc<Mutex<RecPhase>>,
    stop: Arc<AtomicBool>,
    control: Option<Arc<ControlServer>>,
    ctx: &egui::Context,
) -> anyhow::Result<PathBuf> {
    let recorder = Recorder::start(opts)?;
    *phase.lock().unwrap() = RecPhase::Recording { since: Instant::now() };
    ctx.request_repaint();
    let tray = control.as_ref().and_then(|c| c.tray());
    let pid = std::process::id();
    if hide {
        let _ = hyprland::move_window_to_workspace(pid, "special:open-studio", false);
    }
    let mut failure = None;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if let Some(e) = recorder.error() {
            failure = Some(e);
            break;
        }
        if stop.load(Ordering::Relaxed) || control.as_ref().is_some_and(|c| c.take_stop()) {
            break;
        }
        for _ in 0..control.as_ref().map_or(0, |c| c.take_flags()) {
            recorder.flag();
        }
    }
    drop(tray);
    *phase.lock().unwrap() = RecPhase::Saving;
    if hide {
        if let Ok(ws) = hyprland::active_workspace() {
            let _ = hyprland::move_window_to_workspace(pid, &ws.to_string(), true);
        }
    }
    ctx.request_repaint();
    let dir = recorder.stop();
    if let Some(e) = failure {
        anyhow::bail!("recording failed: {e}");
    }
    dir
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let Some(rs) = frame.wgpu_render_state().cloned() else {
            ui.label("Open Studio needs the wgpu renderer.");
            return;
        };
        // Closing the window with unsaved changes asks first.
        if ui.ctx().input(|i| i.viewport().close_requested()) && !self.may_close {
            if let Mode::Editor(ed) = &mut self.mode {
                if ed.has_unsaved_changes() || !ed.running_tasks().is_empty() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    ed.request_leave(Leaving::Quit);
                }
            }
        }
        match &mut self.mode {
            Mode::Home => self.home(ui, &rs),
            Mode::Recording(_) => self.recording(ui, &rs),
            Mode::Editor(ed) => match ed.ui(ui, &rs) {
                Action::GoHome => {
                    self.projects = scan_projects();
                    self.mode = Mode::Home;
                }
                Action::Quit => {
                    self.may_close = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Action::Deleted => {
                    // Close the app and go back to the recording bar.
                    RELAUNCH_RECORDER.store(true, Ordering::Relaxed);
                    self.mode = Mode::Home;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Action::None => {}
            },
        }
    }

}

/// Set when the app closes to return to the recording bar.
pub static RELAUNCH_RECORDER: AtomicBool = AtomicBool::new(false);

pub fn run(open: Option<PathBuf>) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Open Studio")
            .with_app_id("open-studio")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 640.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("Open Studio", options, Box::new(move |cc| Ok(Box::new(App::new(cc, open)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}
