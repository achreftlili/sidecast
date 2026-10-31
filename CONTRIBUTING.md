# Contributing to Sidecast

Thanks for helping! Bug reports, fixes, docs and new platforms are all welcome.

## Getting set up

```bash
make doctor     # checks your tools and says how to fix what's missing
make test       # Mac (XCTest, needs Xcode) and Android (JUnit) unit tests
make install-mac
make install-android
```

You can work on the Mac side without a phone: see "Testing without a phone" in the [README](README.md).

## How the code is organised

- `mac/Sources/SidecastKit`: pure, unit-tested logic (protocol codec, pairing, flow control, gesture engine, keyboard mapping, audio jitter buffer), and in `Client/` the iOS client's (connect planner, session state machine, touch, keyboard, microphone). It builds for macOS and iOS: no AppKit, UIKit or capture code here.
- `ios/`: the iPhone / iPad app (UIKit). `Sidecast.xcodeproj` is generated from `project.yml` with [XcodeGen](https://github.com/yonaskolb/XcodeGen): edit `project.yml`, run `xcodegen` in `ios/` and commit both. `make test-ios` runs its unit tests on a simulator; the end-to-end UI test needs a test-pattern host (see the top of `ios/SidecastUITests/EndToEndUITests.swift`).
- `mac/Sources/Sidecast`: the menu-bar app (server, capture, encoding, virtual display, input injection, UI).
- `mac/Sources/SidecastMicDriver`: the "Sidecast Microphone" Core Audio driver (C).
- `android/app/src/main/java/.../`: `protocol/` and `net/` (codec, TLS, session), `video/`, `input/`, `audio/`, `store/`, `ui/`.
- `rust/`: the Windows/Linux host in Rust: `sidecast-core` (pure, tested logic shared by the new hosts), `sidecast-server` (the host and the backend traits platform code implements), `sidecast-host` (the CLI, the Windows backends in `src/platform/windows/` and the Windows tray app in `src/tray/`, the Linux backends and tray icon in `src/platform/linux/`), `sidecast-probe` (conformance client). See [rust/README.md](rust/README.md); `make test-rust` and `make conformance` run its checks (Rust is optional for Mac and Android work). Windows-only code builds and runs its tests in CI's Windows jobs; on a Windows PC, `scripts/windows-e2e.sh` (Git Bash) also streams a real monitor to the probe. On Linux, `make e2e-linux` runs the real GNOME backend on a headless GNOME (Mutter, PipeWire) against the probe.
- `docs/PROTOCOL.md`: the wire protocol, the contract between the two sides.

## Ground rules

- **The protocol is the contract.** Any change to the wire format updates `docs/PROTOCOL.md` (including the §8 test vectors) and every codec (Swift, Kotlin, Rust `sidecast-core`) and their tests in the same pull request. Keep changes backward compatible, or bump the protocol version.
- **Put logic where it can be tested**: `SidecastKit` on the Mac, non-UI packages on Android, `sidecast-core` in Rust, each with unit tests.
- **Record decisions.** A new behaviour or trade-off gets a line in `docs/DECISIONS.md`.
- **Match the code around you**: naming, comment density, idioms. Comments explain *why*.
- **Keep dependencies minimal.** The Mac app uses only Apple frameworks, the Android app only AndroidX, Material Components and Kotlin coroutines, and the Rust crates only permissively licensed crates (MIT, Apache-2.0, BSD, ISC, Zlib; no GPL or LGPL), listed in THIRD_PARTY_NOTICES.md. System libraries under the LGPL (GStreamer on Linux) may only be linked dynamically, behind an optional feature. Discuss in an issue before adding a library.
- **Never weaken security** (TLS, pinning, pairing, tokens) to make something easier. See [SECURITY.md](SECURITY.md).

## New platforms

The Windows and Linux hosts are experimental and the iOS client a preview, and all need testing on real devices (Windows: say which GPU, Windows version, and whether the Virtual Display Driver or a monitor was streamed; Linux: the distribution, desktop and GPU). If you want to work on one, open an issue first so we can agree on the approach. A new client or host is done when it passes the same protocol test vectors and works with the existing apps.

## Pull requests

- One topic per pull request, with a clear description of what and why.
- `make test` passes, and you tried the change on a real device when it touches capture, input, audio or networking (say which models and OS versions).
- CI must be green.

## License

By contributing, you agree that your contributions are licensed under the [Apache License 2.0](LICENSE), like the rest of the project.
