#!/usr/bin/env bash
# Builds the Linux host and installs it for this user (no root needed):
#   ~/.local/bin/sidecast-host
#   ~/.local/share/applications/sidecast.desktop   (shows "Sidecast" in the app grid)
#   ~/.config/autostart/sidecast.desktop           (with --autostart or SIDECAST_AUTOSTART=1)
#
#   make install-linux                    or   scripts/install-linux.sh [--autostart] [--no-gstreamer]
#   scripts/install-linux.sh --uninstall  removes the three files (pairings in ~/.local/share/sidecast stay)
#
# Hardware encoding (VA-API / NVENC through GStreamer) is built in when the
# GStreamer development files are installed; OpenH264 is always there as the fallback.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
APPS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
AUTOSTART="${SIDECAST_AUTOSTART:-0}"
GSTREAMER=auto

for arg in "$@"; do
    case "$arg" in
        --autostart) AUTOSTART=1 ;;
        --no-gstreamer) GSTREAMER=0 ;;
        --uninstall)
            rm -f "$BIN_DIR/sidecast-host" "$APPS_DIR/sidecast.desktop" "$AUTOSTART_DIR/sidecast.desktop"
            echo "Removed Sidecast. Pairings and the host identity stay in ${XDG_DATA_HOME:-$HOME/.local/share}/sidecast."
            exit 0
            ;;
        -h|--help) sed -n '2,12p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "unknown argument $arg" >&2; exit 2 ;;
    esac
done

if [[ "$(uname -s)" != Linux ]]; then
    echo "This installs the Linux host; on a Mac use make install-mac." >&2
    exit 1
fi
if ! command -v cargo >/dev/null; then
    echo "Rust is missing: install it from https://rustup.rs, then run make doctor." >&2
    exit 1
fi
if ! pkg-config --exists libpipewire-0.3 2>/dev/null; then
    echo "PipeWire's development files are missing: sudo apt install libpipewire-0.3-dev clang (make doctor lists everything)." >&2
    exit 1
fi

FEATURES=()
if [[ "$GSTREAMER" == auto ]] && pkg-config --exists gstreamer-1.0 gstreamer-app-1.0 2>/dev/null; then
    FEATURES=(--features gstreamer)
    echo "==> Building with hardware encoding through GStreamer (VA-API / NVENC when available)"
else
    echo "==> Building with the OpenH264 software encoder only (install libgstreamer-plugins-base1.0-dev for hardware encoding)"
fi
(cd "$REPO_DIR/rust" && cargo build --release -p sidecast-host "${FEATURES[@]+"${FEATURES[@]}"}")
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_DIR/rust/target}"

mkdir -p "$BIN_DIR" "$APPS_DIR"
install -m 755 "$TARGET_DIR/release/sidecast-host" "$BIN_DIR/sidecast-host"
DESKTOP="[Desktop Entry]
Type=Application
Name=Sidecast
GenericName=Phone as a second screen
Comment=Use your phone or tablet as an extra display, touchpad, keyboard and microphone
Exec=$BIN_DIR/sidecast-host
Icon=video-display
Terminal=false
Categories=Utility;Network;
Keywords=display;monitor;screen;phone;tablet;
StartupNotify=false
X-GNOME-Autostart-enabled=true"
printf '%s\n' "$DESKTOP" >"$APPS_DIR/sidecast.desktop"
echo "Installed $BIN_DIR/sidecast-host and $APPS_DIR/sidecast.desktop"
if [[ "$AUTOSTART" == 1 ]]; then
    mkdir -p "$AUTOSTART_DIR"
    printf '%s\n' "$DESKTOP" >"$AUTOSTART_DIR/sidecast.desktop"
    echo "Sidecast starts when you log in ($AUTOSTART_DIR/sidecast.desktop)"
fi
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: $BIN_DIR is not in your PATH; the app grid entry works anyway." ;;
esac
echo
echo "Start it from the app grid (\"Sidecast\") or run sidecast-host in a terminal."
echo "The tray icon needs Ubuntu's AppIndicator extension (on by default); the pairing code also appears as a notification."
