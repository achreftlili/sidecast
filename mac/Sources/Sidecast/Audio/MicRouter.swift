import CoreAudio
import Foundation
import SidecastKit

/// What the UI shows about the phone microphone.
struct PhoneMicStatus: Equatable {
    enum Phone: Equatable {
        case noPhone
        /// The phone's Sidecast app did not advertise `mic`.
        case unsupported
        /// "Use the phone's microphone" is off on the Mac.
        case off
        /// Sidecast Microphone is not installed (or Core Audio has not loaded it).
        case driverMissing
        /// Ready; no Mac app is recording from Sidecast Microphone.
        case idle
        /// MIC_CONTROL active:true sent, no MIC_STATE yet.
        case requested
        case listening
        /// MIC_STATE capturing:false with this reason.
        case phoneSays(String)
    }

    var driver: MicDriverCommands.Status = .notInstalled
    var phone: Phone = .noPhone
    /// Sidecast switched the Mac's default input to Sidecast Microphone.
    var isDefaultInput = false

    /// "Phone mic: …" (Status section and menu).
    var line: String {
        switch phone {
        case .noPhone: return "Phone mic: no phone connected"
        case .unsupported: return "Phone mic: not supported by the phone's Sidecast app"
        case .off: return "Phone mic: off"
        case .driverMissing: return "Phone mic: Sidecast Microphone not installed"
        case .idle: return "Phone mic: idle"
        case .requested: return "Phone mic: starting on the phone…"
        case .listening: return "Phone mic: listening (phone capturing)"
        case .phoneSays(let reason): return "Phone mic: phone says: \(Self.describe(reason))"
        }
    }

    var driverLine: String {
        switch driver {
        case .notBundled: return "Not available in this build"
        case .notInstalled: return "Not installed"
        case .installed: return "Installed"
        case .updateAvailable: return "Update available"
        }
    }

    var driverInstalled: Bool {
        switch driver {
        case .installed, .updateAvailable: return true
        case .notBundled, .notInstalled: return false
        }
    }

    static func describe(_ reason: String) -> String {
        switch MicStateReason(rawValue: reason) {
        case .permissionDenied: return "permission denied"
        case .disabled: return "disabled"
        case .background: return "background"
        case .unavailable: return "unavailable"
        case .stopped: return "stopped"
        case nil: return reason
        }
    }
}

/// Routes the phone's microphone into "Sidecast Microphone" (PROTOCOL §7.2,
/// DECISIONS Q23–Q25):
/// - watches `DeviceIsRunningSomewhere` on Sidecast Microphone (listener + a
///   1 s safety poll while a phone streams; re-resolved when devices change);
/// - running → MIC_CONTROL active:true and the feed player starts; not running
///   (or session over) → active:false and the player stops;
/// - optionally makes Sidecast Microphone the default input while a mic-capable
///   phone streams, restoring the previous one afterwards — on disconnect, on
///   quit, and at the next launch if a marker file says we left it switched.
/// All state lives on `queue`; Core Audio listeners are delivered there too.
final class MicRouter: MicBridge {
    struct Settings: Equatable {
        var enabled = true
        var makeDefaultInput = true
    }

    private struct Session {
        let id: UUID
        let deviceName: String
        let supportsMic: Bool
        let control: (Bool) -> Void
    }

    /// Main queue. Set before `startUp()`.
    var onStatus: ((PhoneMicStatus) -> Void)?

    private let queue = DispatchQueue(label: "com.achreftlili.sidecast.mic", qos: .userInitiated)
    private let installer: MicDriverInstaller
    private let markerURL: URL
    private let jitter = SharedJitterBuffer()
    private let player: MicFeedPlayer
    private let clock = MonotonicTimeSource()

    private var settings = Settings()
    private var session: Session?
    private var driverStatus: MicDriverCommands.Status = .notInstalled
    private var inputDevice = AudioObjectID(kAudioObjectUnknown)
    private var runningListener: AudioObjectPropertyListenerBlock?
    private var listenedDevice = AudioObjectID(kAudioObjectUnknown)
    private var devicesListener: AudioObjectPropertyListenerBlock?
    private var poll: DispatchSourceTimer?
    private var micActive = false
    private var phoneState: MicState?
    private var switchedDefault = false
    private var removingDriver = false
    private var lastStartFailure: TimeInterval?
    private var published: PhoneMicStatus?

    init(markerURL: URL, installer: MicDriverInstaller = MicDriverInstaller()) {
        self.markerURL = markerURL
        self.installer = installer
        player = MicFeedPlayer(buffer: jitter)
    }

    // MARK: - Control (any thread)

    /// Restores a default input a previous run left switched, starts watching devices.
    func startUp() {
        queue.async {
            self.installDevicesListener()
            self.refresh()
        }
    }

    /// `applicationWillTerminate`: stop and give the default input back, synchronously.
    func shutDown() {
        queue.sync {
            self.deactivate(tellPhone: false)
            self.stopPolling()
            self.restoreDefaultInput()
        }
    }

    func update(settings: Settings) {
        queue.async {
            guard settings != self.settings else { return }
            self.settings = settings
            self.refresh()
        }
    }

    /// Before the remove prompt: stop using the device and restore the default input.
    func prepareForDriverRemoval() {
        queue.sync {
            self.removingDriver = true
            self.deactivate(tellPhone: true)
            self.restoreDefaultInput()
            self.publish()
        }
    }

    /// After an install/remove attempt (Core Audio restarts: re-check a few times).
    func driverChanged() {
        queue.async {
            self.removingDriver = false
            self.refresh()
        }
        for delay in [2.0, 5.0] {
            queue.asyncAfter(deadline: .now() + delay) { self.refresh() }
        }
    }

    // MARK: - MicBridge (server queue)

    func sessionStarted(id: UUID, deviceName: String, supportsMic: Bool, control: @escaping (Bool) -> Void) {
        queue.async {
            self.deactivate(tellPhone: false)
            self.session = Session(id: id, deviceName: deviceName, supportsMic: supportsMic, control: control)
            self.phoneState = nil
            Log.info(supportsMic ? "mic: \(deviceName) can lend its microphone"
                                 : "mic: \(deviceName)'s Sidecast app has no microphone support")
            self.startPolling()
            self.refresh()
        }
    }

    func sessionEnded(id: UUID) {
        queue.async {
            guard self.session?.id == id else { return }
            self.deactivate(tellPhone: false)
            self.session = nil
            self.stopPolling()
            self.refresh()
        }
    }

    func handleAudio(_ packet: AudioPacket, from id: UUID) {
        // The session forwards AUDIO only while active:true is pending; the
        // buffer was reset before that was sent.
        jitter.push(packet)
    }

    func handleMicState(_ state: MicState, from id: UUID) {
        queue.async {
            guard self.session?.id == id, self.micActive else { return }
            self.phoneState = state
            Log.info("mic: phone reports capturing=\(state.capturing)\(state.reason.map { " (\($0))" } ?? "")")
            self.publish()
        }
    }

    // MARK: - Core (on queue)

    private func refresh() {
        driverStatus = installer.status()
        resolveDevice()
        applyDefaultInputPolicy()
        evaluate()
        publish()
    }

    private func resolveDevice() {
        let device = CoreAudioDevices.device(forUID: MicDriverCommands.inputDeviceUID) ?? AudioObjectID(kAudioObjectUnknown)
        guard device != inputDevice else { return }
        if inputDevice != kAudioObjectUnknown { Log.info("mic: Sidecast Microphone went away") }
        if micActive { deactivate(tellPhone: true) }   // its feed unit is gone too
        inputDevice = device
        if device != kAudioObjectUnknown { Log.info("mic: Sidecast Microphone is audio device \(device)") }
        watchRunning(device)
    }

    /// Mic wanted ⇔ mic-capable phone streaming, setting on, device present and some app recording from it.
    private func evaluate() {
        let want = session?.supportsMic == true && settings.enabled && !removingDriver
            && inputDevice != kAudioObjectUnknown && CoreAudioDevices.isRunningSomewhere(inputDevice)
        if want, !micActive {
            activate()
        } else if !want, micActive {
            deactivate(tellPhone: true)
        }
    }

    private func activate() {
        guard let session else { return }
        if let failed = lastStartFailure, clock.now() - failed < 5 { return }
        guard let feed = CoreAudioDevices.device(forUID: MicDriverCommands.feedDeviceUID) else {
            lastStartFailure = clock.now()
            return Log.error("mic: the hidden Sidecast Mic Feed device is missing")
        }
        jitter.reset()
        _ = jitter.takeStats()
        do {
            try player.start(device: feed)
        } catch {
            lastStartFailure = clock.now()
            return Log.error("mic: cannot play into Sidecast Mic Feed: \(error)")
        }
        lastStartFailure = nil
        micActive = true
        phoneState = nil
        Log.info("mic: an app is recording from Sidecast Microphone — asking \(session.deviceName) for its microphone")
        session.control(true)
    }

    private func deactivate(tellPhone: Bool) {
        guard micActive else { return }
        micActive = false
        player.stop()
        if tellPhone { session?.control(false) }
        phoneState = nil
        let s = jitter.takeStats()
        Log.info("mic: stopped — \(s.packets) packets, \(s.missingPackets) missing, \(s.underruns) underruns, "
                 + "\(s.droppedFrames) frames dropped")
    }

    // MARK: - Default input

    private func applyDefaultInputPolicy() {
        let want = session?.supportsMic == true && settings.enabled && settings.makeDefaultInput && !removingDriver
            && inputDevice != kAudioObjectUnknown
        if want {
            if !switchedDefault { switchDefaultInput() }
        } else {
            restoreDefaultInput()
        }
    }

    private func switchDefaultInput() {
        guard let current = CoreAudioDevices.defaultInputDevice(), current != inputDevice else { return }
        let previousUID = CoreAudioDevices.uid(of: current) ?? ""
        // Marker first: if we die right after switching, the next launch restores.
        do {
            try SecureFile.write(Data(previousUID.utf8), to: markerURL)
        } catch {
            return Log.error("mic: cannot write the default-input marker: \(error)")
        }
        let status = CoreAudioDevices.setDefaultInputDevice(inputDevice)
        if status == noErr {
            switchedDefault = true
            Log.info("mic: Sidecast Microphone is now the default input (was \(previousUID))")
        } else {
            try? FileManager.default.removeItem(at: markerURL)
            Log.error("mic: could not make Sidecast Microphone the default input (\(status))")
        }
    }

    /// Puts back the input the marker remembers, if Sidecast Microphone is still the default.
    private func restoreDefaultInput() {
        switchedDefault = false
        guard let data = try? Data(contentsOf: markerURL) else { return }
        defer { try? FileManager.default.removeItem(at: markerURL) }
        let previousUID = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        guard let ours = CoreAudioDevices.device(forUID: MicDriverCommands.inputDeviceUID),
              CoreAudioDevices.defaultInputDevice() == ours else {
            return Log.info("mic: the default input is no longer Sidecast Microphone — leaving it")
        }
        guard !previousUID.isEmpty, let previous = CoreAudioDevices.device(forUID: previousUID) else {
            return Log.info("mic: the previous default input (\(previousUID)) is gone — leaving the default as is")
        }
        let status = CoreAudioDevices.setDefaultInputDevice(previous)
        Log.info(status == noErr ? "mic: default input restored (\(previousUID))"
                                 : "mic: could not restore the default input (\(status))")
    }

    // MARK: - Watching Core Audio

    private func installDevicesListener() {
        guard devicesListener == nil else { return }
        var a = CoreAudioDevices.address(kAudioHardwarePropertyDevices)
        let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in self?.refresh() }
        if AudioObjectAddPropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &a, queue, block) == noErr {
            devicesListener = block
        }
    }

    private func watchRunning(_ device: AudioObjectID) {
        var a = CoreAudioDevices.address(kAudioDevicePropertyDeviceIsRunningSomewhere)
        if let old = runningListener, listenedDevice != kAudioObjectUnknown {
            AudioObjectRemovePropertyListenerBlock(listenedDevice, &a, queue, old)
        }
        runningListener = nil
        listenedDevice = AudioObjectID(kAudioObjectUnknown)
        guard device != kAudioObjectUnknown else { return }
        let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
            self?.evaluate()
            self?.publish()
        }
        if AudioObjectAddPropertyListenerBlock(device, &a, queue, block) == noErr {
            runningListener = block
            listenedDevice = device
        }
    }

    private func startPolling() {
        guard poll == nil else { return }
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now() + 1, repeating: 1, leeway: .milliseconds(100))
        t.setEventHandler { [weak self] in
            guard let self else { return }
            self.resolveDevice()
            self.applyDefaultInputPolicy()
            self.evaluate()
            self.publish()
        }
        t.resume()
        poll = t
    }

    private func stopPolling() {
        poll?.cancel()
        poll = nil
    }

    // MARK: - Status

    private func phoneStatus() -> PhoneMicStatus.Phone {
        guard let session else { return .noPhone }
        guard session.supportsMic else { return .unsupported }
        guard settings.enabled else { return .off }
        guard inputDevice != kAudioObjectUnknown else { return .driverMissing }
        guard micActive else { return .idle }
        guard let state = phoneState else { return .requested }
        return state.capturing ? .listening : .phoneSays(state.reason ?? MicStateReason.unavailable.rawValue)
    }

    private func publish() {
        let status = PhoneMicStatus(driver: driverStatus, phone: phoneStatus(), isDefaultInput: switchedDefault)
        guard status != published else { return }
        published = status
        if let onStatus { DispatchQueue.main.async { onStatus(status) } }
    }
}
