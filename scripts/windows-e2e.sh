#!/usr/bin/env bash
# The Windows host with its real backends, end to end on this computer (CI runs
# it on windows-latest runners; run it from Git Bash):
#   1. `sidecast-host --diagnose`: monitors, DXGI outputs, Virtual Display Driver,
#      Media Foundation encoders, audio devices;
#   2. the Rust probe pairs with a test-pattern host (fixed codes are test-only),
#      then the host restarts with the real backends on the same data directory
#      and the probe streams with its stored token: Desktop Duplication of a
#      monitor (the Virtual Display Driver's when installed, else mirroring),
#      a Media Foundation or OpenH264 encoder, frames decoded by the probe,
#      SendInput for taps, scrolls, keys and text, and CURSOR in trackpad mode.
#
#   scripts/windows-e2e.sh                          the steps above
#   scripts/windows-e2e.sh --expect-virtual-display also require that the stream used a
#                                                   Virtual Display Driver monitor and put it back
#   scripts/windows-e2e.sh --tray                   also start the tray app in test-pattern mode,
#                                                   pair with it and quit it with --quit
#
# It moves the pointer and types into whatever window has focus: run it on a
# throwaway desktop. Skips step 2 (exit 0) when the session has no monitor that
# Desktop Duplication can capture. Ports: SIDECAST_WIN_PORT (default 47897) and
# the next one for the tray app.
set -uo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${SIDECAST_WIN_PORT:-47897}"
TRAY_PORT=$((PORT + 1))
CODE=123456

EXPECT_VDD=0
TRAY=0
for arg in "$@"; do
    case "$arg" in
        --expect-virtual-display) EXPECT_VDD=1 ;;
        --tray) TRAY=1 ;;
        -h|--help) sed -n '2,24p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "unknown argument $arg" >&2; exit 2 ;;
    esac
done

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
FAILED=0
fail() { echo "FAIL: $*"; FAILED=1; }

# wait_for_line FILE TEXT PID : waits (≤ 30 s) until FILE contains TEXT, while PID lives.
wait_for_line() {
    local file="$1" text="$2" pid="$3"
    for _ in $(seq 1 150); do
        if grep -qF -- "$text" "$file" 2>/dev/null; then return 0; fi
        if ! kill -0 "$pid" 2>/dev/null; then break; fi
        sleep 0.2
    done
    return 1
}

# start_host LOG ARGS… : starts the console host in the background and waits for its listener.
start_host() {
    local log="$1"; shift
    "$HOST" "$@" >"$log" 2>&1 &
    PIDS+=("$!")
    if ! wait_for_line "$log" "listening on port" "$!"; then
        echo "    the host did not start; its log:"
        sed 's/^/      /' "$log"
        return 1
    fi
}

stop_host() {
    local pid="${PIDS[${#PIDS[@]}-1]}"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    unset 'PIDS[${#PIDS[@]}-1]'
}

step "Building the host, the tray app and the probe (release)"
(cd "$REPO_DIR/rust" && cargo build --release -p sidecast-host -p sidecast-probe) || exit 1
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_DIR/rust/target}"
HOST="$TARGET_DIR/release/sidecast-host.exe"
TRAY_EXE="$TARGET_DIR/release/sidecast-tray.exe"
PROBE="$TARGET_DIR/release/sidecast-probe.exe"

step "What this computer offers (sidecast-host --diagnose)"
"$HOST" --diagnose | tee "$WORK/diagnose.txt" || exit 1

if (( TRAY )); then
    step "Tray app (test pattern, port $TRAY_PORT): starts, shows a pairing, quits on request"
    TRAY_LOG="$(cygpath -u "${LOCALAPPDATA:-$TEMP}")/Sidecast/sidecast.log"
    rm -f "$TRAY_LOG"
    "$TRAY_EXE" --test-pattern --data-dir "$WORK/tray" --port "$TRAY_PORT" --test-pairing-code "$CODE" --no-mdns &
    TRAY_PID="$!"
    PIDS+=("$TRAY_PID")
    if ! wait_for_line "$TRAY_LOG" "listening on port $TRAY_PORT" "$TRAY_PID"; then
        fail "the tray app did not start"
    elif ! "$PROBE" --host 127.0.0.1 --port "$TRAY_PORT" --code "$CODE" --state "$WORK/tray-probe.json" \
            --seconds 2 --expect-platform windows >"$WORK/tray-probe.log" 2>&1; then
        sed 's/^/    probe | /' "$WORK/tray-probe.log" | tail -15
        fail "the probe could not pair with and stream from the tray app"
    elif ! "$TRAY_EXE" --quit; then
        fail "sidecast-tray --quit did not make the tray app quit"
    else
        for _ in $(seq 1 50); do kill -0 "$TRAY_PID" 2>/dev/null || break; sleep 0.2; done
        if kill -0 "$TRAY_PID" 2>/dev/null; then fail "the tray app is still running after --quit"; fi
        grep -q "tray: quitting" "$TRAY_LOG" || fail "the tray app did not shut its server down"
        grep -q "pairing started with" "$TRAY_LOG" || fail "the tray app saw no pairing"
    fi
    sed 's/^/    tray | /' "$TRAY_LOG" 2>/dev/null | grep -E "tray:|pairing|streaming|ERROR|WARN" | head -20
fi

# A capturable monitor: an attached GDI monitor and a DXGI output for it.
if ! sed -n '/^Outputs (DXGI/,/^Virtual Display Driver/p' "$WORK/diagnose.txt" | grep -q " on "; then
    if (( EXPECT_VDD )); then
        echo "FAIL: no DXGI output, but a virtual display was expected"; exit 1
    fi
    echo "SKIPPED: no DXGI output in this session, so nothing to capture (a service or remote session?)"
    exit "$FAILED"
fi

step "Pairing the probe with a test-pattern host (port $PORT)"
start_host "$WORK/pairing-host.log" --test-pattern --data-dir "$WORK/host" --port "$PORT" \
    --test-pairing-code "$CODE" --no-mdns || exit 1
"$PROBE" --host 127.0.0.1 --port "$PORT" --code "$CODE" --state "$WORK/probe.json" --seconds 1 \
    >"$WORK/pairing-probe.log" 2>&1 || { cat "$WORK/pairing-probe.log"; exit 1; }
stop_host

step "Streaming from the real Windows backends"
start_host "$WORK/host.log" --data-dir "$WORK/host" --port "$PORT" --no-mdns --verbose || exit 1
P=(--host 127.0.0.1 --port "$PORT" --state "$WORK/probe.json")
if ! "$PROBE" "${P[@]}" --seconds 6 --expect-platform windows --expect-codec h264 --keyframe-request \
        --no-input --gesture tap --gesture scroll2 --type "héllo 👋" --key 6 --key 7:1 >"$WORK/probe-touch.log" 2>&1; then
    fail "stream with touch input"
fi
sed 's/^/    probe | /' "$WORK/probe-touch.log" | tail -25
if ! "$PROBE" "${P[@]}" --seconds 4 --no-input --input-mode trackpad --gesture trackpad-move \
        --gesture trackpad-tap >"$WORK/probe-trackpad.log" 2>&1; then
    fail "trackpad mode with CURSOR"
fi
sed 's/^/    probe | /' "$WORK/probe-trackpad.log" | tail -12
# Let the last stream's teardown (and the virtual monitor's restore) finish.
sleep 3
stop_host

step "What the host did"
grep -E "display:|capture:|encoder:|stream: encoder|input:|power:|mic:|ERROR|WARN" "$WORK/host.log" | sed 's/^/    host | /' | head -80
for expected in "capture: duplicating" "stream: encoder" "power: keeping the computer"; do
    grep -q "$expected" "$WORK/host.log" || fail "the host log has no \"$expected\""
done
if grep -q "Windows blocked injected input" "$WORK/host.log"; then
    fail "SendInput was blocked"
fi
if (( EXPECT_VDD )); then
    grep -q "display: virtual monitor .* attached at" "$WORK/host.log" || fail "the stream did not use a virtual monitor"
    grep -Eq "display: virtual monitor .* (detached|back to its previous mode)" "$WORK/host.log" \
        || fail "the virtual monitor was not put back after the stream"
    step "Displays afterwards"
    "$HOST" --diagnose | sed -n '/^Monitors/,/^Outputs/p'
fi
if (( FAILED )); then
    echo "Full host log:"; sed 's/^/    /' "$WORK/host.log"
    exit 1
fi
echo "PASS: the Windows host streamed a real monitor to the probe (logs in $WORK)"
