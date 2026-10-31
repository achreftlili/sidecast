import AVFoundation
import SidecastKit
import UIKit

/// Where to connect: a row of the list or "Add by IP address".
struct SessionTarget: Equatable {
    var host: String
    var port: Int
    var serverId: String?
    var name: String
    /// Bonjour instance name while the computer is discovered on this network.
    var bonjourService: String?
    /// Raw platform as last known (TXT `os` / pairing); nil = unknown yet.
    var platform: String?
}

/// Rates over the last sampling interval, for the stats overlay.
struct StatsSnapshot {
    var rxFps: Int
    var mbps: Double
    var shownFps: Int
    var rttMs: Int?
    var dropped: Int
}

/// Everything behind one display screen: the reconnect loop, the video decoder
/// and the microphone, confined to one serial queue (the network's). The view
/// controller talks to it from the main thread; its callbacks arrive on main.
final class DisplayController {
    var onState: ((DisplaySession.State) -> Void)?
    var onStreamConfig: ((StreamConfig) -> Void)?
    var onCursor: ((CursorPosition?) -> Void)?
    var onRoute: ((Route?) -> Void)?
    var onMicCapturing: ((Bool) -> Void)?
    var onDecoderStatus: ((DecoderStatus) -> Void)? {
        didSet { decoder.onStatus = onDecoderStatus }
    }

    /// Main-thread copy of the user's input mode (§7.3).
    private(set) var inputMode: InputMode
    let target: SessionTarget

    private let queue = DispatchQueue(label: "sidecast.session", qos: .userInteractive)
    private let settings = AppSettings.shared
    private let pairings = KeychainPairingStore.shared
    private let decoder: VideoDecoder
    private var session: DisplaySession!
    private var mic: MicStreamer!
    private var started = false
    private var lastSample: (time: TimeInterval, frames: Int, bytes: Int, rendered: Int)?

    init(target: SessionTarget, screen: ScreenInfo, renderer: AVSampleBufferVideoRenderer) {
        self.target = target
        inputMode = settings.inputMode
        let queue = queue
        // Weak: the session owns the client factory below, which must not own it back.
        weak var session: DisplaySession?
        decoder = VideoDecoder(renderer: renderer, sessionQueue: queue) { session?.requestKeyframe() }
        let hello = Hello(deviceId: DeviceIdentity.deviceId, deviceName: UIDevice.current.name, screen: screen,
                          codecs: DecoderCapabilities.advertised(override: settings.codecOverride),
                          // Always advertised; the setting is enforced by answering MIC_STATE disabled.
                          features: [ClientFeature.mic])
        Log.session.info("HELLO screen \(screen.width)x\(screen.height)@\(screen.refreshRate) codecs \(hello.codecs.map(\.name), privacy: .public)")
        let pairings = pairings, settings = settings
        let scheduler = DispatchQueueScheduler(queue: queue)
        var firstAttempt = true
        let s = DisplaySession(name: target.name, serverId: target.serverId, platform: target.platform,
                               pairings: pairings, scheduler: scheduler, inputMode: inputMode) {
            expectedServerId, counters, inputMode in
            let paired = expectedServerId.flatMap(pairings.pairing(serverId:))
            // A Bonjour service is only "current" for the first attempt; reconnects (e.g. after
            // leaving the Wi-Fi) run the full plan of the pairing (§1.1) when there is one.
            let bonjour = firstAttempt || paired == nil ? target.bonjourService : nil
            firstAttempt = false
            let plan = ConnectPlanner.plan(host: target.host, port: target.port, bonjourService: bonjour, paired: paired)
            Log.session.info("connect plan: \(plan.map(\.description), privacy: .public)")
            // Every candidate is pinned when paired: the token never goes to another computer.
            let connector = NetworkConnector(candidates: plan, pin: paired?.fingerprint, queue: queue,
                                             onAttempt: { session?.reportRoute($0.route) },
                                             log: { Log.session.info("\($0, privacy: .public)") })
            // Evaluated per connection: the device may have moved to (or off) cellular.
            let metered = NetworkMonitor.shared.isMetered
            return StreamClient(connector: connector,
                                hello: DataSaver.apply(to: hello, metered: metered, enabled: settings.dataSaver),
                                pairings: pairings, endpoint: .init(host: target.host, port: target.port),
                                scheduler: scheduler, counters: counters, inputMode: inputMode,
                                log: { Log.session.info("\($0, privacy: .public)") })
        }
        session = s
        self.session = s
        mic = MicStreamer(sink: SessionMicSink(session: s), makeRecorder: { AudioEngineRecorder(queue: queue) },
                          permission: SystemMicPermission(queue: queue), isEnabled: { settings.micEnabled },
                          log: { Log.mic.info("\($0, privacy: .public)") })
        wire()
    }

    private func wire() {
        let decoder = decoder, mic = mic!
        session.onState = { [weak self] state in
            switch state {
            case .streaming: break
            default: decoder.reset()   // a new connection starts with a new STREAM_CONFIG
            }
            DispatchQueue.main.async { self?.onState?(state) }
        }
        session.onStreamConfig = { [weak self] config in
            decoder.configure(config)
            DispatchQueue.main.async { self?.onStreamConfig?(config) }
        }
        session.onVideoFrame = { decoder.decode($0) }
        session.onCursor = { [weak self] cursor in DispatchQueue.main.async { self?.onCursor?(cursor) } }
        session.onRoute = { [weak self] route in DispatchQueue.main.async { self?.onRoute?(route) } }
        session.onMicControl = { mic.handle($0) }
        session.onMicSessionEnded = { mic.sessionEnded() }
        mic.onCapturingChange = { [weak self] on in DispatchQueue.main.async { self?.onMicCapturing?(on) } }
    }

    /// The computer's OS as currently known (main thread, best effort for overlay advice).
    var platform: HostPlatform? {
        queue.sync { session.platform }.flatMap(HostPlatform.init(rawValue:)) ?? target.platform.flatMap(HostPlatform.init(rawValue:))
    }

    func start() {
        guard !started else { return }
        started = true
        queue.async { [session] in session?.start() }
    }

    /// The screen is visible again: reconnect if the app was in the background.
    func enterForeground() {
        queue.async { [session, mic] in
            mic?.enterForeground()
            session?.resume()
        }
    }

    /// iOS suspends apps in the background, so let go of the stream (and never record there).
    func enterBackground() {
        queue.async { [session, mic] in
            mic?.enterBackground()
            session?.pause()
        }
    }

    /// Terminal.
    func stop() {
        queue.async { [session, mic, decoder] in
            mic?.release()
            session?.stop()
            decoder.reset()
        }
    }

    func retry() { queue.async { [session] in session?.retryNow() } }
    func pairAgain() { queue.async { [session] in session?.forgetAndRepair() } }
    func expirePairing() { queue.async { [session] in session?.expirePairing() } }

    /// False when the code was not accepted for sending.
    func submitCode(_ code: String) -> Bool { queue.sync { session.submitCode(code) } }

    func setInputMode(_ mode: InputMode) {
        inputMode = mode
        settings.inputMode = mode
        queue.async { [session] in session?.setInputMode(mode) }
    }

    func micSettingChanged() { queue.async { [mic] in mic?.settingChanged() } }

    func send(_ touch: TouchEvent) { queue.async { [session] in session?.sendTouch(touch) } }

    func send(_ event: KeyboardEvent) {
        queue.async { [session] in
            switch event {
            case .text(let text): session?.sendText(text)
            case .key(let key): session?.sendKey(key)
            }
        }
    }

    /// Main thread, about once a second.
    func sampleStats() -> StatsSnapshot? {
        let counters = session.counters
        let now = ProcessInfo.processInfo.systemUptime
        let current = (time: now, frames: counters.videoFrames, bytes: counters.videoBytes, rendered: decoder.rendered)
        defer { lastSample = current }
        guard let last = lastSample, now > last.time else { return nil }
        let seconds = now - last.time
        let rtt = counters.rttMicros
        return StatsSnapshot(rxFps: Int((Double(current.frames - last.frames) / seconds).rounded()),
                             mbps: Double(current.bytes - last.bytes) * 8 / seconds / 1e6,
                             shownFps: Int((Double(current.rendered - last.rendered) / seconds).rounded()),
                             rttMs: rtt >= 0 ? Int((rtt + 500) / 1000) : nil,
                             dropped: decoder.dropped)
    }
}

/// MIC_STATE / AUDIO → the stream that asked for the microphone (session queue).
private final class SessionMicSink: MicSink {
    private weak var session: DisplaySession?

    init(session: DisplaySession) { self.session = session }

    func sendMicState(_ state: MicState) { session?.sendMicState(state) }
    func sendAudio(_ packet: AudioPacket) { session?.sendAudio(packet) }
}

extension ScreenInfo {
    /// HELLO `screen` (§5): the whole panel in landscape pixels (the stream fills it,
    /// under the Dynamic Island and home indicator too). iOS points are close to
    /// Android dp, so `densityDpi` is 160 per point scale; refresh is the panel's peak.
    static func landscape(of screen: UIScreen) -> ScreenInfo {
        let pixels = screen.nativeBounds.size
        return ScreenInfo(width: Int(max(pixels.width, pixels.height)), height: Int(min(pixels.width, pixels.height)),
                          densityDpi: Int((screen.nativeScale * 160).rounded()),
                          refreshRate: Double(max(60, screen.maximumFramesPerSecond)))
    }
}
