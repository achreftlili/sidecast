import CoreGraphics
import Foundation
import SidecastKit

/// The real backend: a CGVirtualDisplay sized for the phone, captured with
/// ScreenCaptureKit, with touches injected onto it (and keyboard input typed
/// into the focused app). The display exists only
/// while streaming and is destroyed on stop.
final class DisplayCaptureBackend: StreamBackend {
    var onRuntimeFailure: ((Error) -> Void)?
    var onCursor: ((CursorPosition, Bool) -> Void)? {
        didSet { input.onCursor = onCursor }
    }
    var onHold: (() -> Void)? {
        didSet { input.onHold = onHold }
    }
    /// Main queue: input was dropped for lack of Accessibility.
    var onAccessibilityMissing: (() -> Void)? {
        didSet { input.onAccessibilityMissing = onAccessibilityMissing }
    }

    private var display: VirtualDisplay?          // main thread
    private var capture: ScreenCaptureSource?     // main thread
    private let input = InputController()

    /// Bumped by every start and stop, so a start that is still awaiting the helper
    /// can tell it was cancelled (user disconnected) or superseded. Main thread.
    private var generation = 0

    func start(plan: StreamPlan, client: ClientInfo, onFrame: @escaping FrameHandler,
               completion: @escaping (Result<Void, Error>) -> Void) {
        Task { @MainActor in
            self.tearDown()
            let myGeneration = self.generation
            let display: VirtualDisplay
            do {
                display = try await VirtualDisplay.start(mode: plan.display, name: "Sidecast (\(client.deviceName))",
                                                         serial: VirtualDisplay.serial(for: client.deviceId))
            } catch {
                return completion(.failure(StreamStartError.displayFailed("\(error)")))
            }
            guard self.generation == myGeneration else {   // stopped or restarted while the helper was starting
                display.stop()
                return completion(.failure(StreamStartError.displayFailed("cancelled while the display was starting")))
            }
            display.onUnexpectedExit = { [weak self, weak display] in
                guard let self, let display, self.display === display else { return }
                self.onRuntimeFailure?(StreamStartError.displayFailed("the virtual display helper exited unexpectedly"))
            }
            self.display = display
            let capture = ScreenCaptureSource(displayID: display.displayID, width: plan.encodeWidth,
                                              height: plan.encodeHeight, fps: plan.fps, onFrame: onFrame)
            capture.onFailure = { [weak self] error in self?.onRuntimeFailure?(error) }
            self.capture = capture
            capture.start { [weak self] result in
                guard let self, self.display === display else { return completion(result) }
                if case .success = result {
                    display.ensurePreferredMode()
                    display.ensurePhysicalDisplayStaysMain()
                    let size = CGSize(width: plan.display.width, height: plan.display.height)
                    self.input.configure(displayID: display.displayID, size: size,
                                         pointsPerDp: Self.pointsPerDp(plan: plan, screen: client.screen))
                }
                completion(result)
            }
        }
    }

    func stop() {
        input.reset()
        DispatchQueue.main.async { self.tearDown() }
    }

    func requestRefresh() {
        DispatchQueue.main.async { self.capture?.refresh() }
    }

    func handleTouch(_ event: TouchEvent) {
        input.handle(event)
    }

    func handleText(_ text: String) {
        input.handleText(text)
    }

    func handleKey(_ event: KeyEvent) {
        input.handleKey(event)
    }

    func setInputMode(_ mode: InputMode) {
        input.setInputMode(mode)
    }

    func refreshCursor() {
        input.refreshCursor()
    }

    private func tearDown() {
        generation += 1
        capture?.stop()          // stop capture first...
        capture = nil
        display?.stop()          // ...then stop the helper, which removes the virtual display
        display = nil
    }

    /// Display points per phone dp: the display spans the phone's width.
    static func pointsPerDp(plan: StreamPlan, screen: ScreenInfo) -> Double {
        let widthPx = Double(max(screen.width, screen.height))
        let density = Double(max(screen.densityDpi, 1)) / 160
        let widthDp = widthPx / density
        return widthDp > 0 ? Double(plan.display.width) / widthDp : 1
    }
}
