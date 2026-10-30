#!/usr/bin/env bash
# Builds the Android app and installs it on your phone over adb (USB debugging
# or wireless debugging), then opens it.
#
#   make install-android
#   ANDROID_SERIAL=<serial>   pick a phone when several are connected (see `adb devices`)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/env.sh"
sidecast_android_env
REPO_DIR="$(cd "$SIDECAST_ANDROID_DIR/.." && pwd)"
PACKAGE="com.achreftlili.sidecast"

if ! ADB="$(sidecast_adb)"; then
  echo "error: adb not found. Install the Android SDK platform-tools (sdkmanager \"platform-tools\")." >&2
  exit 1
fi

# Check for a phone before the (long) build. (bash 3.2 on macOS: no mapfile.)
DEVICES=()
while read -r serial; do
  DEVICES+=("$serial")
done < <("$ADB" devices | awk 'NR > 1 && $2 == "device" { print $1 }')
if [[ -n "${ANDROID_SERIAL:-}" ]]; then
  if [[ " ${DEVICES[*]-} " != *" $ANDROID_SERIAL "* ]]; then
    echo "error: $ANDROID_SERIAL is not connected (see '$ADB devices')" >&2
    exit 1
  fi
elif (( ${#DEVICES[@]} == 0 )); then
  echo "error: no phone found. On the phone: Settings → About phone → tap Build number 7 times," >&2
  echo "       then Developer options → USB debugging (or Wireless debugging). Connect it, accept" >&2
  echo "       the prompt on the phone, check with '$ADB devices', and run this again." >&2
  exit 1
elif (( ${#DEVICES[@]} > 1 )); then
  echo "error: several phones are connected (${DEVICES[*]}); choose one with ANDROID_SERIAL=<serial>" >&2
  exit 1
else
  export ANDROID_SERIAL="${DEVICES[0]}"
fi

"$SCRIPT_DIR/build-android.sh"
VERSION="$(tr -d '[:space:]' < "$REPO_DIR/VERSION")"
APK="$REPO_DIR/dist/Sidecast-$VERSION.apk"

printf '\n==> Installing on %s\n' "$ANDROID_SERIAL"
if ! OUTPUT="$("$ADB" install -r "$APK" 2>&1)"; then
  echo "$OUTPUT" >&2
  if grep -qE 'INSTALL_FAILED_UPDATE_INCOMPATIBLE|signatures do not match' <<<"$OUTPUT"; then
    echo "" >&2
    echo "The phone has a Sidecast signed with a different key (built on another computer or" >&2
    echo "installed from a store). Uninstall it first, then run this again (this forgets its pairings):" >&2
    echo "  $ADB uninstall $PACKAGE" >&2
  fi
  exit 1
fi
"$ADB" shell monkey -p "$PACKAGE" -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1 || true
echo "Installed Sidecast $VERSION on $ANDROID_SERIAL"
