//! Direct screen capture through ext-image-copy-capture-v1 (no portal):
//! whole outputs, a cropped region of an output, or a single window. Frames
//! are copied into shared memory by the compositor and handed to a sink.

use super::clock::RecClock;
use anyhow::{Context, Result, anyhow, bail};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use wayland_client::protocol::{wl_buffer, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1 as tl_handle, ext_foreign_toplevel_list_v1 as tl_list,
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1 as tl_src_mgr, ext_image_capture_source_v1,
    ext_output_image_capture_source_manager_v1 as out_src_mgr,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_cursor_session_v1 as cursor_session, ext_image_copy_capture_frame_v1 as frame,
    ext_image_copy_capture_manager_v1 as cap_mgr, ext_image_copy_capture_session_v1 as session,
};

#[derive(Clone, Debug)]
pub enum Target {
    /// Whole output, optionally cropped to a region in buffer pixels.
    Output { name: String, crop: Option<[u32; 4]> },
    /// A window, matched by app id and title.
    Window { app_id: String, title: String },
}

/// What the capture produces: tightly packed 4-byte pixels.
#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    /// GStreamer format name ("BGRx", "RGBx", "BGR10x2_LE", ...).
    pub gst_format: &'static str,
}

pub type Sink = Box<dyn FnMut(&[u8], i64) + Send>;

#[derive(Default)]
struct Toplevel {
    handle: Option<tl_handle::ExtForeignToplevelHandleV1>,
    app_id: String,
    title: String,
}

#[derive(Default)]
struct State {
    shm: Option<wl_shm::WlShm>,
    out_mgr: Option<out_src_mgr::ExtOutputImageCaptureSourceManagerV1>,
    tl_mgr: Option<tl_src_mgr::ExtForeignToplevelImageCaptureSourceManagerV1>,
    cap: Option<cap_mgr::ExtImageCopyCaptureManagerV1>,
    list: Option<tl_list::ExtForeignToplevelListV1>,
    seat: Option<wl_seat::WlSeat>,
    has_pointer: bool,
    cursor: Option<CursorSink>,
    outputs: Vec<(wl_output::WlOutput, String)>,
    toplevels: Vec<Toplevel>,
    size: (u32, u32),
    formats: Vec<wl_shm::Format>,
    constraints_done: bool,
    stopped: bool,
    ready: bool,
    failed: bool,
}

struct Shm {
    ptr: *mut u8,
    len: usize,
    _fd: OwnedFd,
    buffer: wl_buffer::WlBuffer,
}
unsafe impl Send for Shm {}
impl Drop for Shm {
    fn drop(&mut self) {
        self.buffer.destroy();
        unsafe { libc::munmap(self.ptr as *mut _, self.len) };
    }
}

fn gst_format(f: wl_shm::Format) -> Option<&'static str> {
    use wl_shm::Format as F;
    Some(match f {
        F::Xrgb8888 => "BGRx",
        F::Argb8888 => "BGRA",
        F::Xbgr8888 => "RGBx",
        F::Abgr8888 => "RGBA",
        F::Xrgb2101010 | F::Argb2101010 => "BGR10A2_LE",
        F::Xbgr2101010 | F::Abgr2101010 => "RGB10A2_LE",
        _ => return None,
    })
}

/// Where cursor positions go: (monotonic ns, x, y) in video pixels.
pub type Moves = Arc<Mutex<Vec<(i64, f64, f64)>>>;

struct CursorSink {
    /// Top-left of the video in the captured buffer (the crop).
    origin: (f64, f64),
    moves: Moves,
    last: Option<(f64, f64)>,
    last_push: i64,
}

impl CursorSink {
    fn push(&mut self, p: (f64, f64)) {
        let t = crate::util::monotonic_ns();
        self.moves.lock().unwrap().push((t, p.0, p.1));
        self.last = Some(p);
        self.last_push = t;
    }
}

/// Connects and creates a capture source for `target`.
fn connect(target: &Target) -> Result<(Connection, EventQueue<State>, State, ext_image_capture_source_v1::ExtImageCaptureSourceV1)> {
    let conn = Connection::connect_to_env().context("connecting to Wayland")?;
    let mut queue: EventQueue<State> = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut st = State::default();
    queue.roundtrip(&mut st)?;
    queue.roundtrip(&mut st)?;
    let source = match target {
        Target::Output { name, .. } => {
            let mgr = st.out_mgr.clone().ok_or_else(|| anyhow!("no output capture source manager"))?;
            let out = st.outputs.iter().find(|(_, n)| n == name).ok_or_else(|| anyhow!("output {name} not found"))?.0.clone();
            mgr.create_source(&out, &qh, ())
        }
        Target::Window { app_id, title } => {
            let mgr = st.tl_mgr.clone().ok_or_else(|| anyhow!("no window capture source manager"))?;
            let tl = st
                .toplevels
                .iter()
                .find(|t| &t.app_id == app_id && &t.title == title)
                .or_else(|| st.toplevels.iter().find(|t| &t.app_id == app_id))
                .ok_or_else(|| anyhow!("window {app_id} \"{title}\" not found"))?;
            mgr.create_source(tl.handle.as_ref().unwrap(), &qh, ())
        }
    };
    Ok((conn, queue, st, source))
}

/// Tracks the pointer over `target` with a cursor capture session, on its
/// own thread, until `stop`. The compositor reports positions relative to
/// the captured image, so they need no layout or window geometry. Returns
/// an error when the compositor doesn't support it.
pub fn spawn_cursor_tracker(target: &Target, stop: Arc<AtomicBool>, moves: Moves) -> Result<()> {
    let (conn, mut queue, mut st, source) = connect(target)?;
    let qh = queue.handle();
    let cap = st.cap.clone().ok_or_else(|| anyhow!("compositor lacks ext-image-copy-capture"))?;
    anyhow::ensure!(st.has_pointer, "no pointer");
    let pointer = st.seat.as_ref().unwrap().get_pointer(&qh, ());
    let origin = match target {
        Target::Output { crop: Some(c), .. } => (c[0] as f64, c[1] as f64),
        _ => (0.0, 0.0),
    };
    st.cursor = Some(CursorSink { origin, moves, last: None, last_push: 0 });
    let session = cap.create_pointer_cursor_session(&source, &pointer, &qh, ());
    // An unsupported request ends the connection with a protocol error.
    queue.roundtrip(&mut st).context("cursor session refused")?;
    std::thread::spawn(move || {
        let _keep = (conn, source, pointer, session);
        while !stop.load(Ordering::Relaxed) {
            if queue.dispatch_pending(&mut st).is_err() {
                return;
            }
            if let Some(guard) = queue.prepare_read() {
                let fd = guard.connection_fd().as_raw_fd();
                let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
                if unsafe { libc::poll(&mut pfd, 1, 50) } > 0 && guard.read().is_err() {
                    return;
                }
            }
            if queue.flush().is_err() {
                return;
            }
            // A heartbeat so idle time is represented.
            let c = st.cursor.as_mut().unwrap();
            if let Some(p) = c.last.filter(|_| crate::util::monotonic_ns() - c.last_push > 500_000_000) {
                c.push(p);
            }
        }
    });
    Ok(())
}

pub struct Direct {
    queue: EventQueue<State>,
    st: State,
    session: session::ExtImageCopyCaptureSessionV1,
    shm: Shm,
    format: wl_shm::Format,
    crop: Option<[u32; 4]>,
    pub info: FrameInfo,
    _conn: Connection,
}

impl Direct {
    /// Connects and creates the capture session; returns once the frame size
    /// and format are known.
    pub fn open(target: &Target, paint_cursor: bool) -> Result<Self> {
        let (conn, mut queue, mut st, source) = connect(target)?;
        let qh = queue.handle();
        let cap = st.cap.clone().ok_or_else(|| anyhow!("compositor lacks ext-image-copy-capture"))?;
        let opts = if paint_cursor { cap_mgr::Options::PaintCursors } else { cap_mgr::Options::empty() };
        let session = cap.create_session(&source, opts, &qh, ());
        while !st.constraints_done && !st.stopped {
            queue.blocking_dispatch(&mut st)?;
        }
        if st.stopped {
            bail!("capture session was refused");
        }
        let order = [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888, wl_shm::Format::Xbgr8888, wl_shm::Format::Abgr8888];
        let format = order
            .iter()
            .copied()
            .find(|f| st.formats.contains(f))
            .or_else(|| st.formats.iter().copied().find(|f| gst_format(*f).is_some()))
            .ok_or_else(|| anyhow!("no usable shm format offered: {:?}", st.formats))?;
        let (w, h) = st.size;
        let shm = make_shm(st.shm.as_ref().unwrap(), w, h, format, &qh)?;
        let crop = match target {
            Target::Output { crop: Some(c), .. } => {
                // Keep the crop inside the frame, with even dimensions for 4:2:x encoders.
                let x = c[0].min(w.saturating_sub(2));
                let y = c[1].min(h.saturating_sub(2));
                let cw = c[2].min(w - x) & !1;
                let ch = c[3].min(h - y) & !1;
                Some([x, y, cw.max(2), ch.max(2)])
            }
            _ => None,
        };
        let (iw, ih) = crop.map(|c| (c[2], c[3])).unwrap_or((w & !1, h & !1));
        let info = FrameInfo { width: iw, height: ih, gst_format: gst_format(format).unwrap() };
        Ok(Self { queue, st, session, shm, format, crop, info, _conn: conn })
    }

    /// Runs the capture loop on the current thread until `stop` is set.
    /// While waiting for the screen to change, the last frame is re-sent
    /// every 100ms so the encoder's constant frame rate has input.
    pub fn run(mut self, stop: Arc<AtomicBool>, clock: Arc<RecClock>, mut sink: Sink) -> Result<()> {
        let qh = self.queue.handle();
        let (w, h) = self.st.size;
        let stride = (w * 4) as usize;
        let (ox, oy, ow, oh) = match self.crop {
            Some(c) => (c[0], c[1], c[2], c[3]),
            None => (0, 0, self.info.width, self.info.height),
        };
        let mut frame_buf = vec![0u8; (ow * oh * 4) as usize];
        let mut have_frame = false;
        let mut last_push = std::time::Instant::now();
        let _ = self.format;
        while !stop.load(Ordering::Relaxed) {
            if clock.is_paused() {
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            self.st.ready = false;
            self.st.failed = false;
            let f = self.session.create_frame(&qh, ());
            f.attach_buffer(&self.shm.buffer);
            f.damage_buffer(0, 0, w as i32, h as i32);
            f.capture();
            self.queue.flush()?;
            // Wait for the frame, re-sending the previous one meanwhile.
            while !self.st.ready && !self.st.failed && !self.st.stopped && !stop.load(Ordering::Relaxed) {
                self.queue.dispatch_pending(&mut self.st)?;
                if self.st.ready || self.st.failed {
                    break;
                }
                if let Some(guard) = self.queue.prepare_read() {
                    let fd = guard.connection_fd().as_raw_fd();
                    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
                    if unsafe { libc::poll(&mut pfd, 1, 20) } > 0 {
                        guard.read()?;
                    }
                }
                if have_frame && last_push.elapsed().as_millis() >= 100 && !clock.is_paused() {
                    if let Some(t) = clock.now() {
                        sink(&frame_buf, t);
                        last_push = std::time::Instant::now();
                    }
                }
            }
            f.destroy();
            if self.st.stopped {
                bail!("capture source went away");
            }
            if !self.st.ready {
                // Failed (e.g. resize): renegotiate on the next round.
                if self.st.size != (w, h) {
                    bail!("source size changed");
                }
                continue;
            }
            let data = unsafe { std::slice::from_raw_parts(self.shm.ptr, self.shm.len) };
            for row in 0..oh as usize {
                let src = (oy as usize + row) * stride + ox as usize * 4;
                frame_buf[row * ow as usize * 4..][..ow as usize * 4].copy_from_slice(&data[src..src + ow as usize * 4]);
            }
            have_frame = true;
            if let Some(t) = clock.now() {
                sink(&frame_buf, t);
                last_push = std::time::Instant::now();
            }
        }
        Ok(())
    }
}

fn make_shm(shm: &wl_shm::WlShm, w: u32, h: u32, format: wl_shm::Format, qh: &QueueHandle<State>) -> Result<Shm> {
    let stride = w * 4;
    let len = (stride * h) as usize;
    let fd = unsafe { libc::memfd_create(c"open-studio-capture".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        bail!("memfd_create failed");
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if unsafe { libc::ftruncate(fd.as_raw_fd(), len as i64) } != 0 {
        bail!("ftruncate failed");
    }
    let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
    if ptr == libc::MAP_FAILED {
        bail!("mmap failed");
    }
    let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
    let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, qh, ());
    pool.destroy();
    Ok(Shm { ptr: ptr as *mut u8, len, _fd: fd, buffer })
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(st: &mut Self, reg: &wl_registry::WlRegistry, ev: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = ev {
            match interface.as_str() {
                "wl_shm" => st.shm = Some(reg.bind(name, 1, qh, ())),
                "ext_output_image_capture_source_manager_v1" => st.out_mgr = Some(reg.bind(name, 1, qh, ())),
                "ext_foreign_toplevel_image_capture_source_manager_v1" => st.tl_mgr = Some(reg.bind(name, 1, qh, ())),
                "ext_image_copy_capture_manager_v1" => st.cap = Some(reg.bind(name, 1, qh, ())),
                "ext_foreign_toplevel_list_v1" => st.list = Some(reg.bind(name, 1, qh, ())),
                "wl_seat" if st.seat.is_none() => st.seat = Some(reg.bind(name, version.min(5), qh, ())),
                "wl_output" => {
                    let idx = st.outputs.len();
                    let o = reg.bind(name, version.min(4), qh, idx);
                    st.outputs.push((o, String::new()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(st: &mut Self, _: &wl_output::WlOutput, ev: wl_output::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = ev {
            st.outputs[*idx].1 = name;
        }
    }
}

impl Dispatch<tl_list::ExtForeignToplevelListV1, ()> for State {
    fn event(st: &mut Self, _: &tl_list::ExtForeignToplevelListV1, ev: tl_list::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let tl_list::Event::Toplevel { toplevel } = ev {
            st.toplevels.push(Toplevel { handle: Some(toplevel), ..Default::default() });
        }
    }
    wayland_client::event_created_child!(State, tl_list::ExtForeignToplevelListV1, [
        tl_list::EVT_TOPLEVEL_OPCODE => (tl_handle::ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<tl_handle::ExtForeignToplevelHandleV1, ()> for State {
    fn event(st: &mut Self, h: &tl_handle::ExtForeignToplevelHandleV1, ev: tl_handle::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some(t) = st.toplevels.iter_mut().find(|t| t.handle.as_ref() == Some(h)) else { return };
        match ev {
            tl_handle::Event::Title { title } => t.title = title,
            tl_handle::Event::AppId { app_id } => t.app_id = app_id,
            _ => {}
        }
    }
}

impl Dispatch<session::ExtImageCopyCaptureSessionV1, ()> for State {
    fn event(st: &mut Self, _: &session::ExtImageCopyCaptureSessionV1, ev: session::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            // Constraints arrive as a batch ending in `done`; a new batch
            // (after a resize) replaces the previous one.
            session::Event::BufferSize { width, height } => st.size = (width, height),
            session::Event::ShmFormat { format: WEnum::Value(f) } => {
                if st.constraints_done {
                    st.formats.clear();
                    st.constraints_done = false;
                }
                st.formats.push(f);
            }
            session::Event::Done => st.constraints_done = true,
            session::Event::Stopped => st.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<frame::ExtImageCopyCaptureFrameV1, ()> for State {
    fn event(st: &mut Self, _: &frame::ExtImageCopyCaptureFrameV1, ev: frame::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            frame::Event::Ready { .. } => st.ready = true,
            frame::Event::Failed { .. } => st.failed = true,
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(st: &mut Self, _: &wl_seat::WlSeat, ev: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = ev {
            st.has_pointer = c.contains(wl_seat::Capability::Pointer);
        }
    }
}

impl Dispatch<cursor_session::ExtImageCopyCaptureCursorSessionV1, ()> for State {
    fn event(st: &mut Self, _: &cursor_session::ExtImageCopyCaptureCursorSessionV1, ev: cursor_session::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some(c) = st.cursor.as_mut() else { return };
        if let cursor_session::Event::Position { x, y } = ev {
            c.push((x as f64 - c.origin.0, y as f64 - c.origin.1));
        }
    }
}

delegate_noop!(State: ignore wl_pointer::WlPointer);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: out_src_mgr::ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(State: tl_src_mgr::ExtForeignToplevelImageCaptureSourceManagerV1);
delegate_noop!(State: ext_image_capture_source_v1::ExtImageCaptureSourceV1);
delegate_noop!(State: cap_mgr::ExtImageCopyCaptureManagerV1);
