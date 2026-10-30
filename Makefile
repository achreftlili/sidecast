.PHONY: all mac android test test-mac test-android clean

export JAVA_HOME ?= /opt/homebrew/opt/openjdk@21
export ANDROID_HOME ?= /opt/homebrew/share/android-commandlinetools

# Build both deliverables into dist/
all: mac android

mac:
	./mac/scripts/build-mac.sh

android:
	./android/scripts/build-android.sh

test: test-mac test-android

test-mac:
	cd mac && swift test

test-android:
	cd android && ./gradlew --no-daemon testDebugUnitTest

clean:
	rm -rf dist mac/.build android/build android/app/build
