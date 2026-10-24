//! Offline render of the edited timeline to an MP4.

use crate::decode::Decoder;
use crate::edit::EditConfig;
use crate::engine::{Tracks, frame_state};
use crate::project::{ProjectPaths, Recording};
use crate::render::{Compositor, FORMAT};
use anyhow::{Context, Result, anyhow, bail};
use egui_wgpu::wgpu;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub out: PathBuf,
    /// Output height in pixels; width follows the canvas aspect.
    pub height: u32,
    pub fps: u32,
    pub format: crate::settings::Format,
    pub quality: crate::settings::Quality,
    /// ProRes flavour, for `Format::ProRes`.
    pub prores: crate::settings::ProRes,
}

#[derive(Default)]
pub struct Progress {
    /// 0..=1000
    pub permille: AtomicU32,
    pub cancel: AtomicBool,
}

pub fn output_size(aspect: f64, height: u32) -> (u32, u32) {
    let h = height & !1;
    let w = ((h as f64 * aspect).round() as u32 + 1) & !1;
    (w, h)
}

pub fn export(
    comp: &mut Compositor,
    dir: &Path,
    rec: &Recording,
    edit: &EditConfig,
    opts: &ExportOptions,
    progress: &Progress,
) -> Result<()> {
    let paths = ProjectPaths::new(dir);
    crate::privacy::ensure(rec, &edit.privacy)?;
    let began = std::time::Instant::now();
    let tracks = Tracks::build(rec, edit);
    let (w, h) = output_size(tracks.layout.aspect, opts.height);
    let (start, end) = edit.trim_range(rec.duration);
    // Recording time of every output frame, skipping cuts.
    let mut times = vec![];
    let mut acc = 0.0;
    for (a, b, speed) in edit.segments(rec.duration) {
        let len = (b - a) / speed;
        let n0 = (acc * opts.fps as f64).ceil() as u64;
        let n1 = ((acc + len) * opts.fps as f64).ceil() as u64;
        for n in n0..n1 {
            let out_t = n as f64 / opts.fps as f64;
            times.push(a + (out_t - acc) * speed);
        }
        acc += len;
    }
    if times.is_empty() {
        bail!("nothing to export: everything is trimmed or cut");
    }
    let total = times.len() as u64;

    let target = Target::new(comp, w, h);

    // GIFs are encoded from an intermediate MP4 with a generated palette.
    let gif = opts.format == crate::settings::Format::Gif;
    let video_out = if gif { opts.out.with_extension("tmp.mp4") } else { opts.out.clone() };
    let (crf, preset) = if gif { (12, "veryfast") } else { opts.quality.x264() };
    let prores = opts.format == crate::settings::Format::ProRes;
    // GIFs go through a throwaway intermediate; keep that on x264. ProRes
    // is encoded on the CPU.
    let gpu = if gif || prores || !crate::settings::Settings::load().gpu_encoding { None } else { crate::hwenc::detect() };
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    let input = ["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r", &opts.fps.to_string(), "-i", "-"].map(String::from);
    if prores {
        let (profile, pix) = opts.prores.ffmpeg();
        cmd.args(input)
            .args(["-vf", "scale=out_color_matrix=bt709:out_range=tv"])
            .args(["-c:v", "prores_ks", "-profile:v", &profile.to_string(), "-vendor", "apl0", "-pix_fmt", pix])
            .args(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"]);
    } else if let Some(gpu) = gpu {
        cmd.args(gpu.input_args()).args(input).args(gpu.output_args(opts.quality.gpu_qp(), opts.fps * 2));
    } else {
        cmd.args(input)
            .args(["-vf", "scale=out_color_matrix=bt709:out_range=tv"])
            .args(["-c:v", "libx264", "-preset", preset, "-crf", &crf.to_string(), "-pix_fmt", "yuv420p"])
            .args(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"]);
    }
    let mut ffmpeg = cmd
        .args(["-movflags", "+faststart"])
        .arg(&video_out)
        .stdin(Stdio::piped())
        .spawn()
        .context("launching ffmpeg")?;
    let mut stdin = ffmpeg.stdin.take().unwrap();

    let mut decoder = Decoder::open(&paths.video(), start, (rec.width, rec.height), None)?;
    let first_src = (start * rec.fps as f64).round() as i64;
    let mut src_index = first_src - 1; // index of the frame currently in `frame`
    let mut frame = Vec::new();
    let mut row = vec![0u8; (w * 4 * h) as usize];
    // Camera track, stepped alongside: camera time = video time - offset - delay.
    let cam_time = |t: f64, c: &crate::project::CameraTrack| (t - c.offset - edit.camera.delay).max(0.0);
    let mut camera = match &rec.camera {
        Some(c) => {
            let t0 = cam_time(times[0], c);
            let d = Decoder::open(&dir.join(&c.file), t0, (c.width, c.height), Some(1920))?;
            Some((d, (t0 * c.fps as f64).round() as i64 - 1, Vec::new(), c.clone()))
        }
        None => None,
    };
    // Background-removal matte, stepped with the camera.
    let mut matte = match rec.camera.as_ref().and_then(|c| c.matte.as_ref().map(|m| (c, m))) {
        Some((c, m)) => {
            let t0 = cam_time(times[0], c);
            let d = Decoder::open(&dir.join(m), t0, crate::cutout::matte_size(c.width, c.height), None)?;
            Some((d, (t0 * c.fps as f64).round() as i64 - 1, Vec::new(), c.clone()))
        }
        None => None,
    };

    for (i, &t) in times.iter().enumerate() {
        let i = i as u64;
        if progress.cancel.load(Ordering::Relaxed) {
            drop(stdin);
            let _ = ffmpeg.kill();
            let _ = std::fs::remove_file(&video_out);
            bail!("export cancelled");
        }
        let want = ((t * rec.fps as f64) + 1e-6).floor() as i64;
        let mut fresh = false;
        while src_index < want {
            if !decoder.read(&mut frame)? {
                break;
            }
            src_index += 1;
            fresh = true;
        }
        if fresh {
            comp.upload_frame(&frame, decoder.width, decoder.height);
        }
        if !comp.has_frame() {
            bail!("no video frames decoded");
        }
        if let Some((dec, index, buf, track)) = &mut camera {
            let want = (cam_time(t, track) * track.fps as f64 + 1e-6).floor() as i64;
            let mut fresh = false;
            while *index < want {
                if !dec.read(buf)? {
                    break;
                }
                *index += 1;
                fresh = true;
            }
            if fresh {
                comp.upload_camera_frame(buf, dec.width, dec.height);
            }
        }
        if let Some((dec, index, buf, track)) = &mut matte {
            let want = (cam_time(t, track) * track.fps as f64 + 1e-6).floor() as i64;
            let mut fresh = false;
            while *index < want {
                if !dec.read(buf)? {
                    break;
                }
                *index += 1;
                fresh = true;
            }
            if fresh {
                comp.upload_matte_frame(buf, dec.width, dec.height);
            }
        }
        let fs = frame_state(rec, edit, &tracks, t, opts.fps as f64);
        comp.render(&target.view, (w, h), &tracks.layout, edit, &fs);

        target.read(comp, &mut row)?;
        stdin.write_all(&row).context("writing to ffmpeg")?;
        progress.permille.store(((i + 1) * 1000 / total) as u32, Ordering::Relaxed);
    }
    drop(stdin);
    let status = ffmpeg.wait()?;
    if !status.success() {
        bail!("ffmpeg exited with {status}");
    }
    if !gif && !edit.audio.mute {
        let clicks = crate::clicks::render_track(rec, edit);
        let tracks = audio_tracks(rec, edit, clicks.as_deref());
        if !tracks.is_empty() {
            mux_audio(dir, &tracks, &video_out, start, end, &edit.segments(rec.duration), &edit.slices)?;
        }
    }
    if gif {
        let filter = "split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle";
        let out = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&video_out)
            .args(["-filter_complex", filter, "-loop", "0"])
            .arg(&opts.out)
            .output()
            .context("converting to GIF")?;
        let _ = std::fs::remove_file(&video_out);
        if !out.status.success() {
            bail!("GIF conversion failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
    }
    // Remember this machine's pace for the next estimate (GIFs and ProRes
    // differ, so only plain MP4s set it).
    if opts.format == crate::settings::Format::Mp4 {
        let mpx = w as f64 * h as f64 * total as f64 / 1e6;
        let secs = began.elapsed().as_secs_f64();
        if secs > 0.5 {
            let _ = std::fs::create_dir_all(crate::util::config_dir());
            let _ = std::fs::write(speed_file(), format!("{}", mpx / secs));
        }
    }
    Ok(())
}

fn speed_file() -> PathBuf {
    crate::util::config_dir().join("export-speed")
}

/// Output megapixels per second the last MP4 export managed here, if known.
pub fn measured_speed() -> Option<f64> {
    std::fs::read_to_string(speed_file()).ok()?.trim().parse().ok().filter(|v: &f64| *v > 0.0)
}

/// Estimated export time (seconds) and largest likely file size (bytes)
/// for `secs` of output at `size` and `fps`, as Screen Studio shows under
/// its export buttons. `speed`: output megapixels per second (measured, or
/// a cautious guess).
pub fn estimate(
    format: crate::settings::Format,
    quality: crate::settings::Quality,
    prores: crate::settings::ProRes,
    size: (u32, u32),
    fps: u32,
    secs: f64,
    speed: Option<f64>,
    audio: bool,
) -> (f64, f64) {
    use crate::settings::{Format, Quality};
    let px = size.0 as f64 * size.1 as f64;
    let frames = secs * fps as f64;
    let speed = speed.unwrap_or(40.0);
    let mut time = px * frames / 1e6 / speed + 1.0;
    let video_bits = match format {
        // Screen recordings compress well; these are generous ceilings.
        Format::Mp4 => {
            let bpp = match quality {
                Quality::Studio => 0.15,
                Quality::Social => 0.08,
                Quality::Web => 0.045,
                Quality::WebLow => 0.025,
            };
            bpp * px * frames
        }
        Format::ProRes => {
            time *= 1.3;
            prores.mbps_1080p30() * 1e6 * (px * fps as f64) / (1920.0 * 1080.0 * 29.97) * secs
        }
        Format::Gif => {
            time *= 2.5;
            0.25 * 8.0 * px * frames
        }
    };
    let audio_bits = match (audio, format) {
        (false, _) | (_, Format::Gif) => 0.0,
        (_, Format::ProRes) => 48_000.0 * 2.0 * 16.0 * secs,
        _ => 192_000.0 * secs,
    };
    (time, (video_bits + audio_bits) / 8.0)
}

/// "6 seconds", "2 minutes", "1 hour 5 minutes".
pub fn format_duration(secs: f64) -> String {
    let s = secs.round().max(1.0) as u64;
    let plural = |n: u64, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    match s {
        0..=59 => plural(s, "second"),
        60..=3599 => plural((s + 30) / 60, "minute"),
        _ => format!("{} {}", plural(s / 3600, "hour"), plural((s % 3600 + 30) / 60, "minute")),
    }
}

/// "20.8MB", "1.4GB".
pub fn format_size(bytes: f64) -> String {
    if bytes >= 1e9 { format!("{:.1}GB", bytes / 1e9) } else { format!("{:.1}MB", (bytes / 1e6).max(0.1)) }
}

#[cfg(test)]
mod estimate_tests {
    use super::*;
    use crate::settings::{Format, ProRes, Quality};

    #[test]
    fn estimates_grow_with_what_costs() {
        let base = estimate(Format::Mp4, Quality::Social, ProRes::Hq, (1920, 1080), 60, 30.0, Some(60.0), true);
        // 30 s of 1080p60 at 60 MP/s: about 62 s, plus a second.
        assert!((base.0 - (1920.0 * 1080.0 * 1800.0 / 1e6 / 60.0 + 1.0)).abs() < 1e-6);
        // Quality changes the size, not the time (as Screen Studio says).
        let studio = estimate(Format::Mp4, Quality::Studio, ProRes::Hq, (1920, 1080), 60, 30.0, Some(60.0), true);
        assert_eq!(studio.0, base.0);
        assert!(studio.1 > base.1);
        // 4K costs about four times 1080p; half the frames about half.
        let k4 = estimate(Format::Mp4, Quality::Social, ProRes::Hq, (3840, 2160), 60, 30.0, Some(60.0), true);
        assert!((k4.0 - 1.0) / (base.0 - 1.0) > 3.9);
        let fps30 = estimate(Format::Mp4, Quality::Social, ProRes::Hq, (1920, 1080), 30, 30.0, Some(60.0), true);
        assert!(fps30.0 < base.0 * 0.6 && fps30.1 < base.1 * 0.6);
        // ProRes HQ at 1080p30: about Apple's 220 Mbit/s plus PCM audio.
        let pr = estimate(Format::ProRes, Quality::Social, ProRes::Hq, (1920, 1080), 30, 10.0, Some(60.0), true);
        let expect = (220e6 * 30.0 / 29.97 * 10.0 + 1_536_000.0 * 10.0) / 8.0;
        assert!((pr.1 / expect - 1.0).abs() < 1e-6);
        assert!(estimate(Format::ProRes, Quality::Social, ProRes::Lt, (1920, 1080), 30, 10.0, None, false).1 < pr.1);
        // GIFs take longer.
        assert!(estimate(Format::Gif, Quality::Social, ProRes::Hq, (1920, 1080), 30, 30.0, Some(60.0), false).0 > fps30.0 * 2.0);
        assert_eq!(format_duration(6.2), "6 seconds");
        assert_eq!(format_duration(1.0), "1 second");
        assert_eq!(format_duration(130.0), "2 minutes");
        assert_eq!(format_duration(3900.0), "1 hour 5 minutes");
        assert_eq!(format_size(20.8e6), "20.8MB");
        assert_eq!(format_size(1.43e9), "1.4GB");
    }
}

/// An offscreen render target with a buffer to read it back.
struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    w: u32,
    h: u32,
    padded_row: u32,
}

impl Target {
    fn new(comp: &Compositor, w: u32, h: u32) -> Self {
        let device = comp.device();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("export target"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let padded_row = (w * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self { texture, view, readback, w, h, padded_row }
    }

    /// Copies the rendered frame into `out` as tightly packed RGBA.
    fn read(&self, comp: &Compositor, out: &mut [u8]) -> Result<()> {
        let (w, h, padded_row) = (self.w, self.h, self.padded_row);
        let mut enc = comp.device().create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        comp.queue().submit([enc.finish()]);
        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        comp.device().poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("gpu poll: {e:?}"))?;
        {
            let data = slice.get_mapped_range().map_err(|e| anyhow!("map readback: {e:?}"))?;
            for y in 0..h as usize {
                let src = &data[y * padded_row as usize..][..(w * 4) as usize];
                out[y * (w * 4) as usize..][..(w * 4) as usize].copy_from_slice(src);
            }
        }
        self.readback.unmap();
        Ok(())
    }
}

/// Renders the edited frame at recording time `t` to a PNG `height` pixels tall.
pub fn export_frame(comp: &mut Compositor, dir: &Path, rec: &Recording, edit: &EditConfig, t: f64, height: u32, out: &Path) -> Result<()> {
    render_frame(comp, dir, rec, edit, t, height)?.save(out).context("writing the image")
}

/// The edited frame at recording time `t` as a PNG file's bytes.
pub fn frame_png(comp: &mut Compositor, dir: &Path, rec: &Recording, edit: &EditConfig, t: f64, height: u32) -> Result<Vec<u8>> {
    let img = render_frame(comp, dir, rec, edit, t, height)?;
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).context("encoding the image")?;
    Ok(png.into_inner())
}

/// The edited frame at recording time `t`, `height` pixels tall.
pub fn render_frame(comp: &mut Compositor, dir: &Path, rec: &Recording, edit: &EditConfig, t: f64, height: u32) -> Result<image::RgbaImage> {
    let paths = ProjectPaths::new(dir);
    crate::privacy::ensure(rec, &edit.privacy)?;
    let tracks = Tracks::build(rec, edit);
    let (w, h) = output_size(tracks.layout.aspect, height);
    let target = Target::new(comp, w, h);
    let mut frame = Vec::new();
    let mut decoder = Decoder::open(&paths.video(), t, (rec.width, rec.height), None)?;
    if !decoder.read(&mut frame)? {
        bail!("no video frame at {t:.2}s");
    }
    comp.upload_frame(&frame, decoder.width, decoder.height);
    if let Some(c) = &rec.camera {
        let ct = (t - c.offset - edit.camera.delay).max(0.0);
        let mut buf = Vec::new();
        let mut d = Decoder::open(&dir.join(&c.file), ct, (c.width, c.height), Some(1920))?;
        if d.read(&mut buf)? {
            comp.upload_camera_frame(&buf, d.width, d.height);
        }
        if let Some(m) = &c.matte {
            let mut d = Decoder::open(&dir.join(m), ct, crate::cutout::matte_size(c.width, c.height), None)?;
            if d.read(&mut buf)? {
                comp.upload_matte_frame(&buf, d.width, d.height);
            }
        }
    }
    let fs = frame_state(rec, edit, &tracks, t, 60.0);
    comp.render(&target.view, (w, h), &tracks.layout, edit, &fs);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    target.read(comp, &mut rgba)?;
    image::RgbaImage::from_raw(w, h, rgba).ok_or_else(|| anyhow!("frame buffer size"))
}

/// One input to the export mix.
pub struct MixTrack {
    pub path: std::path::PathBuf,
    /// Start on the video timeline, seconds.
    pub offset: f64,
    pub gain: f64,
    /// Extra ffmpeg filters (mic processing).
    pub filter: String,
    /// Repeat to fill the video (background music).
    pub looped: bool,
    /// "microphone", "system", "music" or "clicks": slices can change the
    /// first two's volume.
    pub kind: &'static str,
}

/// Audio sources after applying the Audio tab: microphone (with its mode),
/// system audio, background music and click sounds.
pub fn audio_tracks(rec: &Recording, edit: &EditConfig, clicks: Option<&Path>) -> Vec<MixTrack> {
    let a = &edit.audio;
    let mut out = vec![];
    for t in &rec.audio {
        let (muted, gain, filter) = match t.kind.as_str() {
            "microphone" => (a.mute_microphone || a.mic_mode == crate::edit::MicMode::Off, a.microphone_volume, a.mic_mode.filter().to_string()),
            "device" => (a.mute_device, a.device_volume, String::new()),
            _ => (a.mute_system, a.system_volume, String::new()),
        };
        if !muted {
            let kind = match t.kind.as_str() {
                "microphone" => "microphone",
                "device" => "device",
                _ => "system",
            };
            out.push(MixTrack { path: rec.dir.join(&t.file), offset: t.offset, gain, filter, looped: false, kind });
        }
    }
    if let (Some(m), false) = (&a.music, a.mute_music) {
        out.push(MixTrack { path: m.into(), offset: 0.0, gain: a.music_volume, filter: String::new(), looped: true, kind: "music" });
    }
    if let Some(c) = clicks {
        out.push(MixTrack { path: c.to_path_buf(), offset: 0.0, gain: edit.cursor.click_volume, filter: String::new(), looped: false, kind: "clicks" });
    }
    out
}

/// Mixes audio into the exported MP4: each track is shifted by its offset,
/// trimmed to the edited range, filtered and gain-adjusted.
#[allow(clippy::too_many_arguments)]
fn mux_audio(_dir: &Path, tracks: &[MixTrack], video: &Path, start: f64, end: f64, segments: &[(f64, f64, f64)], slices: &[crate::edit::SliceProps]) -> Result<()> {
    // ProRes .mov files carry uncompressed audio, as editors expect.
    let mov = video.extension().is_some_and(|e| e.eq_ignore_ascii_case("mov"));
    let tmp = video.with_extension(if mov { "audio.mov" } else { "audio.mp4" });
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(video);
    let mut filters = vec![];
    let mut labels = String::new();
    for (i, track) in tracks.iter().enumerate() {
        if track.looped {
            cmd.args(["-stream_loop", "-1"]);
        }
        cmd.arg("-i").arg(&track.path);
        // Audio time = video time - offset.
        let (a0, a1) = ((start - track.offset).max(0.0), (end - track.offset).max(0.0));
        let lead_ms = ((track.offset - start).max(0.0) * 1000.0).round() as i64;
        let extra = if track.filter.is_empty() { String::new() } else { format!("{},", track.filter) };
        // Slice volumes, as a piecewise expression of time since `start`.
        let slice_volume = |p: &crate::edit::SliceProps| match track.kind {
            "microphone" => p.volume,
            "system" => p.system_volume,
            "device" => p.device_volume,
            _ => 1.0,
        };
        let mut expr = "1".to_string();
        for p in slices.iter().rev().filter(|p| (slice_volume(p) - 1.0).abs() > 1e-6) {
            expr = format!("if(between(t,{:.4},{:.4}),{:.3},{expr})", p.start - start, p.end - start, slice_volume(p));
        }
        let per_slice = if expr == "1" { String::new() } else { format!(",volume='{expr}':eval=frame") };
        filters.push(format!(
            "[{}:a]aresample=48000,aformat=channel_layouts=stereo,atrim=start={a0:.4}:end={a1:.4},asetpts=PTS-STARTPTS,{extra}adelay={lead_ms}:all=1,volume={:.3}{per_slice}[a{i}]",
            i + 1,
            track.gain
        ));
        labels.push_str(&format!("[a{i}]"));
    }
    let n = tracks.len();
    // Keep the video's stretches (dropping cuts) at their speeds, end to end.
    let segs: Vec<(f64, f64, f64)> = segments.iter().map(|&(a, b, s)| (a - start, b - start, s)).filter(|(a, b, _)| b > a).collect();
    filters.push(format!("{labels}amix=inputs={n}:normalize=0:duration=longest,apad,atrim=end={:.4},asplit={}{}", end - start, segs.len().max(1), (0..segs.len().max(1)).map(|k| format!("[s{k}]")).collect::<String>()));
    let mut joined = String::new();
    for (k, (a, b, speed)) in segs.iter().enumerate() {
        // atempo takes 0.5..100 per instance; chain one for slower speeds.
        let tempo = if (speed - 1.0).abs() < 1e-6 {
            String::new()
        } else if *speed < 0.5 {
            format!(",atempo=0.5,atempo={:.4}", speed / 0.5)
        } else {
            format!(",atempo={speed:.4}")
        };
        filters.push(format!("[s{k}]atrim=start={a:.4}:end={b:.4},asetpts=PTS-STARTPTS{tempo}[c{k}]"));
        joined.push_str(&format!("[c{k}]"));
    }
    if segs.is_empty() {
        filters.push("[s0]anull[aout]".into());
    } else {
        filters.push(format!("{joined}concat=n={}:v=0:a=1,apad[aout]", segs.len()));
    }
    cmd.args(["-filter_complex", &filters.join(";")]);
    cmd.args(["-map", "0:v", "-map", "[aout]", "-c:v", "copy"]);
    if mov {
        cmd.args(["-c:a", "pcm_s16le"]);
    } else {
        cmd.args(["-c:a", "aac", "-b:a", "192k"]);
    }
    cmd.args(["-shortest", "-movflags", "+faststart"]);
    cmd.arg(&tmp);
    let out = cmd.output().context("muxing audio")?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!("audio mux failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    std::fs::rename(&tmp, video)?;
    Ok(())
}
