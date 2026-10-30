import AppKit
import Foundation

setvbuf(stdout, nil, _IOLBF, 0)

let launchOptions: LaunchOptions
do {
    launchOptions = try LaunchOptions.parse(CommandLine.arguments)
} catch {
    FileHandle.standardError.write(Data("\(error)\n".utf8))
    exit(2)
}

/// Global, so the test-mode runner (and its server) lives as long as the process.
var testModeRunner: TestModeRunner?

if launchOptions.testPattern {
    testModeRunner = TestModeRunner(options: launchOptions)
    testModeRunner?.run()
}

MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = AppDelegate(options: launchOptions)
    app.delegate = delegate
    app.setActivationPolicy(.accessory)
    withExtendedLifetime(delegate) { app.run() }
}
