# Roadmap

## Vision

Any phone or tablet becomes a **second screen, touchpad, keyboard and microphone for any computer**: macOS, Windows or Linux, from Android or iOS.

- **Free and open source** (Apache 2.0), built and installed from source. No app stores, no subscriptions, no signing fees.
- **Private**: no accounts, no cloud, no telemetry. Devices talk directly, encrypted end to end; remote use goes through the user's own Tailscale.
- **Protocol first**: [PROTOCOL.md](PROTOCOL.md) is the contract. Any host or client that speaks it works with all the others.

## Status

| | Android client | iOS / iPadOS client |
|---|---|---|
| **macOS host** | ✅ Works (0.6) | 🧪 Preview (P4): built from source, tested in the iOS Simulator |
| **Windows host** | 🧪 Experimental (P2): built from source (`scripts/install-windows.ps1`), tested in CI on Windows, not yet tried on a real PC with a phone | 🧪 Same protocol; untested |
| **Linux host** | 🧪 Experimental (P3): GNOME on Wayland, built from source (`make install-linux`), tested end to end on a headless GNOME in CI, not yet with a real phone | 🧪 Same protocol; untested |

## Phases

| Phase | Content |
|---|---|
| **P0: Open-source launch** ✅ | Apache 2.0, build-from-source installs (`make doctor`, `make install-mac`, `make install-android`), a per-user local signing certificate for macOS, CI, security policy. |
| **P1: Open protocol** (in progress) | Protocol v2, backward compatible: hosts announce their OS and capabilities; semantic actions (`desktop.next`, `overview`, `copy`, `paste`…) that each host maps to its own shortcuts. A conformance tool, grown from `sidecast-probe`, that every host must pass. A shared Rust core (`sidecast-core`: protocol, pairing, flow control, gestures) for the new hosts. **Done:** hosts announce their OS (Q34); the Rust workspace `rust/` (Q36) with `sidecast-core`, the host server with backend traits, a test-pattern host and the Rust conformance probe, checked against the Mac app and the Swift probe by `make conformance` in CI; the gesture engine already emits semantic actions inside the host. **Next:** capabilities and semantic actions on the wire (protocol v2). |
| **P2: Windows host** 🧪 | **Experimental (Q37):** `sidecast-host` and the `sidecast-tray` notification-area app on the Rust core. A virtual monitor at the phone's resolution through the signed, MIT-licensed [Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver) (Windows only loads Microsoft-signed drivers, so users install that driver's official build), or an existing monitor without it; DXGI Desktop Duplication; Media Foundation H.264 (hardware, then Microsoft's software encoder, then OpenH264); SendInput; SetThreadExecutionState; the phone mic through a virtual audio cable (VB-CABLE); `scripts/install-windows.ps1`. **Next:** testing on real PCs with phones, HEVC, zero-copy GPU encoding, touch injection (InjectTouchInput) for touch-first apps, a signed installer. |
| **P3: Linux host** 🧪 | Ubuntu GNOME on Wayland first. **Done (experimental, Q38):** a virtual monitor and capture through Mutter's ScreenCast (`RecordVirtual`) and PipeWire, input through Mutter's RemoteDesktop session, keep-awake through a logind inhibitor, the phone mic as a PipeWire source, hardware encoding through GStreamer (VA-API / NVENC) with OpenH264 as the fallback, a tray icon, `make install-linux`, and an end-to-end test on a headless GNOME in CI. Other Wayland desktops go through the xdg-desktop-portal (virtual source where offered, else mirroring). **Next:** testing with real phones and GPUs, HEVC through GStreamer, KDE and wlroots virtual outputs, X11. |
| **P4: iOS / iPadOS client** 🧪 | Preview in `ios/` (`make install-ios`): the client logic lives in `SidecastKit` and is unit-tested on macOS; the app is UIKit. Swift app reusing `SidecastKit`: VideoToolbox decoding, Network.framework TLS with pinning, Bonjour, touch, Apple Pencil, keyboard and dictation, microphone. Built from Xcode with a free Apple ID; the app then has to be reinstalled every 7 days (a paid Apple developer account removes that limit). |
| **P5: Community** | Documentation site, translations, a maintainer per platform. |

## Per-platform building blocks

| | macOS ✅ | Windows 🧪 | Linux 🧪 |
|---|---|---|---|
| Virtual display | `CGVirtualDisplay` (private API) | IddCx driver (signed, from VirtualDrivers), controlled through its settings file and pipe; an existing monitor without it | Mutter `RecordVirtual`; portal virtual source (KDE, wlroots); EVDI or xrandr on X11 later |
| Capture | ScreenCaptureKit | DXGI Desktop Duplication | PipeWire |
| Encoding | VideoToolbox | Media Foundation (GPU MFT, Microsoft's software MFT), OpenH264 fallback | VA-API / NVENC through GStreamer, OpenH264 fallback |
| Input | CGEvent | SendInput | Mutter RemoteDesktop, RemoteDesktop portal (libei later) |
| Keep awake | `pmset` via a scoped sudoers rule | SetThreadExecutionState | logind inhibitor |
| Phone mic | Own HAL driver | Existing virtual audio cable (VB-CABLE) | PipeWire virtual source |
| Discovery | Bonjour | mdns-sd (DNS-SD in Rust) | mdns-sd (DNS-SD in Rust) |

## Risks

- **macOS private API**: a macOS update can break `CGVirtualDisplay`; it stays isolated in one module.
- **Windows drivers**: building our own display or audio driver would need an EV certificate and Microsoft signing, so we reuse signed open-source drivers.
- **Linux variety**: each desktop has its own path; GNOME first, the others behind capability checks.
- **Licensing**: keep dependencies compatible with Apache 2.0 (no GPL encoders such as x264; LGPL FFmpeg builds only, dynamically linked).

## How to help

Pick a phase, open an issue to agree on the approach, and see [CONTRIBUTING.md](../CONTRIBUTING.md).
