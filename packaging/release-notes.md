Open Studio is a screen recorder for Linux with automatic zooms, a smooth cursor and polished exports.

**This is a beta.** Expect bugs and changes between versions. It has been tested on Hyprland only; other Wayland desktops should work but are untested. Reports are welcome in [Issues](https://github.com/IHAVENOARMS/open-studio/issues).

## Download

**AppImage (easiest).** Download `open-studio-*-x86_64.AppImage` below, then:

```sh
chmod +x open-studio-*-x86_64.AppImage
./open-studio-*-x86_64.AppImage
```

It brings its own GStreamer and FFmpeg. It needs a Wayland desktop with PipeWire and xdg-desktop-portal, and runs on distributions from 2022 on (Ubuntu 22.04, Debian 12, Fedora 36 and newer).

**Tarball.** `open-studio-*-x86_64-linux.tar.gz` is smaller and uses your system's libraries. Unpack it and run `./install.sh`, which adds Open Studio to your app launcher (`./install.sh --uninstall` removes it). It needs glibc 2.39 or newer (Ubuntu 24.04, Debian 13, Fedora 40, Arch) and these packages:

| | |
|---|---|
| Arch | `sudo pacman -S ffmpeg gst-plugins-base gst-plugins-good gst-plugin-pipewire` |
| Ubuntu, Debian | `sudo apt install ffmpeg gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-pipewire` |
| Fedora | `sudo dnf install ffmpeg gstreamer1-plugins-base gstreamer1-plugins-good pipewire-gstreamer` (FFmpeg from RPM Fusion) |

## After installing

For click and key capture, add yourself to the `input` group: `sudo usermod -aG input $USER`, then log in again.

Optional: `scrcpy` and `adb` for phones, `tesseract` for hiding sensitive data, `whisper-cpp` for captions.

Check a download with `sha256sum -c <file>.sha256`.
