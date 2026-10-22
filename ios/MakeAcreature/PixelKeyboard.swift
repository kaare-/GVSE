import UIKit

/// Stock system keyboard. A tinted or accessory keyboard crashed when the
/// tank name was edited, so this alert leaves the field alone.
enum PixelPrompt {
    static func singleLine(title: String, text: String, placeholder: String, onDone: @escaping (String) -> Void) {
        let alert = UIAlertController(title: title, message: nil, preferredStyle: .alert)
        alert.addTextField { field in
            field.text = text
            field.placeholder = placeholder
            field.autocapitalizationType = .words
            field.clearButtonMode = .whileEditing
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
              let root = scene.windows.first(where: \.isKeyWindow)?.rootViewController
                ?? scene.windows.first?.rootViewController
        else { return }
        var top = root
        while let next = top.presentedViewController, !next.isBeingDismissed {
            top = next
        }
        top.present(vc, animated: true)
    }
}

/// Notes page. System keyboard, no accessory bar and no extra window.
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
        view.backgroundColor = .systemBackground
        editor.font = .preferredFont(forTextStyle: .body)
        editor.autocorrectionType = .default
        editor.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(editor)

        let done = UIButton(type: .system)
        done.setTitle("Done", for: .normal)
        done.addTarget(self, action: #selector(finish), for: .touchUpInside)
        done.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(done)

        let cancel = UIButton(type: .system)
        cancel.setTitle("Cancel", for: .normal)
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
            editor.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -12),
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
