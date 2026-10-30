import AppKit
import Foundation
import IOKit.pwr_mgt
import SidecastKit

/// Keeps the Mac awake while a phone streams (DECISIONS Q19–Q21):
/// - IOPM assertions (no root, released automatically if we die);
/// - `pmset -a disablesleep 1` through the sudoers rule, so a closed lid does
///   not sleep either; restored 30 s after the last stream ends, on launch if
///   a marker says we left it disabled, on quit, and by a detached watcher
///   process if the app dies.
/// sudo is only ever run with `-n` (never prompts), off the main thread.
final class SleepGuard {
    struct Status: Equatable {
        var ruleInstalled = false
        var sleepDisabled = false
    }

    static let gracePeriod: TimeInterval = 30

    /// Main queue.
    var onChange: ((Status) -> Void)?
    /// Main-thread copy of the state.
    private(set) var status = Status()
    /// Authoritative state, owned by `queue`.
    private var current = Status()

    private let markerURL: URL
    private let queue = DispatchQueue(label: "com.achreftlili.sidecast.sleepguard")
    private var assertions: [IOPMAssertionID] = []
    private var graceTimer: DispatchWorkItem?

    init(markerURL: URL) {
        self.markerURL = markerURL
    }

    // MARK: - Lifecycle

    /// Restores sleep left disabled by a previous run, spawns the crash watcher.
    func startUp() {
        spawnWatcher()
        queue.async {
            if FileManager.default.fileExists(atPath: self.markerURL.path) {
                Log.info("sleep: previous run left sleep disabled — restoring")
                self.restoreSleep()
            }
            self.refreshRuleStatus()
        }
    }

    /// `applicationWillTerminate`: restore synchronously (bounded by the sudo timeout).
    func shutDown() {
        graceTimer?.cancel()
        releaseAssertions()
        queue.sync { self.restoreSleep() }
    }

    /// Call on the main thread whenever streaming starts or stops.
    func setStreaming(_ streaming: Bool) {
        dispatchPrecondition(condition: .onQueue(.main))
        if streaming {
            graceTimer?.cancel()
            graceTimer = nil
            holdAssertions()
            queue.async { self.disableSleep() }
        } else if graceTimer == nil, !assertions.isEmpty || status.sleepDisabled {
            // A Wi-Fi blip with the lid closed must be able to reconnect.
            let work = DispatchWorkItem { [weak self] in
                guard let self else { return }
                self.graceTimer = nil
                self.releaseAssertions()
                self.queue.async { self.restoreSleep() }
            }
            graceTimer = work
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.gracePeriod, execute: work)
        }
    }

    // MARK: - Sudoers rule (admin prompt)

    /// Installs `/etc/sudoers.d/sidecast` after `visudo -c` validation. Main thread (shows a prompt).
    func installRule() throws {
        dispatchPrecondition(condition: .onQueue(.main))
        let content = try SudoersRule.content(forUser: NSUserName())
        let staged = FileManager.default.temporaryDirectory
            .appendingPathComponent("sidecast-sudoers-\(UUID().uuidString)")
        try Data(content.utf8).write(to: staged, options: .atomic)
        defer { try? FileManager.default.removeItem(at: staged) }
        try runAdmin(SudoersRule.installCommand(stagedFile: staged.path))
        queue.sync { self.refreshRuleStatus() }
        // Already streaming? Disable sleep now that we can.
        if !assertions.isEmpty { queue.async { self.disableSleep() } }
    }

    /// Restores sleep first (while the rule still works), then removes the rule.
    func removeRule() throws {
        dispatchPrecondition(condition: .onQueue(.main))
        queue.sync { self.restoreSleep() }
        try runAdmin(SudoersRule.removeCommand)
        queue.sync { self.refreshRuleStatus() }
    }

    private func runAdmin(_ command: String) throws {
        do {
            try AdminPrompt.run(command)
        } catch {
            throw GuardError("\(error)")
        }
    }

    struct GuardError: Error, CustomStringConvertible {
        let description: String
        init(_ d: String) { description = d }
    }

    // MARK: - pmset (on `queue`)

    private func refreshRuleStatus() {
        let fileExists = FileManager.default.fileExists(atPath: SudoersRule.installPath)
        let allowed = fileExists && sudo(["-l", SudoersRule.pmsetPath] + SudoersRule.disableSleepArgs).succeeded
        update { $0.ruleInstalled = allowed }
    }

    private func disableSleep() {
        guard !current.sleepDisabled else { return }
        refreshRuleStatus()
        guard current.ruleInstalled else { return }
        if Self.sleepAlreadyDisabledByUser() {
            Log.info("sleep: already disabled outside Sidecast — leaving it alone")
            return
        }
        // Marker first: if we die right after pmset, the watcher/next launch still restores.
        do {
            try SecureFile.write(Data("1\n".utf8), to: markerURL)
        } catch {
            Log.error("sleep: cannot write marker: \(error)")
            return
        }
        if sudo([SudoersRule.pmsetPath] + SudoersRule.disableSleepArgs).succeeded {
            Log.info("sleep: system sleep disabled while connected")
            update { $0.sleepDisabled = true }
        } else {
            try? FileManager.default.removeItem(at: markerURL)
            Log.error("sleep: pmset disablesleep 1 failed")
        }
    }

    private func restoreSleep() {
        guard FileManager.default.fileExists(atPath: markerURL.path) else {
            update { $0.sleepDisabled = false }
            return
        }
        if sudo([SudoersRule.pmsetPath] + SudoersRule.enableSleepArgs).succeeded {
            try? FileManager.default.removeItem(at: markerURL)
            Log.info("sleep: system sleep restored")
            update { $0.sleepDisabled = false }
        } else {
            Log.error("sleep: could not restore sleep (marker kept; will retry at next launch)")
        }
    }

    /// `pmset -g` reports "SleepDisabled 1" when sleep is disabled system-wide.
    private static func sleepAlreadyDisabledByUser() -> Bool {
        let r = ProcessRunner.run(SudoersRule.pmsetPath, ["-g"], timeout: 5)
        return r.stdout.split(separator: "\n").contains { line in
            let parts = line.split(whereSeparator: { $0 == " " || $0 == "\t" })
            return parts.count == 2 && parts[0] == "SleepDisabled" && parts[1] == "1"
        }
    }

    private func sudo(_ args: [String]) -> ProcessRunner.Result {
        ProcessRunner.run(SudoersRule.sudoPath, ["-n"] + args, timeout: 10)
    }

    /// Mutates the queue-owned state and publishes it to the main thread.
    private func update(_ change: (inout Status) -> Void) {
        change(&current)
        let snapshot = current
        DispatchQueue.main.async {
            guard snapshot != self.status else { return }
            self.status = snapshot
            self.onChange?(snapshot)
        }
    }

    // MARK: - IOPM assertions (main thread)

    private func holdAssertions() {
        guard assertions.isEmpty else { return }
        let reason = "Sidecast is streaming to a phone" as CFString
        for type in [kIOPMAssertionTypePreventUserIdleDisplaySleep, kIOPMAssertionTypePreventUserIdleSystemSleep] {
            var id = IOPMAssertionID(0)
            if IOPMAssertionCreateWithName(type as CFString, IOPMAssertionLevel(kIOPMAssertionLevelOn),
                                           reason, &id) == kIOReturnSuccess {
                assertions.append(id)
            }
        }
    }

    private func releaseAssertions() {
        assertions.forEach { IOPMAssertionRelease($0) }
        assertions.removeAll()
    }

    // MARK: - Crash watcher

    /// Detached `/bin/sh` that restores sleep once our pid is gone (if the marker exists).
    private func spawnWatcher() {
        var attr: posix_spawnattr_t?
        var actions: posix_spawn_file_actions_t?
        posix_spawnattr_init(&attr)
        posix_spawn_file_actions_init(&actions)
        defer {
            posix_spawnattr_destroy(&attr)
            posix_spawn_file_actions_destroy(&actions)
        }
        // New session (survives us), and inherit no file descriptors but /dev/null.
        posix_spawnattr_setflags(&attr, Int16(POSIX_SPAWN_SETSID | POSIX_SPAWN_CLOEXEC_DEFAULT))
        posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_addopen(&actions, 1, "/dev/null", O_WRONLY, 0)
        posix_spawn_file_actions_addopen(&actions, 2, "/dev/null", O_WRONLY, 0)

        let args = ["/bin/sh", "-c", SudoersRule.watcherScript, "sidecast-watcher", String(getpid()), markerURL.path]
        let env = ["PATH=/usr/bin:/bin:/usr/sbin:/sbin"]
        var cArgs = args.map { strdup($0) } + [nil]
        var cEnv = env.map { strdup($0) } + [nil]
        defer {
            cArgs.forEach { free($0) }
            cEnv.forEach { free($0) }
        }
        var pid: pid_t = 0
        let rc = posix_spawn(&pid, "/bin/sh", &actions, &attr, &cArgs, &cEnv)
        if rc == 0 {
            Log.info("sleep: crash watcher running (pid \(pid))")
        } else {
            Log.error("sleep: could not start the crash watcher (\(rc))")
        }
    }
}
