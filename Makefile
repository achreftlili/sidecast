.PHONY: all doctor mac mac-signing install-mac android install-android ios install-ios install-linux test test-mac test-android test-ios rust test-rust conformance e2e-linux clean

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

# Linux computer (experimental): the host into ~/.local/bin plus an app-grid entry.
# AUTOSTART=1 also starts it at login.
install-linux:
	SIDECAST_AUTOSTART=$(if $(AUTOSTART),1,0) ./scripts/install-linux.sh

test: test-mac test-android

test-mac:
	cd mac && swift test

test-android:
	./android/scripts/gradle.sh testDebugUnitTest

# The iOS app's own unit tests on a simulator (its client logic is in SidecastKit: test-mac)
test-ios:
	./ios/scripts/build-ios.sh --test

# The Rust workspace (Windows/Linux host, shared core, conformance probe): release build into rust/target/
rust:
	cd rust && cargo build --release

test-rust:
	cd rust && cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test

# Every probe against every host in test-pattern mode (Rust ↔ Rust; on macOS also Swift ↔ Rust)
conformance:
	./scripts/conformance.sh

# Linux: the real GNOME backend on a headless GNOME (Mutter, PipeWire), checked by the probe
e2e-linux:
	./scripts/linux-e2e.sh

clean:
	rm -rf dist mac/.build android/build android/app/build ios/build rust/target
