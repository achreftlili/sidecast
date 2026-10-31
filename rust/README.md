# Sidecast in Rust: the Windows and Linux host

A Cargo workspace with the shared host core for Windows and Linux (ROADMAP P1),
and a conformance client for every host. It speaks [PROTOCOL.md](../docs/PROTOCOL.md)
exactly like the Mac app, so the Android app works with it unchanged.

| Crate | What it is |
|---|---|
| `sidecast-core` | Pure logic, no I/O: framing and every message, pairing (HMAC proof, codes, tokens), the paired-device model, flow control and adaptive bitrate, stream planning, the gesture engine, keyboard mapping, the microphone jitter buffer, address classification. A port of the Mac's `SidecastKit` with its tests. |
| `sidecast-server` | The host: TLS (rustls, ring), DNS-SD, the session state machine, streaming with flow control, input timing, microphone routing, persistence. Platform code plugs in through the traits in [`backend.rs`](sidecast-server/src/backend.rs). Optional software H.264 encoder (`openh264` feature). |
| `sidecast-host` | The host program: `sidecast-host` on the command line (`--test-pattern` on any OS) and, on Windows, the `sidecast-tray` notification-area app. The Windows backends are in `src/platform/windows/` (experimental, DECISIONS Q37), the Linux backends and the Linux tray icon in `src/platform/linux/` (experimental, GNOME on Wayland, DECISIONS Q38). |
| `sidecast-probe` | A scripted fake phone that checks a host against the protocol and exits non-zero on any violation. |

## Build and test

You need Rust (stable, via [rustup](https://rustup.rs)) and a C/C++ compiler for
OpenH264 and ring (Xcode Command Line Tools, `build-essential`, or Visual Studio
Build Tools). `make doctor` checks it.

```bash
make rust          # release build: rust/target/release/{sidecast-host,sidecast-probe}
make test-rust     # rustfmt, clippy, unit and integration tests
make conformance   # every probe against every host (see below)
make e2e-linux     # Linux: the real GNOME backend on a headless GNOME (see "The Linux host")
```

Run a host and a probe without a phone:

```bash
rust/target/release/sidecast-host --test-pattern --data-dir /tmp/sidecast-rust \
    --port 47895 --test-pairing-code 123456 --no-mdns
rust/target/release/sidecast-probe --host 127.0.0.1 --port 47895 --code 123456 \
    --state /tmp/sidecast-probe.json --seconds 5
```

Test-pattern mode runs the real server (TLS, pairing, flow control, encoder) on a
moving pattern and logs the touches, gestures, text, keys and audio it would
inject. The Android app connects to it too (**Add by IP**). `--test-mic-active`
asks a phone for its microphone at once; `--verbose` adds debug logs.

`scripts/conformance.sh` runs the Rust probe against the Rust host on any OS,
and on macOS also the Swift probe against the Rust host and the Rust probe
against the Mac app (pairing, AUTH, wrong codes, unknown tokens, codec
negotiation, data saver, mic, gestures, keyboard, trackpad CURSOR, liveness).

## The Windows host

Install it with `scripts/install-windows.ps1` (README: *Windows computer*), or
run it from the build folder: `sidecast-tray.exe` (notification-area app, logs
to `%LOCALAPPDATA%\Sidecast\sidecast.log`) or `sidecast-host.exe` (console).
Both take the options above; `--monitor \\.\DISPLAY2` (or `primary`) streams an
existing monitor instead of a virtual one, and `sidecast-host --diagnose` lists
the monitors, DXGI outputs, Virtual Display Driver, Media Foundation encoders
and audio devices it finds. `sidecast-tray --quit` asks a running tray app to quit.

| Backend | Windows implementation (`src/platform/windows/`) |
|---|---|
| Display | `screen.rs`: a Virtual Display Driver monitor at the phone's resolution while a phone streams (`vdd.rs`, `vdd_config.rs`: its settings file and control pipe; `display.rs`: attach, mode, detach), else an existing monitor |
| Capture | `capture.rs`: DXGI Desktop Duplication, pointer drawn in, NV12 at the stream's size (`convert.rs`) |
| Encoders | `encoder.rs`: Media Foundation H.264, the GPU's MFT then Microsoft's software MFT (output normalized by `bitstream.rs`); OpenH264 last |
| Input | `input.rs`: SendInput (`keymap.rs`: keys and Windows shortcuts; `mapping.rs`: coordinates, wheel, zoom) |
| Power | `power.rs`: SetThreadExecutionState |
| Microphone | `mic.rs`: WASAPI into a virtual audio cable's "CABLE Input" while an app records "CABLE Output" (`cable.rs`) |
| UI | `src/tray/`: icon, menu, pairing code window and notification, Start at login |

The pure modules (`keymap`, `mapping`, `convert`, `bitstream`, `vdd_config`,
`cable`) build and run their tests on every OS. On Windows, `cargo test` also
encodes with the Media Foundation encoders it finds and checks keep-awake;
with `SIDECAST_TEST_REAL_INPUT=1` it moves the real pointer and checks it.
`scripts/windows-e2e.sh` (Git Bash) streams a real monitor to the probe with
the real backends (pairing through a test-pattern host first, since fixed
codes are test-only); `--tray` also smoke-tests the tray app and
`--expect-virtual-display` requires a Virtual Display Driver monitor. CI runs
all of it on `windows-latest`, including the driver's signed release.

## How a platform backend plugs in

A host is a [`Backends`](sidecast-server/src/backend.rs) value passed to
`sidecast_server::start`. The server owns the protocol, pairing, flow control,
bitrate, keyframe cadence, gesture recognition and click timing; a backend only
produces frames and injects input. `sidecast-host/src/platform/windows/mod.rs`
and `linux/mod.rs` return the platform's `Backends`, given the host's
`PlatformOptions` (today: which existing monitor to stream).

```rust
let mut backends = Backends::new(Arc::new(MyDisplay::new()), vec![Arc::new(MyHardwareEncoder), Arc::new(OpenH264Factory)]);
backends.input = Some(Arc::new(MyInput));          // optional
backends.mic = Some(Arc::new(MyVirtualMic::new())); // optional
backends.power = Some(Arc::new(MyPowerGuard));      // optional
let server = sidecast_server::start(ServerConfig::new(DataDir::new(dir), 47800), backends, on_event).await?;
```

Every method may block; the server calls them on blocking threads or its input
task, never under a lock.

### `DisplayBackend`: the screen

- `preflight()` runs before every stream; return `BackendError::permission_missing`
  while the user still has to allow screen capture (the phone gets BYE `permission_missing`).
- `start(plan, client, frames)` creates the virtual display for `plan.display`
  (points; pixels = points × 2 when `hi_dpi`), or picks the monitor the user chose,
  and delivers frames of `plan.encode_width × plan.encode_height` at about
  `plan.fps`. It returns a `CaptureSession`; dropping it stops capture and removes
  the virtual display. Only one stream owns the display at a time: on takeover,
  the old session is dropped before the new `start`.
- `CaptureSession::geometry()` says where input goes (size in points, origin in
  desktop coordinates, an opaque display id); `request_refresh()` re-delivers the
  latest frame so a keyframe request on a static screen is answered.
- Frames go to the `FrameSink` from any thread. Call `frames.try_reserve()`
  *before* converting or copying a frame: `None` means 4 frames are in flight
  (or the encoder is not ready) and the frame must be dropped. That is the
  protocol's "drop before encoding" rule; the server counts drops for the bitrate.

```rust
if let Some(slot) = frames.try_reserve() {
    slot.submit(CapturedFrame { pts_micros: monotonic_micros(), width, height, pixels: PixelData::Bgra { data, stride } });
} // else: skip this frame
```

`PixelData` is `I420`, `Nv12`, `Bgra`, or `Native(Box<dyn Any + Send>)` for a GPU
surface (D3D11 texture, DMA-BUF) that only the platform's own encoder understands.

### `EncoderFactory` / `VideoEncoder`: compression

- `capabilities()` lists codecs and their largest frame; the planner only
  negotiates codecs some factory supports (HEVC when the phone can decode it).
- `create(config)` opens an encoder for codec, size, fps, start bitrate.
  Low latency: no B-frames, one access unit out per frame in.
- `encode(frame, force_keyframe)` returns Annex-B (00 00 00 01 start codes) for
  one access unit; keyframes carry SPS/PPS (and VPS for HEVC). The server forces
  a keyframe at session start, on KEYFRAME_REQUEST and at least every 2 s.
- `set_bitrate(bps)` is called when the adaptive controller moves.
- Factories are tried in order: put hardware encoders first; `OpenH264Factory`
  is the software fallback (H.264 Constrained Baseline, BT.709).

### `InputBackend` / `InputSink`: touch, trackpad, keyboard

`create(geometry)` returns a sink per stream. The sink receives
[`InputEvent`](sidecast-core/src/input/mod.rs)s in display points (top-left
origin), already timed (a click is move → 12 ms → press → 40 ms → release, DECISIONS Q32):

- `Pointer(PointerEvent)`: `Moved`, `ButtonDown/ButtonUp(Left|Right)` with
  `click_count` (2 = second click of a double click), `Dragged` (motion with the
  left button held). Add `geometry.origin` for absolute desktop coordinates.
- `Scroll { dx, dy, phase }`: pixels, natural direction; phases mirror macOS
  (`Began`, `Changed`, `Ended`, `Momentum…`); hosts without phases just scroll.
- `Zoom { delta }`: pinch steps (positive = zoom in): Ctrl+wheel on Windows/Linux.
- `Semantic(SemanticAction)`: 3/4-finger swipes. Map `DesktopNext` /
  `DesktopPrevious` (Windows: Win+Ctrl+→/←; GNOME: Super+Page Down/Up),
  `Overview` (Win+Tab; GNOME Activities) and `AppWindows` (Windows has no
  App Exposé: Win+D, show the desktop, as a precision touchpad's three-finger
  swipe down; GNOME: application windows).
- `Key { key, modifiers }`: `Key` is OS-neutral; `Key::is_navigation()` marks
  the keys Windows sends as extended. `KeyModifiers::META` is the Windows/Super key.
- `Text(String)`: Unicode text, control characters already removed; type it
  independent of the keyboard layout (Windows: `KEYEVENTF_UNICODE`).
- `pointer_position()` returns the system pointer on the streamed display (or
  `None`): trackpad mode starts from it and it is sent to the phone as CURSOR.
- `release_all()` runs on disconnect: release every button and key you hold.

### `MicSink`: the phone's microphone

`session_started(id, name, supports_mic, control)` starts with the mic off; call
`control.set_active(true)` while an app records from your virtual input device
and `false` when it stops. Feed `audio()` packets into a
`sidecast_core::audio::AudioJitterBuffer` and read it from the audio device's
render callback (48 kHz mono `f32`).

### `PowerGuard`: keep awake

`keep_awake(reason)` returns a guard held while a phone streams (Windows:
`SetThreadExecutionState`; Linux: a logind inhibitor). It is used when the
`keepAwake` setting is on (default).

### Events, settings and data

`sidecast_server::start` takes a callback for `ServerEvent`s: listening, the
pairing code to show (`PairingStarted`), streaming started/stopped, input mode,
per-second stats, paired devices changed. `ServerHandle` updates settings
(a new STREAM_CONFIG when the plan changes), forgets devices and shuts down
(BYE `server_shutdown`).

The data directory (`%APPDATA%\Sidecast`, `$XDG_DATA_HOME/sidecast`, or
`--data-dir`) holds `server-id`, `tls-certificate.der` + `tls-private-key.der`
(the identity phones pin: keep it), `paired-devices.json` (token hashes only)
and `settings.json`. Files are written atomically, owner-only on Unix.

SERVER_HELLO `platform` and the DNS-SD TXT `os` are the OS the host runs on
(`ServerConfig::platform`, defaulting to the current OS; DECISIONS Q34).

## The Linux host

`src/platform/linux/` (DECISIONS Q38). On GNOME the backend talks to Mutter
directly; elsewhere on Wayland to the xdg-desktop-portal; X11 is refused.

| Module | Does |
|---|---|
| `mutter.rs` | `org.gnome.Mutter.RemoteDesktop` session + linked `ScreenCast` `RecordVirtual` (cursor as metadata): the virtual monitor, sized by the PipeWire format we ask for; input calls on that session |
| `display_config.rs`, `monitors.rs` | 200 % scale for a HiDPI plan through `DisplayConfig.ApplyMonitorsConfig` (temporary, other monitors unchanged) |
| `portal.rs` | the same through `ashpd`: RemoteDesktop + ScreenCast portal, virtual source where offered, else a monitor |
| `capture.rs`, `pixels.rs` | the PipeWire stream (SHM/MemFd BGRx…), I420 conversion with the composited pointer, flow control, static-screen repeats |
| `remote_input.rs`, `translate.rs`, `keymap.rs` | `InputEvent` → RemoteDesktop calls on a worker thread: evdev keys/buttons, keysyms, finger scroll, GNOME shortcuts |
| `gst_encoder.rs` | `gstreamer` feature: `nvh264enc` / `vah264enc` / `vah264lpenc` / `vaapih264enc` → Annex-B |
| `mic.rs` | "Sidecast Microphone", a PipeWire `Audio/Source` fed from the jitter buffer; Streaming ⇔ MIC_CONTROL |
| `power.rs` | logind `sleep:idle` + GNOME suspend inhibitors while streaming |
| `tray.rs`, `menu.rs` | StatusNotifierItem + dbusmenu, pairing notification |
| `desktop.rs` | session detection and the backend choice |

Build dependencies (Ubuntu): `build-essential pkg-config clang libpipewire-0.3-dev`,
plus `libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev` for `--features gstreamer`.
D-Bus is spoken by `zbus` (pure Rust, run with `async-io` on plain threads, never
inside the tokio runtime). Environment overrides: `SIDECAST_LINUX_BACKEND=mutter|portal`,
`SIDECAST_TEXT_INPUT=keysyms|unicode-entry`.

`scripts/linux-e2e.sh` (`make e2e-linux`, and the CI job "Linux host on headless
GNOME") starts `mutter --headless --virtual-monitor 1280x720`, PipeWire and
WirePlumber on a private `dbus-run-session`, stand-ins for the notification
daemon and the tray watcher (`scripts/linux-e2e-desktop.py`) and `wev` as a
Wayland client, then runs the host and three probe sessions. It needs
`mutter pipewire pipewire-bin wireplumber dbus-daemon libglib2.0-bin python3-gi
adwaita-icon-theme wev`; no GPU (Mutter renders with llvmpipe) and no root.

## Conventions

- `unsafe` is denied workspace-wide; FFI backends allow it locally with a
  `// SAFETY:` comment for every block.
- Logic goes in `sidecast-core` with unit tests; backends stay thin. Server
  behaviour is covered by `sidecast-server/tests/session.rs` (TLS end to end
  with fake backends).
- Integration tests listen on 127.0.0.1:47894 (`SIDECAST_TEST_PORT=0` picks a
  free port, as CI does).
- `cargo fmt` (120 columns), `cargo clippy --all-targets -- -D warnings`.
