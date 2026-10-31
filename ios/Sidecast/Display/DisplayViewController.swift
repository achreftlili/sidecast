import SidecastKit
import UIKit

/// The extended display: full screen, landscape, screen kept awake. Shows the
/// stream letterboxed, forwards touches (Apple Pencil included), the keyboard
/// and the microphone, and overlays connection / pairing / error states.
final class DisplayViewController: UIViewController {
    /// Extra (non-first) pointers starting this close to an edge are a gripping thumb.
    private static let edgeBand: CGFloat = 16
    /// Margin around floating controls inside which a pointer never reaches the computer.
    private static let controlSlop: CGFloat = 4

    private let target: SessionTarget
    private let screenInfo: ScreenInfo
    private var controller: DisplayController!

    private let surface = TouchSurfaceView()
    private let videoView = VideoView()
    private let pointer = PointerView()
    private let overlay = OverlayView()
    private let stats = InsetLabel()
    private let modeLabel = InsetLabel()
    private let micChip = InsetLabel()
    private let keyboardField = KeyboardField(frame: CGRect(x: -10, y: -10, width: 1, height: 1))
    private let keyboardButton = FloatingButton(symbol: "keyboard", label: "Keyboard", identifier: "keyboard")
    private let modeButton = FloatingButton(symbol: "hand.tap", label: "Input mode", identifier: "inputMode")
    private let closeButton = FloatingButton(symbol: "xmark", label: "Disconnect", identifier: "disconnect")
    private lazy var faders = [keyboardButton, modeButton, closeButton].map { IdleFader($0) }
    private lazy var pairingPrompt = PairingPrompt(
        host: self, submit: { [weak self] in self?.controller.submitCode($0) ?? false },
        cancel: { [weak self] in self?.finish() }, expired: { [weak self] in self?.controller.expirePairing() })

    private var state = DisplaySession.State.idle
    private var decoderStatus = DecoderStatus.idle
    private var config: StreamConfig?
    private var route: Route?
    private var cursor: CursorPosition?
    private var playing = false
    private var videoRect = VideoRect.zero
    private let sequencer = TouchSequencer<ObjectIdentifier>()
    private lazy var filter = PointerFilter { [weak self] x, y, first in self?.isHeldBack(x, y, first: first) ?? false }
    private let tapDetector = ThreeFingerTapDetector(slop: 12)
    private var takenPresses = Set<ObjectIdentifier>()
    private var statsTimer: Timer?
    private var modeLabelTimer: Timer?
    private var finished = false
    private var observers: [NSObjectProtocol] = []

    init(target: SessionTarget, screen: ScreenInfo) {
        self.target = target
        self.screenInfo = screen
        super.init(nibName: nil, bundle: nil)
        modalPresentationStyle = .fullScreen
        overrideUserInterfaceStyle = .dark   // over video, alerts and overlay read better dark
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func loadView() { view = surface }

    override var supportedInterfaceOrientations: UIInterfaceOrientationMask { .landscape }
    override var preferredInterfaceOrientationForPresentation: UIInterfaceOrientation { .landscapeRight }
    override var prefersStatusBarHidden: Bool { true }
    override var prefersHomeIndicatorAutoHidden: Bool { true }
    // The first swipe from an edge goes to the stream (3/4-finger swipes, scrolls), not to iOS.
    override var preferredScreenEdgesDeferringSystemGestures: UIRectEdge { .all }
    override var canBecomeFirstResponder: Bool { true }

    override func viewDidLoad() {
        super.viewDidLoad()
        controller = DisplayController(target: target, screen: screenInfo,
                                       renderer: videoView.displayLayer.sampleBufferRenderer)
        buildViews()
        wireController()
        surface.onTouches = { [weak self] phase, touches, event in self?.handle(phase, touches, event) }
        let center = NotificationCenter.default
        observers = [
            center.addObserver(forName: UIScene.didEnterBackgroundNotification, object: nil, queue: .main) { [weak self] _ in
                self?.keyboardField.resignFirstResponder()
                self?.controller.enterBackground()
            },
            center.addObserver(forName: UIScene.willEnterForegroundNotification, object: nil, queue: .main) { [weak self] _ in
                self?.controller.enterForeground()
            },
        ]
        render()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        UIApplication.shared.isIdleTimerDisabled = true
        becomeFirstResponder()   // hardware-keyboard keys
        controller.start()
        controller.enterForeground()
        statsTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.updateStats() }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        UIApplication.shared.isIdleTimerDisabled = false
        statsTimer?.invalidate()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        overlay.frame = surface.bounds
        layoutVideo()
    }

    deinit {
        observers.forEach(NotificationCenter.default.removeObserver)
    }

    // MARK: - Setup

    private func buildViews() {
        surface.addSubview(videoView)
        surface.addSubview(pointer)
        surface.addSubview(keyboardField)
        keyboardField.onKeyboard = { [weak self] in self?.controller.send($0) }

        stats.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        stats.isHidden = true
        stats.accessibilityIdentifier = "stats"
        modeLabel.font = .preferredFont(forTextStyle: .headline)
        modeLabel.isHidden = true
        micChip.text = "● Mic"
        micChip.font = .systemFont(ofSize: 13, weight: .bold)
        micChip.backgroundColor = UIColor.systemRed.withAlphaComponent(0.9)
        micChip.isHidden = true
        micChip.accessibilityLabel = "Microphone on: the computer is using this device’s microphone"
        micChip.accessibilityIdentifier = "micIndicator"

        for v in [stats, modeLabel, micChip, keyboardButton, modeButton, closeButton] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            surface.addSubview(v)
        }
        surface.addSubview(overlay)
        overlay.onAction = { [weak self] kind in
            switch kind {
            case .retry: self?.controller.retry()
            case .pairAgain: self?.controller.pairAgain()
            case .close: self?.finish()
            }
        }

        let safe = surface.safeAreaLayoutGuide
        // The video ignores the safe area; controls stay clear of the Dynamic Island and
        // ride above the on-screen keyboard (the stream itself never moves).
        let bottom = surface.keyboardLayoutGuide.topAnchor
        NSLayoutConstraint.activate([
            stats.leadingAnchor.constraint(equalTo: safe.leadingAnchor, constant: 8),
            stats.topAnchor.constraint(equalTo: safe.topAnchor, constant: 8),
            stats.trailingAnchor.constraint(lessThanOrEqualTo: closeButton.leadingAnchor, constant: -8),
            closeButton.trailingAnchor.constraint(equalTo: safe.trailingAnchor, constant: -12),
            closeButton.topAnchor.constraint(equalTo: safe.topAnchor, constant: 12),
            keyboardButton.trailingAnchor.constraint(equalTo: safe.trailingAnchor, constant: -12),
            keyboardButton.bottomAnchor.constraint(equalTo: bottom, constant: -12),
            modeButton.trailingAnchor.constraint(equalTo: keyboardButton.leadingAnchor, constant: -12),
            modeButton.bottomAnchor.constraint(equalTo: bottom, constant: -12),
            micChip.trailingAnchor.constraint(equalTo: closeButton.leadingAnchor, constant: -12),
            micChip.centerYAnchor.constraint(equalTo: closeButton.centerYAnchor),
            modeLabel.centerXAnchor.constraint(equalTo: surface.centerXAnchor),
            modeLabel.bottomAnchor.constraint(equalTo: bottom, constant: -20),
        ])

        keyboardButton.addAction(UIAction { [weak self] _ in self?.toggleKeyboard() }, for: .primaryActionTriggered)
        modeButton.addAction(UIAction { [weak self] _ in self?.toggleInputMode() }, for: .primaryActionTriggered)
        closeButton.addAction(UIAction { [weak self] _ in self?.confirmDisconnect() }, for: .primaryActionTriggered)
        showInputMode(controller.inputMode)
    }

    private func wireController() {
        controller.onState = { [weak self] in
            self?.state = $0
            self?.render()
        }
        controller.onDecoderStatus = { [weak self] in
            self?.decoderStatus = $0
            self?.render()
        }
        controller.onStreamConfig = { [weak self] in
            self?.config = $0
            self?.layoutVideo()
        }
        controller.onRoute = { [weak self] in
            self?.route = $0
            self?.render()
        }
        controller.onCursor = { [weak self] in
            self?.cursor = $0
            self?.renderPointer()
        }
        controller.onMicCapturing = { [weak self] in self?.micChip.isHidden = !$0 }
    }

    // MARK: - Rendering

    private func render() {
        if state == .stopped { return finish() }
        if case .awaitingCode(let name, let prompt) = state {
            pairingPrompt.show(prompt, name: name)
        } else {
            pairingPrompt.dismiss()
        }
        let message = OverlayPresenter.present(state, decoder: decoderStatus, route: route, fallbackName: target.name,
                                               platform: controller.platform)
        if case .streaming = state { playing = message == nil } else { playing = false }
        overlay.isHidden = message == nil
        if let message { overlay.show(message) }
        renderControls(available: playing)
        renderPointer()
    }

    /// Keyboard and input-mode buttons only while the stream is up.
    private func renderControls(available: Bool) {
        guard keyboardButton.isHidden == available else { return }
        for button in [keyboardButton, modeButton, closeButton] { button.isHidden = !available }
        if available {
            faders.forEach { $0.poke() }
        } else {
            faders.forEach { $0.stop() }
            if keyboardField.isFirstResponder { keyboardField.resignFirstResponder() }
        }
    }

    /// The video view takes the stream's aspect ratio, centered (letterbox / pillarbox).
    private func layoutVideo() {
        let bounds = surface.bounds
        guard bounds.width > 0, bounds.height > 0 else { return }
        videoRect = Letterbox.fit(containerWidth: bounds.width, containerHeight: bounds.height,
                                  videoWidth: config?.width ?? 0, videoHeight: config?.height ?? 0,
                                  scale: traitCollection.displayScale)
        videoView.frame = config == nil ? bounds : videoRect.cgRect
        renderPointer()
    }

    private func renderPointer() {
        pointer.place(at: PointerPlacement.position(cursor: cursor, mode: controller.inputMode, playing: playing,
                                                    rect: videoRect))
    }

    private func updateStats() {
        let sample = controller.sampleStats()
        guard !stats.isHidden, let s = sample else { return }
        let c = config
        var line = "\(c?.codec ?? "-") \(c?.width ?? 0)×\(c?.height ?? 0)@\(c?.fps ?? 0) · rx \(s.rxFps) fps · "
            + String(format: "%.1f Mbps", s.mbps) + " · shown \(s.shownFps) fps · RTT "
            + (s.rttMs.map { "\($0) ms" } ?? "–") + " · dropped \(s.dropped)"
        // "Remote" whenever the stream does not come over the local network (Tailscale, internet).
        if let route, route != .lan { line = "Remote · " + line }
        stats.text = line
    }

    private func toggleStats() {
        stats.isHidden.toggle()
        if !stats.isHidden { stats.text = "Three-finger tap toggles stats" }
    }

    // MARK: - Touch (PROTOCOL §7)

    private func handle(_ phase: UITouch.Phase, _ touches: Set<UITouch>, _ event: UIEvent?) {
        let ordered = touches.sorted { $0.timestamp < $1.timestamp }
        let tracked = ordered.map { touch -> TrackedTouch<ObjectIdentifier> in
            let p = touch.location(in: surface)
            return TrackedTouch(key: ObjectIdentifier(touch), x: p.x, y: p.y)
        }
        let time = ordered.last?.timestamp ?? ProcessInfo.processInfo.systemUptime
        let events: [PointerEvent]
        switch phase {
        case .began:
            let down = (event?.allTouches ?? touches).filter { $0.phase != .ended && $0.phase != .cancelled }
            events = sequencer.began(tracked, stillDown: Set(down.map(ObjectIdentifier.init)))
        case .moved:
            events = sequencer.moved(tracked)
        case .ended:
            events = sequencer.ended(tracked)
        default:
            events = sequencer.cancelled()
        }
        for e in events {
            // Forward first: nothing may delay a touch.
            if let forwarded = filter.filter(e),
               let touch = TouchTranslator.translate(forwarded, eventTimeMs: TouchTranslator.eventTimeMs(uptime: time),
                                                     rect: videoRect) {
                controller.send(touch)
            }
            if e.action == .down {
                // A touch on the stream may click elsewhere on the computer: typing context is stale.
                keyboardField.forgetContext()
                if playing { faders.forEach { $0.poke() } }
            }
            if tapDetector.onEvent(e, time: time) { toggleStats() }
        }
    }

    /// Pointers the computer must not see (PointerFilter): one landing right next to a
    /// floating control, and an extra one in the edge band, where a thumb holding the device rests.
    private func isHeldBack(_ x: Double, _ y: Double, first: Bool) -> Bool {
        let point = CGPoint(x: x, y: y)
        for control in [keyboardButton, modeButton, closeButton, micChip] as [UIView] where !control.isHidden {
            if control.frame.insetBy(dx: -Self.controlSlop, dy: -Self.controlSlop).contains(point) { return true }
        }
        if first { return false }
        let b = surface.bounds
        return x < Self.edgeBand || y < Self.edgeBand || x > b.width - Self.edgeBand || y > b.height - Self.edgeBand
    }

    // MARK: - Keyboard (PROTOCOL §7.1)

    private func toggleKeyboard() {
        if keyboardField.isFirstResponder {
            keyboardField.resignFirstResponder()
            becomeFirstResponder()
        } else {
            keyboardField.forgetContext()
            keyboardField.becomeFirstResponder()
        }
        faders.forEach { $0.poke() }
    }

    /// A hardware keyboard while the text field is not focused.
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var rest = Set<UIPress>()
        for press in presses {
            guard playing, let key = press.key,
                  let keyboardEvent = HardwareKeyboard.translate(hidUsage: key.keyCode.rawValue,
                                                                 modifierFlags: key.modifierFlags.rawValue,
                                                                 characters: key.characters) else {
                rest.insert(press)
                continue
            }
            takenPresses.insert(ObjectIdentifier(press))
            if case .key = keyboardEvent { keyboardField.forgetContext() }
            controller.send(keyboardEvent)
        }
        if !rest.isEmpty { super.pressesBegan(rest, with: event) }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        let rest = presses.filter { takenPresses.remove(ObjectIdentifier($0)) == nil }
        if !rest.isEmpty { super.pressesEnded(rest, with: event) }
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        let rest = presses.filter { takenPresses.remove(ObjectIdentifier($0)) == nil }
        if !rest.isEmpty { super.pressesCancelled(rest, with: event) }
    }

    // MARK: - Input mode (PROTOCOL §7.3)

    private func toggleInputMode() {
        let next: InputMode = controller.inputMode == .trackpad ? .touch : .trackpad
        controller.setInputMode(next)
        showInputMode(next)
        modeLabel.text = next == .trackpad ? "Trackpad mode" : "Touch mode"
        modeLabel.alpha = 1
        modeLabel.isHidden = false
        modeLabelTimer?.invalidate()
        modeLabelTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: false) { [weak self] _ in
            UIView.animate(withDuration: 0.2, animations: { self?.modeLabel.alpha = 0 }) { _ in self?.modeLabel.isHidden = true }
        }
        renderPointer()
    }

    /// Icon and description show the current mode; in trackpad mode the button stays more visible.
    private func showInputMode(_ mode: InputMode) {
        let trackpad = mode == .trackpad
        modeButton.setSymbol(trackpad ? "rectangle.and.hand.point.up.left" : "hand.tap")
        modeButton.accessibilityValue = trackpad ? "trackpad" : "touch"
        faders[1].idleAlpha = trackpad ? 0.7 : 0.4
        if !modeButton.isHidden { faders[1].poke() }
    }

    // MARK: - Leaving

    private func confirmDisconnect() {
        let name: String
        if case .streaming(let n) = state { name = n } else { name = target.name }
        let alert = UIAlertController(title: "Disconnect from \(name)?", message: nil, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Disconnect", style: .destructive) { [weak self] _ in self?.finish() })
        present(alert, animated: true)
    }

    private func finish() {
        guard !finished else { return }
        finished = true
        pairingPrompt.dismiss()
        keyboardField.resignFirstResponder()
        controller.stop()
        (UIApplication.shared.delegate as? AppDelegate)?.orientationLock = .all
        presentingViewController?.dismiss(animated: true)
    }
}

/// A rounded label with padding (stats, mode flash, mic indicator).
final class InsetLabel: UILabel {
    private let insets = UIEdgeInsets(top: 4, left: 10, bottom: 4, right: 10)

    override init(frame: CGRect) {
        super.init(frame: frame)
        textColor = .white
        backgroundColor = UIColor.black.withAlphaComponent(0.6)
        numberOfLines = 0
        layer.cornerRadius = 8
        layer.masksToBounds = true
        isUserInteractionEnabled = false
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func drawText(in rect: CGRect) { super.drawText(in: rect.inset(by: insets)) }

    override var intrinsicContentSize: CGSize {
        let s = super.intrinsicContentSize
        return CGSize(width: s.width + insets.left + insets.right, height: s.height + insets.top + insets.bottom)
    }

    override func textRect(forBounds bounds: CGRect, limitedToNumberOfLines n: Int) -> CGRect {
        let r = super.textRect(forBounds: bounds.inset(by: insets), limitedToNumberOfLines: n)
        return r.inset(by: UIEdgeInsets(top: -insets.top, left: -insets.left, bottom: -insets.bottom, right: -insets.right))
    }
}
