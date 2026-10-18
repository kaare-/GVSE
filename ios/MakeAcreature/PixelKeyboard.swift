import SwiftUI
import UIKit

/// Dark system keyboard. Text is edited in its own UIKit window, never
/// beside a SwiftUI canvas: focusing a field in the door sign used to
/// crash while the keyboard snapshotted `PixelLabel`.
enum PixelKeyboard {
    static func applyAppearance() {
        UITextField.appearance().keyboardAppearance = .dark
        UITextView.appearance().keyboardAppearance = .dark
        UITextField.appearance().tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
        UITextView.appearance().tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
    }
}

enum PixelPrompt {
    private static var host: PromptHost?

    static func singleLine(title: String, text: String, placeholder: String, onDone: @escaping (String) -> Void) {
        let page = LineController(title: title, text: text, placeholder: placeholder) { entered in
            dismiss()
            if let entered { onDone(entered) }
        }
        show(page)
    }

    static func notes(text: String, onDone: @escaping (String) -> Void) {
        let page = NoteController(text: text) { entered in
            dismiss()
            if let entered { onDone(entered) }
        }
        show(page)
    }

    private static func show(_ page: EditorPage) {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        guard let scene = scenes.first(where: { $0.activationState == .foregroundActive }) ?? scenes.first else {
            return
        }
        dismiss()
        let window = UIWindow(windowScene: scene)
        window.windowLevel = .alert + 1
        window.backgroundColor = UIColor(red: 0.09, green: 0.08, blue: 0.07, alpha: 1)
        window.rootViewController = page
        let previous = scene.windows.first(where: \.isKeyWindow)
        page.willEdit = { previous?.isHidden = true }
        host = PromptHost(window: window, previous: previous)
        window.makeKeyAndVisible()
    }

    private static func dismiss() {
        host?.close()
        host = nil
    }
}

private final class PromptHost {
    let window: UIWindow
    let previous: UIWindow?

    init(window: UIWindow, previous: UIWindow?) {
        self.window = window
        self.previous = previous
    }

    func close() {
        window.isHidden = true
        window.rootViewController = nil
        previous?.isHidden = false
        previous?.makeKey()
    }
}

/// The room is full of pixel canvases. Hide it before the keyboard
/// appears, or the snapshot crashes the app.
private protocol EditorPage: UIViewController {
    var willEdit: (() -> Void)? { get set }
}

/// One line, no input accessory. A zero-size accessory crashed on focus.
private final class LineController: UIViewController, UITextFieldDelegate, EditorPage {
    private let field = UITextField()
    private let heading: String
    private let onDone: (String?) -> Void
    var willEdit: (() -> Void)?

    init(title: String, text: String, placeholder: String, onDone: @escaping (String?) -> Void) {
        self.heading = title
        self.onDone = onDone
        super.init(nibName: nil, bundle: nil)
        field.text = text
        field.placeholder = placeholder
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = UIColor(red: 0.09, green: 0.08, blue: 0.07, alpha: 1)

        let title = UILabel()
        title.text = heading
        title.font = .monospacedSystemFont(ofSize: 18, weight: .bold)
        title.textColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)

        field.font = .monospacedSystemFont(ofSize: 18, weight: .regular)
        field.textColor = UIColor(red: 0.93, green: 0.89, blue: 0.78, alpha: 1)
        field.tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
        field.keyboardAppearance = .dark
        field.autocorrectionType = .no
        field.autocapitalizationType = .words
        field.clearButtonMode = .whileEditing
        field.returnKeyType = .done
        field.borderStyle = .none
        field.backgroundColor = UIColor(white: 0.14, alpha: 1)
        field.leftView = UIView(frame: CGRect(x: 0, y: 0, width: 10, height: 1))
        field.leftViewMode = .always
        field.delegate = self
        field.translatesAutoresizingMaskIntoConstraints = false
        title.translatesAutoresizingMaskIntoConstraints = false

        let done = barButton("DONE", bold: true, action: #selector(finish))
        let cancel = barButton("CANCEL", bold: false, action: #selector(close))

        view.addSubview(cancel)
        view.addSubview(done)
        view.addSubview(title)
        view.addSubview(field)

        NSLayoutConstraint.activate([
            cancel.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 16),
            cancel.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 12),
            done.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            done.centerYAnchor.constraint(equalTo: cancel.centerYAnchor),
            title.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 16),
            title.topAnchor.constraint(equalTo: cancel.bottomAnchor, constant: 28),
            field.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 16),
            field.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -16),
            field.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 12),
            field.heightAnchor.constraint(equalToConstant: 44),
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        willEdit?()
        willEdit = nil
        field.becomeFirstResponder()
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        finish()
        return true
    }

    @objc private func finish() {
        let text = field.text ?? ""
        field.resignFirstResponder()
        onDone(text)
    }

    @objc private func close() {
        field.resignFirstResponder()
        onDone(nil)
    }

    private func barButton(_ title: String, bold: Bool, action: Selector) -> UIButton {
        let button = UIButton(type: .system)
        button.setTitle(title, for: .normal)
        button.titleLabel?.font = .monospacedSystemFont(ofSize: 16, weight: bold ? .bold : .regular)
        button.tintColor = bold
            ? UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
            : UIColor(red: 0.75, green: 0.72, blue: 0.65, alpha: 1)
        button.addTarget(self, action: action, for: .touchUpInside)
        button.translatesAutoresizingMaskIntoConstraints = false
        return button
    }
}

/// Notes editor. Same isolated window, still with no accessory bar.
private final class NoteController: UIViewController, EditorPage {
    private let editor = UITextView()
    private let onDone: (String?) -> Void
    var willEdit: (() -> Void)?

    init(text: String, onDone: @escaping (String?) -> Void) {
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
            editor.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -12),
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        willEdit?()
        willEdit = nil
        editor.becomeFirstResponder()
    }

    @objc private func finish() {
        let text = editor.text ?? ""
        editor.resignFirstResponder()
        onDone(text)
    }

    @objc private func close() {
        editor.resignFirstResponder()
        onDone(nil)
    }
}
