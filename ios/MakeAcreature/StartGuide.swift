import SwiftUI

/// Skippable first-tank guide (§2.1 #10): build → water → algae → look → feed.
struct StartGuide: View {
    var onSkip: () -> Void
    var onDone: () -> Void

    @State private var step = 0

    private let steps: [(String, String)] = [
        ("BUILD", "PAINT SAND AND ROCK ON THE FLOOR. USE THE TOOL WHEEL."),
        ("WATER", "TAP FILL OR POUR. SET THE LEVEL WITH THE SLIDER."),
        ("ALGAE", "LIFE SEEDS ITSELF — OR ARCHIVE A STARTER FROM THE BOOK."),
        ("LOOK", "HOLD LOOK ON A CREATURE. THE CARD FOLLOWS IT."),
        ("FEED", "SUGAR, LIME, MULCH. REMOVE CLEARS WHAT YOU DON'T WANT."),
        ("ROOM", "PINCH OUT OR TAP ROOM. THE BOOK AND TREE LIVE THERE."),
    ]

    var body: some View {
        ZStack {
            Color.black.opacity(0.65).ignoresSafeArea()
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    PixelLabel(text: "START GUIDE", scale: 3, color: Palette.highlight)
                    Spacer()
                    Button(action: onSkip) {
                        PixelPanel { PixelLabel(text: "SKIP", scale: 2, color: Palette.paper) }
                    }
                    .buttonStyle(.plain)
                    .a11yButton("Skip start guide")
                }
                PixelLabel(text: "STEP \(step + 1) OF \(steps.count)", scale: 1, color: Palette.dimPaper)
                PixelLabel(text: steps[step].0, scale: 3, color: Palette.highlight)
                PixelLabel(text: steps[step].1, scale: 2, maxWidth: 320)
                HStack {
                    if step > 0 {
                        Button { step -= 1 } label: {
                            PixelPanel { PixelLabel(text: "< BACK", scale: 2) }
                        }
                        .buttonStyle(.plain)
                    }
                    Spacer()
                    Button {
                        if step + 1 >= steps.count {
                            onDone()
                        } else {
                            step += 1
                        }
                    } label: {
                        PixelPanel {
                            PixelLabel(
                                text: step + 1 >= steps.count ? "DONE" : "NEXT >",
                                scale: 2,
                                color: Palette.highlight
                            )
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(16)
            .frame(maxWidth: 360)
            .background(Palette.panel)
            .overlay(Rectangle().stroke(Color(white: 0.4), lineWidth: 3))
        }
        .preferredColorScheme(.dark)
    }
}

/// One-shot tip after a milestone toast (first events).
enum GuideTip {
    static func contextual(kind: UInt32) -> String? {
        switch kind {
        case 32: return "TIP: HOLD LOOK ON IT"
        case 36: return "TIP: ARCHIVE FROM THE CARD"
        case 40: return "TIP: PREDATION — NOTES IN THE BOOK"
        case 43, 50: return "TIP: FOLLOW IT WITH THE LENS"
        case 49: return "TIP: AN EYE — SHINE THE FLASHLIGHT"
        default: return nil
        }
    }
}
