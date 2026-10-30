#!/bin/bash
# Builds Sidecast.app and installs it into /Applications, replacing (and first
# quitting) the copy there, then opens it.
#
#   make install-mac
#   SIDECAST_INSTALL_DIR=<dir>   install somewhere else
#   SIDECAST_NO_LAUNCH=1         don't open it afterwards
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DEST_DIR="${SIDECAST_INSTALL_DIR:-/Applications}"
DEST="$DEST_DIR/Sidecast.app"
BUNDLE_ID="com.achreftlili.sidecast"
APP="$REPO_ROOT/dist/Sidecast.app"

if [[ "$(security find-identity -p codesigning 2>/dev/null)" != *'"Sidecast Local Signing"'* ]]; then
    echo "note: no \"Sidecast Local Signing\" certificate, so this build is ad-hoc signed and macOS"
    echo "      will ask for Screen Recording and Accessibility again after every update."
    echo "      Run 'make mac-signing' once to avoid that."
fi

"$SCRIPT_DIR/build-mac.sh"
codesign --verify --deep --strict "$APP"   # never install a broken build

# PIDs of Sidecast running from $DEST.
running_from_dest() {
    local pid
    for pid in $(pgrep -x Sidecast || true); do
        [[ "$(ps -o comm= -p "$pid")" == "$DEST/Contents/MacOS/Sidecast" ]] && echo "$pid"
    done
    return 0
}

if [[ -n "$(running_from_dest)" ]]; then
    printf '\n==> Quitting the running Sidecast\n'
    # A normal quit, so it restores sleep and the default microphone.
    osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null
    for _ in $(seq 50); do
        [[ -z "$(running_from_dest)" ]] && break
        sleep 0.2
    done
    if [[ -n "$(running_from_dest)" ]]; then
        echo "error: Sidecast is still running; quit it from the menu bar and run this again" >&2
        exit 1
    fi
fi

printf '\n==> Installing %s\n' "$DEST"
mkdir -p "$DEST_DIR"
STAGED="$DEST_DIR/.Sidecast.app.installing"
rm -rf "$STAGED"
ditto "$APP" "$STAGED"
rm -rf "$DEST"
mv "$STAGED" "$DEST"
codesign --verify --deep --strict "$DEST"

if [[ "${SIDECAST_NO_LAUNCH:-}" != 1 ]]; then
    open "$DEST"
fi
echo "Installed $DEST"
