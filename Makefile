.PHONY: all doctor mac mac-signing install-mac android install-android ios install-ios test test-mac test-android test-ios clean

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

# iPhone / iPad app (optional; needs Xcode). Builds for the simulator, unsigned
ios:
	./ios/scripts/build-ios.sh

# Build and install on the iPhone or iPad connected by cable (needs ios/Local.xcconfig)
install-ios:
	./ios/scripts/install-ios.sh

test: test-mac test-android

test-mac:
	cd mac && swift test

test-android:
	./android/scripts/gradle.sh testDebugUnitTest

# The iOS app's own unit tests on a simulator (its client logic is in SidecastKit: test-mac)
test-ios:
	./ios/scripts/build-ios.sh --test

clean:
	rm -rf dist mac/.build android/build android/app/build ios/build
