import Foundation

/// The sudoers rule that lets the user toggle `pmset disablesleep` without a
/// password (DECISIONS Q20), plus the shell/AppleScript quoting used to
/// install it. Pure string logic so it can be unit-tested.
public enum SudoersRule {
    public static let installPath = "/etc/sudoers.d/sidecast"
    public static let pmsetPath = "/usr/bin/pmset"
    public static let sudoPath = "/usr/bin/sudo"

    /// `pmset` arguments the rule allows, as (enable sleep, disable sleep).
    public static let enableSleepArgs = ["-a", "disablesleep", "0"]
    public static let disableSleepArgs = ["-a", "disablesleep", "1"]

    public enum RuleError: Error, Equatable {
        case invalidUserName(String)
    }

    /// User names we are willing to write into a sudoers file.
    public static func isValidUserName(_ name: String) -> Bool {
        guard !name.isEmpty, name.utf8.count <= 64 else { return false }
        return name.unicodeScalars.allSatisfy { s in
            (s >= "A" && s <= "Z") || (s >= "a" && s <= "z") || (s >= "0" && s <= "9")
                || s == "." || s == "_" || s == "-"
        }
    }

    /// The file content (comment line + rule line, trailing newline).
    public static func content(forUser user: String) throws -> String {
        guard isValidUserName(user) else { throw RuleError.invalidUserName(user) }
        let commands = [enableSleepArgs, disableSleepArgs]
            .map { ([pmsetPath] + $0).joined(separator: " ") }
            .joined(separator: ", ")
        return """
        # Installed by Sidecast: lets \(user) toggle system sleep while a phone is connected.
        \(user) ALL=(root) NOPASSWD: \(commands)

        """
    }

    /// Admin shell command: validate the staged file, then install it root:wheel 0440.
    public static func installCommand(stagedFile: String) -> String {
        let tmp = shellQuote(stagedFile)
        return "/usr/sbin/visudo -cf \(tmp) && /usr/bin/install -m 0440 -o root -g wheel \(tmp) \(shellQuote(installPath))"
    }

    /// Admin shell command that removes the rule.
    public static var removeCommand: String { "/bin/rm -f \(shellQuote(installPath))" }

    /// AppleScript source running `command` through an administrator prompt
    /// (optionally with `prompt` as the dialog's explanation).
    public static func appleScript(adminCommand command: String, prompt: String? = nil) -> String {
        let withPrompt = prompt.map { " with prompt \(appleScriptStringLiteral($0))" } ?? ""
        return "do shell script \(appleScriptStringLiteral(command))\(withPrompt) with administrator privileges"
    }

    /// POSIX shell single-quoting: `it's` → `'it'\''s'`.
    public static func shellQuote(_ s: String) -> String {
        "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    /// An AppleScript string literal (backslashes and double quotes escaped).
    public static func appleScriptStringLiteral(_ s: String) -> String {
        let escaped = s.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
        return "\"" + escaped + "\""
    }

    /// The crash-watcher script (DECISIONS Q21). `$1` = app pid, `$2` = marker path.
    public static let watcherScript =
        "while /bin/kill -0 \"$1\" 2>/dev/null; do /bin/sleep 2; done; "
        + "if [ -f \"$2\" ]; then /usr/bin/sudo -n /usr/bin/pmset -a disablesleep 0 && /bin/rm -f \"$2\"; fi"
}
