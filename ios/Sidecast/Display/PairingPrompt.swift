import SidecastKit
import UIKit

/// "Enter the code shown on your computer": attempts left after a wrong code,
/// a countdown to the code's expiry, and submission as soon as six digits are typed.
final class PairingPrompt {
    private weak var host: UIViewController?
    private let submit: (String) -> Bool
    private let cancel: () -> Void
    private let expired: () -> Void
    private var alert: UIAlertController?
    private var shown: StreamClient.PairingPrompt?
    private var name = ""
    private var problem: String?
    private var timer: Timer?
    private let clock = MonotonicTimeSource()

    init(host: UIViewController, submit: @escaping (String) -> Bool, cancel: @escaping () -> Void,
         expired: @escaping () -> Void) {
        self.host = host
        self.submit = submit
        self.cancel = cancel
        self.expired = expired
    }

    /// Shows (or refreshes) the prompt; showing the same prompt again does nothing.
    func show(_ prompt: StreamClient.PairingPrompt, name: String) {
        guard prompt != shown || alert == nil else { return }
        shown = prompt
        self.name = name
        problem = prompt.wrongCode ? "Wrong code. \(prompt.attemptsLeft) attempt\(prompt.attemptsLeft == 1 ? "" : "s") left." : nil
        present()
    }

    func dismiss() {
        shown = nil
        timer?.invalidate()
        timer = nil
        alert?.dismiss(animated: true)
        alert = nil
    }

    private func present() {
        alert?.dismiss(animated: false)
        let alert = UIAlertController(title: "Pair with \(name)", message: message(), preferredStyle: .alert)
        alert.addTextField { field in
            field.placeholder = "6-digit code"
            field.keyboardType = .numberPad
            field.textContentType = .oneTimeCode
            field.textAlignment = .center
            field.font = .monospacedDigitSystemFont(ofSize: 22, weight: .semibold)
            field.accessibilityIdentifier = "pairingCode"
            field.addAction(UIAction { [weak self, weak field] _ in
                guard let self, let code = field?.text, code.count == 6 else { return }
                self.trySubmit(code)
            }, for: .editingChanged)
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel) { [weak self] _ in
            self?.alert = nil
            self?.dismiss()
            self?.cancel()
        })
        alert.addAction(UIAlertAction(title: "Pair", style: .default) { [weak self, weak alert] _ in
            guard let self else { return }
            self.alert = nil
            self.trySubmit(alert?.textFields?.first?.text ?? "")
        })
        self.alert = alert
        host?.present(alert, animated: true)
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.tick() }
    }

    private func trySubmit(_ code: String) {
        guard PairingCode(code) != nil else {
            problem = "The code has 6 digits."
            return present()
        }
        if submit(code) { dismiss() }
    }

    private func tick() {
        guard let shown else { return }
        if clock.now() >= shown.expiresAt {
            dismiss()
            expired()
            return
        }
        alert?.message = message()
    }

    private func message() -> String {
        var lines = ["Enter the code shown on your computer."]
        if let shown {
            let left = max(0, Int((shown.expiresAt - clock.now()).rounded(.up)))
            lines.append(String(format: "Code expires in %d:%02d", left / 60, left % 60))
        }
        if let problem { lines.append(problem) }
        return lines.joined(separator: "\n")
    }
}
