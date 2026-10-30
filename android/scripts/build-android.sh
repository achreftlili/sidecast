#!/usr/bin/env bash
# Builds, tests and packages the Android app:
#   <repo>/dist/Sidecast-<VERSION>.apk
# Signed with android/keystore.properties when present, otherwise with the local debug key
# (fine for personal sideloading).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANDROID_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$ANDROID_DIR/.." && pwd)"

export JAVA_HOME="${JAVA_HOME:-/opt/homebrew/opt/openjdk@21}"
export ANDROID_HOME="${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}"

VERSION="$(tr -d '[:space:]' < "$REPO_DIR/VERSION")"
if [[ -z "$VERSION" ]]; then
  echo "error: $REPO_DIR/VERSION is empty" >&2
  exit 1
fi

if [[ ! -f "$ANDROID_DIR/local.properties" ]]; then
  echo "sdk.dir=$ANDROID_HOME" > "$ANDROID_DIR/local.properties"
fi

cd "$ANDROID_DIR"
./gradlew --no-daemon clean testDebugUnitTest assembleRelease bundleRelease

APK="$ANDROID_DIR/app/build/outputs/apk/release/app-release.apk"
if [[ ! -f "$APK" ]]; then
  echo "error: $APK was not produced" >&2
  exit 1
fi

mkdir -p "$REPO_DIR/dist"
OUT="$REPO_DIR/dist/Sidecast-$VERSION.apk"
cp "$APK" "$OUT"
echo "Built $OUT"

# Play Store bundle (signed with the upload key from android/keystore.properties when present).
AAB="$ANDROID_DIR/app/build/outputs/bundle/release/app-release.aab"
if [[ -f "$AAB" ]]; then
  cp "$AAB" "$REPO_DIR/dist/Sidecast-$VERSION.aab"
  echo "Built $REPO_DIR/dist/Sidecast-$VERSION.aab"
fi
