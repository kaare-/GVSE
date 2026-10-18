import SwiftUI
import UIKit

/// Dark system keyboard. Text is edited in a UIKit alert or page, never
/// in a SwiftUI canvas: a field next to `PixelLabel` crashes on focus,
/// and a text view inside the book page crashes as soon as it appears.
enum PixelKeyboard {
    static func applyAppearance() {
        UITextField.appearance().keyboardAppearance = .dark
        UITextView.appearance().keyboardAppearance = .dark
        UITextField.appearance().tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
        UITextView.appearance().tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
    }
}

enum PixelPrompt {
    static func singleLine(title: String, text: String, placeholder: String, onDone: @escaping (String) -> Void) {
        let alert = UIAlertController(title: title, message: nil, preferredStyle: .alert)
        alert.addTextField { field in
            field.text = text
            field.placeholder = placeholder
            field.keyboardAppearance = .dark
            field.autocorrectionType = .no
            field.autocapitalizationType = .words
            field.clearButtonMode = .whileEditing
            field.inputAccessoryView = UIView(frame: .zero)
            field.inputAssistantItem.leadingBarButtonGroups = []
            field.inputAssistantItem.trailingBarButtonGroups = []
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Done", style: .default) { _ in
            onDone(alert.textFields?.first?.text ?? "")
        })
        present(alert)
    }

    static func notes(text: String, onDone: @escaping (String) -> Void) {
        let editor = NoteController(text: text, onDone: onDone)
        editor.modalPresentationStyle = .pageSheet
        present(editor)
    }

    private static func present(_ vc: UIViewController) {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        guard let scene = scenes.first(where: { $0.activationState == .foregroundActive }) ?? scenes.first,
              let root = scene.windows.first(where: \.isKeyWindow)?.rootViewController ?? scene.windows.first?.rootViewController
        else { return }
        var top = root
        while let next = top.presentedViewController, !next.isBeingDismissed {
            top = next
        }
        top.present(vc, animated: true)
    }
}

/// Notes editor with no SwiftUI and no keyboard accessory bar.
private final class NoteController: UIViewController {
    private let editor = UITextView()
    private let onDone: (String) -> Void

    init(text: String, onDone: @escaping (String) -> Void) {
        self.onDone = onDone
        super.init(nibName: nil, bundle: nil)
        editor.text = text
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = UIColor(red: 0.09, green: 0.08, blue: 0.07, alpha: 1)
        editor.backgroundColor = UIColor(white: 0.14, alpha: 1)
        editor.textColor = UIColor(red: 0.93, green: 0.89, blue: 0.78, alpha: 1)
        editor.tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
        editor.font = .monospacedSystemFont(ofSize: 16, weight: .regular)
        editor.keyboardAppearance = .dark
        editor.autocorrectionType = .no
        editor.inputAccessoryView = UIView(frame: .zero)
        editor.inputAssistantItem.leadingBarButtonGroups = []
        editor.inputAssistantItem.trailingBarButtonGroups = []
        editor.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(editor)

        let done = UIButton(type: .system)
        done.setTitle("DONE", for: .normal)
        done.titleLabel?.font = .monospacedSystemFont(ofSize: 16, weight: .bold)
        done.tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
        done.addTarget(self, action: #selector(finish), for: .touchUpInside)
        done.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(done)

        let cancel = UIButton(type: .system)
        cancel.setTitle("CANCEL", for: .normal)
        cancel.titleLabel?.font = .monospacedSystemFont(ofSize: 16, weight: .regular)
        cancel.tintColor = UIColor(red: 0.75, green: 0.72, blue: 0.65, alpha: 1)
        cancel.addTarget(self, action: #selector(close), for: .touchUpInside)
        cancel.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(cancel)

        NSLayoutConstraint.activate([
            cancel.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 16),
            cancel.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 12),
            done.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            done.centerYAnchor.constraint(equalTo: cancel.centerYAnchor),
            editor.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 12),
            editor.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -12),
            editor.topAnchor.constraint(equalTo: cancel.bottomAnchor, constant: 12),
            editor.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -12),
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        editor.becomeFirstResponder()
    }

    @objc private func finish() {
        let text = editor.text ?? ""
        dismiss(animated: true) { self.onDone(text) }
    }

    @objc private func close() {
        dismiss(animated: true)
    }
}
