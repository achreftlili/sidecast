# Sidecast — design decisions

Outcome of the design interview (2026-10-26). "Sidecast" is a working name.

Sidecast turns an Android phone into an **extended display** for a Mac over **Wi-Fi**.
It is a new, smaller codebase that ports the proven core of
[SideScreen](https://github.com/tranvuongquocdat/SideScreen) (MIT, see `THIRD_PARTY_NOTICES.md`).

| # | Decision |
|---|----------|
| Q1 | Two deliverables: a macOS app shipped as a `.dmg`, and an Android app shipped as an `.apk`. |
| Q2 | Personal use first: ad-hoc signed DMG, sideloaded APK. Structured so Developer ID signing + notarization can be added later without rework. |
| Q3 | True **extended** virtual display (private `CGVirtualDisplay` API). No mirroring in v1. |
| Q5 | Touch on the phone controls the Mac with SideScreen's full gesture set (tap, double tap, long-press = right click, long-press-drag = drag, 1/2-finger scroll with momentum, pinch = zoom). Held buttons are always released on disconnect (SideScreen bug #71). |
| Q6 | Mac: Apple Silicon, **macOS 15+** (bumped from 14 because in-memory TLS identities need `kSecImportToMemoryOnly`). Android: **Android 10+ (minSdk 29)**, targetSdk 36. Codec is negotiated: HEVC when the phone can decode it at the needed size, else H.264. |
| Q7 | Latency first: 60 fps default, adaptive bitrate, a max-bitrate/quality setting for text-heavy use. |
| Q8 | Toolchain via Homebrew (JDK 21, Gradle, Android command-line tools, platform-tools). |
| Q9 | New codebase; port virtual display, capture, encoder, decoder and gesture logic from SideScreen; rewrite app shell, settings and protocol. Drop website, demo mode, QR scanner (ML Kit), Play in-app updates. |
| Q10 | Accept the private `CGVirtualDisplay` API, isolated in one module. No Mac App Store. |
| Q13 | Menu-bar-only Mac app (`LSUIElement`), small SwiftUI settings window. |
| Q14 | Name **Sidecast**; bundle id / applicationId `com.achreftlili.sidecast`. |
| Q15 | **Wi-Fi only** in v1. No adb dependency on the Mac. (USB via `adb reverse` is a v2 candidate.) |
| Q16 | Discovery via **Bonjour** (`_sidecast._tcp`), one-time pairing with a **6-digit code shown on the Mac**. Manual IP entry as fallback. Works on a phone hotspot too. |
| Q17 | **TLS** on every connection; the phone pins the Mac's self-signed certificate at pairing; every connection must authenticate with a device token (no unauthenticated access, unlike SideScreen's loopback hole). |
| Q18 | TCP + **app-level frame acks**: cap frames in flight, drop frames *before* encoding when the link backs up, AIMD adaptive bitrate. Heartbeats so a dead link shows "Connection lost" instead of a frozen frame (SideScreen #72). |
| Q19 | System sleep (`pmset -a disablesleep 1`) is disabled **only while a phone is connected**; restored on disconnect. Menu bar shows when sleep is disabled. |
| Q20 | Root for `pmset` via a **sudoers rule** installed once (one admin prompt) that allows exactly `/usr/bin/pmset -a disablesleep 0` and `… 1`. The rule is validated with `visudo -c` before install. A menu item removes it. |
| Q21 | Crash safety: a detached watcher process restores sleep if the app dies for any reason; the app also restores sleep on every launch if a marker says it had disabled it. |
| Q22 | **Keyboard from the phone** (v0.2): a keyboard button on the phone opens the Android keyboard; typed text goes to the focused Mac app as Unicode (layout-independent), special keys as KEY events. Voice typing from the phone keyboard's mic works too. |
| Q23 | **Phone mic as a Mac microphone** (v0.2): our own HAL audio driver adds a **"Sidecast Microphone"** input device (installed once with an admin prompt). |
| Q24 | The phone records **only while a Mac app is using Sidecast Microphone** (MIC_CONTROL from the Mac), with a phone setting to disable the feature. |
| Q25 | While a phone is connected, Sidecast Microphone becomes the Mac's **default input** (previous default restored on disconnect); toggle in Settings. Audio is 48 kHz mono 16-bit PCM from Android's voice-communication source. |
| Q27 | **Trackpad-style gestures** (v0.3): 2-finger tap = right-click; 3/4-finger swipe left/right = switch Desktops / full-screen apps, up = Mission Control, down = App Exposé (injected as the Mission Control keyboard shortcuts). 3-finger tap stays the phone's stats toggle. |
| Q28 | **Remote access over the internet via Tailscale** (v0.4): no router changes, nothing exposed publicly; Sidecast TLS/pairing on top. The Mac advertises its LAN + Tailscale addresses in SERVER_HELLO; the phone tries LAN first and falls back to Tailscale automatically. |
| Q29 | **Data saver on mobile data** (default on): the phone asks for ≤ 4 Mbps and 30 fps when its network is metered. |
| Q30 | Remote use needs the Mac awake: Settings shows a hint to enable "Prevent automatic sleeping on power adapter when the display is off". No new always-on sleep blocking. |
| Q31 | **Trackpad mode** (v0.5): a toggle on the phone switches between touchscreen (absolute) and trackpad (relative cursor, tap = click at cursor) for precise apps like VS Code. |
| Q32 | **Human-like clicks + visible trackpad pointer** (v0.6): clicks are injected as move → down → ~40 ms → up (Electron apps like VS Code drop instant clicks); in trackpad mode the Mac streams its pointer position (CURSOR) and the phone draws a large pointer overlay. |
| Q26 | Mac builds are signed with a local self-signed **"Sidecast Local Signing"** certificate so macOS keeps permissions across updates. |
| Q33 | **Open source, built from source** (0.6, 2026-10-30): Apache 2.0. Users clone the repository and install with `make install-mac` / `make install-android`; no app stores, notarization or paid certificates. Each Mac creates its own "Sidecast Local Signing" certificate with `make mac-signing`; the phone app is installed over adb (exempt from Android developer verification). Mac builds target the host architecture (Intel compiles, untested). |

## Known limitations (accepted for v1)

- `CGVirtualDisplay` is private API; a macOS update can break it.
- Builds are signed with a per-Mac self-signed certificate (Q26, Q33), not a Developer ID: fine for apps built on the same Mac, but a build copied to another Mac would be blocked by Gatekeeper.
- Pairing: an *active* man-in-the-middle present during the 2-minute pairing window could brute-force the 6-digit code. Acceptable on a home network; a PAKE (e.g. SPAKE2) is the upgrade path.
- Dragging the app to the Trash does not remove the sudoers rule — use the menu item "Remove sleep permission…" first.
- One phone at a time (an *authenticated* new phone takes over; unauthenticated connections never disturb the stream).
- Landscape only; no audio from the Mac to the phone; no stylus pressure; no keyboard shortcuts with letters (Cmd+C etc.) from the phone.
