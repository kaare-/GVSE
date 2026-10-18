import SwiftUI

/// What the brush lays down or lifts. Look, sample, and the needle live
/// on the bottom bar; they are not strokes.
enum BuildTool: Int, CaseIterable {
    case sand, rock, sugar, lime, mulch, water, seed, clean
    case look, sample, needle

    var label: String {
        switch self {
        case .sand: return "SAND"
        case .rock: return "ROCK"
        case .sugar: return "SUGAR"
        case .lime: return "LIME"
        case .mulch: return "MULCH"
        case .water: return "WATER"
        case .seed: return "SEED"
        case .clean: return "CLEAN"
        case .look: return "LOOK"
        case .sample: return "SAMPLE"
        case .needle: return "NEEDLE"
        }
    }

    static let materials: [BuildTool] = [.sand, .rock, .sugar, .lime, .mulch, .water, .seed, .clean]

    var icon: [String] {
        switch self {
        case .sand:
            return [
                "        ",
                "        ",
                "   ##   ",
                "  ####  ",
                " ###### ",
                "########",
                " ###### ",
                "        ",
            ]
        case .rock:
            return [
                "        ",
                "  ####  ",
                " ###### ",
                "########",
                " ###### ",
                "  ####  ",
                "   ##   ",
                "        ",
            ]
        case .sugar:
            return [
                " #  #   ",
                "  #   # ",
                " #  #   ",
                "   #  # ",
                " #    # ",
                "  #  #  ",
                " #   #  ",
                "        ",
            ]
        case .lime:
            return [
                "        ",
                "   ##   ",
                "  #  #  ",
                " #    # ",
                " # ## # ",
                "  #  #  ",
                " ###### ",
                "        ",
            ]
        case .mulch:
            return [
                " # # # #",
                "# # # # ",
                " # # # #",
                "# # # # ",
                " # # # #",
                "# # # # ",
                "        ",
                "        ",
            ]
        case .water:
            return [
                "        ",
                "##  ##  ",
                "  ##  ##",
                "##  ##  ",
                "  ##  ##",
                "##  ##  ",
                "        ",
                "        ",
            ]
        case .seed:
            return [
                "   #    ",
                "   #    ",
                "  # #   ",
                " #   #  ",
                "   #    ",
                "   #    ",
                "  ###   ",
                "        ",
            ]
        case .clean:
            return [
                "      # ",
                "     ## ",
                "    ##  ",
                "   ##   ",
                "  ##    ",
                " ##     ",
                "#       ",
                "        ",
            ]
        case .look:
            return [
                "  ####  ",
                " #    # ",
                "#  ##  #",
                "# #  # #",
                "#  ##  #",
                " #    # ",
                "  ####  ",
                "        ",
            ]
        case .sample:
            return [
                "  #  #  ",
                "  #  #  ",
                "  ####  ",
                " #    # ",
                " # ## # ",
                " # ## # ",
                "  ####  ",
                "        ",
            ]
        case .needle:
            return [
                "#       ",
                " #      ",
                "  #     ",
                "   #    ",
                "    #   ",
                "     #  ",
                "      # ",
                "       #",
            ]
        }
    }

    /// Sugar, lime, and mulch are dissolved feeds. The fingertip light stays off.
    var isFeed: Bool {
        self == .sugar || self == .lime || self == .mulch
    }

    var isStroke: Bool {
        switch self {
        case .look, .sample, .needle: return false
        default: return true
        }
    }

    /// Kind and material for one dab. `hint` is set when remove has nothing
    /// to lift (sugar and lime are pours).
    func stroke(_ mode: StrokeMode) -> (kind: UInt8, material: UInt8, hint: String?) {
        switch self {
        case .sand:
            return mode == .add ? (1, 2, nil) : (2, 2, nil)
        case .rock:
            return mode == .add ? (1, 1, nil) : (2, 1, nil)
        case .sugar:
            return mode == .add ? (10, 0, nil) : (0, 0, "POUR ONLY")
        case .lime:
            return mode == .add ? (11, 0, nil) : (0, 0, "POUR ONLY")
        case .mulch:
            // Add drops litter. Remove lifts organic sediment only.
            return mode == .add ? (12, 0, nil) : (2, 4, nil)
        case .water:
            return mode == .add ? (3, 0, nil) : (4, 0, nil)
        case .seed:
            return (8, 0, nil)
        case .clean:
            return (13, 0, nil)
        case .look, .sample, .needle:
            return (0, 0, nil)
        }
    }
}

enum StrokeMode: Int {
    case add, remove

    var label: String { self == .add ? "ADD" : "REMOVE" }
}

struct PixelIcon: View {
    var rows: [String]
    var ink: Color
    var scale: CGFloat = 2

    var body: some View {
        Canvas { ctx, _ in
            for (y, row) in rows.enumerated() {
                for (x, ch) in row.enumerated() where ch != " " {
                    let rect = CGRect(
                        x: CGFloat(x) * scale,
                        y: CGFloat(y) * scale,
                        width: scale,
                        height: scale
                    )
                    ctx.fill(Path(rect), with: .color(ink))
                }
            }
        }
        .frame(width: 8 * scale, height: 8 * scale)
    }
}

struct ToolWheel: View {
    @Binding var tool: BuildTool
    @Binding var brush: Int
    @Binding var variant: Int
    @Binding var strokeMode: StrokeMode

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 3), count: 4), spacing: 3) {
                ForEach(BuildTool.materials, id: \.rawValue) { t in
                    iconButton(t.label, icon: t.icon, on: tool == t) { tool = t }
                }
            }
            HStack(spacing: 6) {
                iconButton("ADD", icon: StrokeMode.add.mark, on: strokeMode == .add) {
                    strokeMode = .add
                }
                iconButton("REMOVE", icon: StrokeMode.remove.mark, on: strokeMode == .remove) {
                    strokeMode = .remove
                }
                if tool == .sand || tool == .rock {
                    Button {
                        variant = (variant + 1) % 4
                    } label: {
                        PixelLabel(text: "TONE \(variant + 1)", scale: 1, color: Palette.highlight)
                            .padding(.horizontal, 4)
                            .padding(.vertical, 6)
                            .background(Color(white: 0.12))
                            .overlay(Rectangle().stroke(Color(white: 0.4), lineWidth: 1))
                    }
                    .buttonStyle(.plain)
                    .a11yButton("Tone \(variant + 1)", hint: "Next colour tone")
                }
            }
            VStack(alignment: .leading, spacing: 1) {
                PixelLabel(text: "SIZE \(brush)", scale: 1, color: Palette.highlight)
                Slider(
                    value: Binding(
                        get: { Double(brush) },
                        set: { brush = Int($0.rounded()) }
                    ),
                    in: 1...12,
                    step: 1
                )
                .tint(Color(red: 0.95, green: 0.82, blue: 0.45))
                .accessibilityLabel("Tool size")
            }
        }
    }

    private func iconButton(_ title: String, icon: [String], on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 1) {
                PixelIcon(rows: icon, ink: on ? Palette.highlight : Color(red: 0.72, green: 0.68, blue: 0.58), scale: 2)
                PixelLabel(
                    text: title,
                    scale: 1,
                    color: on ? Palette.highlight : Color(red: 0.72, green: 0.68, blue: 0.58)
                )
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 3)
            .background(on ? Color(white: 0.22) : Color(white: 0.08))
            .overlay(Rectangle().stroke(on ? Palette.highlight : Color(white: 0.32), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .a11yButton(title, selected: on)
    }
}

extension StrokeMode {
    var mark: [String] {
        switch self {
        case .add:
            return [
                "        ",
                "   ##   ",
                "   ##   ",
                " ###### ",
                " ###### ",
                "   ##   ",
                "   ##   ",
                "        ",
            ]
        case .remove:
            return [
                "        ",
                "        ",
                "        ",
                " ###### ",
                " ###### ",
                "        ",
                "        ",
                "        ",
            ]
        }
    }
}

enum BarIcon {
    static let pause = [
        "        ",
        " ##  ## ",
        " ##  ## ",
        " ##  ## ",
        " ##  ## ",
        " ##  ## ",
        "        ",
        "        ",
    ]
    static let play = [
        "        ",
        " ##     ",
        " ####   ",
        " ###### ",
        " ###### ",
        " ####   ",
        " ##     ",
        "        ",
    ]
    static let undo = [
        "  ####  ",
        " #      ",
        "#       ",
        "#  ###  ",
        "#    #  ",
        " #   #  ",
        "  ####  ",
        "        ",
    ]
}
