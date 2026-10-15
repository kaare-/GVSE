import SwiftUI

/// Draws with `tank_glyph`. Bit 4 of each row is the left pixel.
/// No `Text` and no system font.
struct PixelLabel: View {
    let text: String
    var scale: CGFloat = 2
    var color: Color = Color(red: 0.93, green: 0.89, blue: 0.78)

    var body: some View {
        let width = CGFloat(text.count) * 6 * scale
        Canvas { ctx, _ in
            var pen: CGFloat = 0
            for scalar in text.unicodeScalars {
                let bits = tank_glyph(scalar.value)
                for row in 0..<7 {
                    let rowBits = UInt32((bits >> (row * 5)) & 0x1F)
                    for col in 0..<5 {
                        let on = (rowBits >> (4 - col)) & 1 == 1
                        if !on { continue }
                        let rect = CGRect(
                            x: pen + CGFloat(col) * scale,
                            y: CGFloat(row) * scale,
                            width: scale,
                            height: scale
                        )
                        ctx.fill(Path(rect), with: .color(color))
                    }
                }
                pen += 6 * scale
            }
        }
        .frame(width: max(width, 1), height: 7 * scale)
        .accessibilityLabel(text)
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
