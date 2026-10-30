import CoreVideo
import Foundation
import SidecastKit

/// The phone a stream is for.
struct ClientInfo {
    let deviceId: String
    let deviceName: String
    let screen: ScreenInfo
    let codecs: [CodecInfo]
    /// HELLO `network` / `limits` (data saver, PROTOCOL §5).
    var network: String? = nil
    var limits: StreamLimits? = nil

    var dataSaver: Bool { limits != nil }
}

/// Produces frames for a stream and consumes its touch and keyboard input. The real
/// backend owns the virtual display, ScreenCaptureKit and CGEvent injection;
/// the test backend renders a synthetic pattern.
protocol StreamBackend: AnyObject {
    /// Captured frame + capture timestamp (monotonic µs). Called on any thread.
    typealias FrameHandler = (CVPixelBuffer, UInt64) -> Void

    /// Set by the stream; called (any thread) when a running stream dies for good.
    var onRuntimeFailure: ((Error) -> Void)? { get set }

    /// Starts producing frames sized `plan.encodeWidth × plan.encodeHeight`.
    /// `completion` runs once, on any thread.
    func start(plan: StreamPlan, client: ClientInfo, onFrame: @escaping FrameHandler,
               completion: @escaping (Result<Void, Error>) -> Void)

    /// Stops frames, releases input (held buttons) and tears down the display.
    func stop()

    /// Re-delivers the latest frame if the source is idle (static screen),
    /// so a keyframe request is answered promptly.
    func requestRefresh()

    /// Handles one TOUCH (already filtered by the touch-enabled setting).
    func handleTouch(_ event: TouchEvent)

    /// Handles one TEXT (already validated and filtered by the input setting).
    func handleText(_ text: String)

    /// Handles one KEY (already filtered by the input setting; unknown keys are ignored).
    func handleKey(_ event: KeyEvent)

    /// Touch or trackpad interpretation of TOUCH (PROTOCOL §7.3); a change ends the current gesture.
    /// Switching to trackpad reports the pointer (forced) through `onCursor`.
    func setInputMode(_ mode: InputMode)

    /// Set by the stream; trackpad-mode pointer positions (any thread). The Bool forces a send.
    var onCursor: ((CursorPosition, Bool) -> Void)? { get set }

    /// Reports the pointer again (forced), e.g. after a new STREAM_CONFIG in trackpad mode.
    func refreshCursor()
}

/// Why a stream could not start; maps to a BYE reason.
enum StreamStartError: Error, CustomStringConvertible {
    case displayFailed(String)
    case captureFailed(String)

    var description: String {
        switch self {
        case .displayFailed(let s): return "virtual display failed: \(s)"
        case .captureFailed(let s): return "capture failed: \(s)"
        }
    }
}
