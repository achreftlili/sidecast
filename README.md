# Sidecast

Turn your **Android phone** into an **extended display, touchpad, keyboard and microphone** for your **Mac**: over Wi-Fi at home, or from anywhere through Tailscale.

Free and open source (Apache 2.0). No account, no cloud, no telemetry: the phone talks straight to your Mac, encrypted end to end. You build and install it yourself from this repository; nothing is sold or tracked.

- **A real second monitor**: the Mac gets a virtual display that you arrange in System Settings → Displays like any screen.
- **Touch and trackpad**: tap, right-click, drag, scroll, pinch, 3/4-finger swipes for Spaces and Mission Control, and a trackpad mode with a visible pointer for precise apps like VS Code.
- **Type from the phone**, including the phone keyboard's voice typing.
- **Phone microphone as a Mac microphone** ("Sidecast Microphone").
- **Keeps the Mac awake** while a phone is connected (optional).
- **Remote access** over [Tailscale](https://tailscale.com): LAN first, Tailscale when you're away, data saver on mobile data.

The Mac creates a virtual monitor, captures it with ScreenCaptureKit, encodes it with the hardware HEVC/H.264 encoder and streams it over TLS. The phone decodes it full screen and sends touches back as mouse and keyboard input. Design decisions: [docs/DECISIONS.md](docs/DECISIONS.md). Wire protocol: [docs/PROTOCOL.md](docs/PROTOCOL.md). Where it's going (Windows, Linux, iOS): [docs/ROADMAP.md](docs/ROADMAP.md).

**Requirements**: a Mac with macOS 15 or later (Apple Silicon; Intel builds compile but are untested) and an Android 10+ phone.

## How Sidecast compares

Using an Android device as a Mac display isn't new: Duet Display, Wing Display and Splashtop are commercial options, and [SideScreen](https://github.com/tranvuongquocdat/SideScreen), which Sidecast builds on, is open source. What Sidecast adds:

- **Everything is free**: commercial apps charge for features such as touch control or wireless use. Here everything is Apache 2.0, with no account and no subscription.
- **More than a screen**: touch, a trackpad mode with a visible pointer, Mac trackpad gestures, typing and the phone's microphone, in one app.
- **Works away from home**: LAN first, then Tailscale automatically, with nothing opened on your router and a data saver on mobile data.
- **Secure by default**: TLS on every connection, the Mac's certificate pinned at pairing, and a token per device.
- **Built for phones**: most alternatives target tablets; Sidecast is designed around the phone you already carry.

Where the alternatives are ahead today: they install from an app store instead of from source, many offer a USB connection for lower latency, and some also run on iPad or Windows. [docs/ROADMAP.md](docs/ROADMAP.md) lists what's planned.

## Install from source

You need **Xcode** (free, App Store; the Command Line Tools alone are enough to install, Xcode is needed to run the tests), **JDK 21** and the **Android SDK**. On a Mac with [Homebrew](https://brew.sh):

```bash
xcode-select --install
brew install openjdk@21
brew install --cask android-commandlinetools
yes | sdkmanager --licenses
sdkmanager "platform-tools"
```

Then:

```bash
git clone https://github.com/achreftlili/sidecast.git
cd sidecast
make doctor
```

`make doctor` checks everything and tells you how to fix what's missing.

### Mac

```bash
make mac-signing
make install-mac
```

`make mac-signing` (once per Mac) creates a personal code-signing certificate in your login keychain, so macOS keeps Sidecast's permissions when you update it. The private key never leaves your Mac. If macOS asks to let `codesign` use the key, enter your login password and choose **Always Allow**.

`make install-mac` builds Sidecast, installs it in `/Applications` and opens it. It lives in the menu bar (no Dock icon). Because you built it on this Mac, macOS opens it without a security warning.

### Android phone

1. On the phone: **Settings → About phone → tap Build number 7 times**, then **Settings → Developer options → USB debugging** (or **Wireless debugging**).
2. Connect the phone with a cable and accept the prompt on the phone.
3. Run:

```bash
make install-android
```

It builds the app, installs it over adb and opens it. Installing over adb works on every Android phone, including those with Google's developer-verification rules.

### Updating

```bash
git pull
make install-mac
make install-android
```

Pairings and settings are kept.

## First run

1. Phone and Mac on the same Wi-Fi (or the Mac joined to the phone's hotspot).
2. On the phone, tap your Mac in the list (or **Add by IP address**, port 47800).
3. The Mac shows a 6-digit code; type it on the phone. Pairing happens once.
4. Grant the Mac permissions (the menu-bar items **Grant Screen Recording…** / **Grant Accessibility…** open the right place):
   - **Screen Recording**: required to stream the virtual display.
   - **Accessibility**: required for touch → mouse and keyboard input.
   - **Local Network**: allow, so the phone can find the Mac.
5. Optional: menu bar → **Keep Mac awake while connected…** asks for your admin password once and installs a sudoers rule that allows only `pmset -a disablesleep 0|1`. While a phone is connected the Mac won't sleep, even with the lid closed. Sleep comes back 30 s after the phone disconnects, and is restored if Sidecast crashes. Remove it with **Remove sleep permission…** (do this *before* deleting the app).

Arrange the new display in System Settings → Displays like any monitor.

### Touch gestures
- Tap = click · double tap = double click · long press or **two-finger tap** = right click · long press then drag = drag
- One/two-finger swipe = scroll · pinch = zoom
- **Three/four-finger swipe** left/right = switch Desktops and full-screen apps · up = Mission Control · down = App Exposé (uses the Mission Control shortcuts in System Settings → Keyboard → Keyboard Shortcuts, on by default)
- Three-finger tap = stats overlay on the phone
- **Trackpad mode** (button on the phone): the phone works like a laptop trackpad with a large visible pointer. Tap clicks where the pointer is; best for small targets such as editor tabs.

### Keyboard and microphone
- Tap the **Keyboard** button on the phone to type into the focused Mac app; the phone keyboard's dictation works too.
- Mac: Settings → Phone microphone → **Install…** adds a "Sidecast Microphone" input (admin password once). While the phone is connected it becomes the default input; the phone records only while a Mac app is listening (red **Mic** chip on the phone).

### Away from home (Tailscale)
Install [Tailscale](https://tailscale.com) on the Mac and the phone and sign both into the same tailnet (the free plan is enough). Sidecast then tries Wi-Fi first and falls back to Tailscale automatically. Nothing is opened on your router. On mobile data, **Data saver** (phone settings) limits the stream to 4 Mbps and 30 fps. Your Mac must stay awake: see Settings → Remote access on the Mac.

## Security

Every connection uses TLS, and the phone pins the Mac's certificate at pairing. Pairing needs the code shown on the Mac, and every later connection authenticates with a per-device token. A paired phone can see the virtual display and control the Mac's mouse and keyboard, so only pair your own devices; remove one with **Forget** in the Mac's Settings. Details and how to report a vulnerability: [SECURITY.md](SECURITY.md).

## Testing without a phone

```bash
dist/Sidecast.app/Contents/MacOS/Sidecast --test-pattern --data-dir /tmp/sidecast-test --port 47899 --test-pairing-code 123456 --no-bonjour
```

Runs the real server/encoder/TLS/flow-control path with a synthetic test pattern. No virtual display, capture, input, sleep changes or prompts. Connect with `"$(cd mac && swift build -c release --show-bin-path)/sidecast-probe" --host 127.0.0.1 --port 47899 --code 123456 --seconds 5`, or from the Android emulator via **Add by IP** → `10.0.2.2:47899`.

## Troubleshooting

- **`make doctor` reports a problem**: follow the hint printed under it.
- **Phone doesn't list the Mac**: some routers block Bonjour (guest/hotel/office networks). Use **Add by IP address**; the Mac's IP is in System Settings → Wi-Fi → Details.
- **Permissions asked again after updating**: run `make mac-signing` once, then `make install-mac`, and grant them one last time.
- **`make install-android` says the signatures don't match**: the phone has a Sidecast built on another computer. Uninstall it (`adb uninstall com.achreftlili.sidecast`, which forgets its pairings) and run it again.
- **"This Mac's identity changed"**: the Mac's certificate was regenerated (e.g. its data folder was deleted). Forget it on the phone (long-press) and pair again.
- **Black / laggy picture**: lower the max bitrate or fps in Settings…; prefer 5 GHz Wi-Fi.

## Development

```bash
make test
```

Runs the Mac (XCTest) and Android (JUnit) unit tests, including the shared protocol test vectors. See [CONTRIBUTING.md](CONTRIBUTING.md).

```
mac/       Swift package: SidecastKit (tested logic), Sidecast (app), SidecastProbe (CLI test client), mic driver
android/   Gradle project: protocol/ net/ video/ input/ audio/ store/ ui/
docs/      DECISIONS.md, PROTOCOL.md, ROADMAP.md
scripts/   doctor.sh
dist/      build outputs (gitignored)
```

Optional Developer ID signing and notarization for maintainers who distribute builds: set `SIDECAST_SIGN_IDENTITY` and `SIDECAST_NOTARY_PROFILE` before `make mac`.

## License

[Apache License 2.0](LICENSE). Includes code derived from SideScreen (MIT); see [NOTICE](NOTICE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
