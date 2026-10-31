import SidecastKit
import UIKit

/// Computers found on this network plus the paired ones; "Add by IP address";
/// swipe to forget; the settings live in the gear menu.
final class ComputerListViewController: UITableViewController {
    private let pairings = KeychainPairingStore.shared
    private let settings = AppSettings.shared
    private let discovery = ComputerDiscovery()
    private var discovered: [DiscoveredComputer] = []
    private var items: [ComputerListItem] = []

    init() {
        super.init(style: .insetGrouped)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Sidecast"
        navigationController?.navigationBar.prefersLargeTitles = true
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "computer")
        let add = UIBarButtonItem(systemItem: .add, primaryAction: UIAction { [weak self] _ in self?.addByAddress() })
        add.accessibilityLabel = "Add by IP address"
        add.accessibilityIdentifier = "addByIP"
        let gear = UIBarButtonItem(image: UIImage(systemName: "gearshape"), menu: settingsMenu())
        gear.accessibilityLabel = "Settings"
        gear.accessibilityIdentifier = "settings"
        navigationItem.rightBarButtonItems = [add, gear]
        discovery.onChange = { [weak self] in
            self?.discovered = $0
            self?.refresh()
        }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        refresh()   // pairings may have changed on the display screen
        discovery.start()
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        discovery.stop()
    }

    private func refresh() {
        items = ComputerList.merge(discovered: discovered, paired: pairings.all())
        tableView.reloadData()
    }

    // MARK: - Settings

    private func settingsMenu() -> UIMenu {
        UIMenu(children: [
            UIDeferredMenuElement.uncached { [weak self] completion in
                guard let self else { return completion([]) }
                completion([
                    UIAction(title: "Use as Microphone", subtitle: "When an app on the computer records from it",
                             image: UIImage(systemName: "mic"), state: self.settings.micEnabled ? .on : .off) { [weak self] _ in
                        self?.settings.micEnabled.toggle()
                    },
                    UIAction(title: "Data Saver on Cellular", subtitle: "At most 4 Mbps and 30 fps",
                             image: UIImage(systemName: "antenna.radiowaves.left.and.right"),
                             state: self.settings.dataSaver ? .on : .off) { [weak self] _ in
                        self?.settings.dataSaver.toggle()
                    },
                ])
            },
        ])
    }

    // MARK: - Table

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { items.count }

    override func tableView(_ tableView: UITableView, titleForFooterInSection section: Int) -> String? {
        items.isEmpty
            ? "Looking for computers… Open Sidecast on your computer. Both devices must be on the same Wi-Fi network, or the computer on this device’s hotspot. Or tap + to add it by IP address."
            : "Swipe a paired computer to forget it."
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let item = items[indexPath.row]
        let cell = tableView.dequeueReusableCell(withIdentifier: "computer", for: indexPath)
        var content = UIListContentConfiguration.subtitleCell()
        content.text = item.name
        content.secondaryText = status(of: item)
        content.secondaryTextProperties.color = .secondaryLabel
        content.image = UIImage(systemName: Self.symbol(for: item.platform))
        content.imageProperties.tintColor = item.online ? .systemGreen : .secondaryLabel
        cell.contentConfiguration = content
        cell.accessoryType = .disclosureIndicator
        cell.accessibilityIdentifier = "computer.\(item.name)"
        let usable = item.online || item.host != nil
        cell.contentView.alpha = usable ? 1 : 0.6
        return cell
    }

    /// "Windows · Online · Paired", "macOS · Offline · last seen at 192.168.1.20"…
    private func status(of item: ComputerListItem) -> String {
        var parts: [String] = []
        // An OS this version does not know shows no label, like a generic computer.
        if let platform = item.platform { parts.append(Self.name(of: platform)) }
        if item.online, !item.compatible {
            parts.append("Online · different Sidecast version (v\(item.protocolVersion ?? "?"))")
        } else if item.online {
            parts.append("Online")
        } else if item.remote {
            parts.append("Remote via Tailscale")
        } else if let host = item.host {
            parts.append("Offline · last seen at \(host)")
        } else {
            parts.append("Offline")
        }
        if item.paired { parts.append("Paired") }
        return parts.joined(separator: " · ")
    }

    static func name(of platform: HostPlatform) -> String {
        switch platform {
        case .macos: return "macOS"
        case .windows: return "Windows"
        case .linux: return "Linux"
        }
    }

    static func symbol(for platform: HostPlatform?) -> String {
        switch platform {
        case .macos: return "laptopcomputer"
        case .windows: return "pc"
        case .linux, nil: return "desktopcomputer"
        }
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        let item = items[indexPath.row]
        guard let host = item.host ?? item.serviceName else { return }
        let platform = item.serverId.flatMap(pairings.pairing(serverId:))?.platform
        open(SessionTarget(host: host, port: item.port, serverId: item.serverId, name: item.name,
                           bonjourService: item.online ? item.serviceName : nil,
                           platform: platform ?? item.platform?.rawValue))
    }

    override func tableView(_ tableView: UITableView,
                            trailingSwipeActionsConfigurationForRowAt indexPath: IndexPath) -> UISwipeActionsConfiguration? {
        let item = items[indexPath.row]
        guard item.paired, let serverId = item.serverId else { return nil }
        let forget = UIContextualAction(style: .destructive, title: "Forget") { [weak self] _, _, done in
            self?.confirmForget(name: item.name, serverId: serverId, done: done)
        }
        return UISwipeActionsConfiguration(actions: [forget])
    }

    private func confirmForget(name: String, serverId: String, done: @escaping (Bool) -> Void) {
        let alert = UIAlertController(title: "Forget “\(name)”?",
                                      message: "You will need to pair again with a code shown on the computer.",
                                      preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel) { _ in done(false) })
        alert.addAction(UIAlertAction(title: "Forget", style: .destructive) { [weak self] _ in
            self?.pairings.remove(serverId: serverId)
            self?.refresh()
            done(true)
        })
        present(alert, animated: true)
    }

    // MARK: - Opening a computer

    private func addByAddress(problem: String? = nil, host: String = "", port: String = "") {
        let alert = UIAlertController(title: "Add by IP Address",
                                      message: problem ?? "The computer’s address is in its Wi-Fi settings. Port \(SidecastProtocol.defaultPort) unless you changed it.",
                                      preferredStyle: .alert)
        alert.addTextField { field in
            field.placeholder = "IP address or host name"
            field.text = host
            field.keyboardType = .URL
            field.autocapitalizationType = .none
            field.autocorrectionType = .no
            field.accessibilityIdentifier = "hostField"
        }
        alert.addTextField { field in
            field.placeholder = "Port (\(SidecastProtocol.defaultPort))"
            field.text = port
            field.keyboardType = .numberPad
            field.accessibilityIdentifier = "portField"
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Connect", style: .default) { [weak self, weak alert] _ in
            let hostText = alert?.textFields?[0].text ?? "", portText = alert?.textFields?[1].text ?? ""
            guard let parsed = HostInput.parse(host: hostText, port: portText) else {
                let why = hostText.trimmingCharacters(in: .whitespaces).isEmpty
                    ? "Enter the computer’s IP address." : "Check the address; the port must be between 1 and 65535."
                self?.addByAddress(problem: why, host: hostText, port: portText)
                return
            }
            self?.open(SessionTarget(host: parsed.host, port: parsed.port, serverId: nil, name: parsed.host))
        })
        present(alert, animated: true)
    }

    private func open(_ target: SessionTarget) {
        guard let screen = view.window?.windowScene?.screen else { return }
        let display = DisplayViewController(target: target, screen: .landscape(of: screen))
        (UIApplication.shared.delegate as? AppDelegate)?.orientationLock = .landscape
        present(display, animated: true) {
            display.setNeedsUpdateOfSupportedInterfaceOrientations()
            display.view.window?.windowScene?.requestGeometryUpdate(.iOS(interfaceOrientations: .landscape))
        }
    }
}
