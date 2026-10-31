import Foundation
import SidecastKit
import os

/// User preferences (UserDefaults; nothing secret here — pairings live in the Keychain).
final class AppSettings: @unchecked Sendable {
    static let shared = AppSettings()

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// "Use as computer microphone" (PROTOCOL §7.2). When off, requests are answered `disabled`.
    var micEnabled: Bool {
        get { defaults.object(forKey: Keys.micEnabled) == nil || defaults.bool(forKey: Keys.micEnabled) }
        set { defaults.set(newValue, forKey: Keys.micEnabled) }
    }

    /// "Data saver on cellular" (§5 `limits`): ≤ 4 Mbps and 30 fps on an expensive or constrained network.
    var dataSaver: Bool {
        get { defaults.object(forKey: Keys.dataSaver) == nil || defaults.bool(forKey: Keys.dataSaver) }
        set { defaults.set(newValue, forKey: Keys.dataSaver) }
    }

    /// Touch screen or trackpad (§7.3); anything unknown reads as touch.
    var inputMode: InputMode {
        get { defaults.string(forKey: Keys.inputMode).flatMap(InputMode.init(rawValue:)) ?? .touch }
        set { defaults.set(newValue.rawValue, forKey: Keys.inputMode) }
    }

    // Each setting can also be given as a launch argument (`-micEnabled NO`): a string
    // in the argument domain, which `bool(forKey:)` reads like a stored boolean.

    /// Testing aid: `-SidecastCodecs hevc,h264` (launch argument) overrides the advertised codecs.
    var codecOverride: [String]? {
        defaults.string(forKey: Keys.codecOverride).map { $0.split(separator: ",").map(String.init) }
    }

    private enum Keys {
        static let micEnabled = "micEnabled"
        static let dataSaver = "dataSaver"
        static let inputMode = "inputMode"
        static let codecOverride = "SidecastCodecs"
    }
}

/// Unified logging (Console.app, `xcrun simctl spawn booted log stream --predicate 'subsystem == "sidecast"'`).
enum Log {
    static let session = Logger(subsystem: "sidecast", category: "session")
    static let video = Logger(subsystem: "sidecast", category: "video")
    static let input = Logger(subsystem: "sidecast", category: "input")
    static let mic = Logger(subsystem: "sidecast", category: "mic")
    static let discovery = Logger(subsystem: "sidecast", category: "discovery")
}
