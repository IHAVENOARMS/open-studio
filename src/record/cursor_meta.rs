//! Cursor position and image from ScreenCast portal streams opened with the
//! "metadata" cursor mode (GNOME, KDE, xdg-desktop-portal-wlr). GStreamer's
//! pipewiresrc drops this metadata, so a second PipeWire stream on the same
//! node reads only the cursor metadata of each buffer and ignores the pixels.

use super::cursor_shape::{self, Shape, ShapeChange};
use anyhow::{Result, anyhow};
use pipewire as pw;
use pw::spa;
use spa::pod::{ChoiceValue, Object, Pod, Property, PropertyFlags, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle};
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Largest cursor image asked for, in pixels per side.
const MAX_CURSOR: u32 = 512;

#[derive(Debug, PartialEq)]
pub struct CursorMeta {
    /// Hotspot position in stream pixels.
    pub position: (i32, i32),
    pub hotspot: (i32, i32),
    /// A new cursor image (premultiplied BGRA, width, height), sent when it
    /// changes.
    pub bitmap: Option<(Vec<u8>, u32, u32)>,
}

/// Reads a `spa_meta_cursor` block (with its bitmap, if any). None when no
/// cursor is over the stream.
pub fn parse(meta: &[u8]) -> Option<CursorMeta> {
    let u32_at = |o: usize| meta.get(o..o + 4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
    let i32_at = |o: usize| u32_at(o).map(|v| v as i32);
    // struct spa_meta_cursor { id, flags, position {x, y}, hotspot {x, y}, bitmap_offset }
    let id = u32_at(0)?;
    if id == 0 {
        return None;
    }
    let position = (i32_at(8)?, i32_at(12)?);
    let hotspot = (i32_at(16)?, i32_at(20)?);
    let bitmap_offset = u32_at(24)? as usize;
    let bitmap = (bitmap_offset >= 28).then(|| {
        // struct spa_meta_bitmap { format, size {width, height}, stride, offset }
        let b = bitmap_offset;
        let format = u32_at(b)?;
        let (w, h) = (u32_at(b + 4)?, u32_at(b + 8)?);
        let stride = i32_at(b + 12)?.max(0) as usize;
        let offset = u32_at(b + 16)? as usize;
        if w == 0 || h == 0 || w > MAX_CURSOR || h > MAX_CURSOR || stride < w as usize * 4 {
            return None;
        }
        // Byte positions of B, G, R, A in each pixel.
        let order = match format {
            spa::sys::SPA_VIDEO_FORMAT_BGRA => [0, 1, 2, 3],
            spa::sys::SPA_VIDEO_FORMAT_RGBA => [2, 1, 0, 3],
            spa::sys::SPA_VIDEO_FORMAT_ARGB => [3, 2, 1, 0],
            spa::sys::SPA_VIDEO_FORMAT_ABGR => [1, 2, 3, 0],
            _ => return None,
        };
        let mut bgra = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let row = meta.get(b + offset + y * stride..b + offset + y * stride + w as usize * 4)?;
            for p in row.chunks(4) {
                bgra.extend(order.map(|i| p[i]));
            }
        }
        Some((bgra, w, h))
    });
    Some(CursorMeta { position, hotspot, bitmap: bitmap.flatten() })
}

/// Reads the cursor metadata of portal node `node_id` (through the portal's
/// PipeWire `fd`) on its own thread until `stop`. Positions are stream
/// pixels; `scale` is pixels per logical unit, for naming the cursor image.
pub fn spawn(
    fd: OwnedFd,
    node_id: u32,
    scale: f64,
    stop: Arc<AtomicBool>,
    moves: super::direct::Moves,
    shapes: Option<Arc<Mutex<Vec<ShapeChange>>>>,
) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Err(e) = run(Some(fd), node_id, scale, stop, moves, shapes, &tx) {
            let _ = tx.send(Err(e));
        }
    });
    rx.recv_timeout(std::time::Duration::from_secs(3)).map_err(|_| anyhow!("PipeWire cursor stream didn't start"))?
}

struct Data {
    moves: super::direct::Moves,
    shapes: Option<Arc<Mutex<Vec<ShapeChange>>>>,
    classifier: cursor_shape::Classifier,
    scale: f64,
    last: Option<(i32, i32)>,
    last_push: i64,
    shape: Option<Shape>,
}

fn run(
    fd: Option<OwnedFd>,
    node_id: u32,
    scale: f64,
    stop: Arc<AtomicBool>,
    moves: super::direct::Moves,
    shapes: Option<Arc<Mutex<Vec<ShapeChange>>>>,
    started: &std::sync::mpsc::Sender<Result<()>>,
) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = match fd { Some(fd) => context.connect_fd_rc(fd, None)?, None => context.connect_rc(None)? };
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Screen",
    };
    let stream = pw::stream::StreamRc::new(core, "open-studio-cursor", props)?;
    let data = Data { moves, shapes, classifier: Default::default(), scale, last: None, last_push: 0, shape: None };
    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|stream, _, id, param| {
            if id != spa::param::ParamType::Format.as_raw() || param.is_none() {
                return;
            }
            // Ask for cursor metadata with room for an image of any size up
            // to MAX_CURSOR (the producer picks within the range).
            let size = |side: u32| {
                (std::mem::size_of::<spa::sys::spa_meta_cursor>() + std::mem::size_of::<spa::sys::spa_meta_bitmap>() + (side * side * 4) as usize) as i32
            };
            let range = ChoiceEnum::Range { default: size(64), min: size(1), max: size(MAX_CURSOR) };
            let meta = object(spa::sys::SPA_TYPE_OBJECT_ParamMeta, spa::sys::SPA_PARAM_Meta, vec![
                prop(spa::sys::SPA_PARAM_META_type, Value::Id(Id(spa::sys::SPA_META_Cursor))),
                prop(spa::sys::SPA_PARAM_META_size, Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), range)))),
            ]);
            let _ = stream.update_params(&mut [Pod::from_bytes(&meta).unwrap()]);
        })
        .process(|stream, d| {
            let raw = stream.as_raw_ptr();
            let buf = unsafe { pw::sys::pw_stream_dequeue_buffer(raw) };
            if buf.is_null() {
                return;
            }
            unsafe {
                let sb = (*buf).buffer;
                let m = spa::sys::spa_buffer_find_meta(sb, spa::sys::SPA_META_Cursor);
                if !m.is_null() && !(*m).data.is_null() {
                    let bytes = std::slice::from_raw_parts((*m).data as *const u8, (*m).size as usize);
                    handle(d, parse(bytes));
                }
                pw::sys::pw_stream_queue_buffer(raw, buf);
            }
        })
        .register()?;

    let format = video_formats();
    stream.connect(
        spa::utils::Direction::Input,
        Some(node_id),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut [Pod::from_bytes(&format).unwrap()],
    )?;
    let _ = started.send(Ok(()));
    let ml = mainloop.clone();
    let _timer = mainloop.loop_().add_timer(move |_| {
        if stop.load(Ordering::Relaxed) {
            ml.quit();
        }
    });
    _timer.update_timer(Some(std::time::Duration::from_millis(50)), Some(std::time::Duration::from_millis(50))).into_result()?;
    mainloop.run();
    let _ = stream.disconnect();
    Ok(())
}

fn handle(d: &mut Data, meta: Option<CursorMeta>) {
    let t = crate::util::monotonic_ns();
    let Some(meta) = meta else { return };
    // A heartbeat so idle time is represented.
    if d.last != Some(meta.position) || t - d.last_push > 500_000_000 {
        d.moves.lock().unwrap().push((t, meta.position.0 as f64, meta.position.1 as f64));
        d.last = Some(meta.position);
        d.last_push = t;
    }
    if let (Some(sink), Some((bgra, w, h))) = (&d.shapes, &meta.bitmap) {
        let hot = (meta.hotspot.0 as f64, meta.hotspot.1 as f64);
        let shape = cursor_shape::classify_image(&mut d.classifier, bgra, *w, *h, hot, d.scale);
        if d.shape.as_ref() != Some(&shape) {
            sink.lock().unwrap().push(ShapeChange { t_ns: t, shape: shape.clone() });
            d.shape = Some(shape);
        }
    }
}

fn prop(key: u32, value: Value) -> Property {
    Property { key, flags: PropertyFlags::empty(), value }
}

fn object(type_: u32, id: u32, properties: Vec<Property>) -> Vec<u8> {
    let obj = Value::Object(Object { type_, id, properties });
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &obj).unwrap().0.into_inner()
}

/// Raw video in any 32-bit RGB layout, size and rate: whatever the portal
/// already negotiated with the video consumer.
fn video_formats() -> Vec<u8> {
    use spa::sys::*;
    let formats = [SPA_VIDEO_FORMAT_BGRx, SPA_VIDEO_FORMAT_BGRA, SPA_VIDEO_FORMAT_RGBx, SPA_VIDEO_FORMAT_RGBA, SPA_VIDEO_FORMAT_xRGB, SPA_VIDEO_FORMAT_ARGB, SPA_VIDEO_FORMAT_xBGR, SPA_VIDEO_FORMAT_ABGR];
    let choice = |e| Value::Choice(e);
    object(SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat, vec![
        prop(SPA_FORMAT_mediaType, Value::Id(Id(SPA_MEDIA_TYPE_video))),
        prop(SPA_FORMAT_mediaSubtype, Value::Id(Id(SPA_MEDIA_SUBTYPE_raw))),
        prop(SPA_FORMAT_VIDEO_format, choice(ChoiceValue::Id(Choice(ChoiceFlags::empty(), ChoiceEnum::Enum {
            default: Id(formats[0]),
            alternatives: formats.iter().map(|f| Id(*f)).collect(),
        })))),
        prop(SPA_FORMAT_VIDEO_size, choice(ChoiceValue::Rectangle(Choice(ChoiceFlags::empty(), ChoiceEnum::Range {
            default: Rectangle { width: 1920, height: 1080 },
            min: Rectangle { width: 1, height: 1 },
            max: Rectangle { width: 16384, height: 16384 },
        })))),
        prop(SPA_FORMAT_VIDEO_framerate, choice(ChoiceValue::Fraction(Choice(ChoiceFlags::empty(), ChoiceEnum::Range {
            default: Fraction { num: 60, denom: 1 },
            min: Fraction { num: 0, denom: 1 },
            max: Fraction { num: 1000, denom: 1 },
        })))),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A portal-like producer (an output stream that drives the graph and
    /// fills cursor metadata, as xdg-desktop-portal-wlr does) with GStreamer's
    /// pipewiresrc taking the video, as recording does: the second stream
    /// still gets the cursor. Opt-in, needs a PipeWire daemon:
    /// `cargo test -- --ignored cursor_alongside_pipewiresrc`.
    #[test]
    #[ignore]
    fn cursor_alongside_pipewiresrc() {
        let name = format!("open-studio-test-{}", std::process::id());
        let (id_tx, id_rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let n2 = name.clone();
        std::thread::spawn(move || produce(&n2, st, id_tx));
        let node = id_rx.recv_timeout(std::time::Duration::from_secs(5)).expect("producer didn't start");
        // The video consumer links first, as when recording.
        let mut video = std::process::Command::new("gst-launch-1.0")
            .args(["-q", "pipewiresrc", &format!("target-object={name}"), "!", "fakesink"])
            .spawn()
            .expect("gst-launch-1.0");
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let moves: super::super::direct::Moves = Arc::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        let (st, m) = (stop.clone(), moves.clone());
        let side = std::thread::spawn(move || run(None, node, 1.0, st, m, None, &tx).map_err(|e| e.to_string()));
        std::thread::sleep(std::time::Duration::from_secs(2));
        let video_alive = video.try_wait().unwrap().is_none();
        stop.store(true, Ordering::Relaxed);
        let _ = video.kill();
        side.join().unwrap().unwrap();
        assert!(video_alive, "pipewiresrc stopped");
        let m = moves.lock().unwrap();
        assert!(m.len() > 10, "cursor positions: {}", m.len());
        assert!(m.iter().any(|p| p.1 > 0.0 && p.2 == 7.0));
    }

    /// Output stream at 30 fps whose buffers carry a moving cursor.
    fn produce(name: &str, stop: Arc<AtomicBool>, id_tx: std::sync::mpsc::Sender<u32>) {
        use spa::sys::*;
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None).unwrap();
        let context = pw::context::ContextRc::new(&mainloop, None).unwrap();
        let core = context.connect_rc(None).unwrap();
        let props = pw::properties::properties! {
            *pw::keys::MEDIA_CLASS => "Video/Source",
            *pw::keys::NODE_NAME => name,
        };
        let stream = pw::stream::StreamRc::new(core, name, props).unwrap();
        let frame = std::cell::Cell::new(0i32);
        let _l = stream
            .add_local_listener_with_user_data(())
            .param_changed(|stream, _, id, param| {
                if id != SPA_PARAM_Format || param.is_none() {
                    return;
                }
                let buffers = object(SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers, vec![
                    prop(SPA_PARAM_BUFFERS_buffers, Value::Int(4)),
                    prop(SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
                    prop(SPA_PARAM_BUFFERS_size, Value::Int(64 * 64 * 4)),
                    prop(SPA_PARAM_BUFFERS_stride, Value::Int(64 * 4)),
                ]);
                let header = object(SPA_TYPE_OBJECT_ParamMeta, SPA_PARAM_Meta, vec![
                    prop(SPA_PARAM_META_type, Value::Id(Id(SPA_META_Header))),
                    prop(SPA_PARAM_META_size, Value::Int(std::mem::size_of::<spa_meta_header>() as i32)),
                ]);
                let size = (std::mem::size_of::<spa_meta_cursor>() + std::mem::size_of::<spa_meta_bitmap>() + 64 * 64 * 4) as i32;
                let cursor = object(SPA_TYPE_OBJECT_ParamMeta, SPA_PARAM_Meta, vec![
                    prop(SPA_PARAM_META_type, Value::Id(Id(SPA_META_Cursor))),
                    prop(SPA_PARAM_META_size, Value::Int(size)),
                ]);
                let _ = stream.update_params(&mut [Pod::from_bytes(&buffers).unwrap(), Pod::from_bytes(&header).unwrap(), Pod::from_bytes(&cursor).unwrap()]);
            })
            .process(move |stream, _| unsafe {
                let raw = stream.as_raw_ptr();
                let buf = pw::sys::pw_stream_dequeue_buffer(raw);
                if buf.is_null() {
                    return;
                }
                let sb = (*buf).buffer;
                let d = &mut *(*sb).datas;
                if !d.chunk.is_null() {
                    (*d.chunk).offset = 0;
                    (*d.chunk).size = 64 * 64 * 4;
                    (*d.chunk).stride = 64 * 4;
                }
                let m = spa_buffer_find_meta_data(sb, SPA_META_Cursor, std::mem::size_of::<spa_meta_cursor>()) as *mut spa_meta_cursor;
                if !m.is_null() {
                    let f = frame.get() + 1;
                    frame.set(f);
                    *m = spa_meta_cursor { id: 1, flags: 0, position: spa_point { x: f, y: 7 }, hotspot: spa_point { x: 0, y: 0 }, bitmap_offset: 0 };
                }
                pw::sys::pw_stream_queue_buffer(raw, buf);
            })
            .register()
            .unwrap();
        let format = object(SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat, vec![
            prop(SPA_FORMAT_mediaType, Value::Id(Id(SPA_MEDIA_TYPE_video))),
            prop(SPA_FORMAT_mediaSubtype, Value::Id(Id(SPA_MEDIA_SUBTYPE_raw))),
            prop(SPA_FORMAT_VIDEO_format, Value::Id(Id(SPA_VIDEO_FORMAT_BGRx))),
            prop(SPA_FORMAT_VIDEO_size, Value::Rectangle(Rectangle { width: 64, height: 64 })),
            prop(SPA_FORMAT_VIDEO_framerate, Value::Fraction(Fraction { num: 30, denom: 1 })),
        ]);
        stream
            .connect(
                spa::utils::Direction::Output,
                None,
                pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::ALLOC_BUFFERS | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut [Pod::from_bytes(&format).unwrap()],
            )
            .unwrap();
        let s2 = stream.clone();
        let ml = mainloop.clone();
        let sent = std::cell::Cell::new(false);
        let timer = mainloop.loop_().add_timer(move |_| {
            if stop.load(Ordering::Relaxed) {
                ml.quit();
            }
            if !sent.get() && s2.node_id() != u32::MAX {
                let _ = id_tx.send(s2.node_id());
                sent.set(true);
            }
            unsafe { pw::sys::pw_stream_trigger_process(s2.as_raw_ptr()) };
        });
        timer.update_timer(Some(std::time::Duration::from_millis(33)), Some(std::time::Duration::from_millis(33))).into_result().unwrap();
        mainloop.run();
    }

    fn meta(id: u32, pos: (i32, i32), hot: (i32, i32), bitmap: Option<(u32, u32, u32, &[u8])>) -> Vec<u8> {
        let mut v = Vec::new();
        let mut put = |x: u32| v.extend(x.to_ne_bytes());
        put(id);
        put(0);
        put(pos.0 as u32);
        put(pos.1 as u32);
        put(hot.0 as u32);
        put(hot.1 as u32);
        put(if bitmap.is_some() { 28 } else { 0 });
        if let Some((format, w, h, px)) = bitmap {
            for x in [format, w, h, w * 4, 20] {
                v.extend(x.to_ne_bytes());
            }
            v.extend(px);
        }
        v
    }

    #[test]
    fn cursor_metadata_is_parsed() {
        assert_eq!(parse(&meta(0, (5, 5), (0, 0), None)), None, "id 0: no cursor over the stream");
        let m = parse(&meta(3, (120, -4), (2, 1), None)).unwrap();
        assert_eq!((m.position, m.hotspot, m.bitmap), ((120, -4), (2, 1), None));
        // One RGBA pixel and one more, reordered to BGRA.
        let px = [10, 20, 30, 255, 1, 2, 3, 4];
        let m = parse(&meta(3, (0, 0), (0, 0), Some((spa::sys::SPA_VIDEO_FORMAT_RGBA, 2, 1, &px)))).unwrap();
        assert_eq!(m.bitmap, Some((vec![30, 20, 10, 255, 3, 2, 1, 4], 2, 1)));
        // Truncated bitmap data is ignored, the position is still used.
        let m = parse(&meta(3, (7, 8), (0, 0), Some((spa::sys::SPA_VIDEO_FORMAT_RGBA, 4, 4, &px)))).unwrap();
        assert_eq!((m.position, m.bitmap), ((7, 8), None));
    }
}
