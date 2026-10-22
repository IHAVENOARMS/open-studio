//! Screen Studio style recording flow, drawn on wlr-layer-shell surfaces so it
//! floats over the desktop:
//!
//! * a picker bar (Display / Window / Area) with full-screen selection
//!   overlays on every monitor,
//! * a small recording widget (finish + timer, pause, restart, delete),
//!   placed on a monitor that isn't being recorded when there is one.
//!
//! egui renders each surface through wgpu. The recording itself uses direct
//! capture (ext-image-copy-capture), so no portal dialog appears.

use crate::icons::{self, Icon};
use crate::record::{self, DirectRequest, RecordOptions, Recorder, hyprland};
use crate::settings::Settings;
use crate::theme;
use anyhow::{Context as _, Result, anyhow};
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use egui_wgpu::wgpu;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{Shm, ShmHandler},
};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use wayland_client::{
    Connection, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface},
};

// Picker bar palette: dark, slightly purple glass, ~72% opaque. On Hyprland
// the area behind it is blurred (see `request_blur`), like macOS vibrancy.
const BAR_BG: Color32 = Color32::from_rgba_premultiplied(22, 19, 32, 184);
const BAR_BORDER: Color32 = Color32::from_rgba_premultiplied(26, 26, 26, 26);
const BAR_TEXT: Color32 = Color32::from_rgb(0xec, 0xe8, 0xf3);
const BAR_DIM: Color32 = Color32::from_rgb(0x8d, 0x86, 0x9c);
const TINT: Color32 = Color32::from_rgba_premultiplied(30, 20, 96, 92);
const RED: Color32 = Color32::from_rgb(0xff, 0x3b, 0x4f);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Display,
    Window,
    Area,
    Device,
}

#[derive(Clone, Debug)]
struct Monitor {
    name: String,
    description: String,
    /// Logical rect in the global layout.
    rect: [f64; 4],
    pixels: (u32, u32),
    refresh: f64,
}

impl Monitor {
    fn global(&self) -> Rect {
        Rect::from_min_size(pos2(self.rect[0] as f32, self.rect[1] as f32), vec2(self.rect[2] as f32, self.rect[3] as f32))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WidgetAction {
    Finish,
    TogglePause,
    Restart,
    Delete,
}

#[derive(Clone, Copy)]
enum AreaDrag {
    Move { grab: Pos2 },
    Handle(i8, i8),
    New { origin: Pos2 },
}

/// State shared by every surface's UI.
struct Shared {
    mode: Mode,
    monitors: Vec<Monitor>,
    windows: Vec<hyprland::Client>,
    /// Monitor that shows the bar (where the pointer was at launch).
    bar_monitor: String,
    /// Area selection in global logical coordinates.
    area: Rect,
    area_drag: Option<AreaDrag>,
    settings: Settings,
    gear_open: bool,
    capturing_shortcut: bool,
    start: Option<DirectRequest>,
    cancel: bool,
    /// Folder button: leave and show the projects list.
    open_projects: bool,
    /// Open input dropdown (1 microphone, 2 system audio).
    menu: Option<usize>,
    audio_devices: Vec<crate::record::audio::Device>,
    /// scrcpy is available, so the Device button works.
    scrcpy: bool,
    phones: Vec<record::device::Device>,
    /// Mirror window of the chosen phone, and its Hyprland window once mapped.
    mirror: Option<record::device::Mirror>,
    mirror_window: Option<hyprland::Client>,
    mirror_checked: Option<Instant>,
    /// Phone whose scrcpy options panel is open.
    device_options: Option<record::device::Device>,
    /// Cameras offered in the camera menu.
    cameras: Vec<record::camera::Source>,
    /// The chosen camera, open for the preview and handed to the recorder.
    camera: Option<Arc<record::camera::CameraFeed>>,
    /// A camera being opened in the background (phones take a second).
    camera_opening: Option<std::sync::mpsc::Receiver<Result<record::camera::CameraFeed, String>>>,
    camera_error: Option<String>,
    /// Background search for wireless phones: (phones, cameras).
    scanning: Option<std::sync::mpsc::Receiver<(Vec<record::device::Device>, Vec<record::camera::Source>)>>,
    // Recording widget state.
    elapsed: f64,
    paused: bool,
    can_pause: bool,
    saving: bool,
    action: Option<WidgetAction>,
}

enum SurfKind {
    /// Full-screen picker overlay for a monitor.
    Overlay,
    /// Small floating recording controls.
    Widget,
}

struct Surf {
    kind: SurfKind,
    monitor: Monitor,
    layer: LayerSurface,
    wgpu: Option<wgpu::Surface<'static>>,
    config: Option<wgpu::SurfaceConfiguration>,
    renderer: Option<egui_wgpu::Renderer>,
    ctx: egui::Context,
    size: (u32, u32),
    scale: i32,
    events: Vec<egui::Event>,
    redraw_at: Option<Instant>,
    cursor: egui::CursorIcon,
}

struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

struct Recording {
    recorder: Option<Recorder>,
    request: DirectRequest,
}

struct Rt {
    registry: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    conn: Connection,
    qh: QueueHandle<Rt>,
    gpu: Option<Gpu>,
    surfaces: Vec<Surf>,
    pointer: Option<ThemedPointer>,
    pointer_surface: Option<wl_surface::WlSurface>,
    keyboard_surface: Option<wl_surface::WlSurface>,
    modifiers: egui::Modifiers,
    shared: Shared,
    recording: Option<Recording>,
    shortcut_hit: Arc<AtomicBool>,
    shortcut_stop: Arc<AtomicBool>,
    start_time: Instant,
    result: Option<Option<PathBuf>>,
    error: Option<String>,
}

/// How the recording UI ended.
pub enum Outcome {
    Recorded(PathBuf),
    Cancelled,
    /// The folder button: show the projects list.
    OpenProjects,
}

/// Runs the picker and recording UI.
pub fn run() -> Result<Outcome> {
    let conn = Connection::connect_to_env().context("connecting to Wayland")?;
    let (globals, queue) = registry_queue_init::<Rt>(&conn)?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context("this compositor has no wlr-layer-shell")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm")?;

    request_blur();
    let monitors = load_monitors()?;
    let cursor = hyprland::cursor_pos().unwrap_or((0.0, 0.0));
    let pinned = std::env::var("OPEN_STUDIO_BAR_MONITOR").ok();
    let bar_monitor = monitors
        .iter()
        .find(|m| Some(&m.name) == pinned.as_ref())
        .or_else(|| monitors.iter().find(|m| m.global().contains(pos2(cursor.0 as f32, cursor.1 as f32))))
        .or(monitors.first())
        .map(|m| m.name.clone())
        .ok_or_else(|| anyhow!("no monitors"))?;
    let bm = monitors.iter().find(|m| m.name == bar_monitor).unwrap().global();
    let area = Rect::from_center_size(bm.center() - vec2(0.0, 40.0), vec2(bm.width() * 0.5, bm.height() * 0.5));
    let settings = Settings::load();

    let mut rt = Rt {
        registry: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        conn: conn.clone(),
        qh: qh.clone(),
        gpu: None,
        surfaces: vec![],
        pointer: None,
        pointer_surface: None,
        keyboard_surface: None,
        modifiers: egui::Modifiers::default(),
        shared: Shared {
            mode: Mode::Display,
            monitors,
            windows: visible_windows(),
            bar_monitor,
            area,
            area_drag: None,
            settings,
            gear_open: false,
            capturing_shortcut: false,
            start: None,
            cancel: false,
            open_projects: false,
            menu: None,
            scrcpy: record::device::scrcpy().is_some(),
            phones: vec![],
            mirror: None,
            mirror_window: None,
            mirror_checked: None,
            device_options: None,
            cameras: vec![],
            camera: None,
            camera_opening: None,
            camera_error: None,
            scanning: None,
            audio_devices: crate::record::audio::list_devices(),
            elapsed: 0.0,
            paused: false,
            can_pause: false,
            saving: false,
            action: None,
        },
        recording: None,
        shortcut_hit: Arc::new(AtomicBool::new(false)),
        shortcut_stop: Arc::new(AtomicBool::new(false)),
        start_time: Instant::now(),
        result: None,
        error: None,
    };

    let mut event_loop: calloop::EventLoop<Rt> = calloop::EventLoop::try_new()?;
    calloop_wayland_source::WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).map_err(|e| anyhow!("{e}"))?;
    // Outputs arrive with the first roundtrips.
    event_loop.dispatch(std::time::Duration::from_millis(50), &mut rt)?;
    event_loop.dispatch(std::time::Duration::from_millis(50), &mut rt)?;
    rt.create_overlays();

    let control = crate::control::ControlServer::start().ok();
    while rt.result.is_none() && rt.error.is_none() {
        event_loop.dispatch(std::time::Duration::from_millis(16), &mut rt)?;
        if control.as_ref().is_some_and(|c| c.take_stop()) {
            rt.shortcut_hit.store(true, Ordering::Relaxed);
        }
        rt.tick();
    }
    rt.shortcut_stop.store(true, Ordering::Relaxed);
    // Surfaces must go before the Wayland objects they render into.
    for s in rt.surfaces.drain(..) {
        drop(s.wgpu);
        drop(s.layer);
    }
    let _ = conn.flush();
    if let Some(e) = rt.error {
        return Err(anyhow!(e));
    }
    if rt.shared.open_projects {
        return Ok(Outcome::OpenProjects);
    }
    Ok(match rt.result.flatten() {
        Some(dir) => Outcome::Recorded(dir),
        None => Outcome::Cancelled,
    })
}

/// Asks Hyprland to blur what's behind our bar and widget. `ignore_alpha`
/// keeps the transparent and lightly tinted parts of the full-screen overlays
/// unblurred, so only the glass panels get the effect. Runtime only; it is
/// not written to the user's config.
fn request_blur() {
    let lua = "hl.layer_rule({ name = 'open-studio-glass', match = { namespace = '^open-studio$' }, blur = true, ignore_alpha = 0.55 })";
    let _ = hyprland::request(&format!("eval {lua}"));
}

fn load_monitors() -> Result<Vec<Monitor>> {
    let infos = hyprland::monitor_infos().unwrap_or_default();
    Ok(hyprland::monitors()?
        .into_iter()
        .map(|m| {
            let info = infos.iter().find(|i| i.name == m.name);
            let (pw, ph) = m.pixel_size();
            let desc = info.map(|i| i.description.clone()).unwrap_or_default();
            // Hyprland descriptions end with serial and connector; keep make and model.
            let short = desc.split_whitespace().take(4).collect::<Vec<_>>().join(" ");
            Monitor {
                name: m.name.clone(),
                description: if short.is_empty() { m.name.clone() } else { short },
                rect: m.logical_rect(),
                pixels: (pw as u32, ph as u32),
                refresh: info.map(|i| i.refresh_rate).unwrap_or(60.0),
            }
        })
        .collect())
}

/// Mapped windows on each monitor's visible workspace, most recently focused first.
fn visible_windows() -> Vec<hyprland::Client> {
    let active: Vec<i64> = hyprland::monitor_infos().map(|m| m.iter().map(|i| i.active_workspace.id).collect()).unwrap_or_default();
    let mut w: Vec<_> = hyprland::clients()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.mapped && !c.hidden && active.contains(&c.workspace.id) && c.class != "open-studio")
        .collect();
    w.sort_by_key(|c| c.focus_history);
    w
}

impl Rt {
    fn gpu(&mut self, surface: &wgpu::Surface<'static>) -> Option<&Gpu> {
        if self.gpu.is_none() {
            let instance = self.gpu_instance();
            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(surface),
                ..Default::default()
            }))
            .ok()?;
            let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
            self.gpu = Some(Gpu { instance, adapter, device, queue });
        }
        self.gpu.as_ref()
    }

    fn gpu_instance(&self) -> wgpu::Instance {
        match &self.gpu {
            Some(g) => g.instance.clone(),
            None => wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env()),
        }
    }

    fn new_surface(&mut self, kind: SurfKind, monitor: Monitor, output: Option<&wl_output::WlOutput>) {
        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(&self.qh, surface, Layer::Overlay, Some("open-studio"), output);
        match kind {
            SurfKind::Overlay => {
                layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
                layer.set_exclusive_zone(-1);
                // Exclusive keyboard focus would also pin the pointer to this
                // surface on Hyprland, so overlays take focus on demand.
                layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
            }
            SurfKind::Widget => {
                layer.set_anchor(Anchor::BOTTOM);
                layer.set_margin(0, 0, 36, 0);
                layer.set_size(WIDGET_W, WIDGET_H);
                layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            }
        }
        layer.commit();
        let instance = self.gpu_instance();
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(self.conn.backend().display_ptr() as *mut _).unwrap(),
        ));
        let window =
            RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::new(layer.wl_surface().id().as_ptr() as *mut _).unwrap()));
        let wgpu_surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle { raw_display_handle: Some(display), raw_window_handle: window })
        }
        .ok();
        if self.gpu.is_none() {
            if let Some(s) = &wgpu_surface {
                // The instance used for the surface must be the GPU's instance.
                let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: Some(s),
                    ..Default::default()
                }));
                if let Ok(adapter) = adapter {
                    if let Ok((device, queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
                        self.gpu = Some(Gpu { instance, adapter, device, queue });
                    }
                }
            }
        }
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        self.surfaces.push(Surf {
            kind,
            monitor,
            layer,
            wgpu: wgpu_surface,
            config: None,
            renderer: None,
            ctx,
            size: (0, 0),
            scale: 1,
            events: vec![],
            redraw_at: Some(Instant::now()),
            cursor: egui::CursorIcon::Default,
        });
    }

    fn output_for(&self, name: &str) -> Option<wl_output::WlOutput> {
        self.output_state.outputs().find(|o| self.output_state.info(o).and_then(|i| i.name).as_deref() == Some(name))
    }

    fn create_overlays(&mut self) {
        let monitors = self.shared.monitors.clone();
        for m in monitors {
            let out = self.output_for(&m.name);
            if std::env::var_os("OPEN_STUDIO_DEBUG_UI").is_some() {
                eprintln!("overlay for {} -> output found: {}", m.name, out.is_some());
            }
            if out.is_some() {
                self.new_surface(SurfKind::Overlay, m, out.as_ref());
            }
        }
    }

    fn destroy_surfaces(&mut self) {
        for s in self.surfaces.drain(..) {
            drop(s.wgpu);
            drop(s.layer);
        }
        let _ = self.conn.flush();
    }

    fn surf_index(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.surfaces.iter().position(|s| s.layer.wl_surface() == surface)
    }

    /// Per-frame housekeeping: state transitions, then redraws.
    fn tick(&mut self) {
        if std::env::var_os("OPEN_STUDIO_DEBUG_UI").is_some() {
            let st = format!(
                "mode={:?} gear={} bake={} paused={} action={:?} start={} cancel={}",
                self.shared.mode,
                self.shared.gear_open,
                self.shared.settings.bake_cursor,
                self.shared.paused,
                self.shared.action.map(|a| a as u8),
                self.shared.start.is_some(),
                self.shared.cancel
            );
            thread_local!(static LAST: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) });
            LAST.with(|l| {
                if *l.borrow() != st {
                    eprintln!("state {st}");
                    *l.borrow_mut() = st;
                }
            });
        }
        if self.shared.cancel && self.recording.is_none() {
            self.result = Some(None);
            return;
        }
        if let Some(req) = self.shared.start.take() {
            self.begin_recording(req);
        }
        if self.recording.is_some() {
            self.update_recording();
        }
        let now = Instant::now();
        for i in 0..self.surfaces.len() {
            let due = self.surfaces[i].redraw_at.is_some_and(|t| t <= now) || !self.surfaces[i].events.is_empty();
            if due {
                self.draw(i);
            }
        }
    }

    fn begin_recording(&mut self, req: DirectRequest) {
        // Remove the picker before capture starts so it isn't recorded.
        self.destroy_surfaces();
        let _ = self.conn.roundtrip();
        std::thread::sleep(std::time::Duration::from_millis(180));
        let recorded = req.name.clone().unwrap_or_default();
        self.shared.elapsed = 0.0;
        self.shared.paused = false;
        match start_recorder(&req, &self.shared.settings, self.shared.camera.clone()) {
            Ok(r) => {
                self.shared.can_pause = r.can_pause();
                self.recording = Some(Recording { recorder: Some(r), request: req });
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        }
        // Shortcut to stop.
        if let Some(sc) = record::input::Shortcut::parse(&self.shared.settings.stop_shortcut) {
            self.shortcut_hit.store(false, Ordering::Relaxed);
            record::input::watch_shortcut(sc, self.shortcut_hit.clone(), self.shortcut_stop.clone());
        }
        // Controls go on another monitor when possible, so they aren't recorded.
        let cursor = hyprland::cursor_pos().unwrap_or((0.0, 0.0));
        let others: Vec<Monitor> = self.shared.monitors.iter().filter(|m| m.name != recorded).cloned().collect();
        let widget_mon = others
            .iter()
            .find(|m| m.global().contains(pos2(cursor.0 as f32, cursor.1 as f32)))
            .or(others.first())
            .cloned()
            .or_else(|| self.shared.monitors.first().cloned());
        if let Some(m) = widget_mon {
            let out = self.output_for(&m.name);
            self.new_surface(SurfKind::Widget, m, out.as_ref());
        }
    }

    fn update_recording(&mut self) {
        let Some(rec) = &mut self.recording else { return };
        if let Some(r) = &rec.recorder {
            self.shared.elapsed = r.elapsed();
            self.shared.paused = r.is_paused();
            if let Some(e) = r.error() {
                self.error = Some(format!("recording failed: {e}"));
                return;
            }
        }
        if self.shortcut_hit.swap(false, Ordering::Relaxed) {
            self.shared.action = Some(WidgetAction::Finish);
        }
        let Some(action) = self.shared.action.take() else {
            for s in &mut self.surfaces {
                s.redraw_at.get_or_insert(Instant::now());
            }
            return;
        };
        match action {
            WidgetAction::TogglePause => {
                if let Some(r) = &rec.recorder {
                    if r.is_paused() {
                        r.resume()
                    } else {
                        r.pause()
                    }
                }
            }
            WidgetAction::Finish => {
                let Some(r) = rec.recorder.take() else { return };
                self.shared.saving = true;
                if let Some(i) = self.surfaces.iter().position(|s| matches!(s.kind, SurfKind::Widget)) {
                    self.draw(i);
                }
                match r.stop() {
                    Ok(dir) => self.result = Some(Some(dir)),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            WidgetAction::Restart => {
                if let Some(r) = rec.recorder.take() {
                    r.discard();
                }
                match start_recorder(&rec.request, &self.shared.settings, self.shared.camera.clone()) {
                    Ok(r) => rec.recorder = Some(r),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            WidgetAction::Delete => {
                if let Some(r) = rec.recorder.take() {
                    r.discard();
                }
                self.result = Some(None);
            }
        }
    }

    fn draw(&mut self, i: usize) {
        let Some(gpu) = self.gpu.as_ref() else { return };
        let surf = &mut self.surfaces[i];
        let (Some(wsurf), Some(config)) = (&surf.wgpu, &surf.config) else { return };
        if surf.renderer.is_none() {
            surf.renderer = Some(egui_wgpu::Renderer::new(&gpu.device, config.format, Default::default()));
        }
        let ppp = surf.scale as f32;
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(surf.size.0 as f32, surf.size.1 as f32))),
            time: Some(self.start_time.elapsed().as_secs_f64()),
            events: std::mem::take(&mut surf.events),
            max_texture_side: Some(gpu.device.limits().max_texture_dimension_2d as usize),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        let monitor = surf.monitor.clone();
        let is_widget = matches!(surf.kind, SurfKind::Widget);
        let shared = &mut self.shared;
        let out = surf.ctx.run_ui(raw, |ui| {
            if is_widget {
                widget_ui(ui, shared);
            } else {
                overlay_ui(ui, shared, &monitor);
            }
        });
        // Cursor shape for this surface.
        if self.pointer_surface.as_ref() == Some(surf.layer.wl_surface()) && out.platform_output.cursor_icon != surf.cursor {
            surf.cursor = out.platform_output.cursor_icon;
            if let Some(p) = &self.pointer {
                let _ = p.set_cursor(&self.conn, map_cursor(surf.cursor));
            }
        }
        let repaint = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay);
        surf.redraw_at = match repaint {
            Some(d) if d < std::time::Duration::from_secs(5) => Some(Instant::now() + d.max(std::time::Duration::from_millis(8))),
            _ => None,
        };
        let frame = match wsurf.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            other => {
                if std::env::var_os("OPEN_STUDIO_DEBUG_UI").is_some() {
                    eprintln!("draw {}: no texture ({})", surf.monitor.name, match other {
                        wgpu::CurrentSurfaceTexture::Timeout => "timeout",
                        wgpu::CurrentSurfaceTexture::Occluded => "occluded",
                        wgpu::CurrentSurfaceTexture::Outdated => "outdated",
                        _ => "other",
                    });
                }
                surf.redraw_at = Some(Instant::now() + std::time::Duration::from_millis(16));
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let renderer = surf.renderer.as_mut().unwrap();
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        let jobs = surf.ctx.tessellate(out.shapes, ppp);
        let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: [config.width, config.height], pixels_per_point: ppp };
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let cmds = renderer.update_buffers(&gpu.device, &gpu.queue, &mut enc, &jobs, &sd);
        {
            let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            renderer.render(&mut pass.forget_lifetime(), &jobs, &sd);
        }
        gpu.queue.submit(cmds.into_iter().chain([enc.finish()]));
        gpu.queue.present(frame);
        for id in &out.textures_delta.free {
            renderer.free_texture(id);
        }
    }

    fn configure_surface(&mut self, i: usize) {
        let Some(gpu) = self.gpu.as_ref() else { return };
        let surf = &mut self.surfaces[i];
        let Some(ws) = &surf.wgpu else { return };
        let caps = ws.get_capabilities(&gpu.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .or(caps.formats.first().copied());
        let Some(format) = format else { return };
        let alpha = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            caps.alpha_modes[0]
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: (surf.size.0 * surf.scale as u32).max(1),
            height: (surf.size.1 * surf.scale as u32).max(1),
            present_mode: wgpu::PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: alpha,
            view_formats: vec![],
            color_space: Default::default(),
        };
        ws.configure(&gpu.device, &config);
        surf.layer.wl_surface().set_buffer_scale(surf.scale);
        surf.config = Some(config);
        surf.redraw_at = Some(Instant::now());
    }
}

fn start_recorder(req: &DirectRequest, settings: &Settings, camera: Option<Arc<record::camera::CameraFeed>>) -> Result<Recorder> {
    Recorder::start(RecordOptions {
        direct: Some(req.clone()),
        show_system_cursor: settings.bake_cursor,
        microphone: settings.microphone.clone(),
        system_audio: settings.system_audio,
        camera,
        ..Default::default()
    })
}

/// Opens the camera with this id in the background; the bar picks it up
/// when ready.
fn open_camera(sh: &mut Shared, id: Option<String>, ctx: &egui::Context) {
    sh.camera = None;
    sh.camera_error = None;
    sh.camera_opening = None;
    let Some(id) = id else { return };
    let (tx, rx) = std::sync::mpsc::channel();
    let known = sh.cameras.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let src = known.into_iter().find(|c| c.id() == id).or_else(|| record::camera::list().into_iter().find(|c| c.id() == id));
        let r = match src {
            Some(src) => record::camera::CameraFeed::open(&src).map_err(|e| format!("{e:#}")),
            None => Err("camera not connected".into()),
        };
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    sh.camera_opening = Some(rx);
}

/// Looks for wireless phones in the background and refreshes both menus.
fn start_scan(sh: &mut Shared, ctx: &egui::Context) {
    if sh.scanning.is_some() {
        return;
    }
    let known = sh.settings.known_devices.clone();
    let ctx = ctx.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let phones = record::device::discover(&known);
        let cameras = record::camera::list();
        let _ = tx.send((phones, cameras));
        ctx.request_repaint();
    });
    sh.scanning = Some(rx);
}

/// Keeps a phone's network address so it can be reconnected next time.
fn remember_device(settings: &mut Settings, serial: &str) {
    // Wireless-debugging mDNS names and ports change; only fixed ip/host:port.
    if serial.contains(':') && !serial.contains("._adb-tls-connect.") && !settings.known_devices.iter().any(|k| k == serial) {
        settings.known_devices.push(serial.to_string());
        settings.save();
    }
}

fn poll_camera(sh: &mut Shared) {
    if let Some(rx) = &sh.scanning {
        if let Ok((phones, cameras)) = rx.try_recv() {
            sh.phones = phones;
            sh.cameras = cameras;
            sh.scanning = None;
        }
    }
    if let Some(rx) = &sh.camera_opening {
        match rx.try_recv() {
            Ok(Ok(feed)) => {
                sh.camera = Some(Arc::new(feed));
                sh.camera_opening = None;
            }
            Ok(Err(e)) => {
                sh.camera_error = Some(e);
                sh.camera_opening = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => sh.camera_opening = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
    if let Some(e) = sh.camera.as_ref().and_then(|c| c.error()) {
        sh.camera_error = Some(e);
        sh.camera = None;
    }
}

fn camera_preview_rect(bar: Rect) -> Rect {
    Rect::from_min_size(pos2(bar.right() - 230.0, bar.top() - 16.0 - 230.0), vec2(230.0, 230.0))
}

/// Live camera bubble above the bar, as Screen Studio shows it: a squircle
/// with the picture cropped to fill it.
fn camera_preview(ui: &mut egui::Ui, sh: &mut Shared, bar: Rect) {
    if !sh.settings.camera_preview || sh.settings.camera.is_none() || sh.gear_open || sh.menu.is_some() || sh.device_options.is_some() {
        return;
    }
    let r = camera_preview_rect(bar);
    let p = ui.painter().clone();
    let radius = 56u8;
    p.add(egui::epaint::Shadow { offset: [0, 10], blur: 36, spread: 0, color: Color32::from_black_alpha(120) }.as_shape(r, CornerRadius::same(radius)));
    let Some(frame) = sh.camera.as_ref().and_then(|c| c.latest()) else {
        p.rect_filled(r, CornerRadius::same(radius), BAR_BG);
        let msg = sh.camera_error.clone().unwrap_or_else(|| "Starting camera…".into());
        p.text(r.center(), Align2::CENTER_CENTER, msg, FontId::proportional(13.0), BAR_DIM);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let id = egui::Id::NULL.with("camera-preview");
    let cached: Option<(egui::TextureHandle, u64)> = ui.ctx().data(|d| d.get_temp(id));
    let tex = match cached {
        Some((tex, seq)) if seq == frame.seq => tex,
        Some((mut tex, _)) => {
            tex.set(egui::ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &frame.rgba), egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|d| d.insert_temp(id, (tex.clone(), frame.seq)));
            tex
        }
        None => {
            let tex = ui.ctx().load_texture(
                "camera-preview",
                egui::ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &frame.rgba),
                egui::TextureOptions::LINEAR,
            );
            ui.ctx().data_mut(|d| d.insert_temp(id, (tex.clone(), frame.seq)));
            tex
        }
    };
    // Cover crop to a square, mirrored like a mirror.
    let (fw, fh) = (frame.width as f32, frame.height as f32);
    let (u0, u1, v0, v1) = if fw > fh {
        let m = (1.0 - fh / fw) / 2.0;
        (m, 1.0 - m, 0.0, 1.0)
    } else {
        let m = (1.0 - fw / fh) / 2.0;
        (0.0, 1.0, m, 1.0 - m)
    };
    let uv = Rect::from_min_max(pos2(u1, v0), pos2(u0, v1));
    p.add(egui::epaint::RectShape::filled(r, CornerRadius::same(radius), Color32::WHITE).with_texture(tex.id(), uv));
    p.rect_stroke(r, CornerRadius::same(radius), Stroke::new(1.0, Color32::from_white_alpha(30)), StrokeKind::Inside);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

fn map_cursor(c: egui::CursorIcon) -> CursorIcon {
    match c {
        egui::CursorIcon::PointingHand => CursorIcon::Pointer,
        egui::CursorIcon::Grab => CursorIcon::Grab,
        egui::CursorIcon::Grabbing => CursorIcon::Grabbing,
        egui::CursorIcon::Move => CursorIcon::Move,
        egui::CursorIcon::ResizeHorizontal => CursorIcon::EwResize,
        egui::CursorIcon::ResizeVertical => CursorIcon::NsResize,
        egui::CursorIcon::ResizeNwSe => CursorIcon::NwseResize,
        egui::CursorIcon::ResizeNeSw => CursorIcon::NeswResize,
        egui::CursorIcon::Crosshair => CursorIcon::Crosshair,
        egui::CursorIcon::Text => CursorIcon::Text,
        egui::CursorIcon::NotAllowed => CursorIcon::NotAllowed,
        _ => CursorIcon::Default,
    }
}

// ---- UI: picker overlay ------------------------------------------------------

const BAR_H: f32 = 76.0;
/// Fade time for the selection highlight, in seconds.
const FADE: f32 = 0.14;

fn overlay_ui(ui: &mut egui::Ui, sh: &mut Shared, mon: &Monitor) {
    let screen = ui.max_rect();
    let origin = vec2(mon.rect[0] as f32, mon.rect[1] as f32);
    let to_local = |r: Rect| r.translate(-origin);
    let is_bar = mon.name == sh.bar_monitor;
    let bar = bar_rect(screen);
    let pointer = ui.ctx().pointer_hover_pos();
    let menu_area = Rect::from_min_max(pos2(bar.left(), bar.top() - 360.0), bar.max);
    let over_bar = is_bar
        && pointer.is_some_and(|p| {
            bar.expand(6.0).contains(p)
                || ((sh.gear_open || sh.menu.is_some()) && menu_area.contains(p))
                || (sh.device_options.is_some() && device_options_rect(ui.ctx(), bar).contains(p))
                || (sh.settings.camera.is_some() && sh.settings.camera_preview && camera_preview_rect(bar).contains(p))
        });
    let p = ui.painter().clone();

    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        if sh.device_options.is_some() {
            sh.device_options = None;
        } else if sh.gear_open {
            sh.gear_open = false;
        } else {
            sh.cancel = true;
        }
    }

    match sh.mode {
        Mode::Display => {
            // Fades in when the pointer arrives on this monitor, out when it leaves.
            let active = pointer.is_some() && !over_bar;
            let a = ui.ctx().animate_bool_with_time(ui.scope_id().with("display-fade"), active, FADE);
            if a > 0.0 {
                p.rect_filled(screen, 0.0, TINT.gamma_multiply(a));
                p.rect_stroke(screen.shrink(1.0), 0.0, Stroke::new(2.0, theme::ACCENT.gamma_multiply(a)), StrokeKind::Inside);
                let sub = format!("{}×{} • {:.0}FPS", mon.pixels.0, mon.pixels.1, mon.refresh.round());
                let clicked = selection_card(ui, screen.center(), &mon.description, &sub, a) && active;
                let bg = ui.interact(screen, ui.scope_id().with("display-click"), Sense::click());
                if clicked || (bg.clicked() && active) {
                    sh.start = Some(DirectRequest {
                        target: record::direct::Target::Output { name: mon.name.clone(), crop: None },
                        kind: "monitor".into(),
                        name: Some(mon.name.clone()),
                        logical_rect: mon.rect,
                    });
                }
            }
        }
        Mode::Window => {
            let hit = pointer.filter(|_| !over_bar).and_then(|ptr| {
                let gp = ptr + origin;
                sh.windows
                    .iter()
                    .find(|w| Rect::from_min_size(pos2(w.at[0] as f32, w.at[1] as f32), vec2(w.size[0] as f32, w.size[1] as f32)).contains(gp))
                    .cloned()
            });
            // Remember the last window so it can fade out after the pointer leaves.
            let mem = ui.scope_id().with("last-window");
            if let Some(w) = &hit {
                ui.ctx().data_mut(|d| d.insert_temp(mem, w.clone()));
            }
            let a = ui.ctx().animate_bool_with_time(ui.scope_id().with("window-fade"), hit.is_some(), FADE);
            let shown = hit.clone().or_else(|| ui.ctx().data(|d| d.get_temp::<hyprland::Client>(mem)));
            if let (Some(w), true) = (shown, a > 0.0) {
                {
                    let r = to_local(Rect::from_min_size(pos2(w.at[0] as f32, w.at[1] as f32), vec2(w.size[0] as f32, w.size[1] as f32)));
                    p.rect_filled(r, 10.0, TINT.gamma_multiply(a));
                    p.rect_stroke(r, 10.0, Stroke::new(2.0, theme::ACCENT.gamma_multiply(a)), StrokeKind::Inside);
                    let title = if w.title.chars().count() > 48 { w.title.chars().take(46).collect::<String>() + "…" } else { w.title.clone() };
                    let sub = format!("{} • {}×{}", w.class, w.size[0] as i32, w.size[1] as i32);
                    let clicked = selection_card(ui, r.center(), &title, &sub, a) && hit.is_some();
                    let bg = ui.interact(r, ui.scope_id().with(("window-click", &w.address)), Sense::click());
                    if clicked || (bg.clicked() && hit.is_some()) {
                        if let Ok(req) = record::direct_window(&w.address) {
                            sh.start = Some(req);
                        }
                    }
                }
            }
        }
        Mode::Area => area_ui(ui, sh, mon, over_bar),
        Mode::Device => device_ui(ui, sh, mon, is_bar),
    }

    if is_bar {
        bar_ui(ui, sh, bar);
    }
    if pointer.is_some() {
        ui.ctx().request_repaint();
    }
}

/// Device mode: the phone's mirror window is highlighted like a window,
/// with a card to start recording it.
fn device_ui(ui: &mut egui::Ui, sh: &mut Shared, mon: &Monitor, is_bar: bool) {
    let origin = vec2(mon.rect[0] as f32, mon.rect[1] as f32);
    if sh.mirror.as_mut().is_some_and(|m| !m.running()) {
        sh.mirror = None;
        sh.mirror_window = None;
    }
    if sh.mirror.is_some() && sh.mirror_checked.is_none_or(|t| t.elapsed().as_millis() > 300) {
        sh.mirror_window = sh.mirror.as_ref().and_then(|m| m.window());
        sh.mirror_checked = Some(Instant::now());
    }
    let p = ui.painter().clone();
    let screen = ui.max_rect();
    let Some(m) = &sh.mirror else {
        if is_bar {
            let c = screen.center();
            p.text(c, Align2::CENTER_CENTER, "Choose a device from the Device menu", theme::semibold(26.0), Color32::WHITE);
        }
        return;
    };
    let Some(w) = sh.mirror_window.clone() else {
        if is_bar {
            p.text(screen.center(), Align2::CENTER_CENTER, format!("Connecting to {}…", m.device.model), theme::semibold(26.0), Color32::WHITE);
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let wr = Rect::from_min_size(pos2(w.at[0] as f32, w.at[1] as f32), vec2(w.size[0] as f32, w.size[1] as f32));
    if !mon.global().intersects(wr) {
        return;
    }
    let r = wr.translate(-origin);
    p.rect_filled(r, 10.0, TINT);
    p.rect_stroke(r, 10.0, Stroke::new(2.0, theme::ACCENT), StrokeKind::Inside);
    let sub = format!("Android • {}×{}", w.size[0] as i32, w.size[1] as i32);
    // The card is wider than a phone window; keep it on screen.
    let c = pos2(r.center().x.clamp(screen.left() + 140.0, screen.right() - 140.0), r.center().y);
    if selection_card(ui, c, &m.device.model, &sub, 1.0) {
        if let Ok(mut req) = record::direct_window(&w.address) {
            req.kind = "device".into();
            req.name = Some(m.device.model.clone());
            sh.start = Some(req);
        }
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
}

fn bar_rect(screen: Rect) -> Rect {
    let w = 1032.0f32.min(screen.width() - 40.0);
    Rect::from_center_size(pos2(screen.center().x, screen.bottom() - 70.0 - BAR_H / 2.0), vec2(w, BAR_H))
}

fn gear_menu_rect(bar: Rect) -> Rect {
    Rect::from_min_size(pos2(bar.right() - 330.0, bar.top() - 196.0), vec2(330.0, 184.0))
}

/// Title, subtitle and a "Start recording" button centred on `c`, drawn at
/// opacity `a`. Returns true when clicked.
fn selection_card(ui: &mut egui::Ui, c: Pos2, title: &str, sub: &str, a: f32) -> bool {
    let p = ui.painter();
    let white = Color32::WHITE.gamma_multiply(a);
    let shadow = Color32::from_black_alpha(140).gamma_multiply(a);
    // Rises slightly as it fades in.
    let c = c + vec2(0.0, 8.0 * (1.0 - a));
    for (d, col) in [(vec2(0.0, 2.0), shadow), (egui::Vec2::ZERO, white)] {
        p.text(c - vec2(0.0, 62.0) + d, Align2::CENTER_CENTER, title, theme::semibold(40.0), col);
    }
    p.text(c - vec2(0.0, 22.0) + vec2(0.0, 1.0), Align2::CENTER_CENTER, sub, FontId::proportional(17.0), shadow);
    p.text(c - vec2(0.0, 22.0), Align2::CENTER_CENTER, sub, FontId::proportional(17.0), white);
    let btn = Rect::from_center_size(c + vec2(0.0, 30.0), vec2(232.0, 56.0));
    let resp = ui.interact(btn, ui.scope_id().with("start"), Sense::click());
    let fill = if resp.hovered() { theme::ACCENT.gamma_multiply(1.12) } else { theme::ACCENT };
    let p = ui.painter();
    p.rect_filled(btn, 12.0, fill.gamma_multiply(a));
    let icon_c = pos2(btn.left() + 46.0, btn.center().y);
    p.circle_stroke(icon_c, 10.0, Stroke::new(2.0, white));
    p.circle_filled(icon_c, 4.5, white);
    p.text(pos2(icon_c.x + 22.0, btn.center().y), Align2::LEFT_CENTER, "Start recording", FontId::proportional(18.0), white);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

fn bar_ui(ui: &mut egui::Ui, sh: &mut Shared, bar: Rect) {
    // The remembered camera opens when the bar first shows.
    if sh.settings.camera.is_some() && sh.camera.is_none() && sh.camera_opening.is_none() && sh.camera_error.is_none() {
        let id = sh.settings.camera.clone();
        open_camera(sh, id, ui.ctx());
    }
    poll_camera(sh);
    camera_preview(ui, sh, bar);
    let p = ui.painter().clone();
    p.add(egui::epaint::Shadow { offset: [0, 10], blur: 40, spread: 0, color: Color32::from_black_alpha(120) }.as_shape(bar, CornerRadius::same(18)));
    p.rect_filled(bar, 18.0, BAR_BG);
    p.rect_stroke(bar, 18.0, Stroke::new(1.0, BAR_BORDER), StrokeKind::Inside);
    let cy = bar.center().y;
    let mut x = bar.left() + 18.0;
    let divider = |p: &egui::Painter, x: f32| p.line_segment([pos2(x, bar.top() + 16.0), pos2(x, bar.bottom() - 16.0)], Stroke::new(1.0, Color32::from_white_alpha(28)));

    // Close.
    let close = Rect::from_center_size(pos2(x + 16.0, cy), vec2(32.0, 32.0));
    let r = ui.interact(close, ui.scope_id().with("close"), Sense::click()).on_hover_text("Close");
    p.circle_filled(close.center(), 15.0, if r.hovered() { Color32::WHITE } else { Color32::from_rgb(0xe9, 0xe6, 0xef) });
    icons::paint(&p, Rect::from_center_size(close.center(), vec2(16.0, 16.0)), Icon::Close, Color32::from_rgb(0x24, 0x1f, 0x2e));
    if r.clicked() {
        sh.cancel = true;
    }
    x += 46.0;
    divider(&p, x);
    x += 10.0;

    // Mode tiles.
    for (mode, icon, label, enabled) in [
        (Some(Mode::Display), Icon::Display, "Display", true),
        (Some(Mode::Window), Icon::Window, "Window", true),
        (Some(Mode::Area), Icon::Resolution, "Area", true),
        (Some(Mode::Device), Icon::Phone, "Device", sh.scrcpy),
    ] {
        let tile = Rect::from_min_size(pos2(x, bar.top() + 9.0), vec2(70.0, BAR_H - 18.0));
        let resp = ui.interact(tile, ui.scope_id().with(("mode", label)), Sense::click());
        let selected = mode == Some(sh.mode);
        if selected {
            p.rect_filled(tile, 10.0, Color32::from_white_alpha(20));
        } else if resp.hovered() && enabled {
            p.rect_filled(tile, 10.0, Color32::from_white_alpha(10));
        }
        let col = if !enabled { BAR_DIM.gamma_multiply(0.6) } else if selected { BAR_TEXT } else { Color32::from_rgb(0xc9, 0xc4, 0xd4) };
        icons::paint(&p, Rect::from_center_size(pos2(tile.center().x, tile.top() + 21.0), vec2(26.0, 26.0)), icon, col);
        p.text(pos2(tile.center().x, tile.bottom() - 12.0), Align2::CENTER_CENTER, label, FontId::proportional(12.5), col);
        let resp = if enabled {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            resp.on_hover_text("scrcpy needs to be installed to record an Android phone or tablet")
        };
        if mode == Some(Mode::Device) {
            if enabled && resp.clicked() {
                sh.phones = record::device::list();
                start_scan(sh, ui.ctx());
                sh.menu = if sh.menu == Some(3) { None } else { Some(3) };
                sh.gear_open = false;
                sh.device_options = None;
            }
            if sh.menu == Some(3) {
                input_menu(ui, sh, 3, tile);
            }
            x += 74.0;
            continue;
        }
        if resp.clicked() {
            if let Some(m) = mode {
                sh.mode = m;
                sh.gear_open = false;
                if m == Mode::Window {
                    sh.windows = visible_windows();
                }
            }
        }
        x += 74.0;
    }
    x += 4.0;
    divider(&p, x);
    x += 18.0;

    // Inputs not supported yet: shown as Screen Studio shows them when off.
    // Camera (not available yet), microphone and system audio menus.
    let mic_name = sh.settings.microphone.as_ref().map(|m| {
        sh.audio_devices.iter().find(|d| &d.name == m).map(|d| d.description.clone()).unwrap_or_else(|| "Microphone".into())
    });
    let sys_on = sh.settings.system_audio;
    let short = |s: &str| if s.chars().count() > 16 { s.chars().take(15).collect::<String>() + "…" } else { s.to_string() };
    let cam_label = sh.settings.camera.as_ref().map(|id| {
        sh.camera
            .as_ref()
            .map(|c| c.source.clone())
            .or_else(|| sh.cameras.iter().find(|c| &c.id() == id).cloned())
            .map(|c| match c {
                // Short form in the bar; the menu has the full name.
                record::camera::Source::Phone { facing, .. } => format!("Phone ({facing})"),
                c => c.label(),
            })
            .unwrap_or_else(|| "Camera".into())
    });
    let entries = [
        (if cam_label.is_some() { Icon::CameraOn } else { Icon::CameraOff }, cam_label.as_deref().map(short).unwrap_or("No camera".into()), 160.0, true, cam_label.is_some()),
        (if mic_name.is_some() { Icon::Mic } else { Icon::MicOff }, mic_name.as_deref().map(short).unwrap_or("No microphone".into()), 176.0, true, mic_name.is_some()),
        (Icon::SystemAudio, if sys_on { "System audio".into() } else { "No system audio".into() }, 176.0, true, sys_on),
    ];
    for (i, (icon, label, w, enabled, on)) in entries.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, cy - 20.0), vec2(w - 6.0, 40.0));
        let resp = ui.interact(r, ui.scope_id().with(("input", i)), Sense::click());
        let col = if on { BAR_TEXT } else { BAR_DIM };
        if enabled && (resp.hovered() || sh.menu == Some(i)) {
            p.rect_filled(r, 10.0, Color32::from_white_alpha(12));
        }
        icons::paint(&p, Rect::from_center_size(pos2(r.left() + 16.0, cy), vec2(22.0, 22.0)), icon, col);
        p.text(pos2(r.left() + 36.0, cy), Align2::LEFT_CENTER, label, FontId::proportional(15.0), col);
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if enabled && resp.clicked() {
            sh.menu = if sh.menu == Some(i) { None } else { Some(i) };
            sh.gear_open = false;
            sh.device_options = None;
            if i == 0 {
                sh.cameras = record::camera::list();
                start_scan(sh, ui.ctx());
            }
            if i == 1 {
                sh.audio_devices = crate::record::audio::list_devices();
            }
        }
        if sh.menu == Some(i) {
            input_menu(ui, sh, i, r);
        }
        x += w;
    }
    let gx = bar.right() - 64.0;
    divider(&p, gx - 8.0);
    // Projects folder (left of the settings button).
    let folder = Rect::from_min_size(pos2(gx - 60.0, cy - 20.0), vec2(44.0, 40.0));
    let fr = ui.interact(folder, ui.scope_id().with("projects"), Sense::click()).on_hover_text("Open projects");
    if fr.hovered() {
        p.rect_filled(folder, 10.0, Color32::from_white_alpha(14));
    }
    icons::paint(&p, Rect::from_center_size(folder.center(), vec2(22.0, 22.0)), Icon::Folder, BAR_TEXT);
    if fr.clicked() {
        sh.open_projects = true;
        sh.cancel = true;
    }
    let gear = Rect::from_min_size(pos2(gx, cy - 20.0), vec2(52.0, 40.0));
    let gr = ui.interact(gear, ui.scope_id().with("gear"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if gr.hovered() || sh.gear_open {
        p.rect_filled(gear, 10.0, Color32::from_white_alpha(14));
    }
    icons::paint(&p, Rect::from_center_size(pos2(gear.left() + 17.0, cy), vec2(24.0, 24.0)), Icon::Gear, BAR_TEXT);
    icons::paint(&p, Rect::from_center_size(pos2(gear.left() + 40.0, cy), vec2(14.0, 14.0)), Icon::ChevronDown, BAR_TEXT);
    if gr.clicked() {
        sh.menu = None;
        sh.gear_open = !sh.gear_open;
        sh.capturing_shortcut = false;
    }
    if sh.gear_open {
        gear_menu(ui, sh, gear_menu_rect(bar));
    }
    if sh.device_options.is_some() {
        device_options_panel(ui, sh, device_options_rect(ui.ctx(), bar));
    }
}

/// The options panel sits above the bar and grows upwards to fit its
/// content, measured on the previous frame.
fn device_options_rect(ctx: &egui::Context, bar: Rect) -> Rect {
    let h = ctx.data(|d| d.get_temp::<f32>(egui::Id::NULL.with("device-options-height"))).unwrap_or(600.0);
    Rect::from_min_max(pos2(bar.left() + 150.0, bar.top() - 14.0 - h), pos2(bar.left() + 590.0, bar.top() - 14.0))
}

/// Row of small chips in the bar's style; returns the clicked index.
fn bar_chips(ui: &mut egui::Ui, labels: &[String], selected: usize) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (i, l) in labels.iter().enumerate() {
            let on = i == selected;
            let b = egui::Button::new(egui::RichText::new(l).size(13.0).color(if on { BAR_TEXT } else { BAR_DIM }))
                .fill(if on { Color32::from_white_alpha(30) } else { Color32::from_white_alpha(10) })
                .stroke(if on { Stroke::new(1.0, Color32::from_rgb(0x4f, 0x4d, 0x6f)) } else { Stroke::NONE })
                .corner_radius(CornerRadius::same(8))
                .min_size(vec2(0.0, 28.0));
            if ui.add(b).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// scrcpy options for the chosen phone, with Connect (or Reconnect when
/// already mirroring it, which applies changed options).
fn device_options_panel(ui: &mut egui::Ui, sh: &mut Shared, r: Rect) {
    let Some(device) = sh.device_options.clone() else { return };
    let p = ui.painter().clone();
    p.add(egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(110) }.as_shape(r, CornerRadius::same(14)));
    p.rect_filled(r, 14.0, BAR_BG);
    p.rect_stroke(r, 14.0, Stroke::new(1.0, BAR_BORDER), StrokeKind::Inside);
    // Laid out from the top of the panel; its measured height sizes the
    // panel on the next frame.
    let content = Rect::from_min_size(r.min + vec2(18.0, 18.0), vec2(r.width() - 36.0, 2000.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(egui::Align::Min)));
    device_options_content(&mut child, sh, &device);
    let needed = child.min_rect().height() + 36.0;
    let id = egui::Id::NULL.with("device-options-height");
    if ui.ctx().data(|d| d.get_temp::<f32>(id)).is_none_or(|h| (h - needed).abs() > 0.5) {
        ui.ctx().data_mut(|d| d.insert_temp(id, needed));
        ui.ctx().request_repaint();
    }
}

fn device_options_content(ui: &mut egui::Ui, sh: &mut Shared, device: &record::device::Device) {
    let device = device.clone();
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.label(egui::RichText::new(&device.model).font(theme::semibold(15.0)).color(BAR_TEXT));
    ui.label(egui::RichText::new("Mirrored with scrcpy. The recording is as sharp as the mirror window on screen.").size(12.0).color(BAR_DIM));
    let o = &mut sh.settings.scrcpy;
    let before = o.clone();
    let section = |ui: &mut egui::Ui, t: &str| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(t).size(13.5).color(BAR_TEXT));
    };

    section(ui, "Resolution");
    let sizes = [0u32, 2560, 1920, 1600, 1280, 1024];
    let labels: Vec<String> = sizes.iter().map(|s| if *s == 0 { "Native".into() } else { s.to_string() }).collect();
    if let Some(i) = bar_chips(ui, &labels, sizes.iter().position(|s| *s == o.max_size).unwrap_or(0)) {
        o.max_size = sizes[i];
    }
    section(ui, "Bitrate");
    let rates = [4u32, 8, 16, 24, 40];
    let labels: Vec<String> = rates.iter().map(|r| format!("{r} Mbps")).collect();
    if let Some(i) = bar_chips(ui, &labels, rates.iter().position(|r| *r == o.video_bitrate_mbps).unwrap_or(2)) {
        o.video_bitrate_mbps = rates[i];
    }
    section(ui, "Frame rate");
    let fps = [0u32, 30, 60, 90, 120];
    let labels: Vec<String> = fps.iter().map(|f| if *f == 0 { "Phone".into() } else { format!("{f} fps") }).collect();
    if let Some(i) = bar_chips(ui, &labels, fps.iter().position(|f| *f == o.max_fps).unwrap_or(2)) {
        o.max_fps = fps[i];
    }
    section(ui, "Codec");
    let codecs = ["h264", "h265", "av1"];
    let labels: Vec<String> = ["H.264", "H.265", "AV1"].iter().map(|s| s.to_string()).collect();
    if let Some(i) = bar_chips(ui, &labels, codecs.iter().position(|c| *c == o.codec).unwrap_or(0)) {
        o.codec = codecs[i].into();
    }
    ui.add_space(6.0);
    for (label, value) in [
        ("Play phone audio on this computer", &mut o.audio),
        ("Keep the phone awake", &mut o.stay_awake),
        ("Turn the phone screen off", &mut o.turn_screen_off),
        ("Show touches on the phone", &mut o.show_touches),
        ("View only (no mouse or keyboard control)", &mut o.view_only),
    ] {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).size(13.0).color(BAR_TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                crate::editor::toggle(ui, value);
            });
        });
    }
    if *o != before {
        sh.settings.save();
    }
    ui.add_space(8.0);
    let connected = sh.mirror.as_ref().is_some_and(|m| m.device == device);
    let stale = sh.mirror.as_ref().is_some_and(|m| m.device == device && m.options != sh.settings.scrcpy);
    let label = if !connected { "Connect" } else if stale { "Reconnect with these options" } else { "Connected" };
    ui.horizontal(|ui| {
        let gap = ui.spacing().item_spacing.x;
        let close_w = 88.0;
        let go_w = ui.available_width() - close_w - gap;
        let go = egui::Button::new(egui::RichText::new(label).font(theme::semibold(13.5)).color(Color32::WHITE))
            .fill(if connected && !stale { Color32::from_white_alpha(16) } else { theme::ACCENT })
            .corner_radius(CornerRadius::same(9));
        if ui.add_enabled_ui(!connected || stale, |ui| ui.add_sized([go_w, 34.0], go)).inner.clicked() {
            sh.mirror = None;
            sh.mirror_window = None;
            remember_device(&mut sh.settings, &device.serial);
            match record::device::Mirror::open(&device, &sh.settings.scrcpy) {
                Ok(m) => sh.mirror = Some(m),
                Err(e) => eprintln!("open-studio: {e:#}"),
            }
            sh.mode = Mode::Device;
            sh.device_options = None;
        }
        let close = egui::Button::new(egui::RichText::new("Close").size(13.5).color(BAR_TEXT))
            .fill(Color32::from_white_alpha(12))
            .corner_radius(CornerRadius::same(9));
        if ui.add_sized([close_w, 34.0], close).clicked() {
            sh.device_options = None;
        }
    });
}

/// Dropdown above a bar entry: microphones (1) or system audio (2).
fn input_menu(ui: &mut egui::Ui, sh: &mut Shared, which: usize, anchor: Rect) {
    let mut items: Vec<(Option<String>, String)> = vec![];
    match which {
        0 => {
            for c in &sh.cameras {
                items.push((Some(c.id()), c.label()));
            }
            if sh.scanning.is_some() {
                items.push((Some(String::new()), "Searching for phone cameras…".into()));
            } else if sh.cameras.is_empty() {
                items.push((Some(String::new()), "No camera found".into()));
            }
            let preview = if sh.settings.camera_preview { "Hide camera preview" } else { "Show camera preview" };
            items.push((Some("preview".into()), preview.into()));
            items.push((None, "Don't record camera".into()));
        }
        1 => {
            items.push((None, "No microphone".into()));
            items.push((Some(crate::record::audio::DEFAULT_MIC.into()), "Default microphone".into()));
            for d in sh.audio_devices.iter().filter(|d| !d.monitor) {
                items.push((Some(d.name.clone()), d.description.clone()));
            }
        }
        3 => {
            for d in &sh.phones {
                // Phones still waiting for "Allow debugging" can't be picked.
                let value = if d.ready { d.serial.clone() } else { String::new() };
                items.push((Some(value), d.label()));
            }
            if sh.scanning.is_some() {
                items.push((Some(String::new()), "Searching for wireless devices…".into()));
            } else if items.is_empty() {
                items.push((Some(String::new()), "No device found. Connect a phone over USB, or turn on Wireless debugging.".into()));
            }
            if let Some(m) = &sh.mirror {
                items.push((Some(format!("options:{}", m.device.serial)), "Mirror options…".into()));
            }
        }
        _ => {
            items.push((None, "No system audio".into()));
            items.push((Some("on".into()), "Record system audio".into()));
        }
    }
    let row = 34.0;
    let w = if which == 3 { 380.0f32 } else { 300.0 };
    let h = items.len() as f32 * row + 16.0;
    let r = Rect::from_min_size(pos2(anchor.left(), anchor.top() - 14.0 - h), vec2(w, h));
    let p = ui.painter().clone();
    p.add(egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(110) }.as_shape(r, CornerRadius::same(12)));
    p.rect_filled(r, 12.0, BAR_BG);
    p.rect_stroke(r, 12.0, Stroke::new(1.0, BAR_BORDER), StrokeKind::Inside);
    let current = match which {
        1 => sh.settings.microphone.clone(),
        0 => sh.settings.camera.clone(),
        3 => sh.mirror.as_ref().map(|m| m.device.serial.clone()),
        _ => sh.settings.system_audio.then(|| "on".to_string()),
    };
    let mut chosen = None;
    for (k, (value, label)) in items.iter().enumerate() {
        let ir = Rect::from_min_size(pos2(r.left() + 8.0, r.top() + 8.0 + k as f32 * row), vec2(w - 16.0, row));
        let resp = ui.interact(ir, ui.scope_id().with(("menu-item", which, k)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.hovered() {
            p.rect_filled(ir, 8.0, Color32::from_white_alpha(16));
        }
        let selected = *value == current;
        if selected {
            p.circle_filled(pos2(ir.left() + 14.0, ir.center().y), 4.0, theme::KNOB);
        }
        let max = if which == 3 { 60 } else { 34 };
        let text = if label.chars().count() > max { label.chars().take(max - 1).collect::<String>() + "…" } else { label.clone() };
        p.text(pos2(ir.left() + 28.0, ir.center().y), Align2::LEFT_CENTER, text, FontId::proportional(14.0), BAR_TEXT);
        if resp.clicked() {
            chosen = Some(value.clone());
        }
    }
    if let Some(v) = chosen {
        match which {
            0 => {
                match v.as_deref() {
                    Some("") => return,
                    Some("preview") => sh.settings.camera_preview = !sh.settings.camera_preview,
                    _ => {
                        if let Some(serial) = v.as_deref().and_then(|id| id.strip_prefix("phone:")).and_then(|r| r.rsplit_once(':')).map(|(s, _)| s.to_string()) {
                            remember_device(&mut sh.settings, &serial);
                        }
                        if v != sh.settings.camera || sh.camera.is_none() {
                            sh.settings.camera = v.clone();
                            open_camera(sh, v, ui.ctx());
                        }
                    }
                }
                sh.settings.save();
                sh.menu = None;
                return;
            }
            3 => {
                // Choosing a phone (or "Mirror options…") opens its options;
                // the panel's Connect button starts the mirror.
                if v.as_deref() == Some("") {
                    return;
                }
                if let Some(v) = v {
                    let serial = v.strip_prefix("options:").unwrap_or(&v).to_string();
                    let device = sh.phones.iter().find(|d| d.serial == serial).cloned().or_else(|| sh.mirror.as_ref().map(|m| m.device.clone()));
                    sh.device_options = device;
                }
                sh.menu = None;
                return;
            }
            1 => sh.settings.microphone = v,
            _ => sh.settings.system_audio = v.is_some(),
        }
        sh.settings.save();
        sh.menu = None;
    }
}

fn gear_menu(ui: &mut egui::Ui, sh: &mut Shared, r: Rect) {
    let p = ui.painter().clone();
    p.add(egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(110) }.as_shape(r, CornerRadius::same(14)));
    p.rect_filled(r, 14.0, BAR_BG);
    p.rect_stroke(r, 14.0, Stroke::new(1.0, BAR_BORDER), StrokeKind::Inside);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(18.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    ui.spacing_mut().item_spacing.y = 10.0;
    ui.label(egui::RichText::new("Recording").font(theme::semibold(14.0)).color(BAR_TEXT));
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Show system cursor in video").size(13.5).color(BAR_TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed |= crate::editor::toggle(ui, &mut sh.settings.bake_cursor).changed();
        });
    });
    ui.label(
        egui::RichText::new("Off records the cursor separately so it can be smoothed and resized.").size(12.0).color(BAR_DIM),
    );
    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        icons::paint(ui.painter(), ir, Icon::Keyboard, BAR_TEXT);
        ui.label(egui::RichText::new("Stop recording").size(13.5).color(BAR_TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if sh.capturing_shortcut { "Press keys…".to_string() } else { sh.settings.stop_shortcut.clone() };
            let b = egui::Button::new(egui::RichText::new(label).size(13.0).color(BAR_TEXT))
                .fill(Color32::from_white_alpha(16))
                .corner_radius(CornerRadius::same(7));
            if ui.add(b).on_hover_text("Click, then press a key combination").clicked() {
                sh.capturing_shortcut = !sh.capturing_shortcut;
            }
        });
    });
    if sh.capturing_shortcut {
        let combo = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } if *key != egui::Key::Escape => {
                    let mut parts = vec![];
                    if modifiers.ctrl {
                        parts.push("Ctrl".to_string());
                    }
                    if modifiers.alt {
                        parts.push("Alt".to_string());
                    }
                    if modifiers.shift {
                        parts.push("Shift".to_string());
                    }
                    if modifiers.mac_cmd || modifiers.command && !modifiers.ctrl {
                        parts.push("Super".to_string());
                    }
                    parts.push(key.name().to_string());
                    Some(parts.join("+"))
                }
                _ => None,
            })
        });
        if let Some(c) = combo {
            if record::input::Shortcut::parse(&c).is_some() {
                sh.settings.stop_shortcut = c;
                changed = true;
            }
            sh.capturing_shortcut = false;
        }
    }
    if changed {
        sh.settings.save();
    }
}

fn area_ui(ui: &mut egui::Ui, sh: &mut Shared, mon: &Monitor, over_bar: bool) {
    let origin = vec2(mon.rect[0] as f32, mon.rect[1] as f32);
    let screen = ui.max_rect();
    let on_this = mon.name == sh.bar_monitor;
    let area = sh.area.translate(-origin);
    let p = ui.painter().clone();
    if !on_this {
        return;
    }
    // Dim everything but the area, tint the area.
    let dim = Color32::from_black_alpha(90);
    for r in [
        Rect::from_min_max(screen.min, pos2(screen.right(), area.top())),
        Rect::from_min_max(pos2(screen.left(), area.bottom()), screen.max),
        Rect::from_min_max(pos2(screen.left(), area.top()), pos2(area.left(), area.bottom())),
        Rect::from_min_max(pos2(area.right(), area.top()), pos2(screen.right(), area.bottom())),
    ] {
        p.rect_filled(r, 0.0, dim);
    }
    p.rect_filled(area, 0.0, Color32::from_rgba_premultiplied(40, 28, 120, 70));
    let dash = Stroke::new(1.5, Color32::WHITE);
    let corners = [area.left_top(), area.right_top(), area.right_bottom(), area.left_bottom(), area.left_top()];
    p.extend(egui::Shape::dashed_line(&corners, dash, 6.0, 4.0));

    // Interaction: handles, body, or drawing a new rectangle.
    let pointer = ui.ctx().pointer_interact_pos();
    let handles: [(i8, i8); 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];
    let hpos = |h: (i8, i8)| {
        pos2(
            match h.0 {
                -1 => area.left(),
                1 => area.right(),
                _ => area.center().x,
            },
            match h.1 {
                -1 => area.top(),
                1 => area.bottom(),
                _ => area.center().y,
            },
        )
    };
    let bg = ui.interact(screen, ui.scope_id().with("area-bg"), Sense::drag());
    let panel = Rect::from_center_size(area.center() - vec2(0.0, 30.0), vec2(330.0, 132.0));
    let start_btn = Rect::from_center_size(pos2(area.center().x, panel.bottom() + 40.0), vec2(232.0, 56.0));
    if bg.drag_started() && !over_bar {
        if let Some(pp) = ui.input(|i| i.pointer.press_origin()) {
            sh.area_drag = if let Some(h) = handles.iter().find(|h| hpos(**h).distance(pp) < 12.0) {
                Some(AreaDrag::Handle(h.0, h.1))
            } else if panel.contains(pp) || start_btn.contains(pp) {
                None
            } else if area.contains(pp) {
                Some(AreaDrag::Move { grab: pp - area.min.to_vec2() })
            } else {
                Some(AreaDrag::New { origin: pp })
            };
        }
    }
    if let (Some(drag), Some(pt)) = (sh.area_drag, pointer) {
        if bg.dragged() {
            let pt = pt.clamp(screen.min, screen.max);
            let mut a = area;
            match drag {
                AreaDrag::Move { grab } => {
                    let min = (pt - grab.to_vec2()).clamp(screen.min, (screen.max - a.size()).max(screen.min));
                    a = Rect::from_min_size(min, a.size());
                }
                AreaDrag::Handle(hx, hy) => {
                    match hx {
                        -1 => a.min.x = pt.x.min(a.max.x - 40.0),
                        1 => a.max.x = pt.x.max(a.min.x + 40.0),
                        _ => {}
                    }
                    match hy {
                        -1 => a.min.y = pt.y.min(a.max.y - 40.0),
                        1 => a.max.y = pt.y.max(a.min.y + 40.0),
                        _ => {}
                    }
                }
                AreaDrag::New { origin } => a = Rect::from_two_pos(origin, pt),
            }
            if a.width() >= 40.0 && a.height() >= 40.0 {
                sh.area = a.translate(origin);
            }
        }
    }
    if bg.drag_stopped() {
        sh.area_drag = None;
    }
    // Hover cursors.
    if let Some(h) = ui.ctx().pointer_hover_pos() {
        if let Some(hd) = handles.iter().find(|hd| hpos(**hd).distance(h) < 12.0) {
            ui.ctx().set_cursor_icon(match hd {
                (0, _) => egui::CursorIcon::ResizeVertical,
                (_, 0) => egui::CursorIcon::ResizeHorizontal,
                (a, b) if a == b => egui::CursorIcon::ResizeNwSe,
                _ => egui::CursorIcon::ResizeNeSw,
            });
        } else if area.contains(h) && !panel.contains(h) && !start_btn.contains(h) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
        } else if !area.contains(h) && !over_bar {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }
    for h in handles {
        let c = hpos(h);
        p.circle_filled(c, 6.5, Color32::WHITE);
        p.circle_filled(c, 4.5, Color32::from_rgb(0x12, 0x10, 0x18));
    }

    // Size / position panel and start button (Screen Studio layout).
    if area.width() > 360.0 && area.height() > 260.0 {
        p.add(egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(120) }.as_shape(panel, CornerRadius::same(12)));
        p.rect_filled(panel, 12.0, Color32::from_rgba_premultiplied(24, 24, 26, 240));
        p.rect_stroke(panel, 12.0, Stroke::new(1.0, Color32::from_white_alpha(20)), StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(panel.shrink2(vec2(20.0, 18.0))).layout(egui::Layout::top_down(egui::Align::Min)));
        let ui2 = &mut child;
        ui2.spacing_mut().item_spacing = vec2(10.0, 12.0);
        let mut g = sh.area;
        let row = |ui: &mut egui::Ui, label: &str, a: &mut f32, b: &mut f32, sep: &str| {
            ui.horizontal(|ui| {
                ui.add_sized(vec2(70.0, 34.0), egui::Label::new(egui::RichText::new(label).size(14.0).color(BAR_DIM)));
                let field = |ui: &mut egui::Ui, v: &mut f32| {
                    ui.add_sized(vec2(78.0, 34.0), egui::DragValue::new(v).speed(1.0).range(0.0..=20000.0).max_decimals(0))
                };
                let c1 = field(ui, a).changed();
                ui.label(egui::RichText::new(sep).color(BAR_DIM));
                let c2 = field(ui, b).changed();
                ui.label(egui::RichText::new("px").color(BAR_DIM));
                c1 || c2
            })
            .inner
        };
        let (mut w, mut h) = (g.width(), g.height());
        let (mut x, mut y) = (g.left() - origin.x, g.top() - origin.y);
        let size_changed = row(ui2, "Size", &mut w, &mut h, "×");
        let pos_changed = row(ui2, "Position", &mut x, &mut y, "  ");
        if size_changed || pos_changed {
            g = Rect::from_min_size(pos2(x, y) + origin, vec2(w.max(40.0), h.max(40.0)));
            sh.area = g;
        }
    }
    if !over_bar {
        let resp = ui.interact(start_btn, ui.scope_id().with("area-start"), Sense::click());
        let fill = if resp.hovered() { theme::ACCENT.gamma_multiply(1.12) } else { theme::ACCENT };
        p.rect_filled(start_btn, 12.0, fill);
        let ic = pos2(start_btn.left() + 46.0, start_btn.center().y);
        p.circle_stroke(ic, 10.0, Stroke::new(2.0, Color32::WHITE));
        p.circle_filled(ic, 4.5, Color32::WHITE);
        p.text(pos2(ic.x + 22.0, start_btn.center().y), Align2::LEFT_CENTER, "Start recording", FontId::proportional(18.0), Color32::WHITE);
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if resp.clicked() || enter {
            sh.start = Some(area_request(mon, sh.area));
        }
    }
}

/// Capture request for `area` (global logical) on `mon`, cropped in buffer pixels.
fn area_request(mon: &Monitor, area: Rect) -> DirectRequest {
    let r = mon.rect;
    let s = mon.pixels.0 as f64 / r[2];
    let x0 = (area.left() as f64).max(r[0]);
    let y0 = (area.top() as f64).max(r[1]);
    let x1 = (area.right() as f64).min(r[0] + r[2]);
    let y1 = (area.bottom() as f64).min(r[1] + r[3]);
    let crop = [((x0 - r[0]) * s) as u32, ((y0 - r[1]) * s) as u32, ((x1 - x0) * s) as u32, ((y1 - y0) * s) as u32];
    let (cw, ch) = ((crop[2] & !1) as f64 / s, (crop[3] & !1) as f64 / s);
    DirectRequest {
        target: record::direct::Target::Output { name: mon.name.clone(), crop: Some(crop) },
        kind: "area".into(),
        name: Some(mon.name.clone()),
        logical_rect: [x0, y0, cw, ch],
    }
}

// ---- UI: recording widget ------------------------------------------------------

const WIDGET_W: u32 = 300;
const WIDGET_H: u32 = 72;

fn widget_ui(ui: &mut egui::Ui, sh: &mut Shared) {
    let r = ui.max_rect().shrink2(vec2(6.0, 6.0));
    let p = ui.painter().clone();
    p.rect_filled(r, 18.0, Color32::from_rgba_premultiplied(32, 19, 41, 184));
    p.rect_stroke(r, 18.0, Stroke::new(1.0, Color32::from_white_alpha(30)), StrokeKind::Inside);
    let cy = r.center().y;
    if sh.saving {
        p.text(r.center(), Align2::CENTER_CENTER, "Saving…", FontId::proportional(16.0), BAR_TEXT);
        return;
    }
    // Finish + timer.
    let finish = Rect::from_min_size(pos2(r.left() + 10.0, r.top() + 8.0), vec2(110.0, r.height() - 16.0));
    let fr = ui.interact(finish, ui.scope_id().with("finish"), Sense::click()).on_hover_text(format!("Finish recording ({})", sh.settings.stop_shortcut));
    if fr.hovered() {
        p.rect_filled(finish, 12.0, Color32::from_white_alpha(12));
    }
    icons::paint(&p, Rect::from_center_size(pos2(finish.left() + 24.0, cy), vec2(28.0, 28.0)), Icon::StopCircle, RED);
    let secs = sh.elapsed as u64;
    p.text(pos2(finish.left() + 46.0, cy), Align2::LEFT_CENTER, format!("{}:{:02}", secs / 60, secs % 60), FontId::proportional(22.0), RED);
    p.line_segment([pos2(finish.right() + 8.0, r.top() + 16.0), pos2(finish.right() + 8.0, r.bottom() - 16.0)], Stroke::new(1.0, Color32::from_white_alpha(30)));
    let mut x = finish.right() + 20.0;
    let mut button = |icon: Icon, tip: &str, enabled: bool| {
        let b = Rect::from_min_size(pos2(x, r.top() + 10.0), vec2(48.0, r.height() - 20.0));
        x += 52.0;
        let resp = ui.interact(b, ui.scope_id().with(("w", tip)), Sense::click()).on_hover_text(tip);
        if resp.hovered() && enabled {
            p.rect_filled(b, 10.0, Color32::from_white_alpha(14));
        }
        let col = if enabled { Color32::WHITE } else { Color32::from_white_alpha(70) };
        icons::paint(&p, Rect::from_center_size(b.center(), vec2(26.0, 26.0)), icon, col);
        enabled && resp.clicked()
    };
    let pause_icon = if sh.paused { Icon::Record } else { Icon::PauseCircle };
    let pause_tip = if sh.paused { "Resume" } else { "Pause" };
    if button(pause_icon, pause_tip, sh.can_pause) {
        sh.action = Some(WidgetAction::TogglePause);
    }
    if button(Icon::Restart, "Restart", true) {
        sh.action = Some(WidgetAction::Restart);
    }
    if button(Icon::Trash, "Delete recording", true) {
        sh.action = Some(WidgetAction::Delete);
    }
    if fr.clicked() {
        sh.action = Some(WidgetAction::Finish);
    }
    if sh.paused {
        p.text(pos2(finish.left() + 46.0, r.bottom() - 10.0), Align2::LEFT_BOTTOM, "Paused", FontId::proportional(10.5), BAR_DIM);
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
}

// ---- Wayland plumbing ------------------------------------------------------------

impl CompositorHandler for Rt {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, factor: i32) {
        if let Some(i) = self.surf_index(surface) {
            self.surfaces[i].scale = factor.max(1);
            if self.surfaces[i].size.0 > 0 {
                self.configure_surface(i);
            }
        }
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for Rt {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for Rt {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.surfaces.retain(|s| &s.layer != layer);
    }
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        let Some(i) = self.surfaces.iter().position(|s| &s.layer == layer) else { return };
        let (mut w, mut h) = configure.new_size;
        if w == 0 || h == 0 {
            let m = &self.surfaces[i].monitor;
            (w, h) = match self.surfaces[i].kind {
                SurfKind::Widget => (WIDGET_W, WIDGET_H),
                SurfKind::Overlay => (m.rect[2] as u32, m.rect[3] as u32),
            };
        }
        self.surfaces[i].size = (w, h);
        if std::env::var_os("OPEN_STUDIO_DEBUG_UI").is_some() {
            eprintln!("configure {} {}x{}", self.surfaces[i].monitor.name, w, h);
        }
        // A sensible initial scale until the compositor tells us.
        if self.surfaces[i].scale == 1 {
            let m = &self.surfaces[i].monitor;
            let s = (m.pixels.0 as f64 / m.rect[2]).ceil() as i32;
            self.surfaces[i].scale = s.max(1);
        }
        if self.gpu.is_none() {
            if let Some(ws) = self.surfaces[i].wgpu.take() {
                let _ = self.gpu(&ws);
                self.surfaces[i].wgpu = Some(ws);
            }
        }
        self.configure_surface(i);
    }
}

impl SeatHandler for Rt {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor.create_surface(qh);
            if let Ok(p) = self.seat_state.get_pointer_with_theme::<Rt, ()>(qh, &seat, self.shm.wl_shm(), surface, ThemeSpec::default()) {
                self.pointer = Some(p);
            }
        }
        if cap == Capability::Keyboard {
            let _ = self.seat_state.get_keyboard(qh, &seat, None);
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, _: Capability) {}
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for Rt {
    fn pointer_frame(&mut self, conn: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for ev in events {
            let Some(i) = self.surf_index(&ev.surface) else { continue };
            if std::env::var_os("OPEN_STUDIO_DEBUG_UI").is_some() && !matches!(ev.kind, PointerEventKind::Motion { .. }) {
                eprintln!("pointer {:?} on {}", ev.kind, self.surfaces[i].monitor.name);
            }
            let pos = pos2(ev.position.0 as f32, ev.position.1 as f32);
            let mods = self.modifiers;
            let s = &mut self.surfaces[i];
            match ev.kind {
                PointerEventKind::Enter { .. } => {
                    self.pointer_surface = Some(ev.surface.clone());
                    s.events.push(egui::Event::PointerMoved(pos));
                    s.cursor = egui::CursorIcon::Default;
                    if let Some(p) = &self.pointer {
                        let _ = p.set_cursor(conn, CursorIcon::Default);
                    }
                }
                PointerEventKind::Leave { .. } => {
                    s.events.push(egui::Event::PointerGone);
                    if self.pointer_surface.as_ref() == Some(&ev.surface) {
                        self.pointer_surface = None;
                    }
                }
                PointerEventKind::Motion { .. } => s.events.push(egui::Event::PointerMoved(pos)),
                PointerEventKind::Press { button, .. } | PointerEventKind::Release { button, .. } => {
                    let pressed = matches!(ev.kind, PointerEventKind::Press { .. });
                    let button = match button {
                        0x110 => egui::PointerButton::Primary,
                        0x111 => egui::PointerButton::Secondary,
                        0x112 => egui::PointerButton::Middle,
                        _ => continue,
                    };
                    s.events.push(egui::Event::PointerButton { pos, button, pressed, modifiers: mods });
                }
                PointerEventKind::Axis { horizontal, vertical, .. } => {
                    s.events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: vec2(-horizontal.absolute as f32, -vertical.absolute as f32),
                        phase: egui::TouchPhase::Move,
                        modifiers: mods,
                        source: egui::MouseWheelSource::Wheel,
                    });
                }
            }
        }
    }
}

fn map_key(k: Keysym) -> Option<egui::Key> {
    use egui::Key;
    Some(match k {
        Keysym::Escape => Key::Escape,
        Keysym::Return | Keysym::KP_Enter => Key::Enter,
        Keysym::BackSpace => Key::Backspace,
        Keysym::Tab => Key::Tab,
        Keysym::Delete => Key::Delete,
        Keysym::Left => Key::ArrowLeft,
        Keysym::Right => Key::ArrowRight,
        Keysym::Up => Key::ArrowUp,
        Keysym::Down => Key::ArrowDown,
        Keysym::space => Key::Space,
        Keysym::F9 => Key::F9,
        Keysym::F10 => Key::F10,
        Keysym::F11 => Key::F11,
        Keysym::F12 => Key::F12,
        _ => {
            let c = char::from_u32(k.key_char().map(|c| c as u32).unwrap_or(0))?;
            return Key::from_name(&c.to_ascii_uppercase().to_string());
        }
    })
}

impl KeyboardHandler for Rt {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, surface: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {
        self.keyboard_surface = Some(surface.clone());
    }
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {
        self.keyboard_surface = None;
    }
    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        self.key(event, true);
    }
    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        self.key(event, true);
    }
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        self.key(event, false);
    }
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, m: Modifiers, _: RawModifiers, _: u32) {
        self.modifiers = egui::Modifiers { alt: m.alt, ctrl: m.ctrl, shift: m.shift, mac_cmd: false, command: m.ctrl };
    }
}

impl Rt {
    fn key(&mut self, event: KeyEvent, pressed: bool) {
        let Some(surface) = self.keyboard_surface.clone() else { return };
        let Some(i) = self.surf_index(&surface) else { return };
        let mods = self.modifiers;
        let s = &mut self.surfaces[i];
        if let Some(key) = map_key(event.keysym) {
            s.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: mods });
        }
        if pressed && !mods.ctrl && !mods.alt {
            if let Some(t) = event.utf8.filter(|t| t.chars().all(|c| !c.is_control())) {
                s.events.push(egui::Event::Text(t));
            }
        }
    }
}

impl ShmHandler for Rt {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Rt {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(Rt);
smithay_client_toolkit::delegate_dispatch2!(Rt);

#[cfg(test)]
mod tests {
    //! Clicks every control of the picker bar and the recording widget
    //! through the real UI code with synthetic pointer events.
    use super::*;

    fn shared() -> Shared {
        Shared {
            mode: Mode::Display,
            monitors: vec![test_monitor()],
            windows: vec![],
            bar_monitor: "T".into(),
            area: Rect::from_min_size(pos2(400.0, 250.0), vec2(900.0, 560.0)),
            area_drag: None,
            settings: Settings::default(),
            gear_open: false,
            capturing_shortcut: false,
            start: None,
            cancel: false,
            open_projects: false,
            menu: None,
            scrcpy: false,
            phones: vec![],
            mirror: None,
            mirror_window: None,
            mirror_checked: None,
            device_options: None,
            cameras: vec![],
            camera: None,
            camera_opening: None,
            camera_error: None,
            scanning: None,
            audio_devices: vec![],
            elapsed: 3.0,
            paused: false,
            can_pause: true,
            saving: false,
            action: None,
        }
    }

    fn test_monitor() -> Monitor {
        Monitor { name: "T".into(), description: "Test Display".into(), rect: [0.0, 0.0, 1920.0, 1080.0], pixels: (1920, 1080), refresh: 60.0 }
    }

    struct Harness {
        ctx: egui::Context,
        size: egui::Vec2,
        widget: bool,
        t: f64,
    }

    impl Harness {
        fn new(widget: bool) -> Self {
            let size = if widget { vec2(WIDGET_W as f32, WIDGET_H as f32) } else { vec2(1920.0, 1080.0) };
            Self { ctx: egui::Context::default(), size, widget, t: 0.0 }
        }
        fn frame(&mut self, sh: &mut Shared, events: Vec<egui::Event>) {
            self.t += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                time: Some(self.t),
                events,
                ..Default::default()
            };
            let mon = test_monitor();
            let widget = self.widget;
            let mut out = self.ctx.run_ui(raw, |ui| if widget { widget_ui(ui, sh) } else { overlay_ui(ui, sh, &mon) });
            // No renderer here to upload the font atlas to.
            out.textures_delta.clear();
        }
        fn click(&mut self, sh: &mut Shared, pos: Pos2) {
            let m = egui::Modifiers::default();
            self.frame(sh, vec![egui::Event::PointerMoved(pos)]);
            self.frame(sh, vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: m }]);
            self.frame(sh, vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: m }]);
            self.frame(sh, vec![]);
        }
    }

    fn bar() -> Rect {
        bar_rect(Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1080.0)))
    }
    fn tile(i: usize) -> Pos2 {
        let b = bar();
        pos2(b.left() + 18.0 + 46.0 + 10.0 + 74.0 * i as f32 + 35.0, b.center().y)
    }

    fn isolate_config() {
        // Keep tests away from the real settings file.
        let dir = std::env::temp_dir().join("open-studio-test-config");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
    }

    #[test]
    fn device_options_panel() {
        isolate_config();
        let mut h = Harness::new(false);
        let mut sh = shared();
        sh.scrcpy = true;
        sh.device_options = Some(record::device::Device { serial: "x".into(), model: "Test Phone".into(), wireless: false, ready: true });
        h.frame(&mut sh, vec![]);
        h.frame(&mut sh, vec![]);
        // The panel fits its content and ends above the bar.
        let r = device_options_rect(&h.ctx, bar());
        assert!(r.height() > 300.0 && r.height() < 600.0 && r.bottom() < bar().top(), "{r:?}");
        // A click on the panel background neither closes it nor starts anything.
        h.click(&mut sh, pos2(r.right() - 30.0, r.top() + 12.0));
        assert!(sh.device_options.is_some() && sh.start.is_none() && !sh.cancel);
        // Escape closes the panel before it would close the bar.
        h.frame(&mut sh, vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
        assert!(sh.device_options.is_none() && !sh.cancel);
        let o = crate::settings::ScrcpyOptions { max_size: 1920, max_fps: 0, audio: false, ..Default::default() };
        let a = o.args();
        assert!(a.contains(&"--max-size=1920".to_string()) && a.contains(&"--no-audio".to_string()) && !a.iter().any(|x| x.starts_with("--max-fps")));
    }

    #[test]
    fn bar_mode_tiles_switch_modes() {
        isolate_config();
        let (mut h, mut sh) = (Harness::new(false), shared());
        h.click(&mut sh, tile(1));
        assert_eq!(sh.mode, Mode::Window);
        h.click(&mut sh, tile(2));
        assert_eq!(sh.mode, Mode::Area);
        h.click(&mut sh, tile(0));
        assert_eq!(sh.mode, Mode::Display);
        // Device is disabled without scrcpy.
        h.click(&mut sh, tile(3));
        assert_eq!(sh.mode, Mode::Display);
        assert!(sh.start.is_none() && !sh.cancel && sh.menu.is_none());
        // With scrcpy it opens the device menu (empty list here).
        sh.scrcpy = true;
        sh.phones = vec![];
        h.click(&mut sh, tile(3));
        assert_eq!(sh.menu, Some(3));
        assert_eq!(sh.mode, Mode::Display);
    }

    #[test]
    fn bar_close_cancels() {
        isolate_config();
        let (mut h, mut sh) = (Harness::new(false), shared());
        let b = bar();
        h.click(&mut sh, pos2(b.left() + 34.0, b.center().y));
        assert!(sh.cancel);
    }

    #[test]
    fn escape_cancels() {
        isolate_config();
        let (mut h, mut sh) = (Harness::new(false), shared());
        h.frame(&mut sh, vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
        assert!(sh.cancel);
    }

    #[test]
    fn gear_menu_opens_and_toggles_cursor_option() {
        isolate_config();
        let (mut h, mut sh) = (Harness::new(false), shared());
        let b = bar();
        let gear = pos2(b.right() - 64.0 + 26.0, b.center().y);
        h.click(&mut sh, gear);
        assert!(sh.gear_open);
        // The switch sits at the right edge of the first option row.
        let m = gear_menu_rect(b).shrink(18.0);
        let mut toggled = false;
        for dy in (0..90).step_by(4) {
            h.click(&mut sh, pos2(m.right() - 20.0, m.top() + 26.0 + dy as f32));
            if sh.settings.bake_cursor {
                toggled = true;
                break;
            }
        }
        assert!(toggled, "system cursor switch didn't toggle");
        h.click(&mut sh, gear);
        assert!(!sh.gear_open);
    }

    #[test]
    fn display_click_starts_recording_that_display() {
        isolate_config();
        let (mut h, mut sh) = (Harness::new(false), shared());
        // "Start recording" button under the centred card.
        h.click(&mut sh, pos2(960.0, 540.0 + 30.0));
        let req = sh.start.expect("no recording started");
        assert_eq!(req.kind, "monitor");
        assert_eq!(req.name.as_deref(), Some("T"));
    }

    #[test]
    fn area_start_button_records_the_area() {
        isolate_config();
        let (mut h, mut sh) = (Harness::new(false), shared());
        h.click(&mut sh, tile(2));
        assert_eq!(sh.mode, Mode::Area);
        let a = sh.area;
        h.click(&mut sh, pos2(a.center().x, a.center().y - 30.0 + 66.0 + 40.0));
        let req = sh.start.expect("area recording didn't start");
        assert_eq!(req.kind, "area");
        assert_eq!(req.logical_rect, [400.0, 250.0, 900.0, 560.0]);
    }

    #[test]
    fn stop_shortcut_can_be_changed() {
        isolate_config();
        let (mut h, mut sh) = (Harness::new(false), shared());
        let b = bar();
        h.click(&mut sh, pos2(b.right() - 64.0 + 26.0, b.center().y));
        let m = gear_menu_rect(b).shrink(18.0);
        // The shortcut button is on the last row, right-aligned.
        for dy in (60..150).step_by(4) {
            h.click(&mut sh, pos2(m.right() - 30.0, m.top() + dy as f32));
            if sh.capturing_shortcut {
                break;
            }
        }
        assert!(sh.capturing_shortcut, "shortcut button not found");
        let mods = egui::Modifiers { ctrl: true, shift: true, command: true, ..Default::default() };
        h.frame(&mut sh, vec![egui::Event::Key { key: egui::Key::S, physical_key: None, pressed: true, repeat: false, modifiers: mods }]);
        assert_eq!(sh.settings.stop_shortcut, "Ctrl+Shift+S");
        assert!(!sh.capturing_shortcut);
    }

    #[test]
    fn widget_buttons() {
        isolate_config();
        let (mut h, mut sh) = (Harness::new(true), shared());
        let r = Rect::from_min_size(Pos2::ZERO, vec2(WIDGET_W as f32, WIDGET_H as f32)).shrink2(vec2(6.0, 6.0));
        let finish = pos2(r.left() + 60.0, r.center().y);
        let first = r.left() + 10.0 + 110.0 + 20.0;
        let button = |i: usize| pos2(first + 52.0 * i as f32 + 24.0, r.center().y);
        for (pos, want) in [
            (button(0), WidgetAction::TogglePause),
            (button(1), WidgetAction::Restart),
            (button(2), WidgetAction::Delete),
            (finish, WidgetAction::Finish),
        ] {
            sh.action = None;
            h.click(&mut sh, pos);
            assert!(sh.action == Some(want), "expected {:?} at {pos:?}", want as u8);
        }
        // Pause is disabled when the backend can't pause.
        sh.action = None;
        sh.can_pause = false;
        h.click(&mut sh, button(0));
        assert!(sh.action.is_none());
    }
}
