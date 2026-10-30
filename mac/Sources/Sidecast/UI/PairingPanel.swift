import AppKit
import SidecastKit
import SwiftUI

/// The floating "Enter this code on <phone>" panel shown during pairing.
@MainActor
final class PairingPanelController {
    private var panel: NSPanel?
    private var session: UUID?

    func show(session: UUID, deviceName: String, code: PairingCode, expiresAt: Date) {
        close()
        let view = PairingView(deviceName: deviceName, code: code.displayString, expiresAt: expiresAt)
        let hosting = NSHostingController(rootView: view)
        let panel = NSPanel(contentViewController: hosting)
        panel.title = "Pair a phone"
        panel.styleMask = [.titled, .closable, .utilityWindow]
        panel.level = .floating
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.center()
        panel.orderFrontRegardless()
        self.panel = panel
        self.session = session
    }

    /// Closes the panel if it belongs to `session` (or unconditionally when nil).
    func close(session: UUID? = nil) {
        if let session, session != self.session { return }
        panel?.close()
        panel = nil
        self.session = nil
    }
}

private struct PairingView: View {
    let deviceName: String
    let code: String
    let expiresAt: Date

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "iphone.and.arrow.forward")
                .font(.system(size: 30))
                .foregroundStyle(.secondary)
            Text("Enter this code on \(deviceName)")
                .font(.headline)
            Text(code)
                .font(.system(size: 44, weight: .semibold, design: .monospaced))
                .textSelection(.enabled)
            TimelineView(.periodic(from: .now, by: 1)) { context in
                let left = max(0, Int(expiresAt.timeIntervalSince(context.date).rounded(.up)))
                Text(left > 0 ? "Expires in \(left / 60):\(String(format: "%02d", left % 60))" : "Code expired")
                    .font(.callout)
                    .monospacedDigit()
                    .foregroundStyle(left > 15 ? Color.secondary : Color.orange)
            }
        }
        .padding(28)
        .frame(minWidth: 320)
    }
}
