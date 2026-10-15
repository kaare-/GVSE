import SwiftUI

@main
struct MakeAcreatureApp: App {
    @StateObject private var session = TankSession()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ZStack(alignment: .top) {
                Color.black.ignoresSafeArea()
                TankScreen(session: session)
                    .ignoresSafeArea()
                VStack(alignment: .leading, spacing: 8) {
                    HStack(alignment: .top) {
                        PixelPanel {
                            PixelLabel(text: session.isDay ? "DAY" : "NIGHT", scale: 3)
                        }
                        Spacer()
                        slotButtons
                        Spacer()
                        VStack(alignment: .trailing, spacing: 4) {
                            Button(action: session.toggleConsent) {
                                PixelLabel(
                                    text: session.consent ? "STATS ON" : "STATS OFF",
                                    scale: 2
                                )
                            }
                            .buttonStyle(.plain)
                            Button(action: session.toggleSwirl) {
                                PixelLabel(
                                    text: session.swirl ? "SWIRL ON" : "SWIRL OFF",
                                    scale: 2
                                )
                            }
                            .buttonStyle(.plain)
                            Button {
                                session.magnifierPinned.toggle()
                            } label: {
                                PixelLabel(
                                    text: session.magnifierPinned ? "LENS PIN" : "LENS",
                                    scale: 2
                                )
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    ToolWheel(
                        tool: $session.tool,
                        brush: $session.brush,
                        variant: $session.variant,
                        fillLevel: $session.fillLevel,
                        onUndo: session.undo
                    )
                    .onChange(of: session.fillLevel) { _ in
                        if session.tool == .fill {
                            session.applyFillSlider()
                        }
                    }
                }
                .padding(10)
            }
            .preferredColorScheme(.dark)
            .statusBarHidden(true)
            .onAppear { session.resume() }
            .onChange(of: scenePhase) { phase in
                if phase == .active {
                    session.resume()
                } else {
                    session.pauseAndSave()
                }
            }
        }
    }

    private var slotButtons: some View {
        HStack(spacing: 4) {
            ForEach(0..<3, id: \.self) { i in
                Button {
                    session.selectSlot(i)
                } label: {
                    PixelPanel {
                        PixelLabel(
                            text: "T\(i + 1)",
                            scale: 2,
                            color: session.slot == i
                                ? Color(red: 1, green: 0.92, blue: 0.7)
                                : Color(red: 0.7, green: 0.68, blue: 0.6)
                        )
                    }
                }
                .buttonStyle(.plain)
            }
            Button(action: session.newTank) {
                PixelPanel {
                    PixelLabel(text: "NEW", scale: 2)
                }
            }
            .buttonStyle(.plain)
        }
    }
}
