import SwiftUI

enum BuildTool: Int, CaseIterable {
    case sand, rock, glass, erase, pour, drain, fill, undo
    case look, scoop, remove, clean, feed, calcium, detritus
    case topUp, cut, enclose

    var label: String {
        switch self {
        case .sand: return "SAND"
        case .rock: return "ROCK"
        case .glass: return "GLASS"
        case .erase: return "DIG"
        case .pour: return "POUR"
        case .drain: return "DRAIN"
        case .fill: return "FILL"
        case .undo: return "UNDO"
        case .look: return "LOOK"
        case .scoop: return "SCOOP"
        case .remove: return "TAKE"
        case .clean: return "CLEAN"
        case .feed: return "SUGAR"
        case .calcium: return "LIME"
        case .detritus: return "MULCH"
        case .topUp: return "TOP UP"
        case .cut: return "CUT"
        case .enclose: return "BOX"
        }
    }

    /// Build tools on the first row, the selection loop on the second,
    /// Phase 4 land / enclosure tools on the third.
    static let buildRow: [BuildTool] = [.sand, .rock, .glass, .erase, .pour, .drain, .fill, .undo]
    static let lifeRow: [BuildTool] = [.look, .scoop, .remove, .clean, .feed, .calcium, .detritus]
    static let landRow: [BuildTool] = [.topUp, .cut, .enclose]

    /// Tool event kind (`tank.h`); 0 for tools that are not strokes.
    var kind: UInt8 {
        switch self {
        case .sand, .rock, .glass: return 1
        case .erase: return 2
        case .pour: return 3
        case .drain: return 4
        case .fill: return 5
        case .undo: return 6
        case .remove: return 9
        case .feed: return 10
        case .calcium: return 11
        case .detritus: return 12
        case .clean: return 13
        case .topUp: return 14
        case .cut: return 15
        case .enclose: return 16
        case .look, .scoop: return 0
        }
    }

    var material: UInt8 {
        switch self {
        case .sand: return 2 // Sand
        case .rock: return 1 // Stone
        case .glass: return UInt8(TANK_GLASS)
        default: return 0
        }
    }

    /// Feeding tools switch the fingertip flashlight off (hold = pour).
    var isFeed: Bool {
        self == .feed || self == .calcium || self == .detritus
    }
}

struct ToolWheel: View {
    @Binding var tool: BuildTool
    @Binding var brush: Int
    @Binding var variant: Int
    @Binding var fillLevel: Double
    /// Enclosure op when `tool == .enclose`: 0 walls, 1 drain, 2 fill, 3 release.
    @Binding var encloseOp: Int
    var onUndo: () -> Void
    var onTopUp: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            row(BuildTool.buildRow)
            row(BuildTool.lifeRow)
            row(BuildTool.landRow)
            HStack(spacing: 8) {
                PixelLabel(text: tool == .scoop ? "BUCKET \(brush)" : "BRUSH \(brush)", scale: 2)
                Button {
                    brush = max(1, brush - 1)
                } label: { PixelLabel(text: "-", scale: 3) }
                .buttonStyle(.plain)
                Button {
                    brush = min(12, brush + 1)
                } label: { PixelLabel(text: "+", scale: 3) }
                .buttonStyle(.plain)
                if tool == .sand || tool == .rock {
                    PixelLabel(text: "TONE \(variant + 1)", scale: 2)
                    Button {
                        variant = (variant + 1) % 4
                    } label: { PixelLabel(text: ">", scale: 3) }
                    .buttonStyle(.plain)
                }
                if tool == .enclose {
                    PixelLabel(text: encloseLabel, scale: 2)
                    Button {
                        encloseOp = (encloseOp + 1) % 4
                    } label: { PixelLabel(text: ">", scale: 3) }
                    .buttonStyle(.plain)
                }
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

    private var encloseLabel: String {
        switch encloseOp {
        case 1: return "DRAIN"
        case 2: return "FILL"
        case 3: return "FREE"
        default: return "WALLS"
        }
    }

    /// Rows scroll sideways on narrow phones; the selected tool stays put.
    private func row(_ tools: [BuildTool]) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 4) {
                ForEach(tools, id: \.rawValue) { t in
                    Button {
                        if t == .undo {
                            onUndo()
                        } else if t == .topUp {
                            tool = t
                            onTopUp()
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
        }
    }
}
