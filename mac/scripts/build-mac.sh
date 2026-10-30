#!/bin/bash
# Builds Sidecast.app and Sidecast-<version>.dmg into <repo>/dist/.
#
#   mac/scripts/build-mac.sh
#
# Signed with the "Sidecast Local Signing" certificate when it is in the login
# keychain (a stable identity, so macOS keeps Screen Recording / Accessibility
# grants across rebuilds), otherwise ad-hoc. Optional Developer ID signing +
# notarization (off unless these are set):
#   SIDECAST_SIGN_IDENTITY   e.g. "Developer ID Application: Jane Doe (TEAMID)"
#   SIDECAST_NOTARY_PROFILE  keychain profile created with `xcrun notarytool store-credentials`
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$MAC_DIR/.." && pwd)"
DIST="$REPO_ROOT/dist"
WORK="$MAC_DIR/.build/package"

APP_NAME="Sidecast"
BUNDLE_ID="com.achreftlili.sidecast"
VERSION="$(tr -d '[:space:]' < "$REPO_ROOT/VERSION")"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: VERSION must look like 1.2.3 (got '$VERSION')" >&2
    exit 1
fi
LOCAL_IDENTITY="Sidecast Local Signing"
SIGN_IDENTITY="${SIDECAST_SIGN_IDENTITY:-}"
if [[ -z "$SIGN_IDENTITY" ]]; then
    if security find-identity -p codesigning 2>/dev/null | grep -q "\"$LOCAL_IDENTITY\""; then
        SIGN_IDENTITY="$LOCAL_IDENTITY"
    else
        SIGN_IDENTITY="-"
    fi
fi
APP="$DIST/$APP_NAME.app"
DMG="$DIST/$APP_NAME-$VERSION.dmg"
ENTITLEMENTS="$SCRIPT_DIR/Sidecast.entitlements"

step() { printf '\n==> %s\n' "$*"; }

step "Building $APP_NAME $VERSION (release, arm64)"
cd "$MAC_DIR"
swift build -c release --arch arm64
BIN_DIR="$(swift build -c release --arch arm64 --show-bin-path)"

step "Rendering the app icon"
mkdir -p "$WORK"
xcrun swiftc -O "$SCRIPT_DIR/make-icon.swift" -o "$WORK/make-icon"
rm -rf "$WORK/AppIcon.iconset"
"$WORK/make-icon" "$WORK/AppIcon.iconset"
iconutil -c icns -o "$WORK/AppIcon.icns" "$WORK/AppIcon.iconset"

step "Assembling $APP"
mkdir -p "$DIST"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_DIR/$APP_NAME" "$APP/Contents/MacOS/$APP_NAME"
cp "$WORK/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
printf 'APPL????' > "$APP/Contents/PkgInfo"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>$APP_NAME</string>
	<key>CFBundleIdentifier</key>
	<string>$BUNDLE_ID</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>$APP_NAME</string>
	<key>CFBundleDisplayName</key>
	<string>$APP_NAME</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$VERSION</string>
	<key>CFBundleVersion</key>
	<string>$VERSION</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>LSMinimumSystemVersion</key>
	<string>15.0</string>
	<key>LSUIElement</key>
	<true/>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.utilities</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSLocalNetworkUsageDescription</key>
	<string>Sidecast listens on your local network so your Android phone can connect and act as an extra display.</string>
	<key>NSBonjourServices</key>
	<array>
		<string>_sidecast._tcp</string>
	</array>
	<key>NSAppleEventsUsageDescription</key>
	<string>Sidecast asks for your administrator password to install the rule that keeps the Mac awake while a phone is connected, and to install the Sidecast Microphone audio driver.</string>
	<key>NSHumanReadableCopyright</key>
	<string>Sidecast. Includes code from SideScreen (MIT).</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null

step "Building SidecastMic.driver (Sidecast Microphone) into the app"
# Signed with the same identity as the app, before the app itself is signed.
"$SCRIPT_DIR/build-mic-driver.sh" "$APP/Contents/Resources/SidecastMic.driver" "$SIGN_IDENTITY"

step "Signing ($([[ "$SIGN_IDENTITY" == "-" ]] && echo ad-hoc || echo "$SIGN_IDENTITY"))"
if [[ "$SIGN_IDENTITY" == "-" || "$SIGN_IDENTITY" == "$LOCAL_IDENTITY" ]]; then
    # Ad-hoc or the local self-signed identity: no hardened runtime, no timestamp.
    codesign --force --deep --sign "$SIGN_IDENTITY" --entitlements "$ENTITLEMENTS" "$APP"
else
    # ---- Optional: Developer ID (hardened runtime + secure timestamp) ----
    codesign --force --deep --options runtime --timestamp \
        --sign "$SIGN_IDENTITY" --entitlements "$ENTITLEMENTS" "$APP"
fi
codesign --verify --deep --strict "$APP"
codesign --verify --strict "$APP/Contents/Resources/SidecastMic.driver"

step "Creating $DMG"
STAGING="$WORK/dmg"
rm -rf "$STAGING"
mkdir -p "$STAGING"
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
rm -f "$DMG"
hdiutil create -volname "$APP_NAME $VERSION" -srcfolder "$STAGING" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
rm -rf "$STAGING"

# ---- Optional: notarization (needs a Developer ID identity) ----
if [[ -n "${SIDECAST_NOTARY_PROFILE:-}" ]]; then
    if [[ "$SIGN_IDENTITY" == "-" || "$SIGN_IDENTITY" == "$LOCAL_IDENTITY" ]]; then
        echo "error: SIDECAST_NOTARY_PROFILE requires SIDECAST_SIGN_IDENTITY (ad-hoc apps cannot be notarized)" >&2
        exit 1
    fi
    step "Notarizing $DMG"
    codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
    xcrun notarytool submit "$DMG" --keychain-profile "$SIDECAST_NOTARY_PROFILE" --wait
    xcrun stapler staple "$DMG"
fi

hdiutil verify "$DMG" >/dev/null
step "Done"
echo "  app: $APP"
echo "  dmg: $DMG"
echo "  probe (test client, not shipped): $BIN_DIR/sidecast-probe"
