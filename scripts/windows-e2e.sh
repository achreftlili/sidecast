#!/usr/bin/env bash
# The Windows host with its real backends, end to end on this computer (CI runs
# it on the windows-latest runner; run it from Git Bash):
#   1. `sidecast-host --diagnose`: monitors, DXGI outputs, Virtual Display Driver,
#      Media Foundation encoders, audio devices;
#   2. the Rust probe pairs with a test-pattern host (fixed codes are test-only),
#      then the host restarts with the real backends on the same data directory
#      and the probe streams with its stored token: Desktop Duplication of a
#      monitor (the Virtual Display Driver's when installed, else mirroring),
#      a Media Foundation or OpenH264 encoder, frames decoded by the probe,
#      SendInput for taps, scrolls, keys and text, and CURSOR in trackpad mode.
#
# It moves the pointer and types into whatever window has focus: run it on a
# throwaway desktop. Skips step 2 (exit 0) when the session has no monitor that
# Desktop Duplication can capture. Port: SIDECAST_WIN_PORT (default 47897).
set -uo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${SIDECAST_WIN_PORT:-47897}"
CODE=123456

case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) ;;
    *) echo "windows-e2e.sh runs on Windows (Git Bash)"; exit 2 ;;
esac

WORK="$(mktemp -d "${TMPDIR:-/tmp}/sidecast-windows.XXXXXX")"
PIDS=()
cleanup() {
    # Only processes this script started, by PID.
    for pid in "${PIDS[@]+"${PIDS[@]}"}"; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT

step() { printf '\n==> %s\n' "$*"; }

# start_host LOG ARGS… : starts the host in the background and waits for its listener.
start_host() {
    local log="$1"; shift
    "$HOST" "$@" >"$log" 2>&1 &
    PIDS+=("$!")
    local pid="$!"
    for _ in $(seq 1 150); do
        if grep -q "listening on port" "$log" 2>/dev/null; then return 0; fi
        if ! kill -0 "$pid" 2>/dev/null; then break; fi
        sleep 0.2
    done
    echo "    the host did not start; its log:"
    sed 's/^/      /' "$log"
    return 1
}

stop_host() {
    local pid="${PIDS[${#PIDS[@]}-1]}"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    unset 'PIDS[${#PIDS[@]}-1]'
}

step "Building the host and the probe (release)"
(cd "$REPO_DIR/rust" && cargo build --release -p sidecast-host -p sidecast-probe) || exit 1
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_DIR/rust/target}"
HOST="$TARGET_DIR/release/sidecast-host.exe"
PROBE="$TARGET_DIR/release/sidecast-probe.exe"

step "What this computer offers (sidecast-host --diagnose)"
"$HOST" --diagnose | tee "$WORK/diagnose.txt" || exit 1

# A capturable monitor: an attached GDI monitor and a DXGI output for it.
if ! grep -q "^Outputs (DXGI" "$WORK/diagnose.txt" \
    || ! sed -n '/^Outputs (DXGI/,/^Virtual Display Driver/p' "$WORK/diagnose.txt" | grep -q "on "; then
    echo "SKIPPED: no DXGI output in this session, so nothing to capture (a service or remote session?)"
    exit 0
fi

step "Pairing the probe with a test-pattern host (port $PORT)"
start_host "$WORK/pairing-host.log" --test-pattern --data-dir "$WORK/host" --port "$PORT" \
    --test-pairing-code "$CODE" --no-mdns || exit 1
"$PROBE" --host 127.0.0.1 --port "$PORT" --code "$CODE" --state "$WORK/probe.json" --seconds 1 \
    >"$WORK/pairing-probe.log" 2>&1 || { cat "$WORK/pairing-probe.log"; exit 1; }
stop_host

step "Streaming from the real Windows backends"
start_host "$WORK/host.log" --data-dir "$WORK/host" --port "$PORT" --no-mdns --verbose || exit 1
FAILED=0
P=(--host 127.0.0.1 --port "$PORT" --state "$WORK/probe.json")
if ! "$PROBE" "${P[@]}" --seconds 6 --expect-platform windows --expect-codec h264 --keyframe-request \
        --no-input --gesture tap --gesture scroll2 --type "héllo 👋" --key 6 --key 7:1 >"$WORK/probe-touch.log" 2>&1; then
    echo "FAIL: stream with touch input"; FAILED=1
fi
sed 's/^/    probe | /' "$WORK/probe-touch.log" | tail -25
if ! "$PROBE" "${P[@]}" --seconds 4 --no-input --input-mode trackpad --gesture trackpad-move \
        --gesture trackpad-tap >"$WORK/probe-trackpad.log" 2>&1; then
    echo "FAIL: trackpad mode with CURSOR"; FAILED=1
fi
sed 's/^/    probe | /' "$WORK/probe-trackpad.log" | tail -12
stop_host

step "What the host did"
grep -E "display:|capture:|encoder:|stream: encoder|input:|power:|mic:|ERROR|WARN" "$WORK/host.log" | sed 's/^/    host | /' | head -80
for expected in "capture: duplicating" "stream: encoder" "power: keeping the computer"; do
    if ! grep -q "$expected" "$WORK/host.log"; then
        echo "FAIL: the host log has no \"$expected\""; FAILED=1
    fi
done
if grep -q "Windows blocked injected input" "$WORK/host.log"; then
    echo "FAIL: SendInput was blocked"; FAILED=1
fi
if (( FAILED )); then
    echo "Full host log:"; sed 's/^/    /' "$WORK/host.log"
    exit 1
fi
echo "PASS: the Windows host streamed a real monitor to the probe (logs in $WORK)"
