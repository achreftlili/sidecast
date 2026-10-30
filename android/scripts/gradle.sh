#!/usr/bin/env bash
# Runs the Gradle wrapper with JAVA_HOME / ANDROID_HOME found automatically.
#   android/scripts/gradle.sh testDebugUnitTest
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
sidecast_android_env
cd "$SIDECAST_ANDROID_DIR"
exec ./gradlew --no-daemon "$@"
