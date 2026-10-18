import SwiftUI
import UIKit

/// Pixel-styled system keyboard appearance (§2.2 #7): dark keyboard +
/// a pixel accessory bar. A full custom keyboard extension is deferred
/// (needs founder / App Store entitlements).
enum PixelKeyboard {
    static func applyAppearance() {
        UITextField.appearance().keyboardAppearance = .dark
        UITextView.appearance().keyboardAppearance = .dark
        UITextField.appearance().tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
        UITextView.appearance().tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
    }
}

/// Monospaced dark field. UIKit, with no input accessory, so the system
/// keyboard bar cannot crash the pixel canvases around it.
struct PixelTextField: View {
    @Binding var text: String
    var placeholder: String = ""
    var onDone: (() -> Void)?

    var body: some View {
        PixelFieldRep(text: $text, placeholder: placeholder, onDone: onDone)
            .frame(height: 36)
            .padding(4)
            .background(Color(white: 0.14))
    }
}

private struct PixelFieldRep: UIViewRepresentable {
    @Binding var text: String
    var placeholder: String
    var onDone: (() -> Void)?

    func makeUIView(context: Context) -> UITextField {
        let field = UITextField()
        field.delegate = context.coordinator
        field.keyboardAppearance = .dark
        field.inputAccessoryView = nil
        field.font = .monospacedSystemFont(ofSize: 16, weight: .regular)
        field.textColor = UIColor(red: 0.93, green: 0.89, blue: 0.78, alpha: 1)
        field.tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
        field.backgroundColor = .clear
        field.autocorrectionType = .no
        field.returnKeyType = .done
        field.placeholder = placeholder
        field.text = text
        return field
    }

    func updateUIView(_ uiView: UITextField, context: Context) {
        context.coordinator.parent = self
        uiView.inputAccessoryView = nil
        if uiView.text != text {
            uiView.text = text
        }
    }

    func makeCoordinator() -> Coord { Coord(self) }

    final class Coord: NSObject, UITextFieldDelegate {
        var parent: PixelFieldRep
        init(_ parent: PixelFieldRep) { self.parent = parent }

        func textField(_ textField: UITextField, shouldChangeCharactersIn range: NSRange, replacementString string: String) -> Bool {
            guard let current = textField.text, let span = Range(range, in: current) else { return true }
            parent.text = current.replacingCharacters(in: span, with: string)
            return true
        }

        func textFieldShouldReturn(_ textField: UITextField) -> Bool {
            textField.resignFirstResponder()
            parent.onDone?()
            return true
        }
    }
}

struct PixelTextEditor: View {
    @Binding var text: String
    var minHeight: CGFloat = 90
    var onDone: (() -> Void)?

    var body: some View {
        PixelEditorRep(text: $text, onDone: onDone)
            .frame(minHeight: minHeight)
            .padding(4)
            .background(Color(white: 0.14))
    }
}

private struct PixelEditorRep: UIViewRepresentable {
    @Binding var text: String
    var onDone: (() -> Void)?

    func makeUIView(context: Context) -> UITextView {
        let view = UITextView()
        view.delegate = context.coordinator
        view.keyboardAppearance = .dark
        view.inputAccessoryView = nil
        view.font = .monospacedSystemFont(ofSize: 16, weight: .regular)
        view.textColor = UIColor(red: 0.93, green: 0.89, blue: 0.78, alpha: 1)
        view.tintColor = UIColor(red: 1, green: 0.92, blue: 0.7, alpha: 1)
        view.backgroundColor = .clear
        view.autocorrectionType = .no
        view.text = text
        return view
    }

    func updateUIView(_ uiView: UITextView, context: Context) {
        context.coordinator.parent = self
        uiView.inputAccessoryView = nil
        if uiView.text != text {
            uiView.text = text
        }
    }

    func makeCoordinator() -> Coord { Coord(self) }

    final class Coord: NSObject, UITextViewDelegate {
        var parent: PixelEditorRep
        init(_ parent: PixelEditorRep) { self.parent = parent }

        func textViewDidChange(_ textView: UITextView) {
            parent.text = textView.text
        }
    }
}
