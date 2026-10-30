#!/usr/bin/env bash
# Builds, tests and packages the Android app:
#   <repo>/dist/Sidecast-<VERSION>.apk
# Signed with android/keystore.properties when present, otherwise with this
# computer's debug key (fine for installing on your own phone). With a keystore
# it also builds the Play bundle <repo>/dist/Sidecast-<VERSION>.aab.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/env.sh"
sidecast_android_env
ANDROID_DIR="$SIDECAST_ANDROID_DIR"
REPO_DIR="$(cd "$ANDROID_DIR/.." && pwd)"

VERSION="$(tr -d '[:space:]' < "$REPO_DIR/VERSION")"
if [[ -z "$VERSION" ]]; then
  echo "error: $REPO_DIR/VERSION is empty" >&2
  exit 1
fi

TASKS=(clean testDebugUnitTest assembleRelease)
[[ -f "$ANDROID_DIR/keystore.properties" ]] && TASKS+=(bundleRelease)

cd "$ANDROID_DIR"
./gradlew --no-daemon "${TASKS[@]}"

APK="$ANDROID_DIR/app/build/outputs/apk/release/app-release.apk"
if [[ ! -f "$APK" ]]; then
  echo "error: $APK was not produced" >&2
  exit 1
fi

mkdir -p "$REPO_DIR/dist"
OUT="$REPO_DIR/dist/Sidecast-$VERSION.apk"
cp "$APK" "$OUT"
echo "Built $OUT"

AAB="$ANDROID_DIR/app/build/outputs/bundle/release/app-release.aab"
if [[ -f "$AAB" ]]; then
  cp "$AAB" "$REPO_DIR/dist/Sidecast-$VERSION.aab"
  echo "Built $REPO_DIR/dist/Sidecast-$VERSION.aab"
fi
