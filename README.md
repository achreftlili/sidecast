# Sidecast

Turn your **Android phone, iPhone or iPad** into an **extended display, touchpad, keyboard and microphone** for your **Mac**: over Wi-Fi at home, or from anywhere through Tailscale.

Free and open source (Apache 2.0). No account, no cloud, no telemetry: the phone talks straight to your Mac, encrypted end to end. You build and install it yourself from this repository; nothing is sold or tracked.

- **A real second monitor**: the Mac gets a virtual display that you arrange in System Settings → Displays like any screen.
- **Touch and trackpad**: tap, right-click, drag, scroll, pinch, 3/4-finger swipes for Spaces and Mission Control, and a trackpad mode with a visible pointer for precise apps like VS Code.
- **Type from the phone**, including the phone keyboard's voice typing.
- **Phone microphone as a Mac microphone** ("Sidecast Microphone").
- **Keeps the Mac awake** while a phone is connected (optional).
- **Remote access** over [Tailscale](https://tailscale.com): LAN first, Tailscale when you're away, data saver on mobile data.

The Mac creates a virtual monitor, captures it with ScreenCaptureKit, encodes it with the hardware HEVC/H.264 encoder and streams it over TLS. The phone decodes it full screen and sends touches back as mouse and keyboard input. Design decisions: [docs/DECISIONS.md](docs/DECISIONS.md). Wire protocol: [docs/PROTOCOL.md](docs/PROTOCOL.md). Where it's going (Windows, Linux, iOS): [docs/ROADMAP.md](docs/ROADMAP.md).

**Requirements**: a Mac with macOS 15 or later (Apple Silicon; Intel builds compile but are untested) and an Android 10+ phone, or an iPhone / iPad with iOS 17 or later (see [iPhone / iPad](#iphone--ipad)). A Windows 10/11 PC works too, experimentally: see [Windows computer](#windows-computer-experimental). A Linux computer with GNOME on Wayland works too, experimentally: see [Linux computer](#linux-computer-experimental).

## How Sidecast compares

Using an Android device as a Mac display isn't new: Duet Display, Wing Display and Splashtop are commercial options, and [SideScreen](https://github.com/tranvuongquocdat/SideScreen), which Sidecast builds on, is open source. What Sidecast adds:

- **Everything is free**: commercial apps charge for features such as touch control or wireless use. Here everything is Apache 2.0, with no account and no subscription.
- **More than a screen**: touch, a trackpad mode with a visible pointer, Mac trackpad gestures, typing and the phone's microphone, in one app.
- **Works away from home**: LAN first, then Tailscale automatically, with nothing opened on your router and a data saver on mobile data.
- **Secure by default**: TLS on every connection, the Mac's certificate pinned at pairing, and a token per device.
- **Built for phones**: most alternatives target tablets; Sidecast is designed around the phone you already carry.

Where the alternatives are ahead today: they install from an app store instead of from source, many offer a USB connection for lower latency, and some also run on Windows. [docs/ROADMAP.md](docs/ROADMAP.md) lists what's planned.

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

### iPhone / iPad

You need **Xcode** (the full app, not only the Command Line Tools) and an **Apple ID**. A **free Apple ID works, but iOS stops opening the app after 7 days**: run `make install-ios` again to renew it (pairings and settings are kept). A paid Apple Developer account makes an install last a year.

1. Xcode → **Settings → Accounts**: sign in with your Apple ID (a free one shows "Personal Team").
2. Copy `ios/Local.xcconfig.example` to `ios/Local.xcconfig` and fill in your **team ID** and a **bundle identifier of your own** (e.g. `com.yourname.sidecast`; Apple refuses one that another team already uses).
3. Connect the iPhone or iPad with a cable, unlock it and tap **Trust**. Turn on **Settings → Privacy & Security → Developer Mode** (the device restarts).
4. Run:

```bash
make install-ios
```

It builds the app, installs it and opens it. The first time, iOS asks you to trust the developer: **Settings → General → VPN & Device Management → your Apple ID → Trust**, then open Sidecast. Allow **Local Network** when asked, so it can find your computers.

An iPad makes a great second screen: the Apple Pencil works like a finger, and a hardware keyboard types on the computer too. The display screen is landscape and keeps the device awake. iOS doesn't let apps run in the background, so the stream stops when you switch apps and reconnects when you come back.

### Windows computer (experimental)

A Windows 10 or 11 PC can be the computer instead of a Mac, with the same phone apps. It is new: it passes its tests on Windows in CI, but it has not been tried on a real PC with a phone yet, so reports are welcome.

You need Git, **Rust** (`winget install Rustlang.Rustup`) and the **Microsoft C++ build tools** (`winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"`, or *Desktop development with C++* in the Visual Studio Installer). Then, in PowerShell:

```powershell
git clone https://github.com/achreftlili/sidecast.git
cd sidecast
powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1
```

It checks the tools, builds Sidecast, installs it for your account in `%LOCALAPPDATA%\Programs\Sidecast` (no administrator rights), adds it to the Start menu and starts it; add `-Autostart` to start it at login too (or use **Start at login** in its menu). It lives in the notification area: click **^** on the taskbar if the icon is hidden. The first time, **Windows Defender Firewall** asks whether Sidecast may accept connections: allow **Private networks**, and make sure your Wi-Fi is a private network (Settings → Network & internet → Wi-Fi).

- **An extra screen** needs the free, signed [Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver/releases): install it from its official release (its *Virtual Driver Control* app installs the driver). Windows only loads drivers signed by Microsoft, so Sidecast doesn't ship one of its own. While a phone is connected, Sidecast turns one of the driver's monitors on at the phone's resolution, to the right of your screens, and turns it off again afterwards. Set its scale (e.g. 200 % for a sharp picture) in **Settings → System → Display**, where Windows remembers it; if you move it there, Sidecast puts it back in that place for the next connection. The first time a phone uses a new resolution, Sidecast adds it to the driver's settings and restarts the driver, so your screens flicker once.
- **Without the driver**, the phone shows an existing monitor: a second monitor if you have one, otherwise your main screen (mirroring). Pick one in the menu under **Screen**.
- **The phone's microphone** needs a virtual audio cable such as [VB-CABLE](https://vb-audio.com/Cable/): Sidecast plays the phone into **CABLE Input**, and Windows apps record **CABLE Output**. The phone records only while an app is listening. Without a cable, the microphone isn't offered.
- **Gestures** are the Mac's, with Windows shortcuts: three/four-finger swipes left and right switch virtual desktops (Win+Ctrl+←/→), up opens Task View (Win+Tab), down shows the desktop (Win+D); a pinch zooms (Ctrl+wheel).
- **The menu** shows the status, the address for **Add by IP**, the pairing code (also in a notification and a large window while a phone pairs), **Phone controls this computer**, **Paired phones** (Forget), **Start at login**, **Open log** and **Quit**.
- Windows lets no app click or type into apps running as administrator, nor capture the sign-in and UAC screens: there the phone's picture pauses and its touches are ignored.

`sidecast-host.exe --diagnose` (in the install folder) lists what Sidecast found: monitors, the driver, encoders and an audio cable; `sidecast-host.exe` alone runs it with a console log. Pairings and settings live in `%APPDATA%\Sidecast`, the log in `%LOCALAPPDATA%\Sidecast\sidecast.log`. Details: [rust/README.md](rust/README.md).

### Linux computer (experimental)

Ubuntu (or another distribution) with **GNOME on Wayland**, the default session of Ubuntu 22.04 and later. The phone gets a real extra monitor through GNOME's own screen-cast API, touches and the keyboard go through GNOME's remote-desktop API, and the phone's microphone becomes a "Sidecast Microphone" input. It works with the same phone app, but it is new: it has been tested end to end on a headless GNOME in CI, **not yet on a real desktop with a real phone**. Reports are welcome.

```bash
sudo apt install build-essential pkg-config clang libpipewire-0.3-dev
# optional, hardware encoding (VA-API / NVENC) instead of OpenH264 on the CPU:
sudo apt install libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-bad
curl -sSf https://sh.rustup.rs | sh     # Rust, if you don't have it
make doctor
make install-linux                      # AUTOSTART=1 make install-linux to start it at login
```

`make install-linux` builds the host and installs `~/.local/bin/sidecast-host` and a **Sidecast** entry in the app grid. Start it from there (or run `sidecast-host` in a terminal to see its log). On the phone, add the computer as usual; the 6-digit code appears as a notification, in the tray menu and in the log. Ubuntu shows the tray icon (status, paired phones with **Forget**, **Quit**) through its AppIndicator extension; on plain GNOME install the "AppIndicator and KStatusNotifierItem Support" extension, or use `--no-tray`.

Good to know:
- GNOME shows its "screen is being shared" indicator while a phone is connected. The virtual monitor appears to the right of your screens at 200 % (100 % if you turned HiDPI off on the phone, or if GNOME's fractional scaling is off); you can move it in **Settings → Displays** while it is connected.
- **3/4-finger swipes**: left/right switch workspaces (Super+Page Down/Up), up opens the Activities overview, down switches to the app's next window (Alt+\`, GNOME has no App Exposé).
- **Typing**: ASCII is typed directly; other characters (é, emoji) use GNOME's Ctrl+Shift+U Unicode entry, which works in GTK, Qt, Firefox and Chromium apps. If your keyboard layout has keys for them, `SIDECAST_TEXT_INPUT=keysyms sidecast-host` types them directly instead.
- **Mirror a screen instead** of adding one: `sidecast-host --monitor DP-1` (or `primary`; `sidecast-host --diagnose` lists the monitors). A screen of another shape than the phone shows between black bars.
- **Microphone**: choose "Sidecast Microphone" in Settings → Sound or in the app; the phone records only while an app listens.
- **Keeps the computer awake** while a phone is connected (systemd-logind); the screen lock and closing the lid behave as usual.
- **Other desktops**: on KDE Plasma 6 and wlroots compositors (Sway, Hyprland…) the host uses the desktop portal instead, which asks before sharing (a virtual monitor where the portal offers one, otherwise it mirrors a screen); this path is untested. **X11 sessions are not supported** yet.
- Uninstall: `scripts/install-linux.sh --uninstall` (pairings stay in `~/.local/share/sidecast`).

### Updating

```bash
git pull
make install-mac
make install-android   # or: make install-ios
```

On Windows: `git pull`, then run `scripts\install-windows.ps1` again (it quits the running copy first).

On Linux: `git pull`, then `make install-linux` again.

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

Runs the real server/encoder/TLS/flow-control path with a synthetic test pattern. No virtual display, capture, input, sleep changes or prompts. Connect with `"$(cd mac && swift build -c release --show-bin-path)/sidecast-probe" --host 127.0.0.1 --port 47899 --code 123456 --seconds 5`, from the Android emulator via **Add by IP** → `10.0.2.2:47899`, or from the iOS Simulator via **+** → `127.0.0.1:47899` (open `ios/Sidecast.xcodeproj` and run it; no Apple ID needed). `ios/SidecastUITests` drives that last one end to end.

## Troubleshooting

- **`make doctor` reports a problem**: follow the hint printed under it.
- **Phone doesn't list the Mac**: some routers block Bonjour (guest/hotel/office networks). Use **Add by IP address**; the Mac's IP is in System Settings → Wi-Fi → Details.
- **Permissions asked again after updating**: run `make mac-signing` once, then `make install-mac`, and grant them one last time.
- **`make install-android` says the signatures don't match**: the phone has a Sidecast built on another computer. Uninstall it (`adb uninstall com.achreftlili.sidecast`, which forgets its pairings) and run it again.
- **"This computer's identity changed"**: the Mac's certificate was regenerated (e.g. its data folder was deleted). Forget it on the phone (long-press) and pair again.
- **Black / laggy picture**: lower the max bitrate or fps in Settings…; prefer 5 GHz Wi-Fi.

## Development

```bash
make test
```

Runs the Mac (XCTest) and Android (JUnit) unit tests, including the shared protocol test vectors and the iOS client's logic (in SidecastKit). `make test-ios` adds the iOS app's own tests on a simulator. See [CONTRIBUTING.md](CONTRIBUTING.md).

The Windows and Linux host is built in Rust ([rust/README.md](rust/README.md)): a shared core, the host server, the Windows backends and tray app, the Linux backends, and a conformance probe. `make test-rust` runs its tests and `make conformance` checks every probe against every host (the Rust host, and on macOS the Mac app) in test-pattern mode. On Windows, `scripts/windows-e2e.sh` (Git Bash) streams a real monitor to the probe; on Linux, `make e2e-linux` runs the real GNOME backend on a headless GNOME (Mutter, PipeWire) against the probe.

```
mac/       Swift package: SidecastKit (tested logic, incl. the iOS client's), Sidecast (app), SidecastProbe (CLI test client), mic driver
android/   Gradle project: protocol/ net/ video/ input/ audio/ store/ ui/
ios/       iPhone / iPad app (UIKit) on SidecastKit; Sidecast.xcodeproj is generated from project.yml (XcodeGen)
rust/      Cargo workspace: sidecast-core, sidecast-server, sidecast-host (Windows host and tray app, Linux host), sidecast-probe
docs/      DECISIONS.md, PROTOCOL.md, ROADMAP.md
scripts/   doctor.sh, conformance.sh, install-windows.ps1, windows-e2e.sh, install-linux.sh, linux-e2e.sh
dist/      build outputs (gitignored)
```

Optional Developer ID signing and notarization for maintainers who distribute builds: set `SIDECAST_SIGN_IDENTITY` and `SIDECAST_NOTARY_PROFILE` before `make mac`.

## License

[Apache License 2.0](LICENSE). Includes code derived from SideScreen (MIT); see [NOTICE](NOTICE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
