import SwiftUI

/// Draws with `tank_glyph`. Bit 4 of each row is the left pixel.
/// No `Text` and no system font.
struct PixelLabel: View {
    let text: String
    var scale: CGFloat = 2
    var color: Color = Color(red: 0.93, green: 0.89, blue: 0.78)
    /// Wrap on spaces once the line would pass this width. Keeps Dynamic
    /// Type from clipping a one-line canvas off the phone.
    var maxWidth: CGFloat? = nil

    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    private var drawnScale: CGFloat {
        let bump: CGFloat
        if dynamicTypeSize.isAccessibilitySize {
            bump = 1.35
        } else if dynamicTypeSize >= .xxLarge {
            bump = 1.12
        } else {
            bump = 1
        }
        return scale * bump
    }

    var body: some View {
        let s = drawnScale
        let advance = 6 * s
        let glyphs = Array(text.unicodeScalars)
        let columns: Int = {
            guard let maxWidth, maxWidth >= advance else { return max(glyphs.count, 1) }
            return max(1, Int(maxWidth / advance))
        }()
        let laid = wrap(glyphs, columns: columns)
        let widest = laid.map(\.count).max() ?? 1
        let width = CGFloat(max(widest, 1)) * advance
        let height = CGFloat(max(laid.count, 1)) * 8 * s
        Canvas { ctx, _ in
            for (line, rowGlyphs) in laid.enumerated() {
                for (col, scalar) in rowGlyphs.enumerated() {
                    let bits = tank_glyph(scalar.value)
                    let originX = CGFloat(col) * advance
                    let originY = CGFloat(line) * 8 * s
                    for row in 0..<7 {
                        let rowBits = UInt32((bits >> (row * 5)) & 0x1F)
                        for colPx in 0..<5 where (rowBits >> (4 - colPx)) & 1 == 1 {
                            let rect = CGRect(
                                x: originX + CGFloat(colPx) * s,
                                y: originY + CGFloat(row) * s,
                                width: s,
                                height: s
                            )
                            ctx.fill(Path(rect), with: .color(color))
                        }
                    }
                }
            }
        }
        .frame(width: max(width, 1), height: max(height, 1), alignment: .topLeading)
        .accessibilityLabel(text)
    }

    /// Word-wrap. A single token longer than `columns` is split.
    private func wrap(_ glyphs: [Unicode.Scalar], columns: Int) -> [[Unicode.Scalar]] {
        if glyphs.isEmpty { return [[]] }
        if glyphs.count <= columns { return [glyphs] }
        let space = Unicode.Scalar(32)!
        var lines: [[Unicode.Scalar]] = []
        var line: [Unicode.Scalar] = []
        var word: [Unicode.Scalar] = []
        func flushWord() {
            guard !word.isEmpty else { return }
            if line.isEmpty {
                if word.count <= columns {
                    line = word
                } else {
                    var rest = word
                    while rest.count > columns {
                        lines.append(Array(rest.prefix(columns)))
                        rest.removeFirst(columns)
                    }
                    line = rest
                }
            } else if line.count + 1 + word.count <= columns {
                line.append(space)
                line.append(contentsOf: word)
            } else {
                lines.append(line)
                if word.count <= columns {
                    line = word
                } else {
                    var rest = word
                    line = []
                    while rest.count > columns {
                        lines.append(Array(rest.prefix(columns)))
                        rest.removeFirst(columns)
                    }
                    line = rest
                }
            }
            word = []
        }
        for scalar in glyphs {
            if scalar == space {
                flushWord()
            } else {
                word.append(scalar)
            }
        }
        flushWord()
        if !line.isEmpty { lines.append(line) }
        return lines.isEmpty ? [[]] : lines
    }
}

extension View {
    /// One VoiceOver name for a pixel control. The drawn glyphs stay visual.
    func a11yButton(_ label: String, hint: String? = nil, selected: Bool = false) -> some View {
        modifier(A11yButton(label: label, hint: hint, selected: selected))
    }
}

private struct A11yButton: ViewModifier {
    var label: String
    var hint: String?
    var selected: Bool

    func body(content: Content) -> some View {
        let traits: AccessibilityTraits = selected ? [.isButton, .isSelected] : .isButton
        content
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(label)
            .accessibilityHint(hint ?? "")
            .accessibilityAddTraits(traits)
    }
}

/// Tank chrome stays as tall as its content so touches below it reach
/// the tank. When Dynamic Type makes the HUD taller than the screen,
/// it scrolls instead of clipping.
struct ChromeScroll<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        ViewThatFits(in: .vertical) {
            content()
            ScrollView(.vertical, showsIndicators: false) {
                content()
            }
        }
    }
}

extension GraphicsContext {
    /// Draw `text` with `tank_glyph` at `origin` (top-left), 6 px advance.
    func pixelText(_ text: String, at origin: CGPoint, scale: CGFloat, color: Color) {
        var pen = origin.x
        for scalar in text.unicodeScalars {
            let bits = tank_glyph(scalar.value)
            for row in 0..<7 {
                let rowBits = UInt32((bits >> (row * 5)) & 0x1F)
                for col in 0..<5 where (rowBits >> (4 - col)) & 1 == 1 {
                    let rect = CGRect(
                        x: pen + CGFloat(col) * scale,
                        y: origin.y + CGFloat(row) * scale,
                        width: scale,
                        height: scale
                    )
                    fill(Path(rect), with: .color(color))
                }
            }
            pen += 6 * scale
        }
    }
}

enum Palette {
    static let paper = Color(red: 0.93, green: 0.89, blue: 0.78)
    static let dimPaper = Color(red: 0.62, green: 0.6, blue: 0.54)
    static let highlight = Color(red: 1, green: 0.92, blue: 0.7)
    static let panel = Color(red: 0.09, green: 0.08, blue: 0.07)
    static let wall = Color(red: 0.16, green: 0.13, blue: 0.11)
    static let floor = Color(red: 0.24, green: 0.17, blue: 0.11)
    static let water = Color(red: 0.4, green: 0.7, blue: 0.9)
}

/// Flat 9-slice-style panel: 2 px border, solid fill. The slices are
/// equal because the Phase 0 chrome has no artwork yet.
struct PixelPanel<Content: View>: View {
    var content: () -> Content

    init(@ViewBuilder content: @escaping () -> Content) {
        self.content = content
    }

    var body: some View {
        content()
            .padding(8)
            .background(panel)
    }

    private var panel: some View {
        Canvas { ctx, size in
            let border: CGFloat = 2
            let fill = CGRect(
                x: border,
                y: border,
                width: max(size.width - border * 2, 0),
                height: max(size.height - border * 2, 0)
            )
            ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(Color(white: 0.42)))
            ctx.fill(Path(fill), with: .color(Color(red: 0.09, green: 0.08, blue: 0.07)))
        }
    }
}
