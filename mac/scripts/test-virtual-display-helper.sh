#!/bin/bash
# Stage 1 check for VirtualDisplayHelper: standalone, no Sidecast app involved.
#   mac/scripts/test-virtual-display-helper.sh
# Run it from a Terminal that has Screen Recording permission (for the ScreenCaptureKit step).
set -uo pipefail
MAC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$MAC_DIR"
swift build -c release --product VirtualDisplayHelper 2>&1 | tail -1
H="$(swift build -c release --show-bin-path)/VirtualDisplayHelper"
T="$(mktemp -d)"
cat > "$T/sck.swift" <<'SWIFT'
import ScreenCaptureKit
let want = UInt32(CommandLine.arguments[1])!
Task.detached {
    do {
        let c = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        let ids = c.displays.map { $0.displayID }
        print("ScreenCaptureKit display IDs: \(ids) -> target \(want) \(ids.contains(want) ? "FOUND" : "MISSING")")
    } catch { print("ScreenCaptureKit error: \(error)") }
    exit(0)
}
dispatchMain()
SWIFT
swiftc -O "$T/sck.swift" -o "$T/sck" 2>&1 | tail -3

# The helper lives while its stdin pipe is open; creating $T/stop ends the feeder, which closes the pipe.
( while [[ ! -e "$T/stop" ]]; do sleep 0.2; done ) | \
    "$H" --width 1280 --height 800 --pixel-width 2560 --pixel-height 1600 --refresh 90 --hidpi 1 \
         --serial 4242 --name Sidecast > "$T/out" 2> "$T/err" &
HP=$!
for _ in $(seq 1 30); do grep -qE '^(READY|ERROR)' "$T/out" 2>/dev/null && break; sleep 0.5; done
echo "== helper stdout:"; cat "$T/out"; echo "== helper diagnostics:"; cat "$T/err"
ID="$(awk '/^READY/{print $2}' "$T/out")"

# Run a command, killing it after 15 s.
limited() { "$@" & local p=$!; ( sleep 15; kill $p 2>/dev/null ) & local w=$!; wait $p 2>/dev/null; kill $w 2>/dev/null; }

if [[ -n "$ID" ]]; then
    echo "== system_profiler:"; system_profiler SPDisplaysDataType | grep -A3 -E "Sidecast|Resolution"
    echo "== ScreenCaptureKit (helper alive):"; limited "$T/sck" "$ID"
fi
touch "$T/stop"            # close the pipe: helper must exit and the display must vanish
wait "$HP" 2>/dev/null
echo "== helper exited"
sleep 1
if [[ -n "$ID" ]]; then echo "== ScreenCaptureKit (helper gone):"; limited "$T/sck" "$ID"; fi
rm -rf "$T"
