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
if (( FAILED )); then
    echo "Fix the ✗ items above, then run 'make doctor' again."
    exit 1
fi
if [[ "$(uname -s)" == Darwin ]]; then
    echo "Ready. Next: make install-mac, then make install-android (or make install-ios) with the phone connected."
else
    echo "Ready to build the Android app: make install-android with the phone connected."
    echo "(The computer side runs on macOS for now; Windows and Linux are on the roadmap.)"
fi
