import SwiftUI

/// The inspect card: species name + generation, age, children, life
/// expectancy, energy bar, lineage line. Never the genome. The sim keeps
/// running underneath; the tracking rectangle is drawn by `TrackingBox`.
struct InspectCard: View {
    let state: InspectState
    var onArchive: () -> Void
    var onClose: () -> Void

    var body: some View {
        PixelPanel {
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 8) {
                    PixelLabel(text: state.speciesName.uppercased(), scale: 2, color: Palette.highlight, maxWidth: 220)
                    Spacer(minLength: 0)
                    Button(action: onClose) {
                        PixelLabel(text: "X", scale: 2)
                    }
                    .buttonStyle(.plain)
                    .a11yButton("Close inspect")
                }
                PixelLabel(
                    text: state.alive ? "GEN \(state.generation)  CELLS \(state.orgCells)\(kinSuffix)" : "GONE",
                    scale: 2,
                    color: state.alive ? Palette.paper : Palette.dimPaper
                )
                energyBar
                PixelLabel(text: "AGE \(seconds(state.ageTicks))S OF \(seconds(state.maxAgeTicks))S", scale: 2)
                PixelLabel(
                    text: "CHILDREN \(state.children)  KIN \(state.lineage.kin_alive)  HEIRS \(state.lineage.descendants_alive)",
                    scale: 2
                )
                if let parent = state.parentName {
                    PixelLabel(text: "FROM \(parent.uppercased())", scale: 2, color: Palette.dimPaper)
                }
                HStack(spacing: 10) {
                    PixelLabel(
                        text: "FORM \(roman(state.lineage.species_depth))  MUT \(state.lineage.mutations_since_seed)",
                        scale: 2,
                        color: Palette.dimPaper
                    )
                    if state.lineage.archived_relatives > 0 {
                        PixelLabel(text: "IN BOOK \(state.lineage.archived_relatives)", scale: 2, color: Palette.dimPaper)
                    }
                }
                if state.alive {
                    Button(action: onArchive) {
                        PixelPanel { PixelLabel(text: "ARCHIVE", scale: 2, color: Palette.highlight) }
                    }
                    .buttonStyle(.plain)
                    .a11yButton("Archive creature")
                }
            }
        }
        .frame(maxWidth: 300, alignment: .leading)
        .accessibilityElement(children: .contain)
    }

    private var kinSuffix: String {
        state.gluedKin > 0 ? "  GLUED \(state.gluedKin)" : ""
    }

    private var energyBar: some View {
        Canvas { ctx, size in
            ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(Color(white: 0.3)))
            let w = (size.width - 4) * CGFloat(state.energyFrac) / 255
            let colour = state.stress > 160 ? Color(red: 0.9, green: 0.4, blue: 0.3) : Color(red: 0.55, green: 0.85, blue: 0.4)
            ctx.fill(Path(CGRect(x: 2, y: 2, width: w, height: size.height - 4)), with: .color(colour))
        }
        .frame(width: 160, height: 10)
        .accessibilityElement()
        .accessibilityLabel("Energy \(Int(state.energyFrac) * 100 / 255) percent")
    }

    private func seconds(_ ticks: UInt32) -> UInt32 { ticks / 30 }
}

/// Roman numeral for the species depth ("form").
func roman(_ n: UInt16) -> String {
    guard n > 0 else { return "0" }
    let table: [(UInt16, String)] = [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"),
        (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ]
    var n = n
    var out = ""
    for (value, glyph) in table {
        while n >= value {
            out += glyph
            n -= value
        }
    }
    return out
}

/// Needle shaft above the hot spot. The finger is below the tip.
struct NeedleCursor: View {
    var tip: CGPoint

    var body: some View {
        Canvas { ctx, _ in
            var shaft = Path()
            shaft.move(to: CGPoint(x: tip.x, y: tip.y - 34))
            shaft.addLine(to: tip)
            ctx.stroke(shaft, with: .color(Palette.highlight), lineWidth: 2)
            let dot = CGRect(x: tip.x - 2, y: tip.y - 2, width: 4, height: 4)
            ctx.fill(Path(ellipseIn: dot), with: .color(Palette.highlight))
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// Corner-marked rectangle that follows the inspected body.
struct TrackingBox: View {
    let rect: CGRect

    var body: some View {
        Canvas { ctx, _ in
            let r = rect.insetBy(dx: -3, dy: -3)
            let arm: CGFloat = min(8, r.width / 2, r.height / 2)
            var path = Path()
            let corners: [(CGFloat, CGFloat, CGFloat, CGFloat)] = [
                (r.minX, r.minY, 1, 1), (r.maxX, r.minY, -1, 1),
                (r.minX, r.maxY, 1, -1), (r.maxX, r.maxY, -1, -1),
            ]
            for (x, y, sx, sy) in corners {
                path.move(to: CGPoint(x: x + arm * sx, y: y))
                path.addLine(to: CGPoint(x: x, y: y))
                path.addLine(to: CGPoint(x: x, y: y + arm * sy))
            }
            ctx.stroke(path, with: .color(Palette.highlight), lineWidth: 2)
        }
        .allowsHitTesting(false)
    }
}

/// Short pixel toast ("NO ROOM", "ARCHIVED").
struct HintToast: View {
    let text: String

    var body: some View {
        PixelPanel { PixelLabel(text: text, scale: 3, color: Palette.highlight) }
            .transition(.opacity)
            .allowsHitTesting(false)
    }
}
