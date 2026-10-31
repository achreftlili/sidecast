import SidecastKit
import UIKit

/// The whole screen is a remote touch surface: it reports raw touches, multi-touch included.
final class TouchSurfaceView: UIView {
    var onTouches: ((_ phase: UITouch.Phase, _ touches: Set<UITouch>, _ event: UIEvent?) -> Void)?

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .black
        isMultipleTouchEnabled = true
        accessibilityIdentifier = "display"
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) { onTouches?(.began, touches, event) }
    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) { onTouches?(.moved, touches, event) }
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) { onTouches?(.ended, touches, event) }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) { onTouches?(.cancelled, touches, event) }
}

/// A round translucent button floating over the stream.
final class FloatingButton: UIButton {
    static let size: CGFloat = 48

    init(symbol: String, label: String, identifier: String) {
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        backgroundColor = UIColor.black.withAlphaComponent(0.6)
        tintColor = .white
        layer.cornerRadius = Self.size / 2
        setSymbol(symbol)
        accessibilityLabel = label
        accessibilityIdentifier = identifier
        NSLayoutConstraint.activate([
            widthAnchor.constraint(equalToConstant: Self.size),
            heightAnchor.constraint(equalToConstant: Self.size),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func setSymbol(_ name: String) {
        setImage(UIImage(systemName: name, withConfiguration: UIImage.SymbolConfiguration(pointSize: 20, weight: .medium)),
                 for: .normal)
    }
}

/// Keeps a floating control at `activeAlpha` while the user interacts and fades
/// it to `idleAlpha` after a few seconds, so it stays findable without covering the stream.
final class IdleFader {
    private weak var view: UIView?
    private let activeAlpha: CGFloat
    /// May change at any time (e.g. a mode indicator that should not fade); applies on the next fade.
    var idleAlpha: CGFloat
    private var timer: Timer?

    init(_ view: UIView, activeAlpha: CGFloat = 1, idleAlpha: CGFloat = 0.4) {
        self.view = view
        self.activeAlpha = activeAlpha
        self.idleAlpha = idleAlpha
    }

    func poke() {
        timer?.invalidate()
        UIView.animate(withDuration: 0.12) { self.view?.alpha = self.activeAlpha }
        timer = Timer.scheduledTimer(withTimeInterval: 3, repeats: false) { [weak self] _ in
            guard let self else { return }
            UIView.animate(withDuration: 0.4) { self.view?.alpha = self.idleAlpha }
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }
}

/// The computer's pointer in trackpad mode (§7.3): a large arrow, tip at `hotspot`.
final class PointerView: UIView {
    static let hotspot = CGPoint(x: 3, y: 3)

    override init(frame: CGRect) {
        super.init(frame: CGRect(x: 0, y: 0, width: 30, height: 44))
        isOpaque = false
        isUserInteractionEnabled = false
        isHidden = true
        accessibilityIdentifier = "pointer"
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func draw(_ rect: CGRect) {
        let p = UIBezierPath()
        p.move(to: CGPoint(x: 3, y: 3))
        p.addLine(to: CGPoint(x: 3, y: 35))
        p.addLine(to: CGPoint(x: 11, y: 27))
        p.addLine(to: CGPoint(x: 17, y: 40))
        p.addLine(to: CGPoint(x: 22.5, y: 37.5))
        p.addLine(to: CGPoint(x: 16.5, y: 25))
        p.addLine(to: CGPoint(x: 27, y: 25))
        p.close()
        p.lineJoinStyle = .round
        p.lineWidth = 2.5
        UIColor.black.setStroke()
        UIColor.white.setFill()
        p.fill()
        p.stroke()
    }

    /// Places the tip at `point` (container coordinates), or hides the pointer.
    func place(at point: CGPoint?) {
        guard let point else {
            isHidden = true
            return
        }
        frame.origin = CGPoint(x: point.x - Self.hotspot.x, y: point.y - Self.hotspot.y)
        isHidden = false
    }
}

/// Connection, pairing and error messages over a dimmed stream.
final class OverlayView: UIView {
    var onAction: ((OverlayAction.Kind) -> Void)?

    private let spinner = UIActivityIndicatorView(style: .large)
    private let title = UILabel()
    private let detail = UILabel()
    private let primary = UIButton(configuration: .filled())
    private let secondary = UIButton(configuration: .gray())
    private var primaryKind: OverlayAction.Kind?
    private var secondaryKind: OverlayAction.Kind?

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = UIColor.black.withAlphaComponent(0.7)
        spinner.color = .white
        title.font = .preferredFont(forTextStyle: .title3)
        title.textColor = .white
        title.textAlignment = .center
        title.numberOfLines = 0
        title.accessibilityIdentifier = "overlayTitle"
        detail.font = .preferredFont(forTextStyle: .subheadline)
        detail.textColor = UIColor(white: 0.85, alpha: 1)
        detail.textAlignment = .center
        detail.numberOfLines = 0
        // Light text on the dark scrim, whatever the system appearance.
        secondary.configuration?.baseForegroundColor = .white
        secondary.configuration?.baseBackgroundColor = UIColor(white: 1, alpha: 0.2)
        primary.addAction(UIAction { [weak self] _ in self?.primaryKind.map { self?.onAction?($0) } }, for: .primaryActionTriggered)
        secondary.addAction(UIAction { [weak self] _ in self?.secondaryKind.map { self?.onAction?($0) } }, for: .primaryActionTriggered)
        let buttons = UIStackView(arrangedSubviews: [secondary, primary])
        buttons.spacing = 12
        let stack = UIStackView(arrangedSubviews: [spinner, title, detail, buttons])
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 14
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
            stack.widthAnchor.constraint(lessThanOrEqualToConstant: 520),
            stack.leadingAnchor.constraint(greaterThanOrEqualTo: safeAreaLayoutGuide.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: safeAreaLayoutGuide.trailingAnchor, constant: -24),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func show(_ overlay: Overlay) {
        title.text = overlay.title
        detail.text = overlay.detail
        detail.isHidden = overlay.detail == nil
        if overlay.progress { spinner.startAnimating() } else { spinner.stopAnimating() }
        spinner.isHidden = !overlay.progress
        bind(primary, overlay.primary) { primaryKind = $0 }
        bind(secondary, overlay.secondary) { secondaryKind = $0 }
    }

    private func bind(_ button: UIButton, _ action: OverlayAction?, _ store: (OverlayAction.Kind?) -> Void) {
        button.isHidden = action == nil
        store(action?.kind)
        button.configuration?.title = action?.label
        button.accessibilityIdentifier = action.map { "overlay.\($0.label)" }
    }
}

/// The device keyboard, typing on the computer (PROTOCOL §7.1). A text field
/// off screen mirrors what was typed since the context was last reset; every
/// edit — keys, autocorrection, predictions, dictation revising its words,
/// marked text of input methods — becomes the keystrokes that turn the old text
/// into the new one (`TextFieldDiff`), so the computer shows what the field holds.
final class KeyboardField: UITextField, UITextFieldDelegate {
    var onKeyboard: ((KeyboardEvent) -> Void)?
    /// What the computer already has, just before its cursor.
    private var mirrored = ""

    override init(frame: CGRect) {
        super.init(frame: frame)
        delegate = self
        alpha = 0.01
        tintColor = .clear
        autocapitalizationType = .sentences
        autocorrectionType = .default
        spellCheckingType = .no
        smartQuotesType = .no
        smartDashesType = .no
        smartInsertDeleteType = .no
        keyboardAppearance = .dark
        returnKeyType = .default
        accessibilityIdentifier = "keyboardField"
        addTarget(self, action: #selector(changed), for: .editingChanged)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    /// The computer's cursor may have moved (the user touched the stream): nothing
    /// before it is known any more, so the field starts empty without typing anything.
    func forgetContext() {
        guard !mirrored.isEmpty || !(text ?? "").isEmpty else { return }
        unmarkText()
        text = ""
        mirrored = ""
    }

    @objc private func changed() {
        let now = text ?? ""
        let events = TextFieldDiff.events(from: mirrored, to: now)
        mirrored = now
        events.forEach { onKeyboard?($0) }
        // Keep the field short; long text gives the keyboard nothing it can still use.
        if markedTextRange == nil, now.count > 200 {
            let tail = String(now.suffix(40))
            text = tail
            mirrored = tail
        }
    }

    /// Backspace with nothing left in the field still deletes on the computer.
    override func deleteBackward() {
        if (text ?? "").isEmpty, markedTextRange == nil {
            onKeyboard?(.press(.backspace))
            return
        }
        super.deleteBackward()
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        onKeyboard?(.press(.returnKey))
        text = ""
        mirrored = ""
        return false
    }

    /// The caret stays at the end: the computer's cursor is there.
    func textFieldDidChangeSelection(_ textField: UITextField) {
        guard markedTextRange == nil, let range = selectedTextRange else { return }
        if range.start != endOfDocument || range.end != endOfDocument {
            selectedTextRange = textRange(from: endOfDocument, to: endOfDocument)
        }
    }

    /// Hardware-keyboard keys the text system would keep (arrows, Tab, Escape…)
    /// go to the computer; Return and Backspace take the text path above.
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var unhandled = Set<UIPress>()
        for press in presses {
            guard let key = press.key, let protocolKey = HardwareKeyboard.protocolKey(hidUsage: key.keyCode.rawValue),
                  protocolKey != .returnKey, protocolKey != .backspace else {
                unhandled.insert(press)
                continue
            }
            forgetContext()
            onKeyboard?(.press(protocolKey, HardwareKeyboard.modifiers(flags: key.modifierFlags.rawValue)))
        }
        if !unhandled.isEmpty { super.pressesBegan(unhandled, with: event) }
    }

    // Presses taken in pressesBegan must not reach the text system half-way through.
    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        let rest = presses.filter { !Self.isTaken($0) }
        if !rest.isEmpty { super.pressesEnded(rest, with: event) }
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        let rest = presses.filter { !Self.isTaken($0) }
        if !rest.isEmpty { super.pressesCancelled(rest, with: event) }
    }

    private static func isTaken(_ press: UIPress) -> Bool {
        guard let key = press.key, let k = HardwareKeyboard.protocolKey(hidUsage: key.keyCode.rawValue) else { return false }
        return k != .returnKey && k != .backspace
    }
}
