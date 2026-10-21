//! Detects which cursor shape is showing (arrow, I-beam, hand, ...) while
//! recording.
//!
//! Hyprland draws most cursors itself (cursor-shape-v1), so the cursor image
//! is not available to clients directly. Instead we use wlr-screencopy to
//! grab a small region around the pointer twice in the same frame, once with
//! the cursor composited and once without. Pixels that differ belong to the
//! cursor; the cursor-free copy is the background it was drawn over. Each
//! cursor of the active XCursor theme is composited over that background and
//! compared with what the compositor drew; the best match names the shape.
//! Unrecognised cursors are kept as images.

use super::hyprland;
use crate::util::monotonic_ns;
use crate::xcursor::{self, XImage};
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_wlr::screencopy::v1::client::{zwlr_screencopy_frame_v1 as frame, zwlr_screencopy_manager_v1 as manager};

/// Region captured around the pointer, in logical pixels.
const REGION: i32 = 64;
/// Offset of the pointer inside the region (cursors mostly extend down-right).
const LEAD: i32 = 24;
const PERIOD_MS: u64 = 50;

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Hidden,
    /// Canonical name from [`xcursor::NAMES`].
    Named(&'static str),
    /// Unrecognised cursor: straight RGBA, hotspot in its pixels, and the
    /// output scale it was captured at.
    Custom { width: u32, height: u32, rgba: Vec<u8>, hotspot: (f64, f64), scale: f64 },
}

#[derive(Clone, Debug)]
pub struct ShapeChange {
    pub t_ns: i64,
    pub shape: Shape,
}

pub fn spawn(stop: Arc<AtomicBool>, sink: Arc<Mutex<Vec<ShapeChange>>>) -> Result<()> {
    let mut links = [Link::connect()?, Link::connect()?];
    let monitors = hyprland::monitors()?;
    std::thread::spawn(move || {
        let mut classifier = Classifier::default();
        let mut last: Option<Shape> = None;
        // A new shape must be seen on two consecutive samples before it is
        // accepted (filters captures taken mid-motion); it is then dated to
        // its first sighting.
        let mut candidate: Option<(Shape, i64)> = None;
        let mut cache = (0u64, None);
        while !stop.load(Ordering::Relaxed) {
            let tick = std::time::Instant::now();
            let t = monotonic_ns();
            if let Some(found) = sample(&mut links, &monitors, &mut classifier, &mut cache) {
                if last.as_ref() == Some(&found) {
                    candidate = None;
                } else {
                    match &candidate {
                        Some((c, t0)) if *c == found => {
                            sink.lock().unwrap().push(ShapeChange { t_ns: *t0, shape: found.clone() });
                            last = Some(found);
                            candidate = None;
                        }
                        _ => candidate = Some((found, t)),
                    }
                }
            }
            let rest = std::time::Duration::from_millis(PERIOD_MS).saturating_sub(tick.elapsed());
            std::thread::sleep(rest);
        }
    });
    Ok(())
}

/// One sample: Some(shape) when it was (re)classified, None when unchanged or
/// the capture failed.
fn sample(
    links: &mut [Link; 2],
    monitors: &[hyprland::Monitor],
    classifier: &mut Classifier,
    last: &mut (u64, Option<Shape>),
) -> Option<Shape> {
    let (px, py) = hyprland::cursor_pos().ok()?;
    let mon = monitors.iter().find(|m| m.contains(px, py))?;
    let r = mon.logical_rect();
    let (lx, ly) = (px - r[0], py - r[1]);
    let rx = ((lx as i32) - LEAD).clamp(0, (r[2] as i32 - REGION).max(0));
    let ry = ((ly as i32) - LEAD).clamp(0, (r[3] as i32 - REGION).max(0));
    let (bare, mut with) = capture_pair(links, &mon.name, (rx, ry))?;
    if bare.w != with.w || bare.h != with.h {
        return None;
    }
    let scale = bare.w as f64 / REGION as f64;
    let hot = ((lx - rx as f64) * scale, (ly - ry as f64) * scale);
    // Ignore differences too far from the pointer to be part of the cursor
    // (cursors extend at most ~1.2x their nominal size from the hotspot).
    let reach = xcursor::base_size() as f64 * scale * 1.25;
    for y in 0..with.h {
        for x in 0..with.w {
            if (x as f64 - hot.0).abs() > reach || (y as f64 - hot.1).abs() > reach {
                let i = ((y * with.w + x) * 3) as usize;
                with.rgb[i..i + 3].copy_from_slice(&bare.rgb[i..i + 3]);
            }
        }
    }

    // Signature of the cursor pixels: skip classification when unchanged.
    let mut sig: u64 = 1469598103934665603;
    let mut any = false;
    for i in 0..(bare.w * bare.h) as usize {
        if differs(&bare.rgb[i * 3..i * 3 + 3], &with.rgb[i * 3..i * 3 + 3]) {
            any = true;
            for &b in &with.rgb[i * 3..i * 3 + 3] {
                sig = (sig ^ b as u64 ^ (i as u64) << 8).wrapping_mul(1099511628211);
            }
        }
    }
    if !any {
        *last = (0, Some(Shape::Hidden));
        return Some(Shape::Hidden);
    }
    // Same pixels as last time: reuse the previous classification.
    if sig == last.0 {
        return last.1.clone();
    }
    let size = (xcursor::base_size() as f64 * scale).round() as u32;
    let shape = classifier.classify(&bare, &with, hot, size, scale);
    *last = (sig, Some(shape.clone()));
    Some(shape)
}

fn differs(a: &[u8], b: &[u8]) -> bool {
    a.iter().zip(b).any(|(x, y)| x.abs_diff(*y) > 10)
}

pub struct Rgb {
    w: u32,
    h: u32,
    rgb: Vec<u8>,
}

#[derive(Default)]
struct Classifier {
    /// Candidates per nominal size: (canonical name, image).
    by_size: HashMap<u32, Vec<(&'static str, XImage)>>,
}

impl Classifier {
    fn candidates(&mut self, size: u32) -> &[(&'static str, XImage)] {
        self.by_size.entry(size).or_insert_with(|| {
            let theme = xcursor::theme_name();
            let mut out = vec![];
            let mut seen = std::collections::HashSet::new();
            for (canon, files) in xcursor::NAMES {
                for f in *files {
                    let Some(path) = xcursor::find(&theme, f) else { continue };
                    let real = std::fs::canonicalize(&path).unwrap_or(path.clone());
                    if !seen.insert(real) {
                        continue;
                    }
                    let imgs = xcursor::load(&path);
                    let Some(best) = imgs.iter().map(|i| i.size).min_by_key(|s| s.abs_diff(size)) else { continue };
                    // Animated cursors: a few frames are enough to recognise them.
                    for img in imgs.into_iter().filter(|i| i.size == best).take(6) {
                        out.push((*canon, img));
                    }
                }
            }
            out
        })
    }

    fn classify(&mut self, bare: &Rgb, with: &Rgb, hot: (f64, f64), size: u32, scale: f64) -> Shape {
        let (w, h) = (bare.w as i32, bare.h as i32);
        // Bounding box of the changed pixels.
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, -1, -1);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                if differs(&bare.rgb[i..i + 3], &with.rgb[i..i + 3]) {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
            }
        }
        let mut best: Option<(&'static str, f64)> = None;
        let debug = std::env::var_os("OPEN_STUDIO_DEBUG_CURSOR").is_some();
        let mut scores: Vec<(&'static str, f64)> = vec![];
        for (name, img) in self.candidates(size) {
            let mut own = f64::MAX;
            // Two placements: by the reported pointer position, and by
            // matching the cursor's visible box to the changed pixels (the
            // pointer may have moved between the position query and capture).
            let (bx, by) = opaque_min(img);
            let anchors = [
                (hot.0.round() as i32 - img.hotspot.0 as i32, hot.1.round() as i32 - img.hotspot.1 as i32),
                (x0 - bx, y0 - by),
            ];
            for (ax, ay) in anchors {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let ox = ax + dx;
                    let oy = ay + dy;
                    // Small bias towards the common shapes on near-ties.
                    let bias = match *name {
                        "default" | "text" | "pointer" => -0.75,
                        _ => 0.0,
                    };
                    let err = composite_error(bare, with, img, ox, oy, (x0, y0, x1, y1)) + bias;
                    own = own.min(err);
                    if best.is_none_or(|(_, e)| err < e) {
                        best = Some((name, err));
                    }
                }
            }
            }
            scores.push((name, own));
        }
        if debug {
            scores.sort_by(|a, b| a.1.total_cmp(&b.1));
            scores.dedup_by(|a, b| a.0 == b.0);
            eprintln!("changed=({x0},{y0})-({x1},{y1}) hot={hot:?} size={size} top: {:?}", &scores[..scores.len().min(4)]);
        }
        if let Some((name, err)) = best {
            if err < 9.0 {
                return Shape::Named(name);
            }
        }
        // Keep the drawn pixels as a custom cursor image.
        let (cw, ch) = ((x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32);
        let mut rgba = vec![0u8; (cw * ch * 4) as usize];
        for y in y0..=y1 {
            for x in x0..=x1 {
                let i = ((y * w + x) * 3) as usize;
                if differs(&bare.rgb[i..i + 3], &with.rgb[i..i + 3]) {
                    let o = (((y - y0) as u32 * cw + (x - x0) as u32) * 4) as usize;
                    rgba[o..o + 3].copy_from_slice(&with.rgb[i..i + 3]);
                    rgba[o + 3] = 255;
                }
            }
        }
        Shape::Custom { width: cw, height: ch, rgba, hotspot: (hot.0 - x0 as f64, hot.1 - y0 as f64), scale }
    }
}

/// Top-left of the visibly drawn part of a cursor image.
fn opaque_min(img: &XImage) -> (i32, i32) {
    let (mut mx, mut my) = (img.width as i32, img.height as i32);
    for y in 0..img.height {
        for x in 0..img.width {
            if img.rgba[((y * img.width + x) * 4 + 3) as usize] > 24 {
                mx = mx.min(x as i32);
                my = my.min(y as i32);
            }
        }
    }
    (mx, my)
}

/// Mean absolute error (0..255) between `with` and the candidate composited
/// over `bare`, over the union of the candidate's box and the changed area.
fn composite_error(bare: &Rgb, with: &Rgb, img: &XImage, ox: i32, oy: i32, changed: (i32, i32, i32, i32)) -> f64 {
    let (w, h) = (bare.w as i32, bare.h as i32);
    let bx0 = changed.0.min(ox).max(0);
    let by0 = changed.1.min(oy).max(0);
    let bx1 = changed.2.max(ox + img.width as i32 - 1).min(w - 1);
    let by1 = changed.3.max(oy + img.height as i32 - 1).min(h - 1);
    let mut err = 0u64;
    let mut n = 0u64;
    for y in by0..=by1 {
        for x in bx0..=bx1 {
            let i = ((y * w + x) * 3) as usize;
            let (ix, iy) = (x - ox, y - oy);
            let (prem, a) = if ix >= 0 && iy >= 0 && ix < img.width as i32 && iy < img.height as i32 {
                let o = ((iy as u32 * img.width + ix as u32) * 4) as usize;
                (&img.rgba[o..o + 3], img.rgba[o + 3] as u32)
            } else {
                (&[0u8, 0, 0][..], 0)
            };
            for c in 0..3 {
                let expected = prem[c] as u32 + bare.rgb[i + c] as u32 * (255 - a) / 255;
                err += (expected.min(255) as i32 - with.rgb[i + c] as i32).unsigned_abs() as u64;
            }
            n += 3;
        }
    }
    if n == 0 { f64::MAX } else { err as f64 / n as f64 }
}

// ---- Wayland plumbing ----------------------------------------------------

#[derive(Default)]
struct Pending {
    format: Option<(wl_shm::Format, u32, u32, u32)>,
    formats: Vec<(wl_shm::Format, u32, u32, u32)>,
    buffer_done: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct State {
    shm: Option<wl_shm::WlShm>,
    manager: Option<manager::ZwlrScreencopyManagerV1>,
    outputs: Vec<(wl_output::WlOutput, String)>,
    pending: Pending,
    pool: Option<ShmBuf>,
    frame: Option<frame::ZwlrScreencopyFrameV1>,
    fmt: Option<(wl_shm::Format, u32, u32, u32)>,
}

/// One Wayland connection. Two are used so the compositor treats the
/// cursor-free and cursor-included captures as separate sessions.
struct Link {
    queue: EventQueue<State>,
    st: State,
    _conn: Connection,
}

impl Link {
    fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env().context("connecting to Wayland")?;
        let mut queue: EventQueue<State> = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut st = State::default();
        queue.roundtrip(&mut st)?;
        queue.roundtrip(&mut st)?;
        if st.manager.is_none() || st.shm.is_none() {
            return Err(anyhow!("compositor lacks wlr-screencopy"));
        }
        Ok(Self { queue, st, _conn: conn })
    }

    fn request(&mut self, output_name: &str, overlay: bool, at: (i32, i32)) -> Option<()> {
        let qh = self.queue.handle();
        let output = self.st.outputs.iter().find(|(_, n)| n == output_name)?.0.clone();
        self.st.pending = Pending::default();
        self.st.frame = Some(self.st.manager.as_ref()?.capture_output_region(overlay as i32, &output, at.0, at.1, REGION, REGION, &qh, ()));
        while !self.st.pending.buffer_done && !self.st.pending.failed {
            self.queue.blocking_dispatch(&mut self.st).ok()?;
        }
        let fmt = self.st.pending.format.or_else(|| pick_format(&self.st.pending.formats))?;
        if self.st.pool.as_ref().is_none_or(|p| p.key != fmt) {
            self.st.pool = Some(make_shm(self.st.shm.as_ref()?, fmt, &qh).ok()?);
        }
        self.st.fmt = Some(fmt);
        Some(())
    }

    fn copy(&mut self) -> Option<()> {
        self.st.frame.as_ref()?.copy(&self.st.pool.as_ref()?.buffer);
        self.queue.flush().ok()
    }

    fn finish(&mut self) -> Option<Rgb> {
        while !self.st.pending.ready && !self.st.pending.failed {
            self.queue.blocking_dispatch(&mut self.st).ok()?;
        }
        if let Some(f) = self.st.frame.take() {
            f.destroy();
        }
        if self.st.pending.failed {
            return None;
        }
        let (format, w, h, stride) = self.st.fmt?;
        let buf = self.st.pool.as_ref()?;
        let data = unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) };
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            let sy = if self.st.pending.y_invert { h - 1 - y } else { y };
            for x in 0..w {
                let o = (sy * stride + x * 4) as usize;
                let v = u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
                let d = ((y * w + x) * 3) as usize;
                rgb[d..d + 3].copy_from_slice(&decode(format, v));
            }
        }
        Some(Rgb { w, h, rgb })
    }
}

/// Captures the region without and with the cursor in the same compositor
/// frame: both copies are requested before either is waited on.
fn capture_pair(links: &mut [Link; 2], output: &str, at: (i32, i32)) -> Option<(Rgb, Rgb)> {
    let [a, b] = links;
    a.request(output, false, at)?;
    b.request(output, true, at)?;
    a.copy()?;
    b.copy()?;
    Some((a.finish()?, b.finish()?))
}

struct ShmBuf {
    ptr: *mut u8,
    len: usize,
    _fd: OwnedFd,
    buffer: wl_buffer::WlBuffer,
    key: (wl_shm::Format, u32, u32, u32),
}

unsafe impl Send for ShmBuf {}

impl Drop for ShmBuf {
    fn drop(&mut self) {
        self.buffer.destroy();
        unsafe { libc::munmap(self.ptr as *mut _, self.len) };
    }
}

fn pick_format(formats: &[(wl_shm::Format, u32, u32, u32)]) -> Option<(wl_shm::Format, u32, u32, u32)> {
    use wl_shm::Format as F;
    let order = [F::Xrgb8888, F::Argb8888, F::Xbgr8888, F::Abgr8888, F::Xrgb2101010, F::Argb2101010, F::Xbgr2101010, F::Abgr2101010];
    order.iter().find_map(|f| formats.iter().find(|x| x.0 == *f).copied())
}

fn decode(format: wl_shm::Format, v: u32) -> [u8; 3] {
    use wl_shm::Format as F;
    let ten = |c: u32| ((c & 0x3ff) * 255 / 1023) as u8;
    match format {
        F::Xrgb8888 | F::Argb8888 => [(v >> 16) as u8, (v >> 8) as u8, v as u8],
        F::Xbgr8888 | F::Abgr8888 => [v as u8, (v >> 8) as u8, (v >> 16) as u8],
        F::Xrgb2101010 | F::Argb2101010 => [ten(v >> 20), ten(v >> 10), ten(v)],
        _ => [ten(v), ten(v >> 10), ten(v >> 20)],
    }
}

fn make_shm(shm: &wl_shm::WlShm, key: (wl_shm::Format, u32, u32, u32), qh: &QueueHandle<State>) -> Result<ShmBuf> {
    let (format, w, h, stride) = key;
    let len = (stride * h) as usize;
    let fd = unsafe { libc::memfd_create(c"open-studio-cursor".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(anyhow!("memfd_create failed"));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if unsafe { libc::ftruncate(fd.as_raw_fd(), len as i64) } != 0 {
        return Err(anyhow!("ftruncate failed"));
    }
    let ptr = unsafe {
        libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0)
    };
    if ptr == libc::MAP_FAILED {
        return Err(anyhow!("mmap failed"));
    }
    let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
    let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, qh, ());
    pool.destroy();
    Ok(ShmBuf { ptr: ptr as *mut u8, len, _fd: fd, buffer, key })
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(st: &mut Self, reg: &wl_registry::WlRegistry, ev: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = ev {
            match interface.as_str() {
                "wl_shm" => st.shm = Some(reg.bind(name, 1, qh, ())),
                "zwlr_screencopy_manager_v1" => st.manager = Some(reg.bind(name, version.min(3), qh, ())),
                "wl_output" => {
                    let idx = st.outputs.len();
                    let out = reg.bind(name, version.min(4), qh, idx);
                    st.outputs.push((out, String::new()));
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

impl Dispatch<frame::ZwlrScreencopyFrameV1, ()> for State {
    fn event(st: &mut Self, _: &frame::ZwlrScreencopyFrameV1, ev: frame::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let p = &mut st.pending;
        match ev {
            frame::Event::Buffer { format: WEnum::Value(f), width, height, stride } => p.formats.push((f, width, height, stride)),
            frame::Event::BufferDone => {
                p.format = pick_format(&p.formats);
                p.buffer_done = true;
            }
            frame::Event::Flags { flags: WEnum::Value(fl) } => p.y_invert = fl.contains(frame::Flags::YInvert),
            frame::Event::Ready { .. } => p.ready = true,
            frame::Event::Failed => p.failed = true,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: manager::ZwlrScreencopyManagerV1);
