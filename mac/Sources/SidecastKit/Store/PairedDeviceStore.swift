import Foundation

/// A phone that completed pairing. The token itself is never stored.
public struct PairedDevice: Codable, Equatable, Identifiable, Sendable {
    public var deviceId: String
    public var deviceName: String
    /// SHA-256 of the device token.
    public var tokenHash: Base64Data
    public var pairedAt: Date
    public var lastSeenAt: Date?

    public var id: String { deviceId }

    public init(deviceId: String, deviceName: String, tokenHash: Data, pairedAt: Date, lastSeenAt: Date? = nil) {
        self.deviceId = deviceId
        self.deviceName = deviceName
        self.tokenHash = Base64Data(tokenHash)
        self.pairedAt = pairedAt
        self.lastSeenAt = lastSeenAt
    }
}

/// Persists paired devices as a JSON file (mode 0600) at an injectable URL.
/// Thread-safe.
public final class PairedDeviceStore: @unchecked Sendable {
    private struct FileFormat: Codable {
        var version = 1
        var devices: [PairedDevice]
    }

    public let fileURL: URL
    private let lock = NSLock()
    private var devices: [String: PairedDevice] = [:]

    /// Loads the store; a missing file is an empty store. A corrupt file is
    /// moved aside (`.corrupt`) rather than silently overwritten.
    public init(fileURL: URL) {
        self.fileURL = fileURL
        load()
    }

    public var all: [PairedDevice] {
        lock.withLock { devices.values.sorted { $0.pairedAt < $1.pairedAt } }
    }

    public func device(id: String) -> PairedDevice? {
        lock.withLock { devices[id] }
    }

    /// Adds or replaces the pairing for `deviceId`.
    @discardableResult
    public func add(deviceId: String, deviceName: String, token: Data, at date: Date = Date()) throws -> PairedDevice {
        let device = PairedDevice(deviceId: deviceId, deviceName: deviceName,
                                  tokenHash: DeviceToken.hash(token), pairedAt: date, lastSeenAt: date)
        try lock.withLock {
            devices[deviceId] = device
            try saveLocked()
        }
        return device
    }

    public func remove(deviceId: String) throws {
        try lock.withLock {
            guard devices.removeValue(forKey: deviceId) != nil else { return }
            try saveLocked()
        }
    }

    /// Returns the device if `token` matches its stored hash (constant-time compare).
    public func authenticate(deviceId: String, token: Data) -> PairedDevice? {
        lock.withLock {
            guard let device = devices[deviceId] else { return nil }
            let ok = constantTimeEquals(DeviceToken.hash(token), device.tokenHash.data)
            return ok ? device : nil
        }
    }

    /// Records a successful connection (and the device's current name).
    public func markSeen(deviceId: String, name: String?, at date: Date = Date()) {
        lock.withLock {
            guard var device = devices[deviceId] else { return }
            device.lastSeenAt = date
            if let name, !name.isEmpty { device.deviceName = name }
            devices[deviceId] = device
            try? saveLocked()
        }
    }

    // MARK: - Persistence

    private func load() {
        guard let data = try? Data(contentsOf: fileURL) else { return }
        do {
            let file = try JSONDecoder.store.decode(FileFormat.self, from: data)
            devices = Dictionary(file.devices.map { ($0.deviceId, $0) }, uniquingKeysWith: { $1 })
        } catch {
            let aside = fileURL.appendingPathExtension("corrupt")
            try? FileManager.default.removeItem(at: aside)
            try? FileManager.default.moveItem(at: fileURL, to: aside)
        }
    }

    private func saveLocked() throws {
        let file = FileFormat(devices: devices.values.sorted { $0.pairedAt < $1.pairedAt })
        let data = try JSONEncoder.store.encode(file)
        try SecureFile.write(data, to: fileURL)
    }
}

/// Writes files atomically with owner-only permissions.
public enum SecureFile {
    public static func write(_ data: Data, to url: URL) throws {
        let fm = FileManager.default
        try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true,
                               attributes: [.posixPermissions: 0o700])
        let tmp = url.deletingLastPathComponent()
            .appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString).tmp")
        guard fm.createFile(atPath: tmp.path, contents: data, attributes: [.posixPermissions: 0o600]) else {
            throw CocoaError(.fileWriteUnknown, userInfo: [NSFilePathErrorKey: tmp.path])
        }
        do {
            if fm.fileExists(atPath: url.path) {
                _ = try fm.replaceItemAt(url, withItemAt: tmp)
            } else {
                try fm.moveItem(at: tmp, to: url)
            }
            try fm.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        } catch {
            try? fm.removeItem(at: tmp)
            throw error
        }
    }
}

extension JSONEncoder {
    static let store: JSONEncoder = {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        e.dateEncodingStrategy = .iso8601
        return e
    }()
}

extension JSONDecoder {
    static let store: JSONDecoder = {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        return d
    }()
}
