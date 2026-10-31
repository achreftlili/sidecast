import Foundation
import SidecastKit

/// A button of the overlay.
struct OverlayAction: Equatable {
    enum Kind: Equatable {
        case retry
        case pairAgain
        case close
    }

    var label: String
    var kind: Kind
}

/// Message layer drawn over the (possibly frozen) video.
struct Overlay: Equatable {
    var title: String
    var detail: String?
    var progress = false
    var primary: OverlayAction?
    var secondary: OverlayAction?
}

/// Maps session + decoder state to what the display screen shows; nil = show the video. Pure.
enum OverlayPresenter {
    /// - Parameters:
    ///   - route: the path being tried / used, shown while connecting (§1.1)
    ///   - platform: the computer's OS once known, for advice that differs per OS
    static func present(_ state: DisplaySession.State, decoder: DecoderStatus, route: Route?, fallbackName: String,
                        platform: HostPlatform?) -> Overlay? {
        switch state {
        case .idle, .paused: return busy("Connecting to \(fallbackName)…", via(route))
        case .connecting(let name): return busy("Connecting to \(name)…", via(route))
        case .pairing(let name): return busy("Pairing with \(name)…", "A 6-digit code will appear on your computer.")
        case .awaitingCode: return Overlay(title: "Enter the code shown on your computer", secondary: cancel)
        case .verifyingCode: return busy("Checking the code…")
        case .streaming: return streaming(decoder)
        case .reconnecting(let name, let cause, let attempt): return reconnecting(name, cause, attempt, route)
        case .failed(let name, let error): return failed(name, error, platform)
        case .stopped: return nil
        }
    }

    private static func streaming(_ decoder: DecoderStatus) -> Overlay? {
        switch decoder {
        case .running(hasPicture: true): return nil
        case .running, .idle: return busy("Starting the stream…")
        case .unsupported(let config, let detail):
            return Overlay(title: "This device can’t play the stream",
                           detail: "The video decoder rejected \(config.codec.uppercased()) \(config.width)×\(config.height) (\(detail)).",
                           primary: retry, secondary: close)
        }
    }

    static func via(_ route: Route?) -> String? {
        switch route {
        case .lan: return "Connecting via home network…"
        case .tailscale: return "Connecting via Tailscale…"
        case .internet: return "Connecting via the internet…"
        case nil: return nil
        }
    }

    private static func reconnecting(_ name: String, _ cause: RetryCause, _ attempt: Int, _ route: Route?) -> Overlay {
        let title: String
        switch cause {
        case .connectionLost: title = "Connection lost — reconnecting…"
        case .unreachable: title = "Can’t reach \(name) — retrying…"
        case .serverStopped: title = "Sidecast stopped on \(name) — waiting for it…"
        case .repair: title = "\(name) no longer knows this device — pairing again…"
        }
        let hint: String?
        if cause == .unreachable {
            hint = "Make sure Sidecast is running on the computer and both devices are on the same network."
        } else {
            hint = attempt > 1 ? "Attempt \(attempt)" : nil
        }
        let detail = [via(route), hint].compactMap { $0 }.joined(separator: "\n")
        return Overlay(title: title, detail: detail.isEmpty ? nil : detail, progress: true, secondary: close)
    }

    private static func failed(_ name: String, _ error: SessionError, _ platform: HostPlatform?) -> Overlay {
        let tryAgain = OverlayAction(label: "Try Again", kind: .retry)
        switch error {
        case .replaced:
            return Overlay(title: "Another device took over",
                           detail: "\(name) is now streaming to another phone or tablet.",
                           primary: OverlayAction(label: "Reconnect", kind: .retry), secondary: close)
        case .permissionMissing:
            // The Screen Recording steps are macOS-only; other hosts get generic advice.
            if platform == .macos {
                return Overlay(title: "Screen Recording permission needed",
                               detail: "On the Mac, open System Settings → Privacy & Security → Screen & System Audio Recording, allow Sidecast, then tap Retry.",
                               primary: retry, secondary: close)
            }
            return Overlay(title: "Screen capture permission needed",
                           detail: "Sidecast on \(name) isn’t allowed to capture the screen. Allow it on the computer, then tap Retry.",
                           primary: retry, secondary: close)
        case .protocolMismatch:
            return Overlay(title: "Update needed",
                           detail: "This computer runs a different version of Sidecast. Install the same version on both devices.",
                           secondary: close)
        case .identityChanged:
            return Overlay(title: "This computer’s identity changed — pair again",
                           detail: "\(name) presented a different certificate than when this device paired with it. If you reinstalled Sidecast on the computer, pair again. Otherwise another device may be impersonating it.",
                           primary: OverlayAction(label: "Pair Again", kind: .pairAgain), secondary: close)
        case .pairingExpired:
            return Overlay(title: "The code expired", detail: "Try again to get a new code on the computer.",
                           primary: tryAgain, secondary: close)
        case .tooManyAttempts:
            return Overlay(title: "Too many wrong codes", detail: "Try again to get a new code on the computer.",
                           primary: tryAgain, secondary: close)
        case .displayFailed:
            return Overlay(title: "The computer couldn’t create the display",
                           detail: "Sidecast on \(name) failed to create its virtual display.", primary: retry, secondary: close)
        case .protocolError(let detail):
            return Overlay(title: "Something went wrong", detail: "Protocol error: \(detail)", primary: retry, secondary: close)
        case .rejected(let reason):
            return Overlay(title: "Disconnected by the computer", detail: "Reason: \(reason)", primary: retry, secondary: close)
        }
    }

    private static func busy(_ title: String, _ detail: String? = nil) -> Overlay {
        Overlay(title: title, detail: detail, progress: true, secondary: cancel)
    }

    private static let retry = OverlayAction(label: "Retry", kind: .retry)
    private static let close = OverlayAction(label: "Close", kind: .close)
    private static let cancel = OverlayAction(label: "Cancel", kind: .close)
}
