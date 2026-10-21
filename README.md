# Open Studio

An open-source screen recorder for Linux (Wayland) in the spirit of
[Screen Studio](https://screen.studio): record your screen, and get automatic
zooms that follow your cursor, a smooth re-rendered cursor, motion blur and a
styled background, ready to export.

Status: early. Recording, automatic zoom, cursor smoothing, the editor and
MP4 export work. Camera, microphone/system audio, area selection and
GIF export are not built yet.

## How it works

- **Capture**: the xdg-desktop-portal ScreenCast API (so the compositor shows
  its own screen picker) feeds a PipeWire stream into GStreamer, which converts
  it to a constant 60 fps. ffmpeg encodes it as H.264 4:4:4 (x264 `veryfast`,
  CRF 14) so text stays sharp when zoomed in. The cursor is captured
  separately rather than baked into the video.
- **Cursor and clicks**: the cursor position comes from Hyprland IPC, and clicks
  and keys are read from evdev. Reading evdev requires being in the `input` group.
- **Cursor shapes**: while recording, a small region around the pointer is
  captured twice per frame through wlr-screencopy, with and without the
  cursor. The difference is the cursor as drawn, and the cursor-free copy is
  the background under it. That is matched against the active XCursor theme
  to tell the arrow from the I-beam, pointing hand, resize arrows, and so on.
  The editor redraws each shape as a high-resolution vector cursor (or the
  theme's 96 px image for rarer shapes). Hyprland's ext-image-copy-capture
  cursor sessions only cover client-drawn cursors, which is why this
  approach is used.
- **Auto zoom**: each burst of clicks becomes a zoom segment (2× by default):
  - it starts 0.4 s before the first click and holds 2.5 s after the last one;
  - bursts with gaps of up to 2.5 s merge into one segment.
  - The camera follows the cursor with an edge-push dead zone. Its scale and
    position move on spring physics (tension/friction/mass, with speed presets
    from Slow to Rapid).
  - The springs are pre-simulated at a fixed time step, so the preview and the
    export produce identical frames.
- **Rendering**: a single wgpu compositor draws:
  - the background (wallpaper, gradient, colour or image, with optional blur);
  - the rounded video card with its shadow;
  - mip-mapped video sampling when zoomed out, bicubic (Catmull-Rom) when zoomed in;
  - shutter-based motion blur;
  - the cursor (a vector-rasterized arrow).

  The editor preview and the exporter share this same code.

## Requirements

- Wayland compositor with a ScreenCast portal (developed on Hyprland +
  xdg-desktop-portal-hyprland; cursor tracking currently needs Hyprland)
- PipeWire, GStreamer (`gst-plugins-base`, `gst-plugins-good`, `gst-plugin-pipewire`)
- ffmpeg with libx264
- Vulkan-capable GPU

## Usage

```sh
cargo build --release
./target/release/open-studio            # home screen: record / open projects
open-studio record [--reuse] [--duration 10]   # headless recording
open-studio stop                        # stop from a keybinding
open-studio edit "~/Videos/Open Studio/Recording …"
open-studio export <project> --height 1080 --fps 60
open-studio wallpapers [--list]         # download the wallpaper collection
```

## Wallpapers

`assets/wallpapers.json` lists a curated set of openly licensed wallpapers
(CC0, CC BY, CC BY-SA, Unsplash License and similar), with author and
licence for each; credits are in `assets/WALLPAPERS.md`. The images are not
stored in the repository. They are downloaded on first use (from the editor's
Background panel or with `open-studio wallpapers`), scaled to at most 3840 px
wide, and kept in `~/.local/share/open-studio/wallpapers/`. Any local JPEG,
PNG or WebP can also be used through the Image background option.

Hyprland keybinding to stop a recording:

```
bind = SUPER SHIFT, R, exec, open-studio stop
```

While recording, a tray icon also stops the recording when clicked. With
"Hide this window" on, the app moves itself to a special workspace so it
isn't captured.

Projects live in `~/Videos/Open Studio/`, one directory per recording:
`screen.mp4` (raw capture), `recording.json` (cursor, clicks, keys) and
`project.json` (your edits).

## Research

`docs/research/` contains the product research (feature inventory, UI
measurements, colours, 90 reference screenshots) and notes on how zoom and
cursor smoothing are implemented by open-source alternatives.
