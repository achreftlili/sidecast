import Foundation
import os

/// App logging: unified logging always, plus stdout in test mode.
enum Log {
    /// Set once at launch (before any other thread starts).
    nonisolated(unsafe) static var echoToStdout = false
    nonisolated(unsafe) static var verbose = false

    private static let logger = Logger(subsystem: "com.achreftlili.sidecast", category: "app")
    private static let start = Date()

    static func info(_ message: @autoclosure () -> String) {
        let text = message()
        logger.info("\(text, privacy: .public)")
        echo(text)
    }

    static func error(_ message: @autoclosure () -> String) {
        let text = message()
        logger.error("\(text, privacy: .public)")
        echo("ERROR " + text)
    }

    static func debug(_ message: @autoclosure () -> String) {
        guard verbose else { return }
        let text = message()
        logger.debug("\(text, privacy: .public)")
        echo(text)
    }

    private static func echo(_ text: String) {
        guard echoToStdout else { return }
        let t = String(format: "%8.3f", Date().timeIntervalSince(start))
        print("[\(t)] \(text)")
    }
}

/// A flag that can be claimed exactly once, from any thread.
final class OnceFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var claimed = false

    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if claimed { return false }
        claimed = true
        return true
    }
}
