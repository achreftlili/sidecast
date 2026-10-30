import Foundation
import SidecastKit

struct ProbeOptions {
    var host = "127.0.0.1"
    var port: UInt16 = SidecastProtocol.defaultPort
    var seconds: Double = 5
    /// Pairing codes to try in order (comma-separated on the command line).
    var codes: [String] = []
    var statePath: String?
    var codec: VideoCodec?
    var width = 2400
    var height = 1080
    var refreshRate = 60.0
    var slowAckMs = 0
    var keyframeRequest = false
    var touch = false
    /// TEXT / KEY messages to send (in command-line order) once the first VIDEO_FRAME arrives.
    var keyboard: [ClientMessage] = []
    /// Advertise `features:["mic"]`; on MIC_CONTROL active:true stream a 440 Hz sine as AUDIO.
    var mic = false
    /// Synthetic multi-touch gestures, played in order once frames flow.
    var gestures: [ProbeGesture] = []
    /// HELLO `network` and `limits` (data saver).
    var network: String?
    var limits: StreamLimits?
    /// INPUT_MODE to send right after authentication.
    var inputMode: InputMode?
    /// Go silent (no acks/pings) after this many seconds; expect the server to drop us after ~5 s.
    var stallAfter: Double?
    /// Wait this long (silently) before each PAIR_PROOF, like a user typing.
    var pairDelayMs = 0
    /// Succeed only if authentication fails with this reason (and the matching BYE).
    var expectAuthFailure: String?

    static let usage = """
    Usage: sidecast-probe --host H --port P --seconds N [--code 123456[,654321…]] [--state FILE]
                          [--codec hevc|h264] [--width W --height H] [--refresh HZ]
                          [--slow-ack-ms N] [--keyframe-request] [--touch] [--stall-after S]
                          [--pair-delay-ms N] [--expect-auth-failure REASON]
                          [--type TEXT]… [--key N[:MODS]]…   (keyboard, sent once streaming;
                                                              MODS: 1 Shift, 2 Ctrl, 4 Option, 8 Cmd)
                          [--mic]   (advertise the microphone; send a 440 Hz tone when asked)
                          [--gesture NAME]…   (tap2 swipe3-left swipe3-right swipe3-up swipe3-down
                                               swipe4-left tap3 scroll2)
                          [--network metered|unmetered] [--limits KBPS:FPS]   (data saver, e.g. 4000:30)
                          [--input-mode touch|trackpad]   (+ gestures trackpad-move trackpad-tap
                                                           trackpad-doubletap-drag)
    """

    static func parse(_ args: [String]) throws -> ProbeOptions {
        var o = ProbeOptions()
        var it = args.dropFirst().makeIterator()
        func value(_ flag: String) throws -> String {
            guard let v = it.next() else { throw ProbeError.usage("\(flag) needs a value") }
            return v
        }
        func int(_ flag: String) throws -> Int {
            guard let v = Int(try value(flag)) else { throw ProbeError.usage("\(flag) must be an integer") }
            return v
        }
        while let arg = it.next() {
            switch arg {
            case "--host": o.host = try value(arg)
            case "--port":
                guard let p = UInt16(try value(arg)), p > 0 else { throw ProbeError.usage("invalid --port") }
                o.port = p
            case "--seconds":
                guard let s = Double(try value(arg)), s > 0 else { throw ProbeError.usage("invalid --seconds") }
                o.seconds = s
            case "--code": o.codes = try value(arg).split(separator: ",").map(String.init)
            case "--pair-delay-ms": o.pairDelayMs = max(0, try int(arg))
            case "--expect-auth-failure": o.expectAuthFailure = try value(arg)
            case "--state": o.statePath = (try value(arg) as NSString).expandingTildeInPath
            case "--codec":
                guard let c = VideoCodec(rawValue: try value(arg)) else { throw ProbeError.usage("--codec must be hevc or h264") }
                o.codec = c
            case "--width": o.width = try int(arg)
            case "--height": o.height = try int(arg)
            case "--refresh":
                guard let r = Double(try value(arg)) else { throw ProbeError.usage("invalid --refresh") }
                o.refreshRate = r
            case "--slow-ack-ms": o.slowAckMs = max(0, try int(arg))
            case "--keyframe-request": o.keyframeRequest = true
            case "--touch": o.touch = true
            case "--type":
                let text = try value(arg)
                guard !text.isEmpty else { throw ProbeError.usage("--type needs non-empty text") }
                o.keyboard.append(.text(text))
            case "--key": o.keyboard.append(.key(try parseKey(try value(arg))))
            case "--mic": o.mic = true
            case "--input-mode":
                guard let mode = InputMode(rawValue: try value(arg)) else {
                    throw ProbeError.usage("--input-mode must be touch or trackpad")
                }
                o.inputMode = mode
            case "--network":
                let n = try value(arg)
                guard ClientNetwork(rawValue: n) != nil else { throw ProbeError.usage("--network must be metered or unmetered") }
                o.network = n
            case "--limits":
                let spec = try value(arg)
                let parts = spec.split(separator: ":", omittingEmptySubsequences: false)
                guard parts.count == 2, let kbps = Int(parts[0]), let fps = Int(parts[1]), kbps > 0, fps > 0 else {
                    throw ProbeError.usage("--limits must be KBPS:FPS, e.g. 4000:30")
                }
                o.limits = StreamLimits(maxBitrateKbps: kbps, maxFps: fps)
            case "--gesture":
                let name = try value(arg)
                guard let gesture = ProbeGesture(rawValue: name) else {
                    throw ProbeError.usage("unknown --gesture \(name); one of "
                                           + ProbeGesture.allCases.map(\.rawValue).joined(separator: ", "))
                }
                o.gestures.append(gesture)
            case "--stall-after":
                guard let s = Double(try value(arg)), s > 0 else { throw ProbeError.usage("invalid --stall-after") }
                o.stallAfter = s
            case "--help", "-h": throw ProbeError.usage(usage)
            default: throw ProbeError.usage("unknown argument \(arg)\n\(usage)")
            }
        }
        return o
    }

    /// `N` or `N:MODS`, both decimal bytes (unknown values are allowed, to exercise the server).
    static func parseKey(_ spec: String) throws -> KeyEvent {
        let parts = spec.split(separator: ":", omittingEmptySubsequences: false)
        guard (1...2).contains(parts.count), let key = UInt8(parts[0]),
              let mods = parts.count == 2 ? UInt8(parts[1]) : 0 else {
            throw ProbeError.usage("--key must be N or N:MODS (0…255 each), got \(spec)")
        }
        return KeyEvent(key: key, modifiers: mods)
    }

    /// The decoders this fake phone claims (order = preference).
    var codecs: [CodecInfo] {
        switch codec {
        case .hevc: return [CodecInfo(name: "hevc", maxWidth: 3840, maxHeight: 2160)]
        case .h264: return [CodecInfo(name: "h264", maxWidth: 1920, maxHeight: 1080)]
        case nil: return [CodecInfo(name: "hevc", maxWidth: 3840, maxHeight: 2160),
                          CodecInfo(name: "h264", maxWidth: 1920, maxHeight: 1080)]
        }
    }
}

enum ProbeError: Error, CustomStringConvertible {
    case usage(String)
    case failed(String)

    var description: String {
        switch self {
        case .usage(let s), .failed(let s): return s
        }
    }
}
