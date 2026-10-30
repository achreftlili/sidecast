import Foundation
import SidecastKit

/// Command-line options. Test mode (`--test-pattern`) runs headless with a
/// synthetic frame source: no virtual display, capture, input injection,
/// sleep guard, permission prompts or menu bar.
struct LaunchOptions {
    var testPattern = false
    var dataDirectory: URL?
    var port: UInt16?
    var testPairingCode: PairingCode?
    var bonjour = true
    var fps: Int?
    var maxBitrateMbps: Int?
    var verbose = false
    /// Test mode: send MIC_CONTROL active:true as soon as a mic-capable client streams.
    var testMicActive = false
    /// Test mode: advertise this extra `tailscale` address in SERVER_HELLO.
    var testFakeTailscale: String?

    static let usage = """
    Usage: Sidecast [--test-pattern --data-dir <dir> [--port N] [--test-pairing-code 123456]
                    [--no-bonjour] [--fps 30|60|120] [--max-mbps N] [--test-mic-active]
                    [--test-fake-tailscale 100.x.y.z] [--verbose]]
    """

    /// Parses `arguments` (including argv[0]). Unknown arguments are ignored,
    /// since macOS may pass its own (e.g. -NSDocumentRevisionsDebugMode).
    static func parse(_ arguments: [String]) throws -> LaunchOptions {
        var o = LaunchOptions()
        var it = arguments.dropFirst().makeIterator()
        func value(_ flag: String) throws -> String {
            guard let v = it.next() else { throw OptionError("\(flag) needs a value") }
            return v
        }
        var pairingCode: String?
        while let arg = it.next() {
            switch arg {
            case "--test-pattern": o.testPattern = true
            case "--data-dir":
                let path = (try value(arg) as NSString).expandingTildeInPath
                o.dataDirectory = URL(fileURLWithPath: path, isDirectory: true).standardizedFileURL
            case "--port":
                guard let p = UInt16(try value(arg)), p > 0 else { throw OptionError("invalid --port") }
                o.port = p
            case "--test-pairing-code": pairingCode = try value(arg)
            case "--no-bonjour": o.bonjour = false
            case "--fps":
                guard let f = Int(try value(arg)), StreamSettings.allowedFps.contains(f) else {
                    throw OptionError("--fps must be 30, 60 or 120")
                }
                o.fps = f
            case "--max-mbps":
                guard let m = Int(try value(arg)), StreamSettings.bitrateRangeMbps.contains(m) else {
                    throw OptionError("--max-mbps must be 10…100")
                }
                o.maxBitrateMbps = m
            case "--verbose": o.verbose = true
            case "--test-mic-active": o.testMicActive = true
            case "--test-fake-tailscale":
                let ip = try value(arg)
                guard NetworkAddresses.octets(ip) != nil else { throw OptionError("--test-fake-tailscale needs an IPv4 address") }
                o.testFakeTailscale = ip
            case "--help", "-h": throw OptionError(usage)
            default: break
            }
        }
        if let pairingCode {
            guard let code = PairingCode(pairingCode) else { throw OptionError("--test-pairing-code must be 6 digits") }
            // Honoured only in test mode: a fixed code must never reach a real install.
            if o.testPattern { o.testPairingCode = code }
        }
        if o.testPattern, o.dataDirectory == nil {
            throw OptionError("--test-pattern requires --data-dir")
        }
        return o
    }

    struct OptionError: Error, CustomStringConvertible {
        let description: String
        init(_ d: String) { description = d }
    }
}
