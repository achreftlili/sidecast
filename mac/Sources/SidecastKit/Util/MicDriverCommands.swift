import Foundation

/// Installing / removing SidecastMic.driver (DECISIONS Q23): the admin shell
/// commands run through one administrator prompt, and the "update available"
/// decision. Pure string logic so it can be unit-tested.
public enum MicDriverCommands {
    public static let bundleName = "SidecastMic.driver"
    public static let pluginDirectory = "/Library/Audio/Plug-Ins/HAL"
    public static let installPath = pluginDirectory + "/" + bundleName
    /// UIDs published by the driver (see SidecastMicDriver.h).
    public static let inputDeviceUID = "SidecastMic_Input"
    public static let feedDeviceUID = "SidecastMic_Feed"

    /// Restarts Core Audio so it loads/unloads the plug-in (audio stops ~2 s).
    public static let restartCoreAudio =
        "/bin/launchctl kickstart -k system/com.apple.audio.coreaudiod || /usr/bin/killall coreaudiod"

    /// Copy the bundled driver into place (root:wheel), then restart Core Audio.
    public static func installCommand(bundledDriver: String) -> String {
        let dst = SudoersRule.shellQuote(installPath)
        return "/bin/mkdir -p \(SudoersRule.shellQuote(pluginDirectory))"
            + " && /bin/rm -rf \(dst)"
            + " && /usr/bin/ditto \(SudoersRule.shellQuote(bundledDriver)) \(dst)"
            + " && /usr/sbin/chown -R root:wheel \(dst)"
            + " && (\(restartCoreAudio))"
    }

    /// Delete the driver, then restart Core Audio.
    public static var removeCommand: String {
        "/bin/rm -rf \(SudoersRule.shellQuote(installPath)) && (\(restartCoreAudio))"
    }

    public enum Status: Equatable, Sendable {
        /// This build has no driver inside (e.g. `swift run`).
        case notBundled
        case notInstalled
        case installed(version: String)
        case updateAvailable(installed: String, bundled: String)
    }

    /// Compares CFBundleVersion strings numerically (1.10 > 1.9).
    public static func status(installedVersion: String?, bundledVersion: String?) -> Status {
        guard let installed = installedVersion else {
            return bundledVersion == nil ? .notBundled : .notInstalled
        }
        guard let bundled = bundledVersion else { return .installed(version: installed) }
        if bundled.compare(installed, options: .numeric) == .orderedDescending {
            return .updateAvailable(installed: installed, bundled: bundled)
        }
        return .installed(version: installed)
    }
}
