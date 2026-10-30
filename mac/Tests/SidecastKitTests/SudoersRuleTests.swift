import XCTest
@testable import SidecastKit

final class SudoersRuleTests: XCTestCase {
    func testRuleContent() throws {
        let content = try SudoersRule.content(forUser: "achref.t_l-1")
        let lines = content.split(separator: "\n", omittingEmptySubsequences: false)
        XCTAssertTrue(lines[0].hasPrefix("# "))
        XCTAssertEqual(lines[1], "achref.t_l-1 ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 0, /usr/bin/pmset -a disablesleep 1")
        XCTAssertTrue(content.hasSuffix("\n"))
    }

    func testRejectsUnsafeUserNames() {
        for bad in ["", "root ALL", "a,b", "x\ny", "$(id)", "us'er", "ünï", "a:b", "%admin", String(repeating: "a", count: 65)] {
            XCTAssertFalse(SudoersRule.isValidUserName(bad), bad)
            XCTAssertThrowsError(try SudoersRule.content(forUser: bad))
        }
        XCTAssertTrue(SudoersRule.isValidUserName("achreftlili"))
    }

    func testShellQuoting() {
        XCTAssertEqual(SudoersRule.shellQuote("/tmp/a b"), "'/tmp/a b'")
        XCTAssertEqual(SudoersRule.shellQuote("it's"), "'it'\\''s'")
    }

    func testInstallAndRemoveCommands() {
        let cmd = SudoersRule.installCommand(stagedFile: "/private/var/folders/x y/sidecast.sudoers")
        XCTAssertEqual(cmd, "/usr/sbin/visudo -cf '/private/var/folders/x y/sidecast.sudoers' && "
                       + "/usr/bin/install -m 0440 -o root -g wheel '/private/var/folders/x y/sidecast.sudoers' '/etc/sudoers.d/sidecast'")
        XCTAssertEqual(SudoersRule.removeCommand, "/bin/rm -f '/etc/sudoers.d/sidecast'")
    }

    func testAppleScriptEscaping() {
        let script = SudoersRule.appleScript(adminCommand: #"echo "a\b""#)
        XCTAssertEqual(script, #"do shell script "echo \"a\\b\"" with administrator privileges"#)
    }
}
