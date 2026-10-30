// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Sidecast",
    platforms: [.macOS(.v15)],
    products: [
        .executable(name: "Sidecast", targets: ["Sidecast"]),
        .executable(name: "sidecast-probe", targets: ["SidecastProbe"]),
        .library(name: "SidecastKit", targets: ["SidecastKit"]),
    ],
    targets: [
        // Declarations for the private CGVirtualDisplay API (the classes live in CoreGraphics).
        .target(name: "CVirtualDisplay"),
        // Pure, testable logic shared by the app and the probe.
        .target(name: "SidecastKit"),
        // The "Sidecast Microphone" AudioServerPlugIn (C). Built here so it is
        // compiled and unit-tested with the package; scripts/build-mic-driver.sh
        // compiles the same sources into SidecastMic.driver.
        .target(
            name: "SidecastMicDriver",
            linkerSettings: [.linkedFramework("CoreAudio"), .linkedFramework("CoreFoundation")]
        ),
        // The menu-bar app (and its headless --test-pattern mode).
        .executableTarget(
            name: "Sidecast",
            dependencies: ["SidecastKit", "CVirtualDisplay"]
        ),
        // CLI test client.
        .executableTarget(
            name: "SidecastProbe",
            dependencies: ["SidecastKit"]
        ),
        .testTarget(
            name: "SidecastKitTests",
            dependencies: ["SidecastKit"]
        ),
        .testTarget(
            name: "SidecastMicDriverTests",
            dependencies: ["SidecastMicDriver"]
        ),
    ],
    swiftLanguageModes: [.v5]
)
