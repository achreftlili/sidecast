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

- `mac/Sources/SidecastKit`: the Mac's pure, unit-tested logic (protocol codec, pairing, flow control, gesture engine, keyboard mapping, audio jitter buffer). No AppKit or capture code here.
- `mac/Sources/Sidecast`: the menu-bar app (server, capture, encoding, virtual display, input injection, UI).
- `mac/Sources/SidecastMicDriver`: the "Sidecast Microphone" Core Audio driver (C).
- `android/app/src/main/java/.../`: `protocol/` and `net/` (codec, TLS, session), `video/`, `input/`, `audio/`, `store/`, `ui/`.
- `docs/PROTOCOL.md`: the wire protocol, the contract between the two sides.

## Ground rules

- **The protocol is the contract.** Any change to the wire format updates `docs/PROTOCOL.md` (including the §8 test vectors) and both codecs and their tests in the same pull request. Keep changes backward compatible, or bump the protocol version.
- **Put logic where it can be tested**: `SidecastKit` on the Mac, non-UI packages on Android, each with unit tests.
- **Record decisions.** A new behaviour or trade-off gets a line in `docs/DECISIONS.md`.
- **Match the code around you**: naming, comment density, idioms. Comments explain *why*.
- **Keep dependencies minimal.** The Mac app uses only Apple frameworks, and the Android app only AndroidX, Material Components and Kotlin coroutines. Discuss in an issue before adding a library.
- **Never weaken security** (TLS, pinning, pairing, tokens) to make something easier. See [SECURITY.md](SECURITY.md).

## New platforms

Windows and Linux computers and iOS phones are on the [roadmap](docs/ROADMAP.md). If you want to work on one, open an issue first so we can agree on the approach. A new client or host is done when it passes the same protocol test vectors and works with the existing apps.

## Pull requests

- One topic per pull request, with a clear description of what and why.
- `make test` passes, and you tried the change on a real device when it touches capture, input, audio or networking (say which models and OS versions).
- CI must be green.

## License

By contributing, you agree that your contributions are licensed under the [Apache License 2.0](LICENSE), like the rest of the project.
