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

/// Monospaced dark field with a DONE accessory — used for archive notes
/// and the door sign.
struct PixelTextField: View {
    @Binding var text: String
    var placeholder: String = ""
    var onDone: (() -> Void)?

    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            TextField(placeholder, text: $text)
                .textFieldStyle(.plain)
                .font(.system(.body, design: .monospaced))
                .foregroundColor(Palette.paper)
                .padding(6)
                .background(Color(white: 0.14))
                .focused($focused)
            if focused {
                Button {
                    focused = false
                    onDone?()
                } label: {
                    PixelLabel(text: "DONE", scale: 2, color: Palette.highlight)
                }
                .buttonStyle(.plain)
            }
        }
    }
}

struct PixelTextEditor: View {
    @Binding var text: String
    var minHeight: CGFloat = 90
    var onDone: (() -> Void)?

    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            TextEditor(text: $text)
                .font(.system(.body, design: .monospaced))
                .foregroundColor(Palette.paper)
                .scrollContentBackground(.hidden)
                .frame(minHeight: minHeight)
                .padding(4)
                .background(Color(white: 0.14))
                .focused($focused)
            if focused {
                Button {
                    focused = false
                    onDone?()
                } label: {
                    PixelLabel(text: "DONE", scale: 2, color: Palette.highlight)
                }
                .buttonStyle(.plain)
            }
        }
    }
}
