import Combine
import Foundation
import SidecastKit

/// Observable runtime state shown by the menu bar and the settings window.
@MainActor
final class AppModel: ObservableObject {
    enum Connection: Equatable {
        case starting
        case waiting(port: UInt16)
        case listenerFailed(String)
        case pairing(deviceName: String)
        case streaming(deviceName: String, fps: Double, mbps: Double, dataSaver: Bool)
    }

    @Published var connection: Connection = .starting
    @Published var screenRecordingGranted = true
    @Published var accessibilityTrusted = true
    @Published var sleep = SleepGuard.Status()
    @Published var pairedDevices: [PairedDevice] = []
    @Published var launchAtLogin = false
    @Published var lastError: String?
    @Published var mic = PhoneMicStatus()
    /// LAN, Tailscale and MagicDNS addresses of this Mac.
    @Published var addresses: [ServerAddress] = []
    /// The streaming phone is in trackpad mode (PROTOCOL §7.3).
    @Published var trackpadMode = false

    var isStreaming: Bool {
        if case .streaming = connection { return true }
        return false
    }

    /// One-line status for the menu.
    var statusLine: String {
        switch connection {
        case .starting: return "Starting…"
        case .waiting: return "Waiting for a phone"
        case .listenerFailed(let why): return "Not listening: \(why)"
        case .pairing(let name): return "Pairing with \(name)"
        case .streaming(let name, let fps, let mbps, let dataSaver):
            return String(format: "Streaming to %@%@%@ · %.0f fps · %.1f Mbps", name, trackpadMode ? " · trackpad" : "",
                          dataSaver ? " · data saver" : "", fps, mbps)
        }
    }
}
