import Foundation
import SidecastKit

/// Where Sidecast keeps its state (TLS identity, paired devices, server id,
/// sleep marker). `~/Library/Application Support/Sidecast`, or `--data-dir`.
struct AppPaths {
    let dataDirectory: URL

    static var defaultDataDirectory: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Sidecast", isDirectory: true)
    }

    init(dataDirectory: URL = AppPaths.defaultDataDirectory) {
        self.dataDirectory = dataDirectory
    }

    var pairedDevices: URL { dataDirectory.appendingPathComponent("paired-devices.json") }
    var identityP12: URL { dataDirectory.appendingPathComponent("identity.p12") }
    var identityPassword: URL { dataDirectory.appendingPathComponent("identity.p12.pass") }
    var serverIdFile: URL { dataDirectory.appendingPathComponent("server-id") }
    var sleepMarker: URL { dataDirectory.appendingPathComponent("sleep-disabled") }
    /// UID of the input device that was the default before Sidecast switched it.
    var defaultInputMarker: URL { dataDirectory.appendingPathComponent("default-input-changed") }

    func ensureDirectory() throws {
        try FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
    }

    /// Stable UUID identifying this install (TXT `id=`, SERVER_HELLO `serverId`).
    func loadOrCreateServerId() throws -> String {
        if let s = try? String(contentsOf: serverIdFile, encoding: .utf8),
           let uuid = UUID(uuidString: s.trimmingCharacters(in: .whitespacesAndNewlines)) {
            return uuid.uuidString
        }
        let id = UUID().uuidString
        try SecureFile.write(Data(id.utf8), to: serverIdFile)
        return id
    }
}
