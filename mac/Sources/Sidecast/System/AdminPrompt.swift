import Foundation
import SidecastKit

/// Runs one shell command as root through the standard administrator prompt
/// (`do shell script … with administrator privileges`). Main thread: it blocks
/// while the dialog is up.
enum AdminPrompt {
    struct Failure: Error, CustomStringConvertible {
        let description: String
    }

    static func run(_ command: String, prompt: String? = nil) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        var error: NSDictionary?
        guard let script = NSAppleScript(source: SudoersRule.appleScript(adminCommand: command, prompt: prompt)) else {
            throw Failure(description: "could not build the AppleScript")
        }
        script.executeAndReturnError(&error)
        if let error {
            throw Failure(description: (error[NSAppleScript.errorMessage] as? String) ?? "\(error)")
        }
    }
}
