import Foundation
import SidecastKit

/// Installs / removes SidecastMic.driver (bundled in Contents/Resources) in
/// /Library/Audio/Plug-Ins/HAL with one administrator prompt, restarting Core
/// Audio so it (un)loads. Status compares the two bundles' CFBundleVersion.
struct MicDriverInstaller {
    var bundledURL: URL? {
        guard let url = Bundle.main.resourceURL?.appendingPathComponent(MicDriverCommands.bundleName),
              FileManager.default.fileExists(atPath: url.path) else { return nil }
        return url
    }

    let installedURL = URL(fileURLWithPath: MicDriverCommands.installPath)

    func status() -> MicDriverCommands.Status {
        MicDriverCommands.status(installedVersion: Self.version(of: installedURL),
                                 bundledVersion: bundledURL.flatMap(Self.version(of:)))
    }

    /// Main thread (admin prompt). Core Audio restarts: all audio stops ~2 s.
    func install() throws {
        guard let bundledURL else { throw AdminPrompt.Failure(description: "this copy of Sidecast has no microphone driver inside") }
        try AdminPrompt.run(MicDriverCommands.installCommand(bundledDriver: bundledURL.path),
                            prompt: "Sidecast wants to install the “Sidecast Microphone” audio driver.")
    }

    /// Main thread (admin prompt). Core Audio restarts: all audio stops ~2 s.
    func remove() throws {
        try AdminPrompt.run(MicDriverCommands.removeCommand,
                            prompt: "Sidecast wants to remove the “Sidecast Microphone” audio driver.")
    }

    /// CFBundleVersion read straight from Info.plist (Bundle caches stale values).
    private static func version(of bundle: URL) -> String? {
        let plist = bundle.appendingPathComponent("Contents/Info.plist")
        guard let info = NSDictionary(contentsOf: plist) else { return nil }
        return info["CFBundleVersion"] as? String
    }
}
