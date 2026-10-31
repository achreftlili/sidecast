#!/usr/bin/env bash
# Builds the iPhone / iPad app, installs it on the device connected by cable (or
# paired over Wi-Fi in Xcode), then opens it.
#
#   make install-ios
#   SIDECAST_DEVICE=<identifier>   pick a device when several are connected
#                                  (see `xcrun devicectl list devices`)
#
# Needs Xcode signed in to your Apple ID (Xcode → Settings → Accounts; a free
# Apple ID works) and ios/Local.xcconfig with your team and a bundle identifier
# (copy ios/Local.xcconfig.example). With a free Apple ID the app stops opening
# after 7 days: run this again to renew it (pairings are kept).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$IOS_DIR/.." && pwd)"
LOCAL="$IOS_DIR/Local.xcconfig"
VERSION="$(tr -d '[:space:]' < "$REPO_DIR/VERSION")"
DERIVED="$IOS_DIR/build/DerivedData"

die() { printf 'error: %s\n' "$1" >&2; shift; for line in "$@"; do printf '       %s\n' "$line" >&2; done; exit 1; }

if [[ "$(uname -s)" != Darwin ]]; then die "the iPhone / iPad app is built on a Mac with Xcode"; fi
if ! xcodebuild -version >/dev/null 2>&1; then
    die "Xcode is needed (the Command Line Tools alone cannot build iOS apps)." \
        "Install Xcode from the App Store, open it once, then: sudo xcode-select -s /Applications/Xcode.app"
fi
if ! xcrun devicectl --version >/dev/null 2>&1; then die "xcrun devicectl is missing: update Xcode (15 or later)."; fi

# Team and bundle identifier (every user needs their own).
if [[ ! -f "$LOCAL" ]]; then
    die "ios/Local.xcconfig is missing: it holds your Apple team and bundle identifier." \
        "cp ios/Local.xcconfig.example ios/Local.xcconfig, fill in both values, then run this again."
fi
setting() { sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\\([^[:space:]/]*\\).*/\\1/p" "$LOCAL" | tail -1; }
TEAM="$(setting DEVELOPMENT_TEAM)"
BUNDLE_ID="$(setting SIDECAST_BUNDLE_ID)"
if [[ ! "$TEAM" =~ ^[A-Z0-9]{10}$ || "$TEAM" == ABCDE12345 ]]; then
    die "set DEVELOPMENT_TEAM in ios/Local.xcconfig to your 10-character Apple team ID (got '${TEAM}')." \
        "Sign in to Xcode → Settings → Accounts with your Apple ID; the ID is in parentheses after" \
        "\"Apple Development\" in Keychain Access, or under Membership in your developer account."
fi
if [[ ! "$BUNDLE_ID" =~ ^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$ || "$BUNDLE_ID" == com.yourname.sidecast || "$BUNDLE_ID" == com.example.sidecast ]]; then
    die "set SIDECAST_BUNDLE_ID in ios/Local.xcconfig to an identifier of your own, e.g. com.<yourname>.sidecast (got '${BUNDLE_ID}')."
fi

# The device, before the (long) build.
DEVICES_JSON="$(mktemp -t sidecast-devices)"
trap 'rm -f "$DEVICES_JSON"' EXIT
xcrun devicectl list devices --quiet --json-output "$DEVICES_JSON" >/dev/null 2>&1 || true
# identifier <TAB> udid <TAB> developer mode <TAB> name, for paired, reachable iPhones and iPads.
DEVICES=()
while IFS= read -r line; do DEVICES+=("$line"); done < <(/usr/bin/python3 -I - "$DEVICES_JSON" <<'PY'
import json, sys
try:
    devices = json.load(open(sys.argv[1]))["result"]["devices"]
except Exception:
    devices = []
for d in devices:
    hw, conn, props = d.get("hardwareProperties", {}), d.get("connectionProperties", {}), d.get("deviceProperties", {})
    if hw.get("platform") != "iOS" or hw.get("reality") != "physical": continue
    if conn.get("pairingState") != "paired" or conn.get("tunnelState") == "unavailable": continue
    print("\t".join([d.get("identifier", ""), hw.get("udid", ""), props.get("developerModeStatus", "unknown"),
                     props.get("name", "device")]))
PY
)
if [[ -n "${SIDECAST_DEVICE:-}" ]]; then
    MATCH=()
    for d in ${DEVICES[@]+"${DEVICES[@]}"}; do
        IFS=$'\t' read -r id udid _ name <<<"$d"
        if [[ "$SIDECAST_DEVICE" == "$id" || "$SIDECAST_DEVICE" == "$udid" || "$SIDECAST_DEVICE" == "$name" ]]; then MATCH+=("$d"); fi
    done
    if (( ${#MATCH[@]} == 0 )); then die "$SIDECAST_DEVICE is not connected (see 'xcrun devicectl list devices')."; fi
    DEVICES=("${MATCH[0]}")
fi
if (( ${#DEVICES[@]} == 0 )); then
    die "no iPhone or iPad found. Connect it with a cable, unlock it and tap Trust on it," \
        "then check that it shows up in Xcode → Window → Devices and Simulators, and run this again."
elif (( ${#DEVICES[@]} > 1 )); then
    printf 'error: several devices are connected; choose one with SIDECAST_DEVICE=<identifier>:\n' >&2
    for d in "${DEVICES[@]}"; do IFS=$'\t' read -r id _ _ name <<<"$d"; printf '         %s  %s\n' "$id" "$name" >&2; done
    exit 1
fi
IFS=$'\t' read -r DEVICE_ID DEVICE_UDID DEV_MODE DEVICE_NAME <<<"${DEVICES[0]}"
if [[ "$DEV_MODE" == disabled ]]; then
    die "Developer Mode is off on $DEVICE_NAME." \
        "On the device: Settings → Privacy & Security → Developer Mode → On, restart it, then run this again."
fi

printf '\n==> Building Sidecast %s for %s (team %s, %s)\n' "$VERSION" "$DEVICE_NAME" "$TEAM" "$BUNDLE_ID"
# -allowProvisioningUpdates registers the device and creates the signing profile
# with the Apple ID signed in to Xcode.
if ! xcodebuild -project "$IOS_DIR/Sidecast.xcodeproj" -scheme Sidecast -configuration Release \
        -destination "id=$DEVICE_UDID" -derivedDataPath "$DERIVED" -allowProvisioningUpdates \
        MARKETING_VERSION="$VERSION" -quiet build; then
    die "the build failed. If it is about signing: open Xcode → Settings → Accounts, check that your" \
        "Apple ID is signed in, and that DEVELOPMENT_TEAM / SIDECAST_BUNDLE_ID in ios/Local.xcconfig are right." \
        "A free Apple ID can sign a limited number of new bundle identifiers per week."
fi
APP="$DERIVED/Build/Products/Release-iphoneos/Sidecast.app"
[[ -d "$APP" ]] || die "build output not found: $APP"

printf '\n==> Installing on %s\n' "$DEVICE_NAME"
xcrun devicectl device install app --device "$DEVICE_ID" "$APP"

printf '\n==> Opening Sidecast\n'
if ! xcrun devicectl device process launch --device "$DEVICE_ID" "$BUNDLE_ID" >/dev/null 2>&1; then
    echo "Installed, but iOS did not open it. The first time, trust the developer on the device:"
    echo "  Settings → General → VPN & Device Management → your Apple ID → Trust, then open Sidecast."
fi
echo "Installed Sidecast $VERSION on $DEVICE_NAME."
echo "With a free Apple ID it stops opening after 7 days: run 'make install-ios' again (pairings are kept)."
