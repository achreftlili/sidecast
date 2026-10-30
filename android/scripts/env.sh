# Sourced by the Android scripts (bash). Finds a JDK (17+, 21 preferred) and the
# Android SDK when JAVA_HOME / ANDROID_HOME are not set, on macOS and Linux.
#
#   sidecast_find_java     sets JAVA_HOME, or returns 1
#   sidecast_find_sdk      sets ANDROID_HOME, or returns 1
#   sidecast_android_env   both, exported, plus android/local.properties; exits with a hint when missing
#   sidecast_adb           prints the adb path, or returns 1

SIDECAST_ANDROID_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Major version of the JDK at $1 (e.g. 21), or nothing.
_sidecast_java_major() {
    [[ -x "$1/bin/java" ]] || return 1
    local v
    v="$("$1/bin/java" -version 2>&1 | awk -F'"' '/version/ {print $2; exit}')"
    v="${v#1.}"
    echo "${v%%[.+_-]*}"
}

sidecast_find_java() {
    local candidates=() c major
    [[ -n "${JAVA_HOME:-}" ]] && candidates+=("$JAVA_HOME")
    if [[ -x /usr/libexec/java_home ]]; then
        c="$(/usr/libexec/java_home -v 21 2>/dev/null)" && candidates+=("$c")
    fi
    candidates+=(
        /opt/homebrew/opt/openjdk@21 /usr/local/opt/openjdk@21
        "/Applications/Android Studio.app/Contents/jbr/Contents/Home"
        /usr/lib/jvm/java-21-openjdk-amd64 /usr/lib/jvm/java-21-openjdk-arm64 /usr/lib/jvm/java-21-openjdk
        /usr/lib/jvm/temurin-21-jdk-amd64 /usr/lib/jvm/temurin-21-jdk-arm64
        /opt/android-studio/jbr "$HOME/android-studio/jbr"
    )
    if [[ -x /usr/libexec/java_home ]]; then
        c="$(/usr/libexec/java_home -v 17+ 2>/dev/null)" && candidates+=("$c")
    fi
    if command -v java >/dev/null 2>&1 && command -v readlink >/dev/null 2>&1; then
        c="$(readlink -f "$(command -v java)" 2>/dev/null)" && candidates+=("$(dirname "$(dirname "$c")")")
    fi
    for c in "${candidates[@]}"; do
        major="$(_sidecast_java_major "$c")" || continue
        if [[ -n "$major" && "$major" -ge 17 ]]; then
            JAVA_HOME="$c"
            return 0
        fi
    done
    return 1
}

sidecast_find_sdk() {
    local candidates=() c
    [[ -n "${ANDROID_HOME:-}" ]] && candidates+=("$ANDROID_HOME")
    [[ -n "${ANDROID_SDK_ROOT:-}" ]] && candidates+=("$ANDROID_SDK_ROOT")
    if [[ -f "$SIDECAST_ANDROID_DIR/local.properties" ]]; then
        c="$(sed -n 's/^sdk\.dir=//p' "$SIDECAST_ANDROID_DIR/local.properties" | head -1)"
        [[ -n "$c" ]] && candidates+=("$c")
    fi
    candidates+=(
        "$HOME/Library/Android/sdk"
        /opt/homebrew/share/android-commandlinetools /usr/local/share/android-commandlinetools
        "$HOME/Android/Sdk" /opt/android-sdk /usr/lib/android-sdk
    )
    for c in "${candidates[@]}"; do
        if [[ -d "$c/platforms" || -d "$c/cmdline-tools" || -d "$c/platform-tools" ]]; then
            ANDROID_HOME="$c"
            return 0
        fi
    done
    return 1
}

sidecast_android_env() {
    if ! sidecast_find_java; then
        echo "error: no JDK 17+ found. Install JDK 21 (macOS: brew install openjdk@21," >&2
        echo "       Ubuntu: sudo apt install openjdk-21-jdk) or set JAVA_HOME. See: make doctor" >&2
        exit 1
    fi
    if ! sidecast_find_sdk; then
        echo "error: no Android SDK found. Install Android Studio or the Android command-line tools" >&2
        echo "       (macOS: brew install --cask android-commandlinetools), or set ANDROID_HOME. See: make doctor" >&2
        exit 1
    fi
    export JAVA_HOME ANDROID_HOME
    if [[ ! -f "$SIDECAST_ANDROID_DIR/local.properties" ]]; then
        echo "sdk.dir=$ANDROID_HOME" > "$SIDECAST_ANDROID_DIR/local.properties"
    fi
}

sidecast_adb() {
    if [[ -n "${ANDROID_HOME:-}" && -x "$ANDROID_HOME/platform-tools/adb" ]]; then
        echo "$ANDROID_HOME/platform-tools/adb"
    elif command -v adb >/dev/null 2>&1; then
        command -v adb
    else
        return 1
    fi
}
