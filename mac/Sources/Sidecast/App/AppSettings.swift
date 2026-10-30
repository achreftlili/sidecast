import Combine
import Foundation
import SidecastKit

/// User settings, persisted in UserDefaults.
@MainActor
final class AppSettings: ObservableObject {
    private enum Key {
        static let hiDPI = "hiDPI"
        static let resolutionScale = "resolutionScale"
        static let fps = "fps"
        static let maxBitrateMbps = "maxBitrateMbps"
        static let touchEnabled = "touchEnabled"
        static let micEnabled = "micEnabled"
        static let micMakeDefaultInput = "micMakeDefaultInput"
        static let port = "port"
    }

    static let portRange = 1024...65535

    private let defaults: UserDefaults
    /// Called after any change.
    var onChange: (() -> Void)?

    @Published var hiDPI: Bool { didSet { save(Key.hiDPI, hiDPI) } }
    @Published var resolutionScale: Double { didSet { save(Key.resolutionScale, resolutionScale) } }
    @Published var fps: Int { didSet { save(Key.fps, fps) } }
    @Published var maxBitrateMbps: Int { didSet { save(Key.maxBitrateMbps, maxBitrateMbps) } }
    @Published var touchEnabled: Bool { didSet { save(Key.touchEnabled, touchEnabled) } }
    /// "Use the phone's microphone" (Sidecast Microphone).
    @Published var micEnabled: Bool { didSet { save(Key.micEnabled, micEnabled) } }
    /// "Make Sidecast Microphone the default input while a phone is connected".
    @Published var micMakeDefaultInput: Bool { didSet { save(Key.micMakeDefaultInput, micMakeDefaultInput) } }
    @Published var port: Int { didSet { save(Key.port, port) } }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let d = StreamSettings()
        hiDPI = defaults.object(forKey: Key.hiDPI) as? Bool ?? d.hiDPI
        let scale = defaults.object(forKey: Key.resolutionScale) as? Double ?? d.resolutionScale
        resolutionScale = StreamSettings.allowedScales.contains(scale) ? scale : d.resolutionScale
        let fps = defaults.object(forKey: Key.fps) as? Int ?? d.fps
        self.fps = StreamSettings.allowedFps.contains(fps) ? fps : d.fps
        let mbps = defaults.object(forKey: Key.maxBitrateMbps) as? Int ?? d.maxBitrateMbps
        maxBitrateMbps = min(max(mbps, StreamSettings.bitrateRangeMbps.lowerBound), StreamSettings.bitrateRangeMbps.upperBound)
        touchEnabled = defaults.object(forKey: Key.touchEnabled) as? Bool ?? d.touchEnabled
        micEnabled = defaults.object(forKey: Key.micEnabled) as? Bool ?? true
        micMakeDefaultInput = defaults.object(forKey: Key.micMakeDefaultInput) as? Bool ?? true
        let port = defaults.object(forKey: Key.port) as? Int ?? Int(SidecastProtocol.defaultPort)
        self.port = Self.portRange.contains(port) ? port : Int(SidecastProtocol.defaultPort)
    }

    var streamSettings: StreamSettings {
        StreamSettings(hiDPI: hiDPI, resolutionScale: resolutionScale, fps: fps,
                       maxBitrateMbps: maxBitrateMbps, touchEnabled: touchEnabled)
    }

    var micSettings: MicRouter.Settings {
        MicRouter.Settings(enabled: micEnabled, makeDefaultInput: micMakeDefaultInput)
    }

    private func save(_ key: String, _ value: Any) {
        defaults.set(value, forKey: key)
        onChange?()
    }
}
