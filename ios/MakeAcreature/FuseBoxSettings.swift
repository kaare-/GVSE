import SwiftUI

/// Settings, opened from the fuse box in the room.
struct FuseBoxSettings: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void
    @State private var confirmGame = false

    var body: some View {
        ZStack {
            Color.black.opacity(0.55).ignoresSafeArea().onTapGesture(perform: onClose)
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    PixelLabel(text: "SETTINGS", scale: 3, color: Palette.highlight)
                    Spacer()
                    Button(action: onClose) {
                        PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                    }
                    .buttonStyle(.plain)
                    .a11yButton("Close settings")
                }
                PixelLabel(text: "THE ROOM", scale: 1, color: Palette.dimPaper)

                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        row(
                            "SWIRL",
                            session.effectiveSwirl ? "ON" : (session.reduceMotion ? "OFF · MOTION" : "OFF"),
                            session.toggleSwirl,
                            hint: session.reduceMotion
                                ? "Reduce Motion is on, so swirl stays off"
                                : "Tilt the tank to stir the water"
                        )
                        row("PAUSE ON EVENT", session.pauseOnEvent ? "ON" : "OFF", session.togglePauseOnEvent)
                        row("SOUND", session.soundOn ? "ON" : "OFF", session.toggleSound)
                        row("HAPTICS", session.hapticsOn ? "ON" : "OFF", session.toggleHaptics)
                        row("CLOCK", session.alwaysDay ? "ALWAYS DAY" : "DAY/NIGHT", session.toggleAlwaysDay)
                        HStack(spacing: 6) {
                            PixelLabel(text: "BASE TEMP", scale: 2, color: Palette.paper)
                            Spacer()
                            Button { session.nudgeBaseTemp(-1) } label: {
                                PixelPanel { PixelLabel(text: "-", scale: 2) }
                            }
                            .buttonStyle(.plain)
                            .a11yButton("Lower base temperature")
                            PixelLabel(text: String(format: "%.0f C", session.tBase), scale: 2)
                            Button { session.nudgeBaseTemp(1) } label: {
                                PixelPanel { PixelLabel(text: "+", scale: 2) }
                            }
                            .buttonStyle(.plain)
                            .a11yButton("Raise base temperature")
                        }
                        HStack(spacing: 6) {
                            PixelLabel(text: "LIGHT", scale: 2, color: Palette.paper)
                            Spacer()
                            Button { session.nudgeLight(-0.1) } label: {
                                PixelPanel { PixelLabel(text: "-", scale: 2) }
                            }
                            .buttonStyle(.plain)
                            .a11yButton("Dim the light")
                            PixelLabel(text: String(format: "%.0f%%", session.lightSlider * 100), scale: 2)
                            Button { session.nudgeLight(0.1) } label: {
                                PixelPanel { PixelLabel(text: "+", scale: 2) }
                            }
                            .buttonStyle(.plain)
                            .a11yButton("Brighten the light")
                        }
                        row(
                            "STATS",
                            session.consent ? "ON" : "OFF",
                            session.toggleConsent,
                            hint: "Telemetry stays off until this is on"
                        )
                        Button { confirmGame = true } label: {
                            PixelPanel { PixelLabel(text: "RESET GAME", scale: 2, color: Palette.paper) }
                        }
                        .buttonStyle(.plain)
                        .a11yButton("Reset game", hint: "Empty every tank and restore the default settings")
                        PixelLabel(
                            text: "STARTS OVER. THE BOOK STAYS. A SINGLE TANK IS EMPTIED FROM ITS VIAL.",
                            scale: 1,
                            color: Palette.dimPaper,
                            maxWidth: 280
                        )
                        .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .padding(14)
            .frame(maxWidth: 340, maxHeight: 480)
            .background(Palette.panel)
            .overlay(Rectangle().stroke(Color(white: 0.78), lineWidth: 3))
        }
        .confirmationDialog(
            "Start from the beginning? All three tanks empty and the settings return to their defaults. The book stays.",
            isPresented: $confirmGame,
            titleVisibility: .visible
        ) {
            Button("Reset game", role: .destructive) {
                session.resetGame()
                onClose()
            }
        }
        .preferredColorScheme(.dark)
    }

    private func row(_ title: String, _ value: String, _ action: @escaping () -> Void, hint: String? = nil) -> some View {
        Button(action: action) {
            HStack {
                PixelLabel(text: title, scale: 2, color: Palette.paper)
                Spacer()
                PixelLabel(text: value, scale: 2, color: Palette.highlight)
            }
            .padding(.vertical, 4)
            .padding(.horizontal, 6)
            .background(Color(white: 0.18))
            .overlay(Rectangle().stroke(Color(white: 0.55), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .a11yButton("\(title), \(value)", hint: hint)
    }
}
