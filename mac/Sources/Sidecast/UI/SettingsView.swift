import AppKit
import SidecastKit
import SwiftUI

struct SettingsActions {
    var setLaunchAtLogin: (Bool) -> Void
    var forgetDevice: (String) -> Void
    var installSleepRule: () -> Void
    var removeSleepRule: () -> Void
    var fixAccessibility: () -> Void
    var fixScreenRecording: () -> Void
    var installMicDriver: () -> Void
    var removeMicDriver: () -> Void
    var openEnergySettings: () -> Void
}

/// The settings window content.
struct SettingsView: View {
    @ObservedObject var settings: AppSettings
    @ObservedObject var model: AppModel
    let actions: SettingsActions
    @State private var portText = ""

    var body: some View {
        Form {
            Section("Status") {
                Label(model.statusLine, systemImage: model.isStreaming ? "checkmark.circle.fill" : "antenna.radiowaves.left.and.right")
                LabeledContent("This Mac") {
                    Text(addressText).monospacedDigit().textSelection(.enabled)
                }
                if !model.screenRecordingGranted {
                    HStack {
                        Label("Needs the Screen Recording permission", systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                        Spacer()
                        Button("Grant…") { actions.fixScreenRecording() }
                    }
                }
                if model.mic.driverInstalled || model.isStreaming {
                    Label(model.mic.line, systemImage: model.mic.phone == .listening ? "mic.fill" : "mic")
                }
                Text("On the phone: open Sidecast and pick this Mac, or tap “Add by IP address” and enter the address above.")
                    .font(.caption).foregroundStyle(.secondary)
            }

            Section("Display") {
                Toggle("HiDPI (sharper, larger text)", isOn: $settings.hiDPI)
                Picker("Resolution", selection: $settings.resolutionScale) {
                    Text("100% of the phone's pixels").tag(1.0)
                    Text("75%").tag(0.75)
                    Text("50%").tag(0.5)
                }
                Picker("Frame rate", selection: $settings.fps) {
                    ForEach(StreamSettings.allowedFps, id: \.self) { Text("\($0) fps").tag($0) }
                }
            }

            Section("Quality") {
                LabeledContent("Maximum bitrate") {
                    HStack {
                        Slider(value: bitrate, in: 10...100, step: 5)
                        Text("\(settings.maxBitrateMbps) Mbps").monospacedDigit().frame(width: 72, alignment: .trailing)
                    }
                }
                Text("Higher keeps text crisp on a fast network; the stream lowers it automatically when Wi-Fi struggles.")
                    .font(.caption).foregroundStyle(.secondary)
            }

            Section("Input") {
                Toggle("Phone controls the Mac (touch & keyboard)", isOn: $settings.touchEnabled)
                Text("Two-finger tap = right click. 3/4-finger swipes: left/right switch Desktops, up = Mission Control, "
                     + "down = App Exposé (uses the Mission Control keyboard shortcuts in System Settings → Keyboard → "
                     + "Keyboard Shortcuts).")
                    .font(.caption).foregroundStyle(.secondary)
                Text("Trackpad mode (switched on the phone) turns the phone into a laptop trackpad: slide to move the "
                     + "cursor, tap to click, double-tap and hold to drag, two fingers to right-click or scroll.")
                    .font(.caption).foregroundStyle(.secondary)
                if settings.touchEnabled && !model.accessibilityTrusted {
                    HStack {
                        Label("Needs the Accessibility permission", systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                        Spacer()
                        Button("Grant…") { actions.fixAccessibility() }
                    }
                }
            }

            Section("Remote access") {
                if tailscaleAddresses.isEmpty {
                    Label("Tailscale not detected — install Tailscale on this Mac and your phone (same account) "
                          + "to use Sidecast from anywhere.", systemImage: "network.slash")
                } else {
                    Label(tailscaleLine, systemImage: "network")
                        .textSelection(.enabled)
                    Text("Paired phones learn this address and use it automatically when they are away from this network.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Text("For remote use keep the Mac awake: System Settings → Battery (or Energy) → Options → "
                     + "Prevent automatic sleeping on power adapter when the display is off.")
                    .font(.caption).foregroundStyle(.secondary)
                HStack {
                    Spacer()
                    Button("Open Battery / Energy Settings…") { actions.openEnergySettings() }
                }
            }

            Section("Phone microphone") {
                Text("Adds a “Sidecast Microphone” input to the Mac. When an app records from it (voice input, "
                     + "calls, dictation…), Sidecast asks the phone for its microphone.")
                    .font(.caption).foregroundStyle(.secondary)
                HStack {
                    Label(model.mic.driverLine, systemImage: model.mic.driverInstalled ? "checkmark.circle" : "mic.slash")
                    Spacer()
                    switch model.mic.driver {
                    case .notBundled:
                        EmptyView()
                    case .notInstalled:
                        Button("Install…") { actions.installMicDriver() }
                    case .installed:
                        Button("Remove…") { actions.removeMicDriver() }
                    case .updateAvailable:
                        Button("Update…") { actions.installMicDriver() }
                        Button("Remove…") { actions.removeMicDriver() }
                    }
                }
                Text("Installing or removing restarts Core Audio: all sound on the Mac stops for about 2 seconds.")
                    .font(.caption).foregroundStyle(.orange)
                Toggle("Use the phone's microphone", isOn: $settings.micEnabled)
                Toggle("Make Sidecast Microphone the default input while a phone is connected",
                       isOn: $settings.micMakeDefaultInput)
                    .disabled(!settings.micEnabled)
            }

            Section("Connection") {
                LabeledContent("Port") {
                    TextField("", text: $portText)
                        .frame(width: 80)
                        .multilineTextAlignment(.trailing)
                        .onSubmit(applyPort)
                }
                Toggle("Launch at login", isOn: Binding(get: { model.launchAtLogin },
                                                        set: { actions.setLaunchAtLogin($0) }))
            }

            Section("Paired phones") {
                if model.pairedDevices.isEmpty {
                    Text("No phone paired yet. Open Sidecast on the phone and pick this Mac.")
                        .foregroundStyle(.secondary)
                }
                ForEach(model.pairedDevices) { device in
                    HStack {
                        VStack(alignment: .leading) {
                            Text(device.deviceName)
                            Text("Paired \(device.pairedAt.formatted(date: .abbreviated, time: .shortened))")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Forget") { actions.forgetDevice(device.deviceId) }
                    }
                }
            }

            Section("Keep Mac awake while connected") {
                Text("Lets Sidecast run “pmset -a disablesleep” (and nothing else) without a password, so the Mac "
                     + "does not sleep — even with the lid closed — while a phone is connected. Sleep is restored "
                     + "30 s after the phone disconnects, and if Sidecast quits or crashes.")
                    .font(.caption).foregroundStyle(.secondary)
                HStack {
                    Label(model.sleep.ruleInstalled ? "Installed" : "Not installed",
                          systemImage: model.sleep.ruleInstalled ? "checkmark.circle" : "moon.zzz")
                    Spacer()
                    if model.sleep.ruleInstalled {
                        Button("Remove…") { actions.removeSleepRule() }
                    } else {
                        Button("Install…") { actions.installSleepRule() }
                    }
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 480, height: 660) // a scrolling Form has no intrinsic height
        .onAppear { portText = String(settings.port) }
    }

    /// LAN then Tailscale addresses, one per line.
    private var addressText: String {
        let lines = model.addresses.compactMap { a -> String? in
            switch a.addressKind {
            case .lan: return "\(a.host)  port \(settings.port)"
            case .tailscale: return "\(a.host)  port \(settings.port)  (Tailscale)"
            case .tailscaleDNS, nil: return nil
            }
        }
        return lines.isEmpty ? "No network" : lines.joined(separator: "\n")
    }

    private var tailscaleAddresses: [ServerAddress] {
        model.addresses.filter { $0.addressKind == .tailscale || $0.addressKind == .tailscaleDNS }
    }

    /// "Tailscale: 100.x.y.z (name.ts.net)"
    private var tailscaleLine: String {
        let ips = tailscaleAddresses.filter { $0.addressKind == .tailscale }.map(\.host)
        let names = tailscaleAddresses.filter { $0.addressKind == .tailscaleDNS }.map(\.host)
        let ipText = ips.isEmpty ? "no address" : ips.joined(separator: ", ")
        return "Tailscale: \(ipText)" + (names.isEmpty ? "" : " (\(names.joined(separator: ", ")))")
    }

    private var bitrate: Binding<Double> {
        Binding(get: { Double(settings.maxBitrateMbps) }, set: { settings.maxBitrateMbps = Int($0) })
    }

    private func applyPort() {
        if let p = Int(portText.trimmingCharacters(in: .whitespaces)), AppSettings.portRange.contains(p) {
            settings.port = p
        } else {
            NSSound.beep()
        }
        portText = String(settings.port)
    }
}

/// Hosts `SettingsView` in a regular window.
@MainActor
final class SettingsWindowController {
    private var window: NSWindow?
    private let makeView: () -> SettingsView

    init(makeView: @escaping () -> SettingsView) {
        self.makeView = makeView
    }

    func show() {
        if window == nil {
            let hosting = NSHostingController(rootView: makeView())
            let w = NSWindow(contentViewController: hosting)
            w.title = "Sidecast"
            w.styleMask = [.titled, .closable, .miniaturizable]
            w.isReleasedWhenClosed = false
            w.center()
            window = w
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}
