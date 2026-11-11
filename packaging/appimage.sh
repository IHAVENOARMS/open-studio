#!/bin/sh
# Builds Open Studio as an AppImage that runs on most current Linux desktops:
# the app, GStreamer with its plugins and a static FFmpeg are bundled. The
# PipeWire client library is left to the system, so it always matches the
# running PipeWire daemon.
#
# Usage: packaging/appimage.sh VERSION   (after `cargo build --release`)
# Intended for Ubuntu 22.04 (the oldest glibc we target); see the release
# workflow for the packages it needs.
set -eu
VERSION="$1"
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BUILD="$ROOT/build/appimage"
APPDIR="$BUILD/AppDir"
OUT="$ROOT/dist/open-studio-$VERSION-x86_64.AppImage"
rm -rf "$BUILD"
mkdir -p "$BUILD" "$APPDIR/usr/bin" "$ROOT/dist"
cd "$BUILD"

# Tools: linuxdeploy and its GStreamer plugin.
curl -fsSLo linuxdeploy https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
curl -fsSLo linuxdeploy-plugin-gstreamer.sh https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gstreamer/master/linuxdeploy-plugin-gstreamer.sh
chmod +x linuxdeploy linuxdeploy-plugin-gstreamer.sh
export PATH="$BUILD:$PATH"
# CI runners have no FUSE.
export APPIMAGE_EXTRACT_AND_RUN=1

# A static FFmpeg (GPL build: x264, ProRes, VAAPI, NVENC).
curl -fsSL https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n7.1-latest-linux64-gpl-7.1.tar.xz | tar xJ
cp ffmpeg-n7.1-latest-linux64-gpl-7.1/bin/ffmpeg ffmpeg-n7.1-latest-linux64-gpl-7.1/bin/ffprobe "$APPDIR/usr/bin/"

arch_libs=/usr/lib/x86_64-linux-gnu
export GSTREAMER_PLUGINS_DIR="$arch_libs/gstreamer-1.0"
export GSTREAMER_HELPERS_DIR="$arch_libs/gstreamer1.0/gstreamer-1.0"

linuxdeploy --appdir "$APPDIR" \
    --executable "$ROOT/target/release/open-studio" \
    --desktop-file "$ROOT/packaging/open-studio.desktop" \
    --icon-file "$ROOT/packaging/open-studio.png" \
    --custom-apprun "$ROOT/packaging/AppRun" \
    --plugin gstreamer

# PipeWire's client library must match the system's daemon and modules.
find "$APPDIR" -name 'libpipewire-0.3.so*' -delete

LINUXDEPLOY_OUTPUT_VERSION="$VERSION" OUTPUT="$OUT" linuxdeploy --appdir "$APPDIR" --output appimage
(cd "$ROOT/dist" && sha256sum "$(basename "$OUT")" > "$(basename "$OUT").sha256")
echo "$OUT"
