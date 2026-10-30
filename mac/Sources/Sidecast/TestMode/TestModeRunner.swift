import Foundation
import SidecastKit

/// Headless test mode: real TLS server, pairing, flow control and encoder,
/// with frames from `TestPatternBackend`. Logs to stdout. Never touches the
/// virtual display, ScreenCaptureKit, input injection (touch or keyboard), Core Audio, sleep settings or
/// permission prompts; all state lives in `--data-dir`.
final class TestModeRunner {
    private let options: LaunchOptions
    private var server: SidecastServer?
    private var signalSources: [DispatchSourceSignal] = []

    init(options: LaunchOptions) {
        self.options = options
    }

    func run() -> Never {
        Log.echoToStdout = true
        Log.verbose = options.verbose
        do {
            try startServer()
        } catch {
            Log.error("test mode: \(error)")
            exit(1)
        }
        withExtendedLifetime(self) { dispatchMain() }
    }

    private func startServer() throws {
        guard let dataDirectory = options.dataDirectory else { throw LaunchOptions.OptionError("--data-dir missing") }
        let paths = AppPaths(dataDirectory: dataDirectory)
        try paths.ensureDirectory()
        let serverId = try paths.loadOrCreateServerId()
        let identity = try TLSIdentity.loadOrCreate(paths: paths)
        let store = PairedDeviceStore(fileURL: paths.pairedDevices)

        var settings = StreamSettings()
        if let fps = options.fps { settings.fps = fps }
        if let mbps = options.maxBitrateMbps { settings.maxBitrateMbps = mbps }

        let name = (Host.current().localizedName ?? "Mac") + " (test)"
        let config = SidecastServer.Configuration(
            port: options.port ?? SidecastProtocol.defaultPort,
            advertiseBonjour: options.bonjour,
            serverId: serverId,
            serverName: name,
            fixedPairingCode: options.testPairingCode,
            addresses: { [fake = options.testFakeTailscale] in
                // Real interface addresses; no Tailscale CLI lookup in test mode.
                LocalAddresses.current() + (fake.map { [ServerAddress(host: $0, kind: .tailscale)] } ?? [])
            })
        let server = SidecastServer(configuration: config, identity: identity, pairedDevices: store,
                                    settings: settings, makeBackend: { TestPatternBackend() },
                                    micBridge: TestMicBridge(activateOnStart: options.testMicActive))
        server.onEvent = { event in
            switch event {
            case .pairingStarted(_, let device, let code, _):
                Log.info("event: pairing started with \(device) — code \(code.displayString)")
            case .pairingEnded(_, let paired):
                Log.info("event: pairing ended (paired: \(paired))")
            case .streamingStarted(let device, let plan, let dataSaver):
                Log.info("event: streaming to \(device) (\(plan.codec.rawValue) \(plan.encodeWidth)x\(plan.encodeHeight)@\(plan.fps)"
                         + "\(dataSaver ? ", data saver" : ""))")
            case .streamingStopped:
                Log.info("event: streaming stopped")
            case .inputModeChanged(let mode):
                Log.info("event: input mode \(mode.rawValue)")
            case .listenerFailed(let why):
                Log.error("event: listener failed: \(why)")
                exit(1)
            case .listening, .stats, .pairedDevicesChanged:
                break
            }
        }
        self.server = server

        Log.info("test mode: data dir \(dataDirectory.path)")
        Log.info("test mode: serverId \(serverId)")
        Log.info("test mode: certificate fingerprint \(identity.fingerprint.hexString)")
        if options.testPairingCode != nil { Log.info("test mode: fixed pairing code in use") }
        if let fake = options.testFakeTailscale { Log.info("test mode: advertising fake Tailscale address \(fake)") }
        let advertised = NetworkAddresses.ordered(config.addresses())
        Log.info("test mode: SERVER_HELLO addresses \(advertised.map { "\($0.host) (\($0.kind))" }.joined(separator: ", "))")
        installSignalHandlers()
        server.start()
    }

    private func installSignalHandlers() {
        for sig in [SIGINT, SIGTERM] {
            signal(sig, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
            source.setEventHandler { [weak self] in
                Log.info("test mode: shutting down")
                self?.server?.shutdown { exit(0) }
                DispatchQueue.main.asyncAfter(deadline: .now() + 2) { exit(0) }
            }
            source.resume()
            signalSources.append(source)
        }
    }
}
