import AppKit
import SidecastKit

/// Normal (menu-bar) mode: wires the server, the real display backend, the
/// sleep guard, permissions and the UI together.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let options: LaunchOptions
    private let model = AppModel()
    private let settings = AppSettings()
    private var server: SidecastServer?
    private var sleepGuard: SleepGuard?
    private var micRouter: MicRouter?
    private let tailscaleName = TailscaleNameLookup()
    private let micInstaller = MicDriverInstaller()
    private var store: PairedDeviceStore?
    private var menu: StatusMenuController?
    private var settingsWindow: SettingsWindowController?
    private let pairingPanel = PairingPanelController()
    private var permissionTimer: Timer?
    private var appliedPort: Int

    init(options: LaunchOptions) {
        self.options = options
        appliedPort = 0
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        Log.verbose = options.verbose
        // One copy at a time: e.g. one opened from the DMG and one from Applications.
        // The second could not bind the port anyway.
        if let other = NSRunningApplication.runningApplications(withBundleIdentifier: Bundle.main.bundleIdentifier ?? "")
            .first(where: { $0 != NSRunningApplication.current }) {
            Log.info("another Sidecast is already running (pid \(other.processIdentifier)); quitting")
            alert("Sidecast is already running", "Look for its icon in the menu bar.")
            NSApp.terminate(nil)
            return
        }
        let paths = AppPaths(dataDirectory: options.dataDirectory ?? AppPaths.defaultDataDirectory)
        do {
            try paths.ensureDirectory()
        } catch {
            return fatal("Cannot create \(paths.dataDirectory.path): \(error)")
        }

        let sleepGuard = SleepGuard(markerURL: paths.sleepMarker)
        sleepGuard.onChange = { [weak self] status in self?.model.sleep = status }
        sleepGuard.startUp()
        self.sleepGuard = sleepGuard

        let micRouter = MicRouter(markerURL: paths.defaultInputMarker, installer: micInstaller)
        micRouter.onStatus = { [weak self] status in MainActor.assumeIsolated { self?.model.mic = status } }
        micRouter.update(settings: settings.micSettings)
        micRouter.startUp()
        self.micRouter = micRouter

        tailscaleName.onChange = { [weak self] _ in MainActor.assumeIsolated { self?.refreshAddresses() } }
        tailscaleName.start()

        setUpUI()
        refreshPermissions()
        if !model.screenRecordingGranted { Permissions.requestScreenRecording() }

        do {
            try startServer(paths: paths)
        } catch {
            return fatal("Sidecast could not start: \(error)")
        }
        permissionTimer = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refreshPermissions() }
        }
        // A menu-bar icon is easy to miss (or hidden behind the notch): until setup is
        // done, open the window on launch so the user sees Sidecast is running.
        if !model.screenRecordingGranted || model.pairedDevices.isEmpty {
            settingsWindow?.show()
        }
    }

    /// Opening the app again (Finder, Spotlight, Launchpad) while it runs shows its window.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        settingsWindow?.show()
        return false
    }

    func applicationWillTerminate(_ notification: Notification) {
        let done = DispatchSemaphore(value: 0)
        server?.shutdown { done.signal() }
        _ = done.wait(timeout: .now() + 1)
        micRouter?.shutDown()
        sleepGuard?.shutDown()
    }

    // MARK: - Setup

    private func startServer(paths: AppPaths) throws {
        let serverId = try paths.loadOrCreateServerId()
        let identity = try TLSIdentity.loadOrCreate(paths: paths)
        let store = PairedDeviceStore(fileURL: paths.pairedDevices)
        self.store = store
        model.pairedDevices = store.all
        model.launchAtLogin = LoginItem.isEnabled

        let config = SidecastServer.Configuration(
            port: UInt16(settings.port), advertiseBonjour: options.bonjour, serverId: serverId,
            serverName: Host.current().localizedName ?? "Mac", fixedPairingCode: nil,
            addresses: { [tailscaleName] in
                LocalAddresses.current() + (tailscaleName.name.map { [ServerAddress(host: $0, kind: .tailscaleDNS)] } ?? [])
            })
        appliedPort = settings.port
        let server = SidecastServer(
            configuration: config, identity: identity, pairedDevices: store, settings: settings.streamSettings,
            makeBackend: { [weak self] in
                let backend = DisplayCaptureBackend()
                backend.onAccessibilityMissing = { MainActor.assumeIsolated { self?.refreshPermissions() } }
                return backend
            },
            micBridge: micRouter,
            streamingBlocker: { Permissions.screenRecordingGranted ? nil : .permissionMissing })
        server.onEvent = { [weak self] event in MainActor.assumeIsolated { self?.handle(event) } }
        settings.onChange = { [weak self] in self?.settingsChanged() }
        self.server = server
        server.start()
        Log.info("Sidecast \(Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev") started; "
                 + "certificate fingerprint \(identity.fingerprint.hexString)")
    }

    private func setUpUI() {
        menu = StatusMenuController(model: model, actions: .init(
            openSettings: { [weak self] in
                self?.tailscaleName.refreshSoon()
                self?.settingsWindow?.show()
            },
            fixScreenRecording: { [weak self] in self?.fixScreenRecording() },
            fixAccessibility: { [weak self] in self?.fixAccessibility() },
            installSleepRule: { [weak self] in self?.installSleepRule() },
            removeSleepRule: { [weak self] in self?.removeSleepRule() },
            installMicDriver: { [weak self] in self?.installMicDriver() },
            removeMicDriver: { [weak self] in self?.removeMicDriver() },
            refresh: { [weak self] in self?.refreshPermissions() }))
        let actions = SettingsActions(
            setLaunchAtLogin: { [weak self] enabled in
                LoginItem.setEnabled(enabled)
                self?.model.launchAtLogin = LoginItem.isEnabled
            },
            forgetDevice: { [weak self] id in self?.server?.forgetDevice(id: id) },
            installSleepRule: { [weak self] in self?.installSleepRule() },
            removeSleepRule: { [weak self] in self?.removeSleepRule() },
            fixAccessibility: { [weak self] in self?.fixAccessibility() },
            fixScreenRecording: { [weak self] in self?.fixScreenRecording() },
            installMicDriver: { [weak self] in self?.installMicDriver() },
            removeMicDriver: { [weak self] in self?.removeMicDriver() },
            openEnergySettings: { Permissions.openEnergySettings() })
        settingsWindow = SettingsWindowController { [unowned self] in
            SettingsView(settings: self.settings, model: self.model, actions: actions)
        }
    }

    // MARK: - Events

    private func handle(_ event: ServerEvent) {
        switch event {
        case .listening(let port):
            if !model.isStreaming { model.connection = .waiting(port: port) }
        case .listenerFailed(let why):
            model.connection = .listenerFailed(why)
        case .pairingStarted(let session, let name, let code, let expiresAt):
            if !model.isStreaming { model.connection = .pairing(deviceName: name) }
            pairingPanel.show(session: session, deviceName: name, code: code, expiresAt: expiresAt)
        case .pairingEnded(let session, _):
            pairingPanel.close(session: session)
            if case .pairing = model.connection { model.connection = .waiting(port: UInt16(appliedPort)) }
        case .streamingStarted(let name, _, let dataSaver):
            if !model.isStreaming { model.trackpadMode = false }   // a new session starts in touch mode
            model.connection = .streaming(deviceName: name, fps: 0, mbps: 0, dataSaver: dataSaver)
            sleepGuard?.setStreaming(true)
        case .inputModeChanged(let mode):
            model.trackpadMode = mode == .trackpad
        case .streamingStopped:
            model.trackpadMode = false
            model.connection = .waiting(port: UInt16(appliedPort))
            sleepGuard?.setStreaming(false)
        case .stats(let stats):
            if case .streaming(let name, _, _, let dataSaver) = model.connection {
                model.connection = .streaming(deviceName: name, fps: stats.fps, mbps: stats.mbps, dataSaver: dataSaver)
            }
        case .pairedDevicesChanged:
            model.pairedDevices = store?.all ?? []
        }
    }

    private func settingsChanged() {
        server?.update(settings: settings.streamSettings)
        micRouter?.update(settings: settings.micSettings)
        if settings.port != appliedPort, let port = UInt16(exactly: settings.port) {
            appliedPort = settings.port
            server?.setPort(port)
        }
    }

    // MARK: - Permissions and sleep rule

    /// Interface addresses + cached MagicDNS name, for the Status and Remote access sections.
    private func refreshAddresses() {
        let addresses = NetworkAddresses.ordered(
            LocalAddresses.current() + (tailscaleName.name.map { [ServerAddress(host: $0, kind: .tailscaleDNS)] } ?? []))
        if model.addresses != addresses { model.addresses = addresses }
    }

    private func refreshPermissions() {
        refreshAddresses()
        let screen = Permissions.screenRecordingGranted
        let ax = Permissions.accessibilityTrusted
        if model.screenRecordingGranted != screen { model.screenRecordingGranted = screen }
        if model.accessibilityTrusted != ax { model.accessibilityTrusted = ax }
    }

    private func fixScreenRecording() {
        Permissions.requestScreenRecording()
        Permissions.openScreenRecordingSettings()
    }

    private func fixAccessibility() {
        if !Permissions.promptAccessibility() { Permissions.openAccessibilitySettings() }
    }

    private func installSleepRule() {
        guard let sleepGuard else { return }
        NSApp.activate(ignoringOtherApps: true)
        do {
            try sleepGuard.installRule()
        } catch {
            alert("Could not install the sleep permission", "\(error)")
        }
    }

    private func removeSleepRule() {
        guard let sleepGuard else { return }
        NSApp.activate(ignoringOtherApps: true)
        do {
            try sleepGuard.removeRule()
        } catch {
            alert("Could not remove the sleep permission", "\(error)")
        }
    }

    // MARK: - Phone microphone driver

    private func installMicDriver() {
        NSApp.activate(ignoringOtherApps: true)
        do {
            try micInstaller.install()
            Log.info("mic: driver installed; Core Audio restarted")
        } catch {
            alert("Could not install Sidecast Microphone", "\(error)")
        }
        micRouter?.driverChanged()
    }

    private func removeMicDriver() {
        NSApp.activate(ignoringOtherApps: true)
        micRouter?.prepareForDriverRemoval()
        do {
            try micInstaller.remove()
            Log.info("mic: driver removed; Core Audio restarted")
        } catch {
            alert("Could not remove Sidecast Microphone", "\(error)")
        }
        micRouter?.driverChanged()
    }

    private func alert(_ title: String, _ text: String) {
        let a = NSAlert()
        a.messageText = title
        a.informativeText = text
        a.runModal()
    }

    private func fatal(_ message: String) {
        Log.error(message)
        alert("Sidecast", message)
        NSApp.terminate(nil)
    }
}
