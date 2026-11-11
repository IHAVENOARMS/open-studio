#!/bin/sh
# Installs Open Studio for the current user: the program into ~/.local/bin,
# and a launcher entry. Run ./install.sh --uninstall to remove both.
set -e
here=$(cd "$(dirname "$0")" && pwd)
bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/256x256/apps"

if [ "$1" = "--uninstall" ]; then
    rm -f "$bin/open-studio" "$apps/open-studio.desktop" "$icons/open-studio.png"
    echo "Open Studio removed. Projects in ~/Videos/Open Studio are kept."
    exit 0
fi

mkdir -p "$bin" "$apps" "$icons"
install -m 755 "$here/open-studio" "$bin/open-studio"
install -m 644 "$here/open-studio.png" "$icons/open-studio.png"
sed "s|^Exec=open-studio|Exec=$bin/open-studio|" "$here/open-studio.desktop" > "$apps/open-studio.desktop"
echo "Installed to $bin/open-studio"

missing=""
command -v ffmpeg >/dev/null 2>&1 || missing="$missing ffmpeg"
command -v gst-inspect-1.0 >/dev/null 2>&1 && ! gst-inspect-1.0 pipewiresrc >/dev/null 2>&1 && missing="$missing gstreamer-pipewire-plugin"
[ -n "$missing" ] && echo "Missing:$missing (see the release notes)"
id -nG | grep -qw input || echo "Add yourself to the input group for click and key capture: sudo usermod -aG input \$USER, then log in again."
case ":$PATH:" in *":$bin:"*) ;; *) echo "Note: $bin is not on your PATH." ;; esac
