The first public beta of Open Studio, a screen recorder for Linux with automatic zooms, a smooth cursor and polished exports.

**This is beta software.** Expect bugs and changes between versions. It has been tested on Hyprland only; other Wayland desktops should work but are untested. Reports are welcome in [Issues](https://github.com/IHAVENOARMS/open-studio/issues).

## Install

Download `open-studio-*-x86_64-linux.tar.gz` below, then:

```sh
tar xf open-studio-*-x86_64-linux.tar.gz
cd open-studio-*-x86_64-linux
./install.sh
```

This puts `open-studio` in `~/.local/bin` and adds it to your app launcher. `./install.sh --uninstall` removes it.

Built on Ubuntu 24.04, so it needs glibc 2.39 or newer (Ubuntu 24.04, Debian 13, Fedora 40, Arch and later).

## Requirements

FFmpeg, GStreamer with the base, good and PipeWire plugins, PipeWire, and a Wayland session with xdg-desktop-portal.

| | |
|---|---|
| Arch | `sudo pacman -S ffmpeg gst-plugins-base gst-plugins-good gst-plugin-pipewire` |
| Ubuntu, Debian | `sudo apt install ffmpeg gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-pipewire` |
| Fedora | `sudo dnf install ffmpeg gstreamer1-plugins-base gstreamer1-plugins-good pipewire-gstreamer` (FFmpeg from RPM Fusion) |

For click and key capture, add yourself to the `input` group: `sudo usermod -aG input $USER`, then log in again.

Optional: `scrcpy` and `adb` for phones, `tesseract` for hiding sensitive data, `whisper-cpp` for captions.

Check the download with `sha256sum -c open-studio-*.sha256`.
