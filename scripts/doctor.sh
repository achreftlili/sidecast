#!/usr/bin/env bash
# Checks that this computer can build and install Sidecast, and says how to fix
# what is missing.   make doctor
set -uo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$REPO_DIR/android/scripts/env.sh"

FAILED=0
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$*"; FAILED=1; }
hint() { printf '      %s\n' "$*"; }

if [[ "$(uname -s)" == Darwin ]]; then
    echo "Mac app (computer side)"
    macos="$(sw_vers -productVersion)"
    if (( ${macos%%.*} >= 15 )); then ok "macOS $macos"; else bad "macOS $macos: Sidecast needs macOS 15 or later"; fi
    ok "$(uname -m) Mac"

    if swift_version="$(xcrun swift --version 2>/dev/null | sed -n 's/.*Swift version \([0-9][0-9.]*\).*/\1/p' | head -1)" && [[ -n "$swift_version" ]]; then
        if (( ${swift_version%%.*} >= 6 )); then ok "Swift $swift_version"; else bad "Swift $swift_version: needs Swift 6 (Xcode 16 or later)"; fi
        developer_dir="$(xcode-select -p 2>/dev/null)"
        if [[ "$developer_dir" == *CommandLineTools* ]]; then
            warn "Command Line Tools only: building and installing work; 'make test' needs Xcode"
        else
            ok "Xcode ($developer_dir)"
        fi
    else
        bad "No Swift toolchain"
        hint "Install Xcode from the App Store, or run: xcode-select --install"
    fi

    if [[ "$(security find-identity -p codesigning 2>/dev/null)" == *'"Sidecast Local Signing"'* ]]; then
        ok "\"Sidecast Local Signing\" certificate (permissions survive updates)"
    else
        warn "No \"Sidecast Local Signing\" certificate: macOS will ask for permissions again after every update"
        hint "Fix once with: make mac-signing"
    fi
    echo
fi

echo "Android app (phone side)"
if sidecast_find_java; then
    ok "JDK $(_sidecast_java_major "$JAVA_HOME") ($JAVA_HOME)"
else
    bad "No JDK 17+ (21 recommended)"
    if [[ "$(uname -s)" == Darwin ]]; then hint "brew install openjdk@21"; else hint "sudo apt install openjdk-21-jdk"; fi
fi

if sidecast_find_sdk; then
    ok "Android SDK ($ANDROID_HOME)"
    if [[ -f "$ANDROID_HOME/licenses/android-sdk-license" ]]; then
        ok "SDK licenses accepted"
    else
        bad "Android SDK licenses not accepted"
        hint "Run: yes | sdkmanager --licenses   (sdkmanager is in $ANDROID_HOME/cmdline-tools/latest/bin)"
    fi
else
    bad "No Android SDK"
    if [[ "$(uname -s)" == Darwin ]]; then
        hint "brew install --cask android-commandlinetools   (or install Android Studio)"
    else
        hint "Install Android Studio, or the command-line tools from https://developer.android.com/studio#command-tools"
    fi
    hint "Or point ANDROID_HOME at an existing SDK."
fi

if adb="$(sidecast_adb)"; then
    ok "adb ($adb)"
    phones="$("$adb" devices 2>/dev/null | awk 'NR > 1 && $2 == "device"' | wc -l | tr -d ' ')"
    unauthorized="$("$adb" devices 2>/dev/null | awk 'NR > 1 && $2 == "unauthorized"' | wc -l | tr -d ' ')"
    if (( phones > 0 )); then
        ok "$phones phone(s) connected over adb"
    elif (( unauthorized > 0 )); then
        warn "A phone is connected but not authorized: accept the USB debugging prompt on the phone"
    else
        warn "No phone connected (needed only for 'make install-android')"
        hint "On the phone: Developer options → USB debugging (or Wireless debugging), then connect it"
    fi
else
    warn "adb not found (needed only for 'make install-android')"
    hint "Run: sdkmanager \"platform-tools\""
fi

# Optional: only warnings, never a failure.
if [[ "$(uname -s)" == Darwin ]]; then
    echo
    echo "iPhone / iPad app (optional)"
    if xcodebuild -version >/dev/null 2>&1; then
        ok "$(xcodebuild -version 2>/dev/null | head -1)"
        if xcodebuild -showsdks 2>/dev/null | grep -q -- '-sdk iphoneos'; then
            ok "iOS SDK"
        else
            warn "No iOS platform in Xcode"
            hint "Xcode → Settings → Components → iOS"
        fi
        if [[ -f "$REPO_DIR/ios/Local.xcconfig" ]] && grep -Eq '^[[:space:]]*DEVELOPMENT_TEAM[[:space:]]*=[[:space:]]*[A-Z0-9]{10}' "$REPO_DIR/ios/Local.xcconfig" \
            && ! grep -q 'ABCDE12345\|com.yourname.sidecast' "$REPO_DIR/ios/Local.xcconfig"; then
            ok "ios/Local.xcconfig (Apple team and bundle identifier)"
        else
            warn "No ios/Local.xcconfig with your Apple team and bundle identifier (needed only for 'make install-ios')"
            hint "cp ios/Local.xcconfig.example ios/Local.xcconfig and fill it in (a free Apple ID works)"
        fi
        devices="$(xcrun devicectl list devices --hide-headers 2>/dev/null | grep -Eci 'iphone|ipad' || true)"
        if (( devices > 0 )); then
            ok "$devices iPhone / iPad known to Xcode"
        else
            warn "No iPhone or iPad connected (needed only for 'make install-ios')"
            hint "Connect it with a cable, unlock it and tap Trust; enable Settings → Privacy & Security → Developer Mode"
        fi
    else
        warn "The iOS app needs Xcode (the Command Line Tools are not enough)"
        hint "Install Xcode from the App Store, open it once, then: sudo xcode-select -s /Applications/Xcode.app"
    fi
fi

echo
echo "Rust (optional: the Windows/Linux host, make test-rust, make conformance)"
# Warnings only: the Mac and Android apps do not need Rust.
if command -v cargo >/dev/null 2>&1 && rust_version="$(rustc --version 2>/dev/null)"; then
    ok "$rust_version"
    if cargo clippy --version >/dev/null 2>&1 && cargo fmt --version >/dev/null 2>&1; then
        ok "clippy and rustfmt"
    else
        warn "clippy or rustfmt missing (needed for make test-rust)"
        hint "rustup component add clippy rustfmt"
    fi
else
    warn "No Rust toolchain (needed only for the Rust host, make test-rust and make conformance)"
    hint "Install it with rustup: https://rustup.rs"
fi

if [[ "$(uname -s)" == Linux ]]; then
    echo
    echo "Linux computer (experimental host: make install-linux)"
    # Warnings only: a Linux machine may be used just to build the phone app.
    LINUX_PACKAGES=()
    if command -v pkg-config >/dev/null 2>&1; then
        ok "pkg-config"
    else
        warn "pkg-config missing"; LINUX_PACKAGES+=(pkg-config)
    fi
    if command -v cc >/dev/null 2>&1; then ok "C compiler"; else warn "No C compiler"; LINUX_PACKAGES+=(build-essential); fi
    if pkg-config --exists libpipewire-0.3 2>/dev/null; then
        ok "PipeWire $(pkg-config --modversion libpipewire-0.3) development files"
    else
        warn "PipeWire development files missing (screen capture and the microphone)"; LINUX_PACKAGES+=(libpipewire-0.3-dev)
    fi
    if ls /usr/lib/llvm-*/lib/libclang*.so* /usr/lib/*/libclang*.so* >/dev/null 2>&1; then
        ok "libclang (generates the PipeWire bindings at build time)"
    else
        warn "libclang missing (needed to build the PipeWire bindings)"; LINUX_PACKAGES+=(clang libclang-dev)
    fi
    if pkg-config --exists gstreamer-1.0 gstreamer-app-1.0 2>/dev/null; then
        ok "GStreamer $(pkg-config --modversion gstreamer-1.0) (hardware encoding through VA-API / NVENC)"
    else
        warn "No GStreamer development files: the host encodes with OpenH264 on the CPU (optional)"
        hint "For hardware encoding: sudo apt install libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-bad"
    fi
    if (( ${#LINUX_PACKAGES[@]} > 0 )); then
        hint "Install with: sudo apt install ${LINUX_PACKAGES[*]}"
    fi
    case "${XDG_SESSION_TYPE:-}:${XDG_CURRENT_DESKTOP:-}" in
        wayland:*GNOME*) ok "GNOME on Wayland (${XDG_CURRENT_DESKTOP}): the supported desktop" ;;
        wayland:*) warn "Wayland but not GNOME (${XDG_CURRENT_DESKTOP:-unknown}): the host uses the desktop portal, which asks before sharing (less tested)" ;;
        x11:*) warn "An X11 session: not supported yet"; hint "Log out, pick \"Ubuntu\" (Wayland) with the gear on the login screen" ;;
        *) warn "No graphical session detected here (over SSH?): run make doctor in the desktop session" ;;
    esac
fi

echo
if (( FAILED )); then
    echo "Fix the ✗ items above, then run 'make doctor' again."
    exit 1
fi
if [[ "$(uname -s)" == Darwin ]]; then
    echo "Ready. Next: make install-mac, then make install-android (or make install-ios) with the phone connected."
elif [[ "$(uname -s)" == Linux ]]; then
    echo "Ready to build the Android app: make install-android with the phone connected."
    echo "This computer as the host (experimental, GNOME on Wayland): make install-linux."
else
    echo "Ready to build the Android app: make install-android with the phone connected."
    echo "This computer as the host (experimental): see \"Windows computer\" in the README (scripts/install-windows.ps1)."
fi
