import Foundation

/// Runs a command synchronously with a timeout. Never call it on the main thread
/// for anything that may block (sudo, pmset).
enum ProcessRunner {
    struct Result {
        var status: Int32
        var stdout: String
        var stderr: String
        var timedOut: Bool

        var succeeded: Bool { status == 0 && !timedOut }
    }

    static func run(_ executable: String, _ arguments: [String], environment: [String: String]? = nil,
                    timeout: TimeInterval = 10) -> Result {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        if let environment { process.environment = environment }
        let out = Pipe(), err = Pipe()
        process.standardOutput = out
        process.standardError = err
        process.standardInput = FileHandle.nullDevice

        let exited = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in exited.signal() }
        do {
            try process.run()
        } catch {
            return Result(status: -1, stdout: "", stderr: "\(error)", timedOut: false)
        }
        // Drain pipes concurrently so a chatty child cannot block on a full pipe.
        var outData = Data(), errData = Data()
        let group = DispatchGroup()
        DispatchQueue.global().async(group: group) { outData = out.fileHandleForReading.readDataToEndOfFile() }
        DispatchQueue.global().async(group: group) { errData = err.fileHandleForReading.readDataToEndOfFile() }

        var timedOut = false
        if exited.wait(timeout: .now() + timeout) == .timedOut {
            timedOut = true
            process.terminate()
            if exited.wait(timeout: .now() + 1) == .timedOut {
                kill(process.processIdentifier, SIGKILL)
                _ = exited.wait(timeout: .now() + 1)
            }
        }
        _ = group.wait(timeout: .now() + 2)
        return Result(status: process.isRunning ? -1 : process.terminationStatus,
                      stdout: String(decoding: outData, as: UTF8.self),
                      stderr: String(decoding: errData, as: UTF8.self),
                      timedOut: timedOut)
    }
}
