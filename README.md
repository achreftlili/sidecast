# Sidecast

Use an Android phone as an **extended display** for your Mac, over **Wi-Fi**.

- **Mac app**: menu-bar app, shipped as `dist/Sidecast-<version>.dmg` (Apple Silicon, macOS 15+).
- **Android app**: `dist/Sidecast-<version>.apk` (Android 10+).

The Mac creates a real virtual monitor, captures it with ScreenCaptureKit, encodes it with the hardware HEVC/H.264 encoder and streams it over TLS. The phone decodes it full-screen and sends touches back as mouse input. Design decisions: [docs/DECISIONS.md](docs/DECISIONS.md). Wire protocol: [docs/PROTOCOL.md](docs/PROTOCOL.md). Built on ideas and code from [SideScreen](https://github.com/tranvuongquocdat/SideScreen) (MIT, see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)).

## Install

### Mac
1. Open `dist/Sidecast-0.1.0.dmg` and drag **Sidecast** to **Applications**.
2. The app is ad-hoc signed (not notarized), so the first time: right-click **Sidecast** in Applications, choose **Open**, then **Open** again. If macOS still refuses, go to System Settings → Privacy & Security and click **Open Anyway**.
3. Sidecast appears in the menu bar (no Dock icon).

### Android
1. Copy `dist/Sidecast-0.1.0.apk` to the phone (or `adb install dist/Sidecast-0.1.0.apk`).
2. Allow installing from that source when Android asks, then open **Sidecast**.

## First run

1. Phone and Mac on the same Wi-Fi (or the Mac joined to the phone's hotspot).
2. On the phone, tap your Mac in the list (or **Add by IP address**, port 47800).
3. The Mac shows a 6-digit code; type it on the phone. Pairing happens once.
4. The Mac asks for permissions (menu-bar items **Grant Screen Recording…** / **Grant Accessibility…** take you to the right place):
   - **Screen Recording**: required to stream the virtual display.
   - **Accessibility**: required for touch → mouse input.
   - **Local Network**: allow, so the phone can find the Mac.
5. Optional: menu bar → **Keep Mac awake while connected…** asks for your admin password once and installs a sudoers rule that allows only `pmset -a disablesleep 0|1`. While a phone is connected, the Mac won't sleep even with the lid closed. Sleep comes back 30 s after the phone disconnects, and is restored if Sidecast crashes. Remove it with **Remove sleep permission…** (do this *before* deleting the app).

Arrange the new display in System Settings → Displays like any monitor.

### Touch gestures
- Tap = click · double tap = double click · long press or **two-finger tap** = right click · long press then drag = drag
- One/two-finger swipe = scroll · pinch = zoom
- **Three/four-finger swipe** left/right = switch Desktops and full-screen apps · up = Mission Control · down = App Exposé (uses the Mission Control shortcuts in System Settings → Keyboard → Keyboard Shortcuts, enabled by default)
- Three-finger tap = stats overlay on the phone

### Keyboard and microphone (0.2+ / 0.3+)
- Tap the keyboard button (bottom-right of the stream) to type on the Mac with the phone's keyboard; the keyboard's 🎤 dictation works too.
- Mac: Settings → Phone microphone → **Install…** adds a "Sidecast Microphone" input (admin password once). While the phone is connected it becomes the default input; the phone records only while a Mac app is listening (red "Mic" chip on the phone).

## Build from source

```bash
make all
```

Builds both into `dist/`. Requirements: Xcode 26+, JDK 21 (`/opt/homebrew/opt/openjdk@21`), Android SDK (`/opt/homebrew/share/android-commandlinetools`). Override with `JAVA_HOME` / `ANDROID_HOME`.

```bash
make test
```

Runs the Mac (XCTest) and Android (JUnit) unit tests, including the shared protocol test vectors.

Developer ID signing + notarization: set `SIDECAST_SIGN_IDENTITY` and `SIDECAST_NOTARY_PROFILE` before `make mac` (off by default).

### Testing without a phone

```bash
dist/Sidecast.app/Contents/MacOS/Sidecast --test-pattern --data-dir /tmp/sidecast-test --port 47899 --test-pairing-code 123456 --no-bonjour
```

Runs the real server/encoder/TLS/flow-control path with a synthetic test pattern. No virtual display, capture, input, sleep changes or prompts. Connect with `mac/.build/arm64-apple-macosx/release/sidecast-probe --host 127.0.0.1 --port 47899 --code 123456 --seconds 5`, or from the Android emulator via **Add by IP** → `10.0.2.2:47899`.

## Troubleshooting

- **Phone doesn't list the Mac**: some routers block Bonjour (guest/hotel/office networks). Use **Add by IP address**; the Mac's IP is in System Settings → Wi-Fi → Details.
- **Permissions asked again after rebuilding**: expected with ad-hoc signing; macOS ties permissions to the exact build.
- **"This Mac's identity changed"**: the Mac's certificate was regenerated (e.g. its data folder was deleted). Forget it on the phone (long-press) and pair again.
- **Black / laggy picture**: lower the max bitrate or fps in Settings…; prefer 5 GHz Wi-Fi.

## Layout

```
mac/       Swift package: SidecastKit (tested logic), Sidecast (app), SidecastProbe (CLI test client)
android/   Gradle project: protocol/ net/ video/ input/ store/ ui/
docs/      DECISIONS.md, PROTOCOL.md
dist/      build outputs (gitignored)
```

## License

[Apache License 2.0](LICENSE). Includes code derived from SideScreen (MIT); see [NOTICE](NOTICE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
