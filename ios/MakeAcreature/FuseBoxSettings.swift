import SwiftUI

/// Settings live in the room's fuse box (Phase 6) — not on the tank HUD.
struct FuseBoxSettings: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void
    @State private var confirmReset = false

    var body: some View {
        ZStack {
            Color.black.opacity(0.55).ignoresSafeArea().onTapGesture(perform: onClose)
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    PixelLabel(text: "FUSE BOX", scale: 3, color: Palette.highlight)
                    Spacer()
                    Button(action: onClose) {
                        PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                    }
                    .buttonStyle(.plain)
                }
                PixelLabel(text: "THE ROOM'S BREAKERS", scale: 1, color: Palette.dimPaper)

                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        row("SWIRL", session.swirl ? "ON" : "OFF", session.toggleSwirl)
                        row("PAUSE ON EVENT", session.pauseOnEvent ? "ON" : "OFF", session.togglePauseOnEvent)
                        row("SOUND", session.soundOn ? "ON" : "OFF", session.toggleSound)
                        row("HAPTICS", session.hapticsOn ? "ON" : "OFF", session.toggleHaptics)
                        row("CLOCK", session.alwaysDay ? "ALWAYS DAY" : "DAY/NIGHT", session.toggleAlwaysDay)
                        HStack(spacing: 6) {
                            PixelLabel(text: "BASE TEMP", scale: 2, color: Palette.dimPaper)
                            Spacer()
                            Button { session.nudgeBaseTemp(-1) } label: {
                                PixelPanel { PixelLabel(text: "-", scale: 2) }
                            }
                            .buttonStyle(.plain)
                            PixelLabel(text: String(format: "%.0f C", session.tBase), scale: 2)
                            Button { session.nudgeBaseTemp(1) } label: {
                                PixelPanel { PixelLabel(text: "+", scale: 2) }
                            }
                            .buttonStyle(.plain)
                        }
                        HStack(spacing: 6) {
                            PixelLabel(text: "LIGHT", scale: 2, color: Palette.dimPaper)
                            Spacer()
                            Button { session.nudgeLight(-0.1) } label: {
                                PixelPanel { PixelLabel(text: "-", scale: 2) }
                            }
                            .buttonStyle(.plain)
                            PixelLabel(text: String(format: "%.0f%%", session.lightSlider * 100), scale: 2)
                            Button { session.nudgeLight(0.1) } label: {
                                PixelPanel { PixelLabel(text: "+", scale: 2) }
                            }
                            .buttonStyle(.plain)
                        }
                        row("STATS", session.consent ? "ON" : "OFF", session.toggleConsent)
                        Button { confirmReset = true } label: {
                            PixelPanel { PixelLabel(text: "RESET THIS TANK", scale: 2, color: Palette.dimPaper) }
                        }
                        .buttonStyle(.plain)
                        PixelLabel(
                            text: "RESET CLEARS THE ACTIVE SLOT. ARCHIVE IS KEPT.",
                            scale: 1,
                            color: Palette.dimPaper
                        )
                        .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .padding(14)
            .frame(maxWidth: 340, maxHeight: 480)
            .background(Palette.panel)
            .overlay(Rectangle().stroke(Color(white: 0.35), lineWidth: 3))
        }
        .confirmationDialog("Wipe tank \(session.slot + 1) and start empty?", isPresented: $confirmReset, titleVisibility: .visible) {
            Button("Reset tank", role: .destructive) {
                session.newTank()
                onClose()
            }
        }
        .preferredColorScheme(.dark)
    }

    private func row(_ title: String, _ value: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                PixelLabel(text: title, scale: 2, color: Palette.dimPaper)
                Spacer()
                PixelLabel(text: value, scale: 2, color: Palette.highlight)
            }
            .padding(.vertical, 4)
            .padding(.horizontal, 6)
            .background(Color(white: 0.12))
        }
        .buttonStyle(.plain)
    }
}
