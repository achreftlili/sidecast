import Foundation
import SidecastKit

/// What a phone remembers about paired Macs (PROTOCOL §5 step 4), as JSON.
struct ProbeState: Codable {
    struct Pairing: Codable {
        var fingerprint: String   // lowercase hex
        var token: String         // base64
        var serverName: String
        var host: String
        var port: UInt16
    }

    var deviceId: String = UUID().uuidString
    var servers: [String: Pairing] = [:]

    static func load(path: String?) -> ProbeState {
        guard let path, let data = FileManager.default.contents(atPath: path),
              let state = try? JSONDecoder().decode(ProbeState.self, from: data) else { return ProbeState() }
        return state
    }

    func save(path: String?) throws {
        guard let path else { return }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try SecureFile.write(try encoder.encode(self), to: URL(fileURLWithPath: path))
    }
}
