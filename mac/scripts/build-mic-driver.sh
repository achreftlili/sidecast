#!/bin/bash
# Builds SidecastMic.driver (the "Sidecast Microphone" AudioServerPlugIn) from
# mac/Sources/SidecastMicDriver, signs it, and validates it WITHOUT installing:
# plutil, exported factory symbol, linked libraries, then a CFPlugIn load +
# property/loopback check in a throwaway process (never touches coreaudiod).
#
#   mac/scripts/build-mic-driver.sh <output.driver> [sign identity, default "-"]
#
# Architecture: $SIDECAST_ARCH (arm64 | x86_64), default this Mac's.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SRC="$MAC_DIR/Sources/SidecastMicDriver"
WORK="$MAC_DIR/.build/mic-driver"
OUT="${1:?usage: build-mic-driver.sh <output.driver> [sign identity]}"
SIGN_IDENTITY="${2:--}"
LOCAL_IDENTITY="Sidecast Local Signing"
ARCH="${SIDECAST_ARCH:-$(uname -m)}"

header_value() { sed -n "s/^#define $1 \"\(.*\)\"$/\1/p" "$SRC/include/SidecastMicDriver.h"; }
DRIVER_VERSION="$(header_value SCM_DRIVER_VERSION)"
FACTORY_UUID="$(header_value SCM_FACTORY_UUID)"
[[ -n "$DRIVER_VERSION" && -n "$FACTORY_UUID" ]] || { echo "error: cannot read driver version/factory UUID" >&2; exit 1; }
# kAudioServerPlugInTypeUUID
PLUGIN_TYPE_UUID="443ABAB8-E7B3-491A-B985-BEB9187030DB"

CFLAGS=(-arch "$ARCH" -mmacosx-version-min=15.0 -std=c11 -O2 -g0
        -Wall -Wextra -Wpedantic -Wno-unused-parameter -Werror
        -fvisibility=hidden -I "$SRC/include")

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources" "$WORK"

xcrun clang "${CFLAGS[@]}" -bundle \
    "$SRC/SidecastMicDriver.c" "$SRC/SidecastRing.c" \
    -framework CoreAudio -framework CoreFoundation \
    -o "$OUT/Contents/MacOS/SidecastMic"

cat > "$OUT/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>SidecastMic</string>
	<key>CFBundleIdentifier</key>
	<string>com.achreftlili.sidecast.mic</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>SidecastMic</string>
	<key>CFBundlePackageType</key>
	<string>BNDL</string>
	<key>CFBundleShortVersionString</key>
	<string>$DRIVER_VERSION</string>
	<key>CFBundleVersion</key>
	<string>$DRIVER_VERSION</string>
	<key>CFBundleSupportedPlatforms</key>
	<array>
		<string>MacOSX</string>
	</array>
	<key>LSMinimumSystemVersion</key>
	<string>15.0</string>
	<key>NSHumanReadableCopyright</key>
	<string>Sidecast Microphone — part of Sidecast.</string>
	<key>CFPlugInFactories</key>
	<dict>
		<key>$FACTORY_UUID</key>
		<string>SidecastMic_Create</string>
	</dict>
	<key>CFPlugInTypes</key>
	<dict>
		<key>$PLUGIN_TYPE_UUID</key>
		<array>
			<string>$FACTORY_UUID</string>
		</array>
	</dict>
</dict>
</plist>
PLIST
plutil -lint "$OUT/Contents/Info.plist"

# Exactly one exported symbol: the factory named in Info.plist.
EXPORTS="$(nm -gU "$OUT/Contents/MacOS/SidecastMic" | awk '{print $3}')"
if [[ "$EXPORTS" != "_SidecastMic_Create" ]]; then
    echo "error: unexpected exported symbols: $EXPORTS" >&2
    exit 1
fi
echo "exported: $EXPORTS"
# Only system libraries.
LIBS="$(otool -L "$OUT/Contents/MacOS/SidecastMic" | tail -n +2 | awk '{print $1}')"
if grep -qvE '^/(System/Library|usr/lib)/' <<<"$LIBS"; then
    echo "error: non-system library linked:" >&2
    echo "$LIBS" >&2
    exit 1
fi
echo "linked:"
sed 's/^/  /' <<<"$LIBS"

if [[ "$SIGN_IDENTITY" == "-" || "$SIGN_IDENTITY" == "$LOCAL_IDENTITY" ]]; then
    codesign --force --sign "$SIGN_IDENTITY" "$OUT"
else
    codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$OUT"
fi
codesign --verify --strict "$OUT"

# Load it in a throwaway process and check devices, formats and the loopback.
xcrun clang -arch "$ARCH" -mmacosx-version-min=15.0 -std=c11 -O1 -Wall -Wno-unused-parameter \
    "$SCRIPT_DIR/check-mic-driver.c" -framework CoreAudio -framework CoreFoundation \
    -o "$WORK/check-mic-driver"
"$WORK/check-mic-driver" "$OUT"
