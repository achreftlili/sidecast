import AppKit
import CoreGraphics
import SidecastKit

/// An extended virtual display owned by a separate helper process
/// (`Contents/Helpers/VirtualDisplayHelper`). Creating CGVirtualDisplay inside this
/// menu-bar process left the display unregistered with WindowServer (never online/
/// active, invisible to ScreenCaptureKit); in its own process it registers
/// reliably. The display exists while the helper lives: stopping closes the
/// helper's stdin (it also exits if this app dies), then SIGTERM as a fallback.
/// Main thread only, except where noted.
final class VirtualDisplay {
    /// "SC". Also used to tell our displays apart from physical ones.
    static let vendorID: UInt32 = 0x5343

    enum DisplayError: Error, CustomStringConvertible {
        case helperMissing(String)
        case launchFailed(String)
        case helperFailed(String)
        case timedOut

        var description: String {
            switch self {
            case .helperMissing(let p): return "VirtualDisplayHelper not found at \(p)"
            case .launchFailed(let e): return "could not launch VirtualDisplayHelper: \(e)"
            case .helperFailed(let e): return "virtual display helper failed: \(e)"
            case .timedOut: return "virtual display helper did not report a display in time"
            }
        }
    }

    let mode: VirtualDisplayMode
    /// Called on the main queue if the helper dies while the display is in use.
    var onUnexpectedExit: (() -> Void)?

    private let process: Process
    private let stdinPipe: Pipe
    private let state = NSLock()
    private var stopping = false          // guarded by state
    private var id: CGDirectDisplayID     // guarded by state
    private var screenObserver: NSObjectProtocol?

    var displayID: CGDirectDisplayID {
        state.lock(); defer { state.unlock() }
        return id
    }

    private init(mode: VirtualDisplayMode, process: Process, stdinPipe: Pipe, id: CGDirectDisplayID) {
        self.mode = mode
        self.process = process
        self.stdinPipe = stdinPipe
        self.id = id
    }

    static func helperURL() -> URL? {
        let fm = FileManager.default
        var candidates = [Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/VirtualDisplayHelper")]
        // swift run / .build: the helper is built next to the app binary.
        if let exe = Bundle.main.executableURL {
            candidates.append(exe.deletingLastPathComponent().appendingPathComponent("VirtualDisplayHelper"))
        }
        return candidates.first { fm.isExecutableFile(atPath: $0.path) }
    }

    /// Starts the helper and returns once the display is registered with CoreGraphics.
    /// - Parameter serial: distinguishes phones, so macOS remembers an arrangement per phone.
    @MainActor
    static func start(mode: VirtualDisplayMode, name: String, serial: UInt32) async throws -> VirtualDisplay {
        guard let url = helperURL() else {
            throw DisplayError.helperMissing(Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/VirtualDisplayHelper").path)
        }
        let process = Process()
        process.executableURL = url
        process.arguments = [
            "--width", "\(mode.width)", "--height", "\(mode.height)",
            "--pixel-width", "\(mode.pixelWidth)", "--pixel-height", "\(mode.pixelHeight)",
            "--refresh", "\(mode.refreshRate)", "--hidpi", mode.hiDPI ? "1" : "0",
            "--serial", "\(serial)", "--name", name,
        ]
        let inPipe = Pipe(), outPipe = Pipe(), errPipe = Pipe()
        process.standardInput = inPipe
        process.standardOutput = outPipe
        process.standardError = errPipe

        // Resolved exactly once: first READY/ERROR line, early exit, or timeout.
        let result = StartResult()
        errPipe.fileHandleForReading.readabilityHandler = { h in
            let data = h.availableData
            if data.isEmpty { h.readabilityHandler = nil; return }
            for line in String(decoding: data, as: UTF8.self).split(whereSeparator: \.isNewline) {
                Log.debug("display helper: \(line)")
            }
        }
        let outBuffer = DataBox()
        outPipe.fileHandleForReading.readabilityHandler = { h in
            let data = h.availableData
            if data.isEmpty { h.readabilityHandler = nil; return }
            outBuffer.data.append(data)
            while let nl = outBuffer.data.firstIndex(of: 0x0A) {
                let line = String(decoding: outBuffer.data[..<nl], as: UTF8.self)
                outBuffer.data.removeSubrange(...nl)
                if line.hasPrefix("READY "), let n = UInt32(line.dropFirst(6)), n != 0 {
                    result.resolve(.success(n))
                } else if line.hasPrefix("ERROR ") {
                    result.resolve(.failure(DisplayError.helperFailed(String(line.dropFirst(6)))))
                }
            }
        }
        let box = ExitBox()
        process.terminationHandler = { p in
            box.status = p.terminationStatus
            // Exit before READY = startup failure; the result is first-wins.
            result.resolve(.failure(DisplayError.helperFailed("helper exited with status \(p.terminationStatus) before reporting a display")))
            box.fire()
        }
        do { try process.run() } catch { throw DisplayError.launchFailed("\(error)") }
        Log.info("display: helper started (pid \(process.processIdentifier))")

        let timeout = DispatchWorkItem { result.resolve(.failure(DisplayError.timedOut)) }
        DispatchQueue.global().asyncAfter(deadline: .now() + 20, execute: timeout)

        let id: UInt32
        do { id = try await result.value() } catch {
            timeout.cancel()
            terminate(process, stdin: inPipe)
            throw error
        }
        timeout.cancel()

        let display = VirtualDisplay(mode: mode, process: process, stdinPipe: inPipe, id: id)
        box.onExit = { [weak display] in
            DispatchQueue.main.async {
                guard let display, !display.isStopping else { return }
                Log.error("display: helper exited unexpectedly (status \(box.status))")
                display.markGone()
                display.onUnexpectedExit?()
            }
        }
        Log.info("display: created \(mode.width)x\(mode.height) pt\(mode.hiDPI ? " HiDPI (\(mode.pixelWidth)x\(mode.pixelHeight) px)" : "") "
                 + "@\(mode.refreshRate) Hz, id \(id) (helper pid \(process.processIdentifier))")
        display.logDiagnostics()
        display.ensurePhysicalDisplayStaysMain()
        display.screenObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak display] _ in display?.ensurePhysicalDisplayStaysMain() }
        return display
    }

    private var isStopping: Bool { state.lock(); defer { state.unlock() }; return stopping }
    private func markGone() { state.lock(); id = kCGNullDirectDisplay; state.unlock() }

    /// Stops the helper, which releases the virtual display. Idempotent.
    func stop() {
        state.lock()
        if stopping { state.unlock(); return }
        stopping = true
        id = kCGNullDirectDisplay
        state.unlock()
        if let screenObserver { NotificationCenter.default.removeObserver(screenObserver) }
        screenObserver = nil
        Self.terminate(process, stdin: stdinPipe)
        Log.info("display: destroyed")
    }

    deinit { stop() }

    /// Close stdin (the helper's cue to exit), wait briefly, then SIGTERM, then SIGKILL.
    private static func terminate(_ p: Process, stdin: Pipe) {
        try? stdin.fileHandleForWriting.close()
        guard p.isRunning else { return }
        let pid = p.processIdentifier
        DispatchQueue.global().async {
            for _ in 0..<20 where p.isRunning { usleep(100_000) }
            if p.isRunning { p.terminate() }
            for _ in 0..<20 where p.isRunning { usleep(100_000) }
            if p.isRunning { kill(pid, SIGKILL) }
        }
    }

    /// Logs CoreGraphics' view of the display (kept from the registration investigation).
    private func logDiagnostics() {
        let id = displayID
        var onlineIDs = [CGDirectDisplayID](repeating: 0, count: 32)
        var onlineCount: UInt32 = 0
        let onlineResult = CGGetOnlineDisplayList(32, &onlineIDs, &onlineCount)
        var activeIDs = [CGDirectDisplayID](repeating: 0, count: 32)
        var activeCount: UInt32 = 0
        let activeResult = CGGetActiveDisplayList(32, &activeIDs, &activeCount)
        Log.info("""
            display diagnostic:
            target ID: \(id)
            online result: \(onlineResult.rawValue)
            online IDs: \(onlineIDs.prefix(Int(onlineCount)))
            active result: \(activeResult.rawValue)
            active IDs: \(activeIDs.prefix(Int(activeCount)))
            online: \(CGDisplayIsOnline(id))
            active: \(CGDisplayIsActive(id))
            bounds: \(CGDisplayBounds(id))
            vendor: \(CGDisplayVendorNumber(id))
            """)
    }

    /// With HiDPI, makes sure the 2× mode (logical size, double pixels) is the one in use.
    func ensurePreferredMode() {
        let id = displayID
        guard mode.hiDPI, id != kCGNullDirectDisplay else { return }
        if let current = CGDisplayCopyDisplayMode(id),
           current.width == mode.width, current.pixelWidth == mode.pixelWidth { return }
        let options = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
        guard let all = CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode],
              let wanted = all.first(where: { $0.width == mode.width && $0.height == mode.height
                  && $0.pixelWidth == mode.pixelWidth && $0.pixelHeight == mode.pixelHeight }) else {
            Log.info("display: HiDPI mode not offered; keeping the current mode")
            return
        }
        var config: CGDisplayConfigRef?
        guard CGBeginDisplayConfiguration(&config) == .success, let config else { return }
        guard CGConfigureDisplayWithDisplayMode(config, id, wanted, nil) == .success else {
            CGCancelDisplayConfiguration(config)
            return
        }
        if CGCompleteDisplayConfiguration(config, .forSession) == .success {
            Log.info("display: switched to the HiDPI mode")
        }
    }

    /// Safety net (SideScreen #39): while a physical display is online, the
    /// main slot must never belong to the virtual display — otherwise the menu
    /// bar, Dock and keyboard focus land on a screen nobody is looking at.
    func ensurePhysicalDisplayStaysMain() {
        let id = displayID
        guard id != kCGNullDirectDisplay, CGMainDisplayID() == id,
              let physical = Self.onlinePhysicalDisplays(excluding: id).first else { return }
        var config: CGDisplayConfigRef?
        guard CGBeginDisplayConfiguration(&config) == .success, let config else { return }
        let physicalWidth = Int32(CGDisplayBounds(physical).width)
        var result = CGConfigureDisplayOrigin(config, physical, 0, 0)
        if result == .success { result = CGConfigureDisplayOrigin(config, id, physicalWidth, 0) }
        guard result == .success else {
            CGCancelDisplayConfiguration(config)
            Log.error("display: could not restore the physical main display (\(result.rawValue))")
            return
        }
        // Session-scoped: never bake the virtual display into the permanent arrangement.
        if CGCompleteDisplayConfiguration(config, .forSession) == .success {
            Log.info("display: physical display restored as main; virtual display parked beside it")
        }
    }

    /// Online displays that are not ours (vendor 0x5343) — a stale display of a
    /// previous run must not count as physical.
    static func onlinePhysicalDisplays(excluding id: CGDirectDisplayID) -> [CGDirectDisplayID] {
        var ids = [CGDirectDisplayID](repeating: 0, count: 16)
        var count: UInt32 = 0
        guard CGGetOnlineDisplayList(16, &ids, &count) == .success else { return [] }
        return ids.prefix(Int(count)).filter { $0 != id && CGDisplayVendorNumber($0) != vendorID }
    }

    /// Stable 32-bit serial for a phone (FNV-1a of its deviceId).
    static func serial(for deviceId: String) -> UInt32 {
        var hash: UInt32 = 0x811C_9DC5
        for byte in deviceId.utf8 {
            hash ^= UInt32(byte)
            hash = hash &* 0x0100_0193
        }
        return hash == 0 ? 1 : hash
    }
}

/// First-wins result of the helper handshake, awaitable from the main actor.
private final class StartResult: @unchecked Sendable {
    private let lock = NSLock()
    private var outcome: Result<UInt32, Error>?
    private var continuation: CheckedContinuation<UInt32, Error>?

    func resolve(_ r: Result<UInt32, Error>) {
        lock.lock()
        guard outcome == nil else { lock.unlock(); return }
        outcome = r
        let c = continuation; continuation = nil
        lock.unlock()
        c?.resume(with: r)
    }

    func value() async throws -> UInt32 {
        try await withCheckedThrowingContinuation { c in
            lock.lock()
            if let o = outcome { lock.unlock(); c.resume(with: o) } else { continuation = c; lock.unlock() }
        }
    }
}

/// Lets the termination handler (registered before the display object exists) reach it later.
private final class ExitBox: @unchecked Sendable {
    private let lock = NSLock()
    private var fired = false
    private var handler: (() -> Void)?
    var status: Int32 = 0
    var onExit: (() -> Void)? {
        get { lock.lock(); defer { lock.unlock() }; return handler }
        set { lock.lock(); handler = newValue; let run = fired ? newValue : nil; lock.unlock(); run?() }
    }
    func fire() { lock.lock(); fired = true; let h = handler; lock.unlock(); h?() }
}

private final class DataBox: @unchecked Sendable { var data = Data() }   // touched only by one readability handler
