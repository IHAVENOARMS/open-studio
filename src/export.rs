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
    let tracks = Tracks::build(rec, edit);
    let (w, h) = output_size(tracks.layout.aspect, opts.height);
    let (start, end) = edit.trim_range(rec.duration);
    // Recording time of every output frame, skipping cuts.
    let mut times = vec![];
    let mut acc = 0.0;
    for (a, b) in edit.kept_ranges(rec.duration) {
        let n0 = (acc * opts.fps as f64).ceil() as u64;
        acc += b - a;
        let n1 = (acc * opts.fps as f64).ceil() as u64;
        for n in n0..n1 {
            let out_t = n as f64 / opts.fps as f64;
            times.push(a + (out_t - (acc - (b - a))));
        }
    }
    if times.is_empty() {
        bail!("nothing to export: everything is trimmed or cut");
    }
    let total = times.len() as u64;

    let device = comp.device().clone();
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("export target"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let padded_row = (w * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    // GIFs are encoded from an intermediate MP4 with a generated palette.
    let gif = opts.format == crate::settings::Format::Gif;
    let video_out = if gif { opts.out.with_extension("tmp.mp4") } else { opts.out.clone() };
    let (crf, preset) = if gif { (12, "veryfast") } else { opts.quality.x264() };
    let mut ffmpeg = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r", &opts.fps.to_string(), "-i", "-"])
        .args(["-vf", "scale=out_color_matrix=bt709:out_range=tv"])
        .args(["-c:v", "libx264", "-preset", preset, "-crf", &crf.to_string(), "-pix_fmt", "yuv420p"])
        .args(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"])
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
        let fs = frame_state(rec, edit, &tracks, t, opts.fps as f64);
        comp.render(&view, (w, h), &tracks.layout, edit, &fs);

        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        comp.queue().submit([enc.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("gpu poll: {e:?}"))?;
        {
            let data = slice.get_mapped_range().map_err(|e| anyhow!("map readback: {e:?}"))?;
            for y in 0..h as usize {
                let src = &data[y * padded_row as usize..][..(w * 4) as usize];
                row[y * (w * 4) as usize..][..(w * 4) as usize].copy_from_slice(src);
            }
        }
        readback.unmap();
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
            // Cuts relative to the trimmed start.
            let gaps: Vec<(f64, f64)> = edit
                .cuts
                .iter()
                .filter(|c| c.end > start && c.start < end)
                .map(|c| (c.start.max(start) - start, c.end.min(end) - start))
                .collect();
            mux_audio(dir, &tracks, &video_out, start, end, &gaps)?;
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
    Ok(())
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
}

/// Audio sources after applying the Audio tab: microphone (with its mode),
/// system audio, background music and click sounds.
pub fn audio_tracks(rec: &Recording, edit: &EditConfig, clicks: Option<&Path>) -> Vec<MixTrack> {
    let a = &edit.audio;
    let mut out = vec![];
    for t in &rec.audio {
        let (muted, gain, filter) = if t.kind == "microphone" {
            (a.mute_microphone || a.mic_mode == crate::edit::MicMode::Off, a.microphone_volume, a.mic_mode.filter().to_string())
        } else {
            (a.mute_system, a.system_volume, String::new())
        };
        if !muted {
            out.push(MixTrack { path: rec.dir.join(&t.file), offset: t.offset, gain, filter, looped: false });
        }
    }
    if let (Some(m), false) = (&a.music, a.mute_music) {
        out.push(MixTrack { path: m.into(), offset: 0.0, gain: a.music_volume, filter: String::new(), looped: true });
    }
    if let Some(c) = clicks {
        out.push(MixTrack { path: c.to_path_buf(), offset: 0.0, gain: edit.cursor.click_volume, filter: String::new(), looped: false });
    }
    out
}

/// Mixes audio into the exported MP4: each track is shifted by its offset,
/// trimmed to the edited range, filtered and gain-adjusted.
fn mux_audio(_dir: &Path, tracks: &[MixTrack], video: &Path, start: f64, end: f64, gaps: &[(f64, f64)]) -> Result<()> {
    let tmp = video.with_extension("audio.mp4");
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
        filters.push(format!(
            "[{}:a]aresample=48000,aformat=channel_layouts=stereo,atrim=start={a0:.4}:end={a1:.4},asetpts=PTS-STARTPTS,{extra}adelay={lead_ms}:all=1,volume={:.3}[a{i}]",
            i + 1,
            track.gain
        ));
        labels.push_str(&format!("[a{i}]"));
    }
    let n = tracks.len();
    // Drop the cut stretches and close the gaps.
    let select = if gaps.is_empty() {
        String::new()
    } else {
        let expr: Vec<String> = gaps.iter().map(|(a, b)| format!("between(t,{a:.4},{b:.4})")).collect();
        format!(",aselect='not({})',asetpts=N/SR/TB", expr.join("+"))
    };
    filters.push(format!("{labels}amix=inputs={n}:normalize=0:duration=longest,atrim=end={:.4}{select},apad[aout]", end - start));
    cmd.args(["-filter_complex", &filters.join(";")]);
    cmd.args(["-map", "0:v", "-map", "[aout]", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart"]);
    cmd.arg(&tmp);
    let out = cmd.output().context("muxing audio")?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!("audio mux failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    std::fs::rename(&tmp, video)?;
    Ok(())
}
