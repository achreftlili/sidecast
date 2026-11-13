// VirtualDisplayHelper — creates and owns one CGVirtualDisplay in its own process.
//
// Protocol (stdout, one line each, nothing else is ever written to stdout):
//   READY <displayID>        display exists AND is in CoreGraphics' online + active lists
//   ERROR <message>          startup failed; the process exits non-zero
// Everything else (diagnostics) goes to stderr, prefixed "[vdh]".
//
// Lifetime: the display lives until stdin reaches EOF (parent closed the pipe or
// died), SIGTERM/SIGINT/SIGHUP arrives, or the system terminates the display.
//
// Usage:
//   VirtualDisplayHelper --width 1280 --height 800 --pixel-width 2560 --pixel-height 1600 \
//       --refresh 90 --hidpi 1 --serial 1234 --name "Sidecast" [options]
// Experiment switches (defaults match what Sidecast does today, except activation):
//   --no-activate        skip SkyLight SLSConfigureDisplayEnabled
//   --primaries 0|1      set sRGB primaries + white point on the descriptor (default 1)
//   --mm WxH             override physical size in millimetres (default: from 220/110 PPI)
//   --standalone         do not exit on stdin EOF (for running by hand)
//   --no-verify-fail     keep running even if the display never registers

import CoreGraphics
import CVirtualDisplay
import Darwin
import Foundation

setvbuf(stdout, nil, _IOLBF, 0)

func diag(_ s: String) { FileHandle.standardError.write(Data("[vdh] \(s)\n".utf8)) }
func fail(_ s: String, code: Int32 = 1) -> Never {
    print("ERROR \(s.replacingOccurrences(of: "\n", with: " "))")
    diag("fatal: \(s)")
    exit(code)
}

// MARK: - Arguments

var opts: [String: String] = [:]
var flags = Set<String>()
do {
    var it = CommandLine.arguments.dropFirst().makeIterator()
    let boolFlags: Set<String> = ["no-activate", "standalone", "no-verify-fail"]
    while let a = it.next() {
        guard a.hasPrefix("--") else { fail("unexpected argument \(a)") }
        let k = String(a.dropFirst(2))
        if boolFlags.contains(k) { flags.insert(k) } else {
            guard let v = it.next() else { fail("missing value for \(a)") }
            opts[k] = v
        }
    }
}
func intOpt(_ k: String) -> Int {
    guard let s = opts[k], let v = Int(s), v > 0 else { fail("missing or invalid --\(k)") }
    return v
}

let width = intOpt("width"), height = intOpt("height")
let pixelW = intOpt("pixel-width"), pixelH = intOpt("pixel-height")
let refresh = intOpt("refresh")
let hiDPI = (opts["hidpi"] ?? "1") != "0"
let serial = UInt32(opts["serial"] ?? "1") ?? 1
let name = opts["name"] ?? "Sidecast"
let usePrimaries = (opts["primaries"] ?? "1") != "0"
let vendorID: UInt32 = 0x5343

// MARK: - SkyLight (resolved at runtime; absence is not fatal)

typealias SLSBeginFn = @convention(c) (UnsafeMutablePointer<CGDisplayConfigRef?>) -> CGError
typealias SLSEnabledFn = @convention(c) (CGDisplayConfigRef?, CGDirectDisplayID, Bool) -> CGError
typealias SLSCompleteFn = @convention(c) (CGDisplayConfigRef?, CGConfigureOption, UInt32) -> CGError

struct SkyLight {
    let begin: SLSBeginFn, enabled: SLSEnabledFn, complete: SLSCompleteFn

    static func load() -> SkyLight? {
        let path = "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight"
        guard let h = dlopen(path, RTLD_LAZY) else {
            diag("dlopen SkyLight failed: \(String(cString: dlerror()))"); return nil
        }
        func sym<T>(_ n: String, _ t: T.Type) -> T? {
            guard let p = dlsym(h, n) else { diag("dlsym \(n): missing"); return nil }
            diag("dlsym \(n): ok")
            return unsafeBitCast(p, to: t)
        }
        guard let b = sym("SLSBeginDisplayConfiguration", SLSBeginFn.self),
              let e = sym("SLSConfigureDisplayEnabled", SLSEnabledFn.self),
              let c = sym("SLSCompleteDisplayConfiguration", SLSCompleteFn.self) else { return nil }
        return SkyLight(begin: b, enabled: e, complete: c)
    }
}

// MARK: - CoreGraphics state

func displayLists() -> (online: [CGDirectDisplayID], active: [CGDirectDisplayID]) {
    var ids = [CGDirectDisplayID](repeating: 0, count: 32)
    var n: UInt32 = 0
    var online: [CGDirectDisplayID] = [], active: [CGDirectDisplayID] = []
    if CGGetOnlineDisplayList(32, &ids, &n) == .success { online = Array(ids.prefix(Int(n))) }
    if CGGetActiveDisplayList(32, &ids, &n) == .success { active = Array(ids.prefix(Int(n))) }
    return (online, active)
}

func snapshot(_ id: CGDirectDisplayID, _ label: String) {
    let l = displayLists()
    diag("\(label): target=\(id) onlineList=\(l.online) activeList=\(l.active) "
         + "isOnline=\(CGDisplayIsOnline(id)) isActive=\(CGDisplayIsActive(id)) "
         + "bounds=\(CGDisplayBounds(id)) vendor=\(CGDisplayVendorNumber(id)) "
         + "mirrors=\(CGDisplayMirrorsDisplay(id)) inMirrorSet=\(CGDisplayIsInMirrorSet(id)) main=\(CGMainDisplayID())")
}

func registered(_ id: CGDirectDisplayID) -> Bool {
    let l = displayLists()
    return l.online.contains(id) && l.active.contains(id) && CGDisplayBounds(id).width > 0
}

// MARK: - Create the display

diag("macOS \(ProcessInfo.processInfo.operatingSystemVersionString), pid \(getpid())")
diag("config: \(width)x\(height) pt, \(pixelW)x\(pixelH) px, \(refresh) Hz, hiDPI=\(hiDPI), "
     + "serial=\(serial), name=\(name), primaries=\(usePrimaries), activate=\(!flags.contains("no-activate"))")

guard NSClassFromString("CGVirtualDisplay") != nil else { fail("CGVirtualDisplay class not available") }

let descriptor = CGVirtualDisplayDescriptor()
descriptor.name = name
descriptor.maxPixelsWide = UInt32(pixelW)
descriptor.maxPixelsHigh = UInt32(pixelH)
let ppi = hiDPI ? 220.0 : 110.0
var mm = CGSize(width: Double(pixelW) * 25.4 / ppi, height: Double(pixelH) * 25.4 / ppi)
if let o = opts["mm"] {
    let p = o.lowercased().split(separator: "x").compactMap { Double($0) }
    guard p.count == 2, p[0] > 0, p[1] > 0 else { fail("--mm expects WxH") }
    mm = CGSize(width: p[0], height: p[1])
}
descriptor.sizeInMillimeters = mm
descriptor.vendorID = vendorID
descriptor.productID = UInt32(truncatingIfNeeded: pixelW * 10_000 + pixelH)
descriptor.serialNum = serial
if usePrimaries {
    // sRGB / Rec.709, D65 — same values Lumina uses.
    descriptor.whitePoint = CGPoint(x: 0.3127, y: 0.3290)
    descriptor.redPrimary = CGPoint(x: 0.64, y: 0.33)
    descriptor.greenPrimary = CGPoint(x: 0.30, y: 0.60)
    descriptor.bluePrimary = CGPoint(x: 0.15, y: 0.06)
}
descriptor.queue = DispatchQueue.global(qos: .userInteractive)
descriptor.terminationHandler = { diag("terminated by the system"); exit(3) }
diag("descriptor: size=\(mm.width)x\(mm.height) mm (\(ppi) PPI) vendor=0x5343 product=\(descriptor.productID)")

let settings = CGVirtualDisplaySettings()
settings.hiDPI = hiDPI ? 1 : 0
var modes: [CGVirtualDisplayMode] = []
if hiDPI { modes.append(CGVirtualDisplayMode(width: UInt32(pixelW), height: UInt32(pixelH), refreshRate: Double(refresh))) }
modes.append(CGVirtualDisplayMode(width: UInt32(width), height: UInt32(height), refreshRate: Double(refresh)))
settings.modes = modes

guard let display = CGVirtualDisplay(descriptor: descriptor) else { fail("CGVirtualDisplay(descriptor:) returned nil") }
let displayID = display.displayID
diag("created: id=\(displayID)")
snapshot(displayID, "after create")
guard display.apply(settings) else { fail("applySettings rejected the modes") }
snapshot(displayID, "after applySettings")

// MARK: - Activate (SkyLight) and force extended layout

if !flags.contains("no-activate") {
    if let sl = SkyLight.load() {
        var cfg: CGDisplayConfigRef?
        let b = sl.begin(&cfg)
        diag("SLSBeginDisplayConfiguration -> \(b.rawValue)")
        if b == .success, let cfg {
            let e = sl.enabled(cfg, displayID, true)
            diag("SLSConfigureDisplayEnabled(\(displayID), true) -> \(e.rawValue)")
            let c = sl.complete(cfg, .forSession, 0)
            diag("SLSCompleteDisplayConfiguration -> \(c.rawValue)")
        }
    } else {
        diag("SkyLight activation unavailable; continuing without it")
    }
    usleep(500_000)
    snapshot(displayID, "after SLS activation")
}

func extendIfMirrored() {
    guard CGDisplayIsInMirrorSet(displayID) != 0 || CGDisplayMirrorsDisplay(displayID) != kCGNullDirectDisplay else { return }
    var cfg: CGDisplayConfigRef?
    guard CGBeginDisplayConfiguration(&cfg) == .success, let cfg else { return }
    let r = CGConfigureDisplayMirrorOfDisplay(cfg, displayID, kCGNullDirectDisplay)
    let c = CGCompleteDisplayConfiguration(cfg, .forSession)
    diag("un-mirror: configure=\(r.rawValue) complete=\(c.rawValue)")
}
extendIfMirrored()

// MARK: - Verify registration (poll up to 6 s)

var ok = false
for _ in 0..<12 {
    if registered(displayID) { ok = true; break }
    usleep(500_000)
    extendIfMirrored()
}
snapshot(displayID, "final")
if !ok && !flags.contains("no-verify-fail") {
    fail("display \(displayID) never appeared in CoreGraphics online+active lists", code: 2)
}
if !ok { diag("WARNING: not registered, continuing because --no-verify-fail") }

// MARK: - Hand the ID to the parent and hold the display

print("READY \(displayID)")

var sources: [DispatchSourceProtocol] = []
func shutdown(_ why: String) -> Never {
    diag("shutting down (\(why)); releasing display \(displayID)")
    _ = display // keep the reference alive until this point
    exit(0)
}
for sig in [SIGTERM, SIGINT, SIGHUP] {
    signal(sig, SIG_IGN)
    let s = DispatchSource.makeSignalSource(signal: sig, queue: .main)
    s.setEventHandler { shutdown("signal \(sig)") }
    s.resume()
    sources.append(s)
}
if !flags.contains("standalone") {
    let s = DispatchSource.makeReadSource(fileDescriptor: STDIN_FILENO, queue: .main)
    s.setEventHandler {
        var buf = [UInt8](repeating: 0, count: 256)
        let n = read(STDIN_FILENO, &buf, buf.count)
        if n == 0 { shutdown("stdin closed") }           // parent closed pipe or died
        if n > 0, String(decoding: buf.prefix(n), as: UTF8.self).contains("quit") { shutdown("quit command") }
    }
    s.resume()
    sources.append(s)
}
RunLoop.main.run()
