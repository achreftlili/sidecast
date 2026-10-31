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
| **Windows host** | Planned (P2) | Planned |
| **Linux host** | Planned (P3) | Planned |

## Phases

| Phase | Content |
|---|---|
| **P0: Open-source launch** ✅ | Apache 2.0, build-from-source installs (`make doctor`, `make install-mac`, `make install-android`), a per-user local signing certificate for macOS, CI, security policy. |
| **P1: Open protocol** | Protocol v2, backward compatible: hosts announce their OS and capabilities; semantic actions (`desktop.next`, `overview`, `copy`, `paste`…) that each host maps to its own shortcuts. A conformance tool, grown from `sidecast-probe`, that every host must pass. A shared Rust core (`sidecast-core`: protocol, pairing, flow control, gestures) for the new hosts. |
| **P2: Windows host** | Rust + Tauri tray app. Virtual monitor through the signed, MIT-licensed [Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver) (Windows only loads Microsoft-signed drivers, so users install that driver's official build). Capture with Windows.Graphics.Capture, hardware encoding (NVENC / AMF / Quick Sync / Media Foundation), input with SendInput and InjectTouchInput, keep-awake with SetThreadExecutionState. Phone mic through an existing virtual audio cable at first. Installed with `install.ps1`. |
| **P3: Linux host** | Ubuntu GNOME on Wayland first: virtual monitor and capture through Mutter's ScreenCast (`RecordVirtual`) and PipeWire, input through the RemoteDesktop portal (libei), keep-awake through a logind inhibitor, phone mic as a PipeWire virtual source, hardware encoding through VA-API / NVENC. Then KDE, wlroots and X11. |
| **P4: iOS / iPadOS client** 🧪 | Preview in `ios/` (`make install-ios`): the client logic lives in `SidecastKit` and is unit-tested on macOS; the app is UIKit. Swift app reusing `SidecastKit`: VideoToolbox decoding, Network.framework TLS with pinning, Bonjour, touch, Apple Pencil, keyboard and dictation, microphone. Built from Xcode with a free Apple ID; the app then has to be reinstalled every 7 days (a paid Apple developer account removes that limit). |
| **P5: Community** | Documentation site, translations, a maintainer per platform. |

## Per-platform building blocks

| | macOS ✅ | Windows | Linux |
|---|---|---|---|
| Virtual display | `CGVirtualDisplay` (private API) | IddCx driver (signed, from VirtualDrivers) | Mutter `RecordVirtual`, KDE virtual output, wlroots headless; EVDI or xrandr on X11 |
| Capture | ScreenCaptureKit | Windows.Graphics.Capture | PipeWire |
| Encoding | VideoToolbox | NVENC / AMF / QSV / Media Foundation | VA-API / NVENC, OpenH264 fallback |
| Input | CGEvent | SendInput, InjectTouchInput | RemoteDesktop portal (libei), uinput fallback |
| Keep awake | `pmset` via a scoped sudoers rule | SetThreadExecutionState | logind inhibitor |
| Phone mic | Own HAL driver | Existing virtual audio cable | PipeWire virtual source |
| Discovery | Bonjour | Windows mDNS API | Avahi |

## Risks

- **macOS private API**: a macOS update can break `CGVirtualDisplay`; it stays isolated in one module.
- **Windows drivers**: building our own display or audio driver would need an EV certificate and Microsoft signing, so we reuse signed open-source drivers.
- **Linux variety**: each desktop has its own path; GNOME first, the others behind capability checks.
- **Licensing**: keep dependencies compatible with Apache 2.0 (no GPL encoders such as x264; LGPL FFmpeg builds only, dynamically linked).

## How to help

Pick a phase, open an issue to agree on the approach, and see [CONTRIBUTING.md](../CONTRIBUTING.md).
