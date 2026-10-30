import Foundation

/// Messages sent by the phone (client) to the Mac (server).
public enum ClientMessage: Equatable, Sendable {
    case hello(Hello)
    case auth(AuthRequest)
    case pairStart
    case pairProof(PairProof)
    case touch(TouchEvent)
    case frameAck(seq: UInt32)
    case keyframeRequest
    case ping(UInt64)
    /// Text typed on the phone (PROTOCOL §7.1).
    case text(String)
    case key(KeyEvent)
    /// Microphone audio (PROTOCOL §7.2).
    case audio(AudioPacket)
    case micState(MicState)
    /// Touch / trackpad (PROTOCOL §7.3).
    case inputMode(InputModeMessage)

    public var type: MessageType {
        switch self {
        case .hello: return .hello
        case .auth: return .auth
        case .pairStart: return .pairStart
        case .pairProof: return .pairProof
        case .touch: return .touch
        case .frameAck: return .frameAck
        case .keyframeRequest: return .keyframeRequest
        case .ping: return .ping
        case .text: return .text
        case .key: return .key
        case .audio: return .audio
        case .micState: return .micState
        case .inputMode: return .inputMode
        }
    }

    public func frame() -> Frame {
        switch self {
        case .hello(let m): return Frame(.hello, payload: WireJSON.encode(m))
        case .auth(let m): return Frame(.auth, payload: WireJSON.encode(m))
        case .pairStart: return Frame(.pairStart)
        case .pairProof(let m): return Frame(.pairProof, payload: WireJSON.encode(m))
        case .touch(let t): return Frame(.touch, payload: t.encodedPayload())
        case .frameAck(let seq):
            var w = ByteWriter(capacity: 4); w.u32(seq)
            return Frame(.frameAck, payload: w.data)
        case .keyframeRequest: return Frame(.keyframeRequest)
        case .ping(let t):
            var w = ByteWriter(capacity: 8); w.u64(t)
            return Frame(.ping, payload: w.data)
        case .text(let s): return Frame(.text, payload: TextMessage.encodedPayload(s))
        case .key(let k): return Frame(.key, payload: k.encodedPayload())
        case .audio(let a): return Frame(.audio, payload: a.encodedPayload())
        case .micState(let m): return Frame(.micState, payload: WireJSON.encode(m))
        case .inputMode(let m): return Frame(.inputMode, payload: WireJSON.encode(m))
        }
    }

    public func encoded() -> Data { frame().encoded() }

    /// Decodes a frame sent by a client. Returns nil for types that are not
    /// client messages of this protocol version (they must be skipped).
    public static func decode(_ frame: Frame) throws -> ClientMessage? {
        guard let type = frame.messageType else { return nil }
        let p = frame.payload
        switch type {
        case .hello: return .hello(try decodeHello(p))
        case .auth: return .auth(try WireJSON.decode(AuthRequest.self, from: p, as: type))
        case .pairStart: return .pairStart
        case .pairProof: return .pairProof(try WireJSON.decode(PairProof.self, from: p, as: type))
        case .touch: return .touch(try TouchEvent.decode(p))
        case .frameAck:
            var r = ByteReader(p)
            guard let seq = r.u32() else { throw ProtocolError.truncated(type) }
            return .frameAck(seq: seq)
        case .keyframeRequest: return .keyframeRequest
        case .ping:
            var r = ByteReader(p)
            guard let t = r.u64() else { throw ProtocolError.truncated(type) }
            return .ping(t)
        case .text: return .text(try TextMessage.decode(p))
        case .key: return .key(try KeyEvent.decode(p))
        case .audio: return .audio(try AudioPacket.decode(p))
        case .micState: return .micState(try WireJSON.decode(MicState.self, from: p, as: type))
        case .inputMode: return .inputMode(try WireJSON.decode(InputModeMessage.self, from: p, as: type))
        default:
            return nil
        }
    }

    /// HELLO with a protocol version other than 1 is reported as
    /// `HelloError.versionMismatch` even if the rest of its shape differs.
    private static func decodeHello(_ payload: Data) throws -> Hello {
        let version = try WireJSON.decode(HelloVersion.self, from: payload, as: .hello)
        guard version.protocol == SidecastProtocol.version else {
            throw HelloError.versionMismatch(version.protocol)
        }
        return try WireJSON.decode(Hello.self, from: payload, as: .hello)
    }
}

public enum HelloError: Error, Equatable {
    case versionMismatch(Int)
}

/// Messages sent by the Mac (server) to the phone (client).
public enum ServerMessage: Equatable, Sendable {
    case serverHello(ServerHello)
    case pairChallenge(PairChallenge)
    case authResult(AuthResult)
    case streamConfig(StreamConfig)
    case videoFrame(VideoFrame)
    /// The PING's 8 bytes, echoed unchanged.
    case pong(UInt64)
    case bye(Bye)
    case micControl(MicControl)
    /// Trackpad-mode pointer position (PROTOCOL §7.3).
    case cursor(CursorPosition)

    public var type: MessageType {
        switch self {
        case .serverHello: return .serverHello
        case .pairChallenge: return .pairChallenge
        case .authResult: return .authResult
        case .streamConfig: return .streamConfig
        case .videoFrame: return .videoFrame
        case .pong: return .pong
        case .bye: return .bye
        case .micControl: return .micControl
        case .cursor: return .cursor
        }
    }

    public func encoded() -> Data {
        switch self {
        case .serverHello(let m): return Frame(.serverHello, payload: WireJSON.encode(m)).encoded()
        case .pairChallenge(let m): return Frame(.pairChallenge, payload: WireJSON.encode(m)).encoded()
        case .authResult(let m): return Frame(.authResult, payload: WireJSON.encode(m)).encoded()
        case .streamConfig(let m): return Frame(.streamConfig, payload: WireJSON.encode(m)).encoded()
        case .videoFrame(let f): return f.encodedFrame()
        case .pong(let t):
            var w = ByteWriter(capacity: 8); w.u64(t)
            return Frame(.pong, payload: w.data).encoded()
        case .bye(let m): return Frame(.bye, payload: WireJSON.encode(m)).encoded()
        case .micControl(let m): return Frame(.micControl, payload: WireJSON.encode(m)).encoded()
        case .cursor(let c): return Frame(.cursor, payload: c.encodedPayload()).encoded()
        }
    }

    /// Decodes a frame sent by a server. Returns nil for unknown types.
    public static func decode(_ frame: Frame) throws -> ServerMessage? {
        guard let type = frame.messageType else { return nil }
        let p = frame.payload
        switch type {
        case .serverHello: return .serverHello(try WireJSON.decode(ServerHello.self, from: p, as: type))
        case .pairChallenge: return .pairChallenge(try WireJSON.decode(PairChallenge.self, from: p, as: type))
        case .authResult: return .authResult(try WireJSON.decode(AuthResult.self, from: p, as: type))
        case .streamConfig: return .streamConfig(try WireJSON.decode(StreamConfig.self, from: p, as: type))
        case .videoFrame: return .videoFrame(try VideoFrame.decode(p))
        case .pong:
            var r = ByteReader(p)
            guard let t = r.u64() else { throw ProtocolError.truncated(type) }
            return .pong(t)
        case .bye: return .bye(try WireJSON.decode(Bye.self, from: p, as: type))
        case .micControl: return .micControl(try WireJSON.decode(MicControl.self, from: p, as: type))
        case .cursor: return .cursor(try CursorPosition.decode(p))
        default:
            return nil
        }
    }
}
