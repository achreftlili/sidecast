import AppKit
import Combine

/// The menu-bar item and its menu (rebuilt each time it opens).
@MainActor
final class StatusMenuController: NSObject, NSMenuDelegate {
    struct Actions {
        var openSettings: () -> Void
        var fixScreenRecording: () -> Void
        var fixAccessibility: () -> Void
        var installSleepRule: () -> Void
        var removeSleepRule: () -> Void
        var installMicDriver: () -> Void
        var removeMicDriver: () -> Void
        var refresh: () -> Void
    }

    private let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    private let model: AppModel
    private let actions: Actions
    private var observation: AnyCancellable?

    init(model: AppModel, actions: Actions) {
        self.model = model
        self.actions = actions
        super.init()
        let menu = NSMenu()
        menu.delegate = self
        menu.autoenablesItems = false
        statusItem.menu = menu
        updateIcon()
        observation = model.objectWillChange.sink { [weak self] _ in
            DispatchQueue.main.async { self?.updateIcon() }
        }
    }

    private func updateIcon() {
        guard let button = statusItem.button else { return }
        let symbol: String
        switch model.connection {
        case .streaming: symbol = "rectangle.fill.on.rectangle.fill"
        case .listenerFailed: symbol = "exclamationmark.triangle"
        default: symbol = "rectangle.on.rectangle"
        }
        let warn = !model.screenRecordingGranted || !model.accessibilityTrusted
        // Keep a display glyph even when warning, so the icon reads as Sidecast.
        let image = NSImage(systemSymbolName: warn && !model.isStreaming ? "display.trianglebadge.exclamationmark" : symbol,
                            accessibilityDescription: "Sidecast")
        image?.isTemplate = true
        button.image = image
        button.title = image == nil ? "Sidecast" : ""
        button.toolTip = "Sidecast — \(model.statusLine)"
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        actions.refresh()
        menu.removeAllItems()
        menu.addItem(disabled(model.statusLine))
        if model.sleep.sleepDisabled {
            menu.addItem(disabled("Sleep disabled while connected"))
        }
        if model.isStreaming || model.mic.phone != .noPhone {
            menu.addItem(disabled(model.mic.line))
        }
        if !model.screenRecordingGranted || !model.accessibilityTrusted {
            menu.addItem(.separator())
        }
        if !model.screenRecordingGranted {
            menu.addItem(disabled("Screen Recording permission is missing"))
            menu.addItem(item("Grant Screen Recording…", #selector(fixScreenRecording)))
        }
        if !model.accessibilityTrusted {
            menu.addItem(disabled("Accessibility permission is missing (touch & keyboard disabled)"))
            menu.addItem(item("Grant Accessibility…", #selector(fixAccessibility)))
        }
        menu.addItem(.separator())
        if !model.sleep.ruleInstalled {
            menu.addItem(item("Keep Mac awake while connected…", #selector(installSleepRule)))
        }
        switch model.mic.driver {
        case .notInstalled: menu.addItem(item("Install phone microphone…", #selector(installMicDriver)))
        case .updateAvailable: menu.addItem(item("Update phone microphone…", #selector(installMicDriver)))
        case .installed, .notBundled: break
        }
        menu.addItem(item("Settings…", #selector(openSettings), key: ","))
        if model.sleep.ruleInstalled {
            menu.addItem(item("Remove sleep permission…", #selector(removeSleepRule)))
        }
        if model.mic.driverInstalled {
            menu.addItem(item("Remove phone microphone…", #selector(removeMicDriver)))
        }
        menu.addItem(.separator())
        let quit = NSMenuItem(title: "Quit Sidecast", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        menu.addItem(quit)
    }

    private func disabled(_ title: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        return item
    }

    private func item(_ title: String, _ action: Selector, key: String = "") -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.target = self
        return item
    }

    @objc private func openSettings() { actions.openSettings() }
    @objc private func fixScreenRecording() { actions.fixScreenRecording() }
    @objc private func fixAccessibility() { actions.fixAccessibility() }
    @objc private func installSleepRule() { actions.installSleepRule() }
    @objc private func removeSleepRule() { actions.removeSleepRule() }
    @objc private func installMicDriver() { actions.installMicDriver() }
    @objc private func removeMicDriver() { actions.removeMicDriver() }
}
