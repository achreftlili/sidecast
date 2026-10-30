import AppKit
import ApplicationServices
import CoreGraphics

/// Screen Recording and Accessibility checks, prompts and System Settings deep links.
enum Permissions {
    static var screenRecordingGranted: Bool { CGPreflightScreenCaptureAccess() }

    /// Shows the system prompt (first time) and registers the app in the list.
    @discardableResult
    static func requestScreenRecording() -> Bool { CGRequestScreenCaptureAccess() }

    static var accessibilityTrusted: Bool { AXIsProcessTrusted() }

    /// Shows the system Accessibility prompt if not trusted yet.
    @discardableResult
    static func promptAccessibility() -> Bool {
        let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        return AXIsProcessTrustedWithOptions([key: true] as CFDictionary)
    }

    static func openScreenRecordingSettings() {
        open("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
    }

    static func openAccessibilitySettings() {
        open("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
    }

    /// System Settings → Battery (laptops) / Energy (desktops): one pane, `com.apple.Battery-Settings.extension`.
    /// Only opens it; never changes a setting.
    static func openEnergySettings() {
        open("x-apple.systempreferences:com.apple.Battery-Settings.extension")
    }

    private static func open(_ url: String) {
        if let url = URL(string: url) { NSWorkspace.shared.open(url) }
    }
}
