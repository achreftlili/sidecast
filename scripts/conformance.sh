#!/usr/bin/env bash
# Cross-implementation conformance: every Sidecast probe against every host,
# all in test-pattern mode on this computer (no display, input or audio changes).
#
#   scripts/conformance.sh             Rust probe ↔ Rust host, and on macOS also
#                                      Swift probe ↔ Rust host and Rust probe ↔ Mac host
#   scripts/conformance.sh --rust-only Rust probe ↔ Rust host only (any OS)
#
# Ports: SIDECAST_RUST_PORT (default 47895) and SIDECAST_MAC_PORT (default 47896).
# The Mac host is dist/Sidecast.app (from `make mac`) when present, else the
# binary from `swift build`; SIDECAST_MAC_HOST overrides it. Exits 1 if any case fails.
set -uo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_PORT="${SIDECAST_RUST_PORT:-47895}"
MAC_PORT="${SIDECAST_MAC_PORT:-47896}"
CODE=123456

RUST_ONLY=0
for arg in "$@"; do
    case "$arg" in
        --rust-only) RUST_ONLY=1 ;;
        -h|--help) sed -n '2,11p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "unknown argument $arg" >&2; exit 2 ;;
    esac
done

case "$(uname -s)" in
    Darwin) OS=macos; EXE= ;;
    Linux) OS=linux; EXE= ;;
    MINGW*|MSYS*|CYGWIN*) OS=windows; EXE=.exe ;;
    *) echo "unsupported OS $(uname -s)" >&2; exit 2 ;;
esac
if [[ "$OS" != macos ]]; then RUST_ONLY=1; fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/sidecast-conformance.XXXXXX")"
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
PASSED=()
FAILED=()

# start_host NAME LOG COMMAND… : starts a host in the background and waits for its listener.
start_host() {
    local name="$1" log="$2"; shift 2
    "$@" >"$log" 2>&1 &
    PIDS+=("$!")
    local pid="$!"
    for _ in $(seq 1 100); do
        if grep -q "listening on port" "$log" 2>/dev/null; then
            echo "    $name is up (pid $pid, log $log)"
            return 0
        fi
        if ! kill -0 "$pid" 2>/dev/null; then break; fi
        sleep 0.2
    done
    echo "    $name did not start; its log:"
    sed 's/^/      /' "$log"
    return 1
}

stop_host() {
    local pid="${PIDS[${#PIDS[@]}-1]}"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    unset 'PIDS[${#PIDS[@]}-1]'
}

# run_case NAME [--expect-output TEXT] COMMAND… : one probe run; PASS = exit 0 (+ the text in its output).
run_case() {
    local name="$1"; shift
    local expect=""
    if [[ "${1:-}" == --expect-output ]]; then expect="$2"; shift 2; fi
    local log="$WORK/case-${#PASSED[@]}-${#FAILED[@]}.log"
    printf '  %-58s ' "$name"
    if "$@" >"$log" 2>&1 && { [[ -z "$expect" ]] || grep -qF -- "$expect" "$log"; }; then
        echo "PASS"
        PASSED+=("$name")
    else
        echo "FAIL (log: $log)"
        sed 's/^/      /' "$log" | tail -25
        FAILED+=("$name")
    fi
}

# A stored pairing whose token the host never issued: AUTH must fail with unknown_device + BYE.
forged_token_auth() {
    local probe="$1" port="$2" state="$3" forged="$WORK/forged-$RANDOM.json"
    local zeros
    zeros="$(head -c 32 /dev/zero | base64)"
    sed "s|\"token\" *: *\"[^\"]*\"|\"token\": \"$zeros\"|" "$state" >"$forged"
    "$probe" --host 127.0.0.1 --port "$port" --state "$forged" --expect-auth-failure unknown_device
}

# A phone that decodes only HEVC, on a host that encodes only H.264: BYE protocol_mismatch.
hevc_only_refused() {
    local probe="$1" port="$2" state="$3"
    ! "$probe" --host 127.0.0.1 --port "$port" --state "$state" --codec hevc --seconds 2
}

step "Building the Rust host and probe (release)"
(cd "$REPO_DIR/rust" && cargo build --release -p sidecast-host -p sidecast-probe) || exit 1
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_DIR/rust/target}"
RUST_HOST="$TARGET_DIR/release/sidecast-host$EXE"
RUST_PROBE="$TARGET_DIR/release/sidecast-probe$EXE"

# ---------------------------------------------------------------- Rust probe ↔ Rust host
step "Rust probe ↔ Rust host ($OS, port $RUST_PORT)"
start_host "Rust host" "$WORK/rust-host.log" "$RUST_HOST" --test-pattern --data-dir "$WORK/rust-host" \
    --port "$RUST_PORT" --test-pairing-code "$CODE" --no-mdns --test-mic-active || exit 1
P=(--host 127.0.0.1 --port "$RUST_PORT")
run_case "pairing, stream, mic, gestures, keyboard, keyframe request" \
    "$RUST_PROBE" "${P[@]}" --code "$CODE" --state "$WORK/rust-probe.json" --seconds 5 --expect-platform "$OS" \
    --expect-codec h264 --mic --keyframe-request --gesture tap --gesture tap2 --gesture swipe3-left \
    --gesture scroll2 --type "héllo 👋" --key 1 --key 6:1
run_case "AUTH with the stored token, trackpad mode + CURSOR" \
    "$RUST_PROBE" "${P[@]}" --state "$WORK/rust-probe.json" --seconds 4 --input-mode trackpad \
    --gesture trackpad-move --gesture trackpad-tap --gesture trackpad-doubletap-drag
run_case "data saver limits (≤ 30 fps)" \
    --expect-output "@30" "$RUST_PROBE" "${P[@]}" --state "$WORK/rust-probe.json" --seconds 3 --network metered --limits 4000:30
run_case "wrong pairing code → bad_code" \
    "$RUST_PROBE" "${P[@]}" --code 000000 --state "$WORK/rust-probe-wrong.json" --expect-auth-failure bad_code
run_case "five wrong codes → too_many_attempts + BYE" \
    "$RUST_PROBE" "${P[@]}" --code 000001,000002,000003,000004,000005 --state "$WORK/rust-probe-wrong2.json" \
    --expect-auth-failure too_many_attempts
run_case "unknown token → unknown_device + BYE" \
    forged_token_auth "$RUST_PROBE" "$RUST_PORT" "$WORK/rust-probe.json"
run_case "HEVC-only phone on an H.264-only host → BYE protocol_mismatch" \
    --expect-output "BYE protocol_mismatch" hevc_only_refused "$RUST_PROBE" "$RUST_PORT" "$WORK/rust-probe.json"
run_case "liveness: a silent client is dropped after ~5 s" \
    "$RUST_PROBE" "${P[@]}" --state "$WORK/rust-probe.json" --stall-after 2
stop_host

if (( RUST_ONLY == 0 )); then
    step "Building the Swift probe"
    SWIFT_BIN="$(cd "$REPO_DIR/mac" && swift build -c release --product sidecast-probe >/dev/null && swift build -c release --show-bin-path)" || exit 1
    SWIFT_PROBE="$SWIFT_BIN/sidecast-probe"

    # ------------------------------------------------------------ Swift probe ↔ Rust host
    step "Swift probe ↔ Rust host (port $RUST_PORT)"
    start_host "Rust host" "$WORK/rust-host-2.log" "$RUST_HOST" --test-pattern --data-dir "$WORK/rust-host-2" \
        --port "$RUST_PORT" --test-pairing-code "$CODE" --no-mdns --test-mic-active || exit 1
    run_case "pairing, stream decoded by VideoToolbox, mic, gestures" \
        --expect-output "platform $OS" "$SWIFT_PROBE" "${P[@]}" --code "$CODE" --state "$WORK/swift-probe.json" \
        --seconds 5 --mic --keyframe-request --gesture tap2 --gesture swipe3-up --type "héllo 👋" --key 1 --key 6:1
    run_case "AUTH with the stored token, trackpad mode + CURSOR" \
        "$SWIFT_PROBE" "${P[@]}" --state "$WORK/swift-probe.json" --seconds 4 --input-mode trackpad \
        --gesture trackpad-move --gesture trackpad-tap
    run_case "unknown token → unknown_device + BYE" \
        forged_token_auth "$SWIFT_PROBE" "$RUST_PORT" "$WORK/swift-probe.json"
    run_case "HEVC-only phone on an H.264-only host → BYE protocol_mismatch" \
        --expect-output "BYE protocol_mismatch" hevc_only_refused "$SWIFT_PROBE" "$RUST_PORT" "$WORK/swift-probe.json"
    run_case "wrong pairing code → bad_code" \
        "$SWIFT_PROBE" "${P[@]}" --code 000000 --state "$WORK/swift-probe-wrong.json" --expect-auth-failure bad_code
    run_case "liveness: a silent client is dropped after ~5 s" \
        "$SWIFT_PROBE" "${P[@]}" --state "$WORK/swift-probe.json" --stall-after 2
    stop_host

    # ------------------------------------------------------------ Rust probe ↔ Mac host
    step "Rust probe ↔ Mac host (port $MAC_PORT)"
    MAC_HOST="${SIDECAST_MAC_HOST:-}"
    if [[ -z "$MAC_HOST" && -x "$REPO_DIR/dist/Sidecast.app/Contents/MacOS/Sidecast" ]]; then
        MAC_HOST="$REPO_DIR/dist/Sidecast.app/Contents/MacOS/Sidecast"
    fi
    if [[ -z "$MAC_HOST" ]]; then
        (cd "$REPO_DIR/mac" && swift build -c release --product Sidecast) || exit 1
        MAC_HOST="$SWIFT_BIN/Sidecast"
    fi
    echo "    Mac host: $MAC_HOST"
    start_host "Mac host" "$WORK/mac-host.log" "$MAC_HOST" --test-pattern --data-dir "$WORK/mac-host" \
        --port "$MAC_PORT" --test-pairing-code "$CODE" --no-bonjour --test-mic-active || exit 1
    M=(--host 127.0.0.1 --port "$MAC_PORT")
    run_case "pairing, HEVC stream (structure), mic, gestures, keyboard" \
        "$RUST_PROBE" "${M[@]}" --code "$CODE" --state "$WORK/rust-probe-mac.json" --seconds 5 \
        --expect-platform macos --expect-codec hevc --mic --keyframe-request --gesture tap2 --gesture swipe3-left \
        --type "héllo 👋" --key 1 --key 6:1
    run_case "AUTH, H.264 stream decoded by OpenH264, trackpad + CURSOR" \
        "$RUST_PROBE" "${M[@]}" --state "$WORK/rust-probe-mac.json" --seconds 4 --codec h264 --expect-codec h264 \
        --input-mode trackpad --gesture trackpad-move --gesture trackpad-tap
    run_case "data saver limits (≤ 30 fps)" \
        --expect-output "@30" "$RUST_PROBE" "${M[@]}" --state "$WORK/rust-probe-mac.json" --seconds 3 --network metered --limits 4000:30
    run_case "wrong pairing code → bad_code" \
        "$RUST_PROBE" "${M[@]}" --code 000000 --state "$WORK/rust-probe-mac-wrong.json" --expect-auth-failure bad_code
    run_case "liveness: a silent client is dropped after ~5 s" \
        "$RUST_PROBE" "${M[@]}" --state "$WORK/rust-probe-mac.json" --stall-after 2
    stop_host
fi

step "Result: ${#PASSED[@]} passed, ${#FAILED[@]} failed (logs in $WORK)"
for name in "${FAILED[@]+"${FAILED[@]}"}"; do echo "  FAILED: $name"; done
(( ${#FAILED[@]} == 0 ))
