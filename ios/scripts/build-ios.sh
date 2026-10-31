#!/usr/bin/env bash
# Builds the iPhone / iPad app for the iOS Simulator without signing (no Apple
# ID needed), and with --test also runs its unit tests on a simulator.
#
#   make ios        (build only)
#   make test-ios   (build and unit tests; CI does this)
#   SIDECAST_SIMULATOR=<name or udid>   simulator for the tests (default: an available iPhone)
#
# The end-to-end UI test (SidecastUITests) needs a running host; see the
# comment at the top of ios/SidecastUITests/EndToEndUITests.swift.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DERIVED="$IOS_DIR/build/DerivedData"
PROJECT=(-project "$IOS_DIR/Sidecast.xcodeproj" -scheme Sidecast -derivedDataPath "$DERIVED")
NO_SIGNING=(CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO)

if ! xcodebuild -version >/dev/null 2>&1; then
    echo "error: Xcode is needed to build the iOS app (the Command Line Tools are not enough)." >&2
    exit 1
fi

printf '==> Building Sidecast for the iOS Simulator (unsigned)\n'
xcodebuild "${PROJECT[@]}" -destination 'generic/platform=iOS Simulator' "${NO_SIGNING[@]}" -quiet build

[[ "${1:-}" == --test ]] || exit 0

# An available iPhone simulator of the newest runtime, unless one is named.
SIMULATOR="$(/usr/bin/python3 -I - "${SIDECAST_SIMULATOR:-}" <<'PY'
import json, subprocess, sys
wanted = sys.argv[1]
data = json.loads(subprocess.run(["xcrun", "simctl", "list", "devices", "available", "-j"],
                                 capture_output=True, text=True, check=True).stdout)
best = None
for runtime, devices in data["devices"].items():
    if ".iOS-" not in runtime:
        continue
    version = tuple(int(p) for p in runtime.rsplit(".iOS-", 1)[1].split("-") if p.isdigit())
    for d in devices:
        if wanted and wanted not in (d["name"], d["udid"]):
            continue
        if not wanted and not d["name"].startswith("iPhone"):
            continue
        if best is None or version > best[0]:
            best = (version, d["udid"], d["name"])
print(best[1] if best else "")
PY
)"
if [[ -z "$SIMULATOR" ]]; then
    echo "error: no ${SIDECAST_SIMULATOR:-iPhone} simulator available (Xcode → Settings → Components → iOS)." >&2
    exit 1
fi
printf '==> Unit tests on simulator %s\n' "$SIMULATOR"
xcodebuild "${PROJECT[@]}" -destination "id=$SIMULATOR" "${NO_SIGNING[@]}" -only-testing:SidecastTests -quiet test
