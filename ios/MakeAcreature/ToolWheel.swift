import SwiftUI

enum BuildTool: Int, CaseIterable {
    case sand, rock, erase, pour, drain, fill, undo

    var label: String {
        switch self {
        case .sand: return "SAND"
        case .rock: return "ROCK"
        case .erase: return "DIG"
        case .pour: return "POUR"
        case .drain: return "DRAIN"
        case .fill: return "FILL"
        case .undo: return "UNDO"
        }
    }

    var kind: UInt8 {
        switch self {
        case .sand, .rock: return 1
        case .erase: return 2
        case .pour: return 3
        case .drain: return 4
        case .fill: return 5
        case .undo: return 6
        }
    }

    var material: UInt8 {
        switch self {
        case .sand: return 2 // Sand
        case .rock: return 1 // Stone
        default: return 0
        }
    }
}

struct ToolWheel: View {
    @Binding var tool: BuildTool
    @Binding var brush: Int
    @Binding var variant: Int
    @Binding var fillLevel: Double
    var onUndo: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 4) {
                ForEach(BuildTool.allCases, id: \.rawValue) { t in
                    Button {
                        if t == .undo {
                            onUndo()
                        } else {
                            tool = t
                        }
                    } label: {
                        PixelPanel {
                            PixelLabel(
                                text: t.label,
                                scale: 2,
                                color: tool == t
                                    ? Color(red: 1, green: 0.92, blue: 0.7)
                                    : Color(red: 0.75, green: 0.72, blue: 0.65)
                            )
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
            HStack(spacing: 8) {
                PixelLabel(text: "BRUSH \(brush)", scale: 2)
                Button {
                    brush = max(1, brush - 1)
                } label: { PixelLabel(text: "-", scale: 3) }
                .buttonStyle(.plain)
                Button {
                    brush = min(12, brush + 1)
                } label: { PixelLabel(text: "+", scale: 3) }
                .buttonStyle(.plain)
                PixelLabel(text: "TONE \(variant + 1)", scale: 2)
                Button {
                    variant = (variant + 1) % 4
                } label: { PixelLabel(text: ">", scale: 3) }
                .buttonStyle(.plain)
            }
            if tool == .fill {
                HStack {
                    PixelLabel(text: "LEVEL", scale: 2)
                    Slider(value: $fillLevel, in: 0...1)
                        .tint(Color(red: 0.4, green: 0.7, blue: 0.9))
                }
                .frame(maxWidth: 220)
            }
        }
        .padding(6)
    }
}
