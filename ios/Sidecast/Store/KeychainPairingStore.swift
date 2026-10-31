import Foundation
import SidecastKit

/// Pairings (pinned fingerprint + device token per computer, PROTOCOL §5) in
/// the Keychain, one item per serverId. Thread-safe: the session queue writes,
/// the list reads.
final class KeychainPairingStore: PairingRepository, @unchecked Sendable {
    static let shared = KeychainPairingStore()

    private let service: String
    private let lock = NSLock()
    private var cache: [String: PairedComputer]?

    init(service: String = (Bundle.main.bundleIdentifier ?? "sidecast") + ".pairings") {
        self.service = service
    }

    func pairing(serverId: String) -> PairedComputer? { lock.withLock { loaded()[serverId] } }

    func all() -> [PairedComputer] {
        lock.withLock { loaded().values.sorted { $0.serverName.localizedCaseInsensitiveCompare($1.serverName) == .orderedAscending } }
    }

    func save(_ pairing: PairedComputer) {
        lock.withLock {
            var all = loaded()
            all[pairing.serverId] = pairing
            cache = all
            do {
                try Keychain.write(try JSONEncoder().encode(pairing), service: service, account: pairing.serverId)
            } catch {
                Log.session.error("could not save the pairing with \(pairing.serverName, privacy: .public): \(String(describing: error), privacy: .public)")
            }
        }
    }

    func remove(serverId: String) {
        lock.withLock {
            var all = loaded()
            all.removeValue(forKey: serverId)
            cache = all
            Keychain.delete(service: service, account: serverId)
        }
    }

    /// Forgets every pairing (tests).
    func removeAll() {
        lock.withLock {
            cache = [:]
            Keychain.delete(service: service)
        }
    }

    /// Holds `lock`.
    private func loaded() -> [String: PairedComputer] {
        if let cache { return cache }
        var out: [String: PairedComputer] = [:]
        for (account, data) in Keychain.all(service: service) {
            // An unreadable item (an older format) is dropped: pairing again fixes it.
            if let p = try? JSONDecoder().decode(PairedComputer.self, from: data), p.serverId == account { out[account] = p }
        }
        cache = out
        return out
    }
}

/// This app's identity towards computers (HELLO `deviceId` / `deviceName`).
enum DeviceIdentity {
    private static let service = (Bundle.main.bundleIdentifier ?? "sidecast") + ".device"

    /// Stable random UUID per install, kept in the Keychain so a reinstall
    /// (e.g. every 7 days with a free Apple ID) stays the same device on the computer.
    static let deviceId: String = {
        if let data = Keychain.read(service: service, account: "deviceId"), let id = String(data: data, encoding: .utf8),
           !id.isEmpty {
            return id
        }
        let id = UUID().uuidString.lowercased()
        try? Keychain.write(Data(id.utf8), service: service, account: "deviceId")
        return id
    }()
}
