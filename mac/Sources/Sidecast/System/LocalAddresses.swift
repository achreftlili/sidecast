import Darwin
import Foundation
import SidecastKit

/// This Mac's reachable IPv4 addresses: Wi-Fi / Ethernet / hotspot (`lan`) and
/// Tailscale (`tailscale`, 100.64.0.0/10 on a tunnel interface). Shown in
/// Settings and advertised in SERVER_HELLO (PROTOCOL §1.1).
enum LocalAddresses {
    /// Interface addresses, LAN first then Tailscale (no MagicDNS name).
    static func current() -> [ServerAddress] {
        var head: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&head) == 0, let first = head else { return [] }
        defer { freeifaddrs(head) }
        var result: [ServerAddress] = []
        for ptr in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let ifa = ptr.pointee
            let flags = Int32(ifa.ifa_flags)
            guard flags & IFF_UP != 0, let addr = ifa.ifa_addr, addr.pointee.sa_family == UInt8(AF_INET) else { continue }
            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            guard getnameinfo(addr, socklen_t(addr.pointee.sa_len), &host, socklen_t(host.count),
                              nil, 0, NI_NUMERICHOST) == 0 else { continue }
            let ip = String(cString: host)
            guard let kind = NetworkAddresses.classify(interface: String(cString: ifa.ifa_name), ipv4: ip) else { continue }
            result.append(ServerAddress(host: ip, kind: kind))
        }
        return NetworkAddresses.ordered(result)
    }

    /// LAN IPv4s only (for "Add by IP address").
    static func ipv4() -> [String] {
        current().filter { $0.addressKind == .lan }.map(\.host)
    }
}

/// The Mac's Tailscale MagicDNS name (`Self.DNSName` of `tailscale status --json`),
/// looked up off the main thread with a 2 s timeout and refreshed every 60 s.
/// Absent Tailscale (or CLI) simply means no name.
final class TailscaleNameLookup: @unchecked Sendable {
    static let cliCandidates = [
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        "/opt/homebrew/bin/tailscale",
        "/usr/local/bin/tailscale",
    ]

    private let queue = DispatchQueue(label: "com.achreftlili.sidecast.tailscale", qos: .utility)
    private let cached = Locked<String?>(nil)
    private var timer: DispatchSourceTimer?
    /// Main queue, when the name changes.
    var onChange: ((String?) -> Void)?

    var name: String? { cached.withLock { $0 } }

    func start() {
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now(), repeating: 60, leeway: .seconds(5))
        t.setEventHandler { [weak self] in self?.refresh() }
        t.resume()
        timer = t
    }

    /// Re-reads now (e.g. when the Settings window opens).
    func refreshSoon() {
        queue.async { self.refresh() }
    }

    private func refresh() {
        let fm = FileManager.default
        var name: String?
        if let cli = Self.cliCandidates.first(where: { fm.isExecutableFile(atPath: $0) }) {
            let r = ProcessRunner.run(cli, ["status", "--json"], timeout: 2)
            if r.succeeded { name = NetworkAddresses.magicDNSName(fromStatusJSON: Data(r.stdout.utf8)) }
        }
        let changed = cached.withLock { old -> Bool in
            defer { old = name }
            return old != name
        }
        if changed, let onChange {
            Log.info("tailscale: MagicDNS name \(name ?? "none")")
            DispatchQueue.main.async { onChange(name) }
        }
    }
}
