import Foundation
import Network
import SidecastKit

/// Browses `_sidecast._tcp` (PROTOCOL §1). Results arrive on the main queue.
/// The first browse shows iOS's Local Network permission prompt.
final class ComputerDiscovery {
    var onChange: (([DiscoveredComputer]) -> Void)?

    private var browser: NWBrowser?

    func start() {
        guard browser == nil else { return }
        let parameters = NWParameters()
        parameters.includePeerToPeer = false
        let browser = NWBrowser(for: .bonjourWithTXTRecord(type: SidecastProtocol.bonjourServiceType, domain: "local."),
                                using: parameters)
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            self?.publish(results)
        }
        browser.stateUpdateHandler = { state in
            switch state {
            case .failed(let error):
                Log.discovery.error("browse failed: \(String(describing: error), privacy: .public)")
            case .waiting(let error):
                // Typically the Local Network permission was refused.
                Log.discovery.info("browse waiting: \(String(describing: error), privacy: .public)")
            default:
                break
            }
        }
        browser.start(queue: .main)
        self.browser = browser
    }

    func stop() {
        browser?.cancel()
        browser = nil
        onChange?([])
    }

    private func publish(_ results: Set<NWBrowser.Result>) {
        var seen = Set<String>()
        var computers: [DiscoveredComputer] = []
        // One result per interface: keep each service name once.
        for result in results.sorted(by: { "\($0.endpoint)" < "\($1.endpoint)" }) {
            guard case .service(let name, _, _, _) = result.endpoint, seen.insert(name).inserted else { continue }
            var txt: [String: String] = [:]
            if case .bonjour(let record) = result.metadata { txt = record.dictionary }
            computers.append(DiscoveredComputer(serviceName: name, serverId: txt[BonjourTXT.serverIdKey],
                                                protocolVersion: txt[BonjourTXT.versionKey],
                                                platform: txt[BonjourTXT.osKey]))
        }
        onChange?(computers)
    }
}

/// Whether the active network costs money or is in Low Data Mode (HELLO
/// `network`, data saver §5). Thread-safe.
final class NetworkMonitor: @unchecked Sendable {
    static let shared = NetworkMonitor()

    private let monitor = NWPathMonitor()
    private let lock = NSLock()
    private var metered = false

    private init() {
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            // Cellular (also through a VPN on top of it) is expensive; Low Data Mode is constrained.
            let value = path.isExpensive || path.isConstrained
            self.lock.withLock { self.metered = value }
        }
        monitor.start(queue: DispatchQueue(label: "sidecast.path-monitor"))
    }

    var isMetered: Bool { lock.withLock { metered } }
}
