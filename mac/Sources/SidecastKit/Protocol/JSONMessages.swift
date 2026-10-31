import Foundation

/// Binary data carried in JSON as standard Base64 with padding (RFC 4648 §4).
/// Decoding tolerates whitespace/line breaks (e.g. Android `Base64.DEFAULT`).
public struct Base64Data: Codable, Equatable, Sendable {
    public var data: Data

    public init(_ data: Data) { self.data = data }

    public init(from decoder: Decoder) throws {
        let string = try decoder.singleValueContainer().decode(String.self)
        guard let data = Data(base64Encoded: string, options: .ignoreUnknownCharacters) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath,
                                                    debugDescription: "invalid base64"))
        }
        self.data = data
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(data.base64EncodedString())
    }
}

// MARK: - Client → Server

public struct ScreenInfo: Codable, Equatable, Sendable {
    public var width: Int
    public var height: Int
    public var densityDpi: Int
    public var refreshRate: Double

    public init(width: Int, height: Int, densityDpi: Int, refreshRate: Double) {
        self.width = width
        self.height = height
        self.densityDpi = densityDpi
        self.refreshRate = refreshRate
    }

    private enum CodingKeys: String, CodingKey { case width, height, densityDpi, refreshRate }

    /// Only the size is essential; density and refresh rate fall back to defaults.
    /// `refreshRate` accepts `120` as well as `120.0`.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        width = try c.decode(Int.self, forKey: .width)
        height = try c.decode(Int.self, forKey: .height)
        densityDpi = try c.decodeIfPresent(Int.self, forKey: .densityDpi) ?? 160
        refreshRate = try c.decodeIfPresent(Double.self, forKey: .refreshRate) ?? 60
    }
}

public struct CodecInfo: Codable, Equatable, Sendable {
    public var name: String
    public var maxWidth: Int
    public var maxHeight: Int

    public init(name: String, maxWidth: Int, maxHeight: Int) {
        self.name = name
        self.maxWidth = maxWidth
        self.maxHeight = maxHeight
    }

    private enum CodingKeys: String, CodingKey { case name, maxWidth, maxHeight }

    /// A codec without a reported limit is assumed to handle 1080p.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        maxWidth = try c.decodeIfPresent(Int.self, forKey: .maxWidth) ?? 1920
        maxHeight = try c.decodeIfPresent(Int.self, forKey: .maxHeight) ?? 1080
    }
}

/// HELLO `network` values (PROTOCOL §5).
public enum ClientNetwork: String, Sendable {
    case metered
    case unmetered
}

/// HELLO `limits` (data saver, PROTOCOL §5). Missing or non-positive fields mean "no limit".
public struct StreamLimits: Codable, Equatable, Sendable {
    public var maxBitrateKbps: Int?
    public var maxFps: Int?

    /// The phone's default data-saver caps: 4000 kbps, 30 fps.
    public static let dataSaver = StreamLimits(maxBitrateKbps: 4_000, maxFps: 30)

    public init(maxBitrateKbps: Int? = nil, maxFps: Int? = nil) {
        self.maxBitrateKbps = maxBitrateKbps
        self.maxFps = maxFps
    }

    private enum CodingKeys: String, CodingKey { case maxBitrateKbps, maxFps }

    /// Tolerant: a field of the wrong type, or ≤ 0, is ignored (floats are truncated).
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        func positive(_ key: CodingKeys) -> Int? {
            if let v = try? c.decodeIfPresent(Int.self, forKey: key) { return v > 0 ? v : nil }
            if let v = try? c.decodeIfPresent(Double.self, forKey: key), v.isFinite, v >= 1 { return Int(min(v, 1e9)) }
            return nil
        }
        maxBitrateKbps = positive(.maxBitrateKbps)
        maxFps = positive(.maxFps)
    }

    /// Bitrate cap in bits per second, if any.
    public var maxBitrate: Int? { maxBitrateKbps.map { $0 * 1_000 } }

    /// No usable cap at all.
    public var isEmpty: Bool { maxBitrateKbps == nil && maxFps == nil }
}

public struct Hello: Codable, Equatable, Sendable {
    public var `protocol`: Int
    public var deviceId: String
    public var deviceName: String
    public var screen: ScreenInfo
    public var codecs: [CodecInfo]
    /// Optional capabilities (`ClientFeature`); absent = none.
    public var features: [String]
    /// Raw `network` ("metered" / "unmetered"); see `clientNetwork`.
    public var network: String?
    /// Data-saver caps; nil when absent, malformed or empty.
    public var limits: StreamLimits?

    public init(protocol: Int = SidecastProtocol.version, deviceId: String, deviceName: String,
                screen: ScreenInfo, codecs: [CodecInfo], features: [String] = [],
                network: String? = nil, limits: StreamLimits? = nil) {
        self.protocol = `protocol`
        self.deviceId = deviceId
        self.deviceName = deviceName
        self.screen = screen
        self.codecs = codecs
        self.features = features
        self.network = network
        self.limits = limits
    }

    public var clientNetwork: ClientNetwork? { network.flatMap(ClientNetwork.init(rawValue:)) }

    /// The client can stream its microphone (PROTOCOL §7.2).
    public var supportsMic: Bool { features.contains(ClientFeature.mic) }

    private enum CodingKeys: String, CodingKey { case `protocol`, deviceId, deviceName, screen, codecs, features, network, limits }

    /// `protocol`, `deviceId` and `screen` are required; a missing name, codec or feature list gets a default.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.protocol = try c.decode(Int.self, forKey: .protocol)
        deviceId = try c.decode(String.self, forKey: .deviceId)
        deviceName = try c.decodeIfPresent(String.self, forKey: .deviceName) ?? "Android phone"
        screen = try c.decode(ScreenInfo.self, forKey: .screen)
        codecs = try c.decodeIfPresent([CodecInfo].self, forKey: .codecs) ?? []
        features = try c.decodeIfPresent([String].self, forKey: .features) ?? []
        // Optional and tolerant: a malformed value is ignored, never a reason to reject HELLO.
        network = try? c.decodeIfPresent(String.self, forKey: .network)
        let limits = try? c.decodeIfPresent(StreamLimits.self, forKey: .limits)
        self.limits = limits?.isEmpty == false ? limits : nil
    }

    /// `features` is omitted when empty, as older clients do.
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(self.protocol, forKey: .protocol)
        try c.encode(deviceId, forKey: .deviceId)
        try c.encode(deviceName, forKey: .deviceName)
        try c.encode(screen, forKey: .screen)
        try c.encode(codecs, forKey: .codecs)
        if !features.isEmpty { try c.encode(features, forKey: .features) }
        try c.encodeIfPresent(network, forKey: .network)
        try c.encodeIfPresent(limits, forKey: .limits)
    }
}

/// Just the version field of HELLO, so a future HELLO shape is still
/// recognised as a version mismatch rather than a malformed message.
struct HelloVersion: Decodable {
    var `protocol`: Int
}

public struct AuthRequest: Codable, Equatable, Sendable {
    public var deviceId: String
    public var token: Base64Data

    public init(deviceId: String, token: Data) {
        self.deviceId = deviceId
        self.token = Base64Data(token)
    }
}

/// How TOUCH is interpreted (PROTOCOL §7.3).
public enum InputMode: String, Sendable, CaseIterable {
    /// The phone is a touchscreen: positions are absolute (default).
    case touch
    /// The phone is a laptop trackpad: finger movement moves the cursor relatively.
    case trackpad
}

/// INPUT_MODE (client → server): `{"mode": "touch" | "trackpad"}`.
public struct InputModeMessage: Codable, Equatable, Sendable {
    /// Raw mode; unknown values decode fine and are ignored (`inputMode` is nil).
    public var mode: String

    public init(_ mode: InputMode) { self.mode = mode.rawValue }
    public init(rawMode: String) { self.mode = rawMode }

    public var inputMode: InputMode? { InputMode(rawValue: mode) }
}

/// MIC_STATE `reason` values (PROTOCOL §7.2).
public enum MicStateReason: String, Sendable, CaseIterable {
    case stopped
    case disabled
    case permissionDenied = "permission_denied"
    case background
    case unavailable
}

/// MIC_STATE (client → server): whether the phone's microphone is capturing.
public struct MicState: Codable, Equatable, Sendable {
    public var capturing: Bool
    /// Raw reason when not capturing; see `knownReason`.
    public var reason: String?

    public init(capturing: Bool, reason: String? = nil) {
        self.capturing = capturing
        self.reason = reason
    }

    public init(capturing: Bool, reason: MicStateReason) {
        self.init(capturing: capturing, reason: reason.rawValue)
    }

    public var knownReason: MicStateReason? { reason.flatMap(MicStateReason.init(rawValue:)) }
}

public struct PairProof: Codable, Equatable, Sendable {
    public var nonce: Base64Data
    public var proof: Base64Data

    public init(nonce: Data, proof: Data) {
        self.nonce = Base64Data(nonce)
        self.proof = Base64Data(proof)
    }
}

// MARK: - Server → Client

/// SERVER_HELLO `addresses[].kind` (PROTOCOL §1.1, §4).
public enum AddressKind: String, Codable, Sendable, CaseIterable {
    case lan
    case tailscale
    case tailscaleDNS = "tailscale-dns"
}

/// One address the Mac can be reached at (PROTOCOL §1.1).
public struct ServerAddress: Codable, Equatable, Hashable, Sendable {
    public var host: String
    /// Raw kind; see `addressKind`.
    public var kind: String

    public init(host: String, kind: AddressKind) {
        self.host = host
        self.kind = kind.rawValue
    }

    public init(host: String, rawKind: String) {
        self.host = host
        self.kind = rawKind
    }

    public var addressKind: AddressKind? { AddressKind(rawValue: kind) }
}

/// The host's operating system: SERVER_HELLO `platform` and Bonjour TXT `os` (PROTOCOL §1, §4).
public enum HostPlatform: String, Codable, Sendable, CaseIterable {
    case macos
    case windows
    case linux
}

public struct ServerHello: Codable, Equatable, Sendable {
    public var `protocol`: Int
    public var serverId: String
    public var serverName: String
    /// The listening port (same for every address).
    public var port: Int?
    /// Reachable addresses: LAN first, then Tailscale, then MagicDNS.
    public var addresses: [ServerAddress]?
    /// Raw host OS; see `hostPlatform`. Always sent by this host.
    public var platform: String

    public init(protocol: Int = SidecastProtocol.version, serverId: String, serverName: String,
                port: Int? = nil, addresses: [ServerAddress]? = nil, platform: HostPlatform = .macos) {
        self.protocol = `protocol`
        self.serverId = serverId
        self.serverName = serverName
        self.port = port
        self.addresses = addresses
        self.platform = platform.rawValue
    }

    /// nil for an OS this version does not know: show a generic computer.
    public var hostPlatform: HostPlatform? { HostPlatform(rawValue: platform) }

    private enum CodingKeys: String, CodingKey { case `protocol`, serverId, serverName, port, addresses, platform }

    /// Mac hosts before `platform` existed omit it, so absent (or malformed) means macOS.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.protocol = try c.decode(Int.self, forKey: .protocol)
        serverId = try c.decode(String.self, forKey: .serverId)
        serverName = try c.decode(String.self, forKey: .serverName)
        port = try c.decodeIfPresent(Int.self, forKey: .port)
        addresses = try c.decodeIfPresent([ServerAddress].self, forKey: .addresses)
        platform = (try? c.decodeIfPresent(String.self, forKey: .platform)) ?? HostPlatform.macos.rawValue
    }
}

public struct PairChallenge: Codable, Equatable, Sendable {
    public var expiresInSec: Int
    public var attemptsLeft: Int

    public init(expiresInSec: Int, attemptsLeft: Int) {
        self.expiresInSec = expiresInSec
        self.attemptsLeft = attemptsLeft
    }
}

public struct AuthResult: Codable, Equatable, Sendable {
    public var ok: Bool
    public var token: Base64Data?
    public var reason: String?
    public var attemptsLeft: Int?

    public init(ok: Bool, token: Data? = nil, reason: ByeReason? = nil, attemptsLeft: Int? = nil) {
        self.ok = ok
        self.token = token.map(Base64Data.init)
        self.reason = reason?.rawValue
        self.attemptsLeft = attemptsLeft
    }
}

public struct StreamConfig: Codable, Equatable, Sendable {
    public var codec: String
    public var width: Int
    public var height: Int
    public var fps: Int

    public init(codec: String, width: Int, height: Int, fps: Int) {
        self.codec = codec
        self.width = width
        self.height = height
        self.fps = fps
    }
}

/// MIC_CONTROL (server → client): start or stop the phone's microphone.
public struct MicControl: Codable, Equatable, Sendable {
    public var active: Bool
    public var sampleRate: Int?
    public var channels: Int?

    public init(active: Bool, sampleRate: Int? = nil, channels: Int? = nil) {
        self.active = active
        self.sampleRate = sampleRate
        self.channels = channels
    }

    /// `{"active": true, "sampleRate": 48000, "channels": 1}`
    public static let start = MicControl(active: true, sampleRate: AudioPacket.sampleRate, channels: AudioPacket.channels)
    /// `{"active": false}`
    public static let stop = MicControl(active: false)
}

public struct Bye: Codable, Equatable, Sendable {
    public var reason: String

    public init(reason: ByeReason) { self.reason = reason.rawValue }
    public init(rawReason: String) { self.reason = rawReason }
}

// MARK: - JSON coding

enum WireJSON {
    static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.outputFormatting = [.withoutEscapingSlashes, .sortedKeys]
        return e
    }()

    static func encode<T: Encodable>(_ value: T) -> Data {
        // Encoding these plain structs cannot fail.
        (try? encoder.encode(value)) ?? Data("{}".utf8)
    }

    static func decode<T: Decodable>(_ type: T.Type, from data: Data, as messageType: MessageType) throws -> T {
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch {
            throw ProtocolError.malformed(messageType, "\(error)")
        }
    }
}
