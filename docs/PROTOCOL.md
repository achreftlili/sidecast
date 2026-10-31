# Sidecast wire protocol — version 1

This is the contract between the computer app (the **host**, or server: macOS today, Windows and Linux planned)
and the phone app (the **client**: Android today, iOS planned). Notes that mention a Mac apply to the macOS host only.
Every implementation MUST pass the test vectors at the end of this file.

## 1. Discovery

- Bonjour / DNS-SD service type: **`_sidecast._tcp`**, domain `local.`
- Default TCP port: **47800** (configurable on the host). The listener uses a fixed port so manual IP entry works.
- Instance name: the host's user-visible computer name (on macOS `Host.current().localizedName`).
- TXT record:
  - `v=1` — protocol version
  - `id=<serverId>` — stable UUID string identifying this host install (generated once, persisted)
  - `os=macos|windows|linux` — the host's operating system, same values as SERVER_HELLO `platform` (§4). Absent = `macos` (Mac hosts before this key); clients show an unknown value as a generic computer.

The client lists discovered computers plus previously paired ones (by `serverId`, with last known host/port) and offers "Add by IP address" (host + optional port).

### 1.1 Remote access (over the internet)

Remote access uses **Tailscale** (a WireGuard mesh VPN) on both devices: the host is reachable at its Tailscale address (`100.64.0.0/10`, or its MagicDNS name) from anywhere. Nothing in the protocol changes; TLS + pairing + tokens still apply on top.

- The host lists its reachable addresses in SERVER_HELLO (`addresses`, §4 table), ordered LAN, then `tailscale`, then `tailscale-dns`. `kind` comes from the host's interface (a 100.64/10 address on Wi-Fi is `lan`), so the client trusts `kind` rather than re-deriving it. The client stores them with the pairing.
- Connecting to a paired host: use the Bonjour-resolved address if the host is discovered on the local network; otherwise try the stored addresses in order — LAN addresses first (2.5 s connect timeout each), then Tailscale ones (6 s) — and remember the one that worked as `lastHost`.
- The client must not force LAN-only routing (e.g. binding the socket to the Wi-Fi network) for non-private destinations: Tailscale addresses go through the VPN, public ones through whatever network is active. Binding to Wi-Fi is only allowed for RFC 1918 / link-local destinations.

## 2. Transport security

- TCP with Nagle disabled (`TCP_NODELAY`) on both ends, wrapped in **TLS (1.2 minimum, 1.3 preferred)**.
- The host presents a **self-signed EC P-256 certificate**, generated once per install and persisted.
- **Certificate fingerprint** = SHA-256 of the leaf certificate's DER encoding (32 raw bytes). Shown as lowercase hex.
- Client trust model (trust on first pairing):
  - Not yet paired with this host: accept any certificate, remember its fingerprint for the pairing proof.
  - Paired: the leaf fingerprint MUST equal the pinned fingerprint, otherwise abort and tell the user "This computer's identity changed — pair again".
- The client does not verify hostname or chain (self-signed); pinning replaces that.

## 3. Framing

Every message, in both directions:

```
type    : u8
length  : u32, big-endian — payload length in bytes, 0 ≤ length ≤ 16 MiB
payload : `length` bytes
```

- Receivers MUST ignore (skip) unknown message types.
- A length above 16 MiB is a protocol error → close the connection.
- All multi-byte integers are **big-endian**. Floats are IEEE-754 binary32, **big-endian** (Java `DataOutputStream.writeFloat` order).
- "JSON" payloads are UTF-8 JSON objects. Receivers ignore unknown keys.
- Binary data inside JSON (tokens, nonces, proofs) is standard Base64 with padding (RFC 4648 §4).

## 4. Message types

### Client → Server

| Type | Name | Payload |
|------|------|---------|
| `0x01` | HELLO | JSON, see §5 |
| `0x02` | AUTH | JSON `{"deviceId": string, "token": b64}` |
| `0x03` | PAIR_START | empty |
| `0x04` | PAIR_PROOF | JSON `{"nonce": b64(16 bytes), "proof": b64(32 bytes)}` |
| `0x10` | TOUCH | binary, see §7 |
| `0x11` | FRAME_ACK | `seq: u32` |
| `0x12` | KEYFRAME_REQUEST | empty |
| `0x13` | PING | `t: u64` (client monotonic microseconds; opaque to server) |
| `0x14` | TEXT | UTF-8 text typed on the phone, see §7.1 |
| `0x15` | KEY | `key: u8`, `modifiers: u8`, see §7.1 |
| `0x16` | AUDIO | microphone audio, see §7.2 |
| `0x17` | MIC_STATE | JSON `{"capturing": bool, "reason"?: string}`, see §7.2 |
| `0x18` | INPUT_MODE | JSON `{"mode": "touch" \| "trackpad"}`, see §7.3 |

### Server → Client

| Type | Name | Payload |
|------|------|---------|
| `0x81` | SERVER_HELLO | JSON `{"protocol": 1, "serverId": string, "serverName": string, "port"?: int, "addresses"?: [{"host": string, "kind": "lan" \| "tailscale" \| "tailscale-dns"}], "platform"?: "macos" \| "windows" \| "linux"}` |
| `0x82` | PAIR_CHALLENGE | JSON `{"expiresInSec": int, "attemptsLeft": int}` |
| `0x83` | AUTH_RESULT | JSON `{"ok": bool, "token"?: b64, "reason"?: string, "attemptsLeft"?: int}` |
| `0x84` | STREAM_CONFIG | JSON, see §6 |
| `0x85` | VIDEO_FRAME | binary, see §6 |
| `0x86` | PONG | the PING's 8 bytes, echoed unchanged |
| `0x87` | BYE | JSON `{"reason": string}` then the server closes |
| `0x88` | MIC_CONTROL | JSON `{"active": bool, "sampleRate"?: 48000, "channels"?: 1}`, see §7.2 |
| `0x89` | CURSOR | binary `x: f32`, `y: f32` (big-endian, normalized 0…1 to the video frame), see §7.3 |

`reason` values: `bad_code`, `too_many_attempts`, `pairing_expired`, `unknown_device`, `protocol_mismatch`, `replaced` (another phone took over), `server_shutdown`, `display_failed`, `permission_missing` (the host may not capture the screen, e.g. Screen Recording not granted on a Mac).

SERVER_HELLO `platform` is the host's operating system. Hosts always send it; a SERVER_HELLO without it comes from an older Mac host, so absent = `"macos"`. Clients show an unknown value as a generic computer, and use the platform only for display and OS-specific advice: the protocol is the same for every OS. See §8.4.

## 5. Session flow

```
C: TLS handshake
C → S  HELLO
S → C  SERVER_HELLO
        ├─ client has a token for serverId ─→ C → S AUTH
        └─ otherwise ───────────────────────→ C → S PAIR_START
                                               S shows code on the host
                                               S → C PAIR_CHALLENGE
                                               C asks the user for the code
                                               C → S PAIR_PROOF   (retry allowed while attemptsLeft > 0)
S → C  AUTH_RESULT {ok:true[, token]}
S → C  STREAM_CONFIG
S → C  VIDEO_FRAME (first one is a keyframe) …
C → S  FRAME_ACK per frame, PING every 1 s, TOUCH as the user touches
```

### HELLO (client → server)

```json
{
  "protocol": 1,
  "deviceId": "8c1f…",           // stable random UUID per app install
  "deviceName": "Pixel 8",       // user-visible (Settings.Global DEVICE_NAME or Build.MODEL)
  "screen": { "width": 2400, "height": 1080, "densityDpi": 420, "refreshRate": 120.0 },
  "codecs": [
    { "name": "hevc", "maxWidth": 3840, "maxHeight": 2160 },
    { "name": "h264", "maxWidth": 1920, "maxHeight": 1080 }
  ]
}
```

- `screen` is the **physical pixel size in landscape** (width ≥ height) of the area the stream will fill (real display size incl. system-bar areas, since the app is immersive).
- `codecs` lists hardware-capable decoders, with the largest landscape size each supports at ≥ 60 fps (`VideoCapabilities.areSizeAndRateSupported`). Order = client preference.
- Optional `"features": ["mic"]` — capabilities the client supports (§7.2). Absent = none.
- Optional `"network": "metered" | "unmetered"` — whether the phone's active network is metered (mobile data).
- Optional `"limits": {"maxBitrateKbps": int, "maxFps": int}` — caps requested by the client (data saver). The server applies `maxBitrate = min(user setting, limit)` and `fps = min(setting, phone refresh rate, limit)` (limit floored at 5 fps); the 4 Mbps minimum bitrate drops to the limit when the limit is lower; the adaptive bitrate controller's start value is clamped to the max. Default data-saver limits: 4000 kbps, 30 fps.
- Server rejects `protocol != 1` with BYE `protocol_mismatch`.

### Pairing

1. On PAIR_START the server generates a uniformly random 6-digit code (`000000`–`999999`), shows it on the host's screen ("Enter this code on <deviceName>"), and starts a 120 s timer. 5 attempts.
2. Client computes, over the TLS connection's server leaf certificate:

```
proof = HMAC-SHA256( key = ASCII(code), message = certFingerprint(32 bytes) ‖ nonce(16 random bytes) )
```

3. Server recomputes with its own certificate fingerprint and compares in constant time.
   - Wrong, attempts remain: AUTH_RESULT `{ok:false, reason:"bad_code", attemptsLeft:n}` (n ≥ 1); the client re-prompts.
   - Wrong, last attempt: AUTH_RESULT `{ok:false, reason:"too_many_attempts", attemptsLeft:0}` + BYE `too_many_attempts`.
   - Expired: AUTH_RESULT `{ok:false, reason:"pairing_expired"}` + BYE.
   - Right: server creates a 32-byte random **token**, stores `SHA-256(token)` with `deviceId`, `deviceName`, `pairedAt`, replies AUTH_RESULT `{ok:true, token}`. The session proceeds to streaming immediately.
4. Client stores per `serverId`: `fingerprint`, `token`, `serverName`, `platform`, last host + port.

### AUTH

- Server looks up `deviceId`, compares `SHA-256(token)` in constant time.
- OK → AUTH_RESULT `{ok:true}`. Unknown/mismatch → AUTH_RESULT `{ok:false, reason:"unknown_device"}` + BYE; the client deletes its stored pairing for that host and pairs again on the next attempt.
- Only an authenticated client can take over the stream. When a second client authenticates, the current one gets BYE `replaced`.
- Any connection that has not authenticated within 150 s (pairing included) is closed.

## 6. Streaming

### STREAM_CONFIG (server → client, JSON)

Sent after successful auth and whenever the stream parameters change (always followed by a keyframe).

```json
{ "codec": "hevc", "width": 2400, "height": 1080, "fps": 60 }
```

- `codec`: `"hevc"` or `"h264"`; `width`/`height`: encoded frame size in pixels (even numbers).
- The client (re)configures its decoder on every STREAM_CONFIG and scales the video to fit its screen, preserving aspect ratio (letterbox if needed).

### VIDEO_FRAME (server → client, binary)

```
seq       : u32   — increments by 1 per frame sent, starts at 0 per connection
flags     : u8    — bit 0 = keyframe; other bits reserved (0)
ptsMicros : u64   — server capture timestamp, monotonic microseconds
data      : Annex-B elementary stream (00 00 00 01 start codes) for exactly one access unit
```

Keyframes include parameter sets (HEVC: VPS, SPS, PPS; H.264: SPS, PPS) before the slice data.

### Flow control (server side)

- Client sends FRAME_ACK(seq) as soon as it has **received** a VIDEO_FRAME (before decoding).
- Server keeps at most **4 frames in flight** (sent, not yet acked). When a captured frame arrives while 4 are in flight, it is dropped **before encoding** (so no decoder reference is ever missing).
- Adaptive bitrate (AIMD), evaluated every 1 s:
  - if any frame was dropped, or smoothed RTT > min RTT + 40 ms → `bitrate = max(minBitrate, bitrate × 0.75)`
  - else after 2 consecutive clean windows → `bitrate = min(maxBitrate, bitrate + max(1 Mbps, 5% of maxBitrate))`
  - RTT sample = ack arrival − frame send time.
  - Defaults: start 20 Mbps, min 4 Mbps, max = user setting (default 50 Mbps).
- On KEYFRAME_REQUEST the server forces the next encoded frame to be a keyframe. The encoder also emits a keyframe at least every 2 s.

### Liveness

- Client sends PING every 1 s; server answers PONG immediately.
- Server closes an **authenticated** client that sent nothing (no ack/ping/touch) for **5 s**. Before authentication the client is silent while the user types the pairing code, so only the 150 s auth timeout applies.
- The client sends KEYFRAME_REQUEST after every decoder (re)configuration, including right after the first STREAM_CONFIG; the server should expect one extra keyframe at session start.
- Client shows "Connection lost — reconnecting…" (never a silently frozen frame) when it has received neither PONG nor VIDEO_FRAME for **3 s**, then reconnects with backoff (1 s, 2 s, 4 s, max 5 s).

## 7. Touch input

### TOUCH (client → server, binary)

```
eventTimeMs     : u32  — client uptime in ms (wraps; server uses differences only)
action          : u8   — Android MotionEvent action: 0 DOWN, 1 UP, 2 MOVE, 3 CANCEL, 5 POINTER_DOWN, 6 POINTER_UP
actionPointerId : u8   — pointer id the action refers to (for MOVE: id of the first pointer)
count           : u8   — number of pointers that follow (1…10)
pointers[count] : { id: u8, x: f32, y: f32 }
```

- `x`, `y` are normalized to the **video frame** (0,0 top-left … 1,1 bottom-right), after removing letterboxing. Values may be slightly outside 0…1 near edges; the server clamps.
- For UP / POINTER_UP the list still contains the lifting pointer at its last position.
- The client sends every MOVE (no batching beyond one message per input event).
- The server turns touches into mouse events (gesture engine) on the virtual display. On disconnect it releases any pressed button.

### 7.1 Keyboard (TEXT / KEY, client → server)

The phone's on-screen keyboard (or a hardware keyboard attached to the phone) types into whatever is focused on the computer.

- **TEXT** (`0x14`): payload = UTF-8 text, 1…4096 bytes. The server types it as Unicode, independent of the computer's keyboard layout (accents, emoji and non-Latin scripts work). Control characters are not sent as TEXT; the client uses KEY for them.
- **KEY** (`0x15`): payload = 2 bytes `key: u8`, `modifiers: u8`. The server presses and releases the key with the modifiers held.

| `key` | Key | | `key` | Key |
|---|---|---|---|---|
| 1 | Return / Enter | | 8 | Up arrow |
| 2 | Backspace (Delete ⌫) | | 9 | Down arrow |
| 3 | Forward delete (⌦) | | 10 | Home |
| 4 | Tab | | 11 | End |
| 5 | Escape | | 12 | Page up |
| 6 | Left arrow | | 13 | Page down |
| 7 | Right arrow | | | |

`modifiers` bitmask: `1` Shift, `2` Control, `4` Option/Alt, `8` Command/Meta. Unknown keys or bits are ignored.

- Both are accepted only from an authenticated, streaming client, and only when the host allows input (on a Mac: the "touch/input" setting is on and Accessibility is granted). Otherwise they are silently ignored (like TOUCH). Malformed payloads are ignored, not fatal.
- Older servers skip both types as unknown; no version bump is needed.

### 7.2 Microphone (phone mic → a virtual microphone on the computer)

The host exposes a virtual input device (on a Mac, **Sidecast Microphone**). When any app on the computer records from it, the host asks the phone to stream its microphone.

- The client advertises support with `"features": ["mic"]` in HELLO.
- **MIC_CONTROL** (`0x88`, server → client, JSON): `{"active": true, "sampleRate": 48000, "channels": 1}` asks the phone to start capturing; `{"active": false}` asks it to stop. The server sends `active:true` only to an authenticated streaming client that advertised `mic`, while the host's virtual microphone is in use by some app; it sends `active:false` when that stops. A new streaming session starts with the mic off.
- **MIC_STATE** (`0x17`, client → server, JSON): the phone reports `{"capturing": true}` once audio flows, or `{"capturing": false, "reason": r}` with `r` ∈ `stopped` (after `active:false`), `disabled` (user turned the feature off on the phone), `permission_denied` (Android RECORD_AUDIO refused), `background` (app not in the foreground), `unavailable` (mic busy / AudioRecord failed). The host shows this to the user.
- **AUDIO** (`0x16`, client → server, binary):
  ```
  seq     : u32 big-endian — 0 for the first packet after each MIC_CONTROL active:true, +1 per packet
  samples : PCM signed 16-bit **little-endian**, mono, 48 000 Hz — 1…4800 samples (480 = 10 ms recommended)
  ```
  The phone captures with Android's voice-communication source (echo cancellation, noise suppression) and sends every 10 ms. It never sends AUDIO unless the last MIC_CONTROL was `active:true` and it is capturing.
- Server side: a jitter buffer targeting ~40 ms; if more than ~150 ms is queued, the oldest audio is dropped. Gaps (seq jumps, underruns) are filled with silence. When no client is capturing, the virtual microphone delivers silence.
- AUDIO/MIC_STATE before authentication, or without a pending `active:true`, are ignored. Malformed payloads (odd byte count, 0 or > 4800 samples) are ignored, not fatal.

### 7.3 Input mode (touch / trackpad)

- **INPUT_MODE** (`0x18`, client → server, JSON): `{"mode": "touch"}` (default: the phone is a touchscreen, positions are absolute) or `{"mode": "trackpad"}` (the phone is a laptop trackpad: finger movement moves the cursor relatively). The client sends it right after authentication and whenever the user toggles; the server applies it to subsequent TOUCH events (a mode change ends the current gesture cleanly: buttons released, scroll ended). Unknown modes are ignored. Older servers skip the message.
- TOUCH payloads are unchanged in both modes (normalized absolute coordinates); the server derives relative motion in trackpad mode.
- **CURSOR** (`0x89`, server → client, binary, 8 bytes): the host's pointer position normalized to the streamed display (0,0 top-left … 1,1 bottom-right). Sent in trackpad mode whenever the pointer moves (throttled to ≤ 60 per second) and once right after the client switches to trackpad mode (and after each STREAM_CONFIG while in trackpad mode). The client draws a large pointer at that position (letterbox-aware) only while in trackpad mode. When the pointer is not on the streamed display the server sends -1,-1; the client hides its pointer whenever x or y is < 0 or > 1.
- Hosts inject clicks human-like (pointer move to the target, button down, ~40 ms hold, button up) because some apps (Chromium/Electron, e.g. VS Code) ignore zero-duration synthetic clicks.
- Trackpad-mode gestures (server side): 1-finger move = move cursor (with acceleration); tap = click at the cursor; double tap = double click; double-tap-and-hold then move = drag; 2-finger tap = right click; 2-finger move = scroll; pinch = zoom; 3/4-finger swipes = navigation (as in touch mode). Long press does nothing in trackpad mode.

## 8. Test vectors

Implementations MUST include unit tests reproducing these exactly.

### 8.1 Pairing proof

```
certFingerprint = SHA-256(ASCII "sidecast-test-cert")
                = 004ac6c710efca069afd7657c7262b316b18e1595f710ea8a5d2289d04b9d196
nonce           = 000102030405060708090a0b0c0d0e0f
code            = "123456"
proof (hex)     = e34b30e707f974d1fa4e3e5afdc523c3fc9a39ab4c7d43f42af27dcb04244afe
proof (b64)     = 40sw5wf5dNH6Tj5a/cUjw/yaOatMfUP0KvJ9ywQkSv4=
nonce (b64)     = AAECAwQFBgcICQoLDA0ODw==
```

### 8.2 Token hash

```
token        = 32 bytes of 0xAA
SHA-256(tok) = e0e77a507412b120f6ede61f62295b1a7b2ff19d3dcc8f7253e51663470c888e
```

### 8.3 Framing

FRAME_ACK seq = 258:

```
11 00 00 00 04 00 00 01 02
```

TOUCH, eventTimeMs = 1000, action = MOVE (2), actionPointerId = 0, one pointer {id 0, x 0.5, y 0.25}:

```
10 00 00 00 10  00 00 03 e8  02  00  01  00  3f 00 00 00  3e 80 00 00
```

VIDEO_FRAME header for seq = 7, keyframe, ptsMicros = 1_000_000, data = `00 00 00 01 40`:

```
85 00 00 00 12  00 00 00 07  01  00 00 00 00 00 0f 42 40  00 00 00 01 40
```

TEXT "é":

```
14 00 00 00 02 c3 a9
```

KEY Return (1), no modifiers; then KEY Left (6) with Shift:

```
15 00 00 00 02 01 00
15 00 00 00 02 06 01
```

AUDIO seq = 1, two samples [1, -2]:

```
16 00 00 00 08 00 00 00 01 01 00 fe ff
```

CURSOR x = 0.5, y = 0.25:

```
89 00 00 00 08 3f 00 00 00 3e 80 00 00
```

PING t = 1:

```
13 00 00 00 08 00 00 00 00 00 00 00 01
```

### 8.4 SERVER_HELLO `platform`

How a client reads the host's OS from these SERVER_HELLO payloads (key order and spacing do not matter):

```
{"protocol":1,"serverId":"S","serverName":"Studio","platform":"macos"}    → macOS
{"protocol":1,"serverId":"S","serverName":"Studio","platform":"windows"}  → Windows
{"protocol":1,"serverId":"S","serverName":"Studio","platform":"linux"}    → Linux
{"protocol":1,"serverId":"S","serverName":"Studio"}                       → macOS (a Mac host older than the field)
{"protocol":1,"serverId":"S","serverName":"Studio","platform":"beos"}     → unknown: a generic computer
```
