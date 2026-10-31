#!/usr/bin/env bash
# End-to-end test of the Linux host's real GNOME backend, without a GPU or a
# phone: a headless GNOME (Mutter + PipeWire + WirePlumber on a private D-Bus
# session bus) runs the host, and sidecast-probe plays the phone.
#
#   scripts/linux-e2e.sh            builds the host and probe (release), then runs
#   SIDECAST_E2E_HOST=… SIDECAST_E2E_PROBE=… scripts/linux-e2e.sh   uses given binaries
#
# Checks: pairing (the code is read from the host's desktop notification),
# a stream from Mutter's virtual monitor decoded by OpenH264 with a non-blank
# picture, keyframe requests, touches/keys/text accepted by Mutter's
# RemoteDesktop (the pointer metadata follows the injected taps; a Wayland
# client receives the keys), trackpad CURSOR from the compositor, the PipeWire
# microphone (an app records the phone's 440 Hz tone), the tray menu and Quit.
#
# Needs (Ubuntu): mutter pipewire pipewire-bin wireplumber dbus-daemon python3 python3-gi
# and, for the keyboard check, wev. Ports: SIDECAST_E2E_PORT (default 47890).
# Only processes this script starts are stopped (by PID). Exits 1 if a check fails.
set -uo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -z "${SIDECAST_E2E_INNER:-}" ]]; then
    # A private session bus: Mutter, PipeWire and the host must share it.
    exec env SIDECAST_E2E_INNER=1 dbus-run-session -- "${BASH_SOURCE[0]}" "$@"
fi

PORT="${SIDECAST_E2E_PORT:-47890}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/sidecast-linux-e2e.XXXXXX")"
export XDG_RUNTIME_DIR="$WORK/runtime"
mkdir -m 700 -p "$XDG_RUNTIME_DIR"
export GSETTINGS_BACKEND=memory
unset WAYLAND_DISPLAY DISPLAY XDG_SESSION_TYPE

PIDS=()
cleanup() {
    for ((i = ${#PIDS[@]} - 1; i >= 0; i--)); do
        kill "${PIDS[$i]}" 2>/dev/null || true
        wait "${PIDS[$i]}" 2>/dev/null || true
    done
}
trap cleanup EXIT

step() { printf '\n==> %s\n' "$*"; }
PASSED=()
FAILED=()
check() { # check NAME COMMAND…
    local name="$1"; shift
    if "$@"; then echo "  PASS  $name"; PASSED+=("$name"); else echo "  FAIL  $name"; FAILED+=("$name"); fi
}
wait_for() { # wait_for SECONDS COMMAND…
    local deadline=$((SECONDS + $1)); shift
    until "$@" 2>/dev/null; do
        if ((SECONDS >= deadline)); then return 1; fi
        sleep 0.2
    done
}
on_bus() { busctl --user list 2>/dev/null | grep -q "^$1 "; }

HOST_BIN="${SIDECAST_E2E_HOST:-}"
PROBE_BIN="${SIDECAST_E2E_PROBE:-}"
if [[ -z "$HOST_BIN" || -z "$PROBE_BIN" ]]; then
    step "Building the host and the probe (release)"
    (cd "$REPO_DIR/rust" && cargo build --release -p sidecast-host -p sidecast-probe) || exit 1
    TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_DIR/rust/target}"
    HOST_BIN="$TARGET_DIR/release/sidecast-host"
    PROBE_BIN="$TARGET_DIR/release/sidecast-probe"
fi

step "Headless GNOME (work dir $WORK)"
pipewire >"$WORK/pipewire.log" 2>&1 &
PIDS+=("$!")
wait_for 10 test -S "$XDG_RUNTIME_DIR/pipewire-0" || { echo "PipeWire did not start"; cat "$WORK/pipewire.log"; exit 1; }
wireplumber >"$WORK/wireplumber.log" 2>&1 &
PIDS+=("$!")
mutter --headless --wayland --no-x11 --virtual-monitor 1280x720 >"$WORK/mutter.log" 2>&1 &
PIDS+=("$!")
wait_for 30 on_bus org.gnome.Mutter.ScreenCast || { echo "Mutter did not start"; cat "$WORK/mutter.log"; exit 1; }
wait_for 10 test -S "$XDG_RUNTIME_DIR/wayland-0" || { echo "no Wayland socket"; exit 1; }
echo "    $(mutter --version 2>/dev/null | head -1), $(pipewire --version 2>/dev/null | tail -1)"

CODE_FILE="$WORK/pairing-code"
if python3 -c 'import gi' 2>/dev/null; then
    python3 "$REPO_DIR/scripts/linux-e2e-desktop.py" "$CODE_FILE" >"$WORK/desktop.log" 2>&1 &
    PIDS+=("$!")
    wait_for 10 on_bus org.freedesktop.Notifications || echo "    (no stand-in notification daemon)"
    HAVE_DESKTOP=1
else
    echo "    python3-gi missing: the pairing code comes from the host log instead of a notification"
    HAVE_DESKTOP=0
fi

HAVE_WEV=0
if command -v wev >/dev/null; then
    # A Wayland client with keyboard focus: it prints every key Mutter delivers.
    WAYLAND_DISPLAY=wayland-0 stdbuf -oL wev >"$WORK/wev.log" 2>&1 &
    PIDS+=("$!")
    wait_for 10 grep -q "xdg_toplevel.*configure: width: [1-9]" "$WORK/wev.log" && HAVE_WEV=1
fi

step "sidecast-host with the GNOME backend (port $PORT)"
"$HOST_BIN" --data-dir "$WORK/host" --port "$PORT" --no-mdns --verbose >"$WORK/host.log" 2>&1 &
HOST_PID=$!
PIDS+=("$HOST_PID")
if ! wait_for 20 grep -q "listening on port" "$WORK/host.log"; then
    echo "the host did not start:"; cat "$WORK/host.log"; exit 1
fi
grep -E "linux: |encoder" "$WORK/host.log" | sed 's/^/    /'
if ((HAVE_DESKTOP == 0)); then
    # Fallback: copy the code from the log line "PAIRING: enter 123456 on …".
    ( while ! grep -q "PAIRING: enter" "$WORK/host.log"; do sleep 0.2; done
      grep -o "PAIRING: enter [0-9]\{6\}" "$WORK/host.log" | head -1 | grep -o "[0-9]\{6\}" >"$CODE_FILE" ) &
    PIDS+=("$!")
fi

probe() { "$PROBE_BIN" --host 127.0.0.1 --port "$PORT" --state "$WORK/probe.json" "$@"; }

step "1. Pairing, stream, keyframe request, keyboard and touch"
# The first key (Escape) only wakes the headless seat's keyboard: a GNOME without a
# physical keyboard announces one to clients when the first virtual key arrives,
# and that key itself is not delivered. A real desktop always has a keyboard.
probe --code-file "$CODE_FILE" --seconds 8 --expect-platform linux --expect-codec h264 --keyframe-request \
    --save-frame "$WORK/frame.pgm" --key 5 --type "sidecast" --key 1 --key 6:1 --type "é" \
    --gesture tap --gesture scroll2 --gesture swipe3-left >"$WORK/probe-1.log" 2>&1
PROBE_1=$?
sed 's/^/    /' "$WORK/probe-1.log" | grep -E "SERVER_HELLO|STREAM_CONFIG|SUMMARY|keyframe arrived|FAILURE|RESULT|VIOLATION" | head -20
check "probe: paired, frames decoded, no violation" test "$PROBE_1" -eq 0
if ((HAVE_DESKTOP)); then
    check "pairing code shown as a desktop notification" grep -q "pairing code [0-9]\{6\} written" "$WORK/desktop.log"
fi
check "Mutter created the virtual monitor (2400x1080)" grep -q "mutter: virtual monitor 2400x1080" "$WORK/host.log"
check "PipeWire delivered frames (format negotiated)" grep -q "capture: PipeWire format" "$WORK/host.log"
picture_ok() {
    # Mutter alone (no GNOME Shell) draws a grey gradient; the pointer is
    # composited from the cursor metadata where Mutter last reported it.
    python3 - "$WORK/frame.pgm" "$WORK/host.log" <<'EOF'
import re, sys
data = open(sys.argv[1], "rb").read()
_, size, _, pixels = data.split(b"\n", 3)
w, h = map(int, size.split())
background = sum(1 for p in pixels if 40 <= p <= 110) / len(pixels)
seen = re.findall(r"capture: pointer at Some\(\((-?\d+), (-?\d+)\)\)", open(sys.argv[2]).read())
px, py = (int(v) for v in seen[-1]) if seen else (-1, -1)
cx, cy = px * w // 2400, py * h // 1080  # stream pixels → encoded picture
near = [pixels[y * w + x] for y in range(max(cy - 4, 0), min(cy + 40, h)) for x in range(max(cx - 4, 0), min(cx + 30, w))]
dark, light = sum(1 for p in near if p < 35), sum(1 for p in near if p > 180)
print(f"    decoded {w}x{h}: {background:.0%} desktop background; pointer at ({cx}, {cy}): {dark} dark, {light} light pixels")
sys.exit(0 if (w, h) == (1920, 864) and background > 0.95 and dark >= 10 and light >= 10 else 1)
EOF
}
check "decoded picture: Mutter's desktop with the pointer drawn where Mutter reports it" picture_ok
check "Mutter accepted the input (no failed calls)" bash -c "! grep -q 'input: .* failed' '$WORK/host.log'"
check "touch → button press and release" bash -c "grep -q 'input: button 0x110 down' '$WORK/host.log' && grep -q 'input: button 0x110 up' '$WORK/host.log'"
check "3-finger swipe left → Super+Page Down (keycodes 125, 109)" \
    bash -c "grep -q 'input: keycode 125 down' '$WORK/host.log' && grep -q 'input: keycode 109 down' '$WORK/host.log'"
check "scroll → smooth axis events with a finish" grep -q "input: scroll (0.0, 0.0) finish" "$WORK/host.log"
pointer_follows() {
    # The last injected motion and the pointer Mutter reports in the stream's metadata agree.
    python3 - "$WORK/host.log" <<'EOF'
import re, sys
log = open(sys.argv[1]).read()
moves = re.findall(r"input: motion to \(([\d.]+), ([\d.]+)\) px", log)
seen = re.findall(r"capture: pointer at Some\(\((-?\d+), (-?\d+)\)\)", log)
if not moves or not seen:
    print(f"    motions {len(moves)}, pointer reports {len(seen)}"); sys.exit(1)
mx, my = map(float, moves[-1]); px, py = map(int, seen[-1])
print(f"    last injected motion ({mx:.0f}, {my:.0f}) px, pointer reported at ({px}, {py}) px")
sys.exit(0 if abs(mx - px) <= 2 and abs(my - py) <= 2 else 1)
EOF
}
check "the compositor's pointer follows the injected taps" pointer_follows
if ((HAVE_WEV)); then
    keys_typed() {
        # wev prints "key: …; state: 1 (pressed)" then "sym: s …": the keysyms of key presses, in order.
        local typed
        typed="$(awk '/state: 1 \(pressed\)/ { pressed = 1; next } pressed && /sym:/ { print $2; pressed = 0 }' \
            "$WORK/wev.log" | tr '\n' ' ')"
        echo "    key presses seen by a Wayland client: $typed"
        [[ "$typed" == *"s i d e c a s t Return Shift_L Left"* ]] && [[ "$typed" =~ Control_L\ Shift_L\ [uU]\ e\ 9\ space ]]
    }
    check "keys and text reach a Wayland client (TEXT, KEY, Ctrl+Shift+U for é)" keys_typed
fi

step "2. AUTH with the stored token, trackpad mode, CURSOR from the compositor"
probe --seconds 4 --input-mode trackpad --gesture trackpad-move --gesture trackpad-tap >"$WORK/probe-2.log" 2>&1
PROBE_2=$?
grep -E "CURSOR received|RESULT|FAILURE" "$WORK/probe-2.log" | sed 's/^/    /'
check "probe: authenticated, trackpad, no violation" test "$PROBE_2" -eq 0
check "CURSOR positions received" grep -q "CURSOR received [1-9]" "$WORK/probe-2.log"

step "3. Microphone: an app records from “Sidecast Microphone”"
probe --seconds 9 --mic --no-input >"$WORK/probe-3.log" 2>&1 &
PROBE_3_PID=$!
PIDS+=("$PROBE_3_PID")
if wait_for 15 bash -c "pw-cli ls Node 2>/dev/null | grep -q sidecast-microphone"; then
    timeout 4 pw-record --target sidecast-microphone --rate 48000 --channels 1 "$WORK/mic.wav" >"$WORK/pw-record.log" 2>&1
fi
wait "$PROBE_3_PID"
PROBE_3=$?
grep -E "MIC_CONTROL|MIC controls|RESULT|FAILURE" "$WORK/probe-3.log" | sed 's/^/    /'
check "probe: mic session, no violation" test "$PROBE_3" -eq 0
check "recording started and stopped the phone's mic (MIC_CONTROL on, off)" grep -q "MIC controls received \[on,off\]" "$WORK/probe-3.log"
tone_recorded() {
    python3 - "$WORK/mic.wav" <<'EOF'
import sys, wave, struct
w = wave.open(sys.argv[1])
rate, n, width = w.getframerate(), w.getnframes(), w.getsampwidth()
raw = w.readframes(n)
if width == 2:
    samples = struct.unpack(f"<{len(raw)//2}h", raw)
elif width == 4:
    samples = struct.unpack(f"<{len(raw)//4}f", raw)
else:
    sys.exit(1)
peak = max(abs(s) for s in samples) if samples else 0
loud = [i for i, s in enumerate(samples) if abs(s) > peak / 4]
if not loud:
    print("    silence"); sys.exit(1)
part = samples[loud[0]:loud[-1]]
crossings = sum(1 for a, b in zip(part, part[1:]) if (a < 0) != (b < 0))
hz = crossings / 2 / (len(part) / rate) if part else 0
print(f"    recorded {n / rate:.1f} s at {rate} Hz, {len(part) / rate:.1f} s of tone at ≈{hz:.0f} Hz")
sys.exit(0 if 420 <= hz <= 460 and len(part) / rate >= 0.5 else 1)
EOF
}
check "the recording contains the phone's 440 Hz tone" tone_recorded

step "4. Tray menu (dbusmenu) and Quit"
TRAY_NAME="org.kde.StatusNotifierItem-$HOST_PID-1"
if ((HAVE_DESKTOP)); then
    check "tray icon registered with the StatusNotifierWatcher" grep -q "tray item registered: $TRAY_NAME" "$WORK/desktop.log"
fi
gdbus call --session --dest "$TRAY_NAME" --object-path /MenuBar --method com.canonical.dbusmenu.GetLayout 0 'int32 -1' '@as []' \
    >"$WORK/menu.txt" 2>&1
grep -o "'label': <'[^']*'>" "$WORK/menu.txt" | sed "s/^/    /"
check "menu: status, a Forget item for the paired probe, Quit" \
    bash -c "grep -q 'Waiting for a phone' '$WORK/menu.txt' && grep -q 'Forget' '$WORK/menu.txt' && grep -q 'Quit Sidecast' '$WORK/menu.txt'"
gdbus call --session --dest "$TRAY_NAME" --object-path /MenuBar --method com.canonical.dbusmenu.Event \
    6 clicked '<int32 0>' 0 >/dev/null 2>&1
quit_by_tray() { wait_for 10 bash -c "! kill -0 $HOST_PID" && grep -q "Quit chosen in the tray" "$WORK/host.log"; }
check "Quit in the tray stops the host" quit_by_tray

step "Result: ${#PASSED[@]} passed, ${#FAILED[@]} failed (logs in $WORK)"
for name in "${FAILED[@]+"${FAILED[@]}"}"; do echo "  FAILED: $name"; done
if ((${#FAILED[@]})); then
    echo; echo "---- host log (tail)"; tail -60 "$WORK/host.log"
    echo; echo "---- mutter log (tail)"; tail -20 "$WORK/mutter.log"
fi
(( ${#FAILED[@]} == 0 ))
