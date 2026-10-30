.PHONY: all doctor mac mac-signing install-mac android install-android test test-mac test-android clean

# Build both into dist/
all: mac android

# Check this computer's tools and say how to fix what's missing
doctor:
	./scripts/doctor.sh

# Once per Mac: a local signing certificate so permissions survive updates
mac-signing:
	./mac/scripts/create-signing-identity.sh

mac:
	./mac/scripts/build-mac.sh

# Build and install into /Applications
install-mac:
	./mac/scripts/install-mac.sh

android:
	./android/scripts/build-android.sh

# Build and install on the phone connected over adb
install-android:
	./android/scripts/install-android.sh

test: test-mac test-android

test-mac:
	cd mac && swift test

test-android:
	./android/scripts/gradle.sh testDebugUnitTest

clean:
	rm -rf dist mac/.build android/build android/app/build
