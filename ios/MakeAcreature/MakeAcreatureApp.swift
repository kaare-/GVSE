import SwiftUI

@main
struct MakeAcreatureApp: App {
    @StateObject private var session = TankSession()
    @Environment(\.scenePhase) private var scenePhase
    @State private var inRoom = false
    @State private var showBook = false
    @State private var showTree = false

    var body: some Scene {
        WindowGroup {
            ZStack {
                Color.black.ignoresSafeArea()
                tankScreen
                    .opacity(inRoom ? 0 : 1)
                    .allowsHitTesting(!inRoom)
                if inRoom {
                    RoomView(
                        session: session,
                        onBack: { withAnimation(.easeOut(duration: 0.2)) { inRoom = false } },
                        onOpenBook: { showBook = true },
                        onOpenTree: { showTree = true }
                    )
                    .transition(.opacity)
                    .simultaneousGesture(zoomGesture(toRoom: false))
                }
            }
            .preferredColorScheme(.dark)
            .statusBarHidden(true)
            .fullScreenCover(isPresented: $showBook) {
                ArchiveBook(session: session) { showBook = false }
            }
            .fullScreenCover(isPresented: $showTree) {
                TreeScreen(session: session) { showTree = false }
            }
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

    /// Pinch out past minimum zoom → the room; pinch in → back to the tank.
    private func zoomGesture(toRoom: Bool) -> some Gesture {
        MagnificationGesture()
            .onEnded { value in
                if toRoom && value < 0.72 {
                    withAnimation(.easeOut(duration: 0.2)) { inRoom = true }
                } else if !toRoom && value > 1.35 {
                    withAnimation(.easeOut(duration: 0.2)) { inRoom = false }
                }
            }
    }

    private var tankScreen: some View {
        ZStack(alignment: .top) {
            TankScreen(session: session)
                .ignoresSafeArea()
            if let rect = session.inspected?.rect {
                TrackingBox(rect: rect).ignoresSafeArea()
            }
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: 4) {
                        PixelPanel {
                            PixelLabel(text: session.isDay ? "DAY" : "NIGHT", scale: 3)
                        }
                        PixelPanel {
                            PixelLabel(
                                text: String(format: "%.0f C", session.temperatureC),
                                scale: 2,
                                color: Palette.highlight
                            )
                        }
                    }
                    Spacer()
                    slotButtons
                    Spacer()
                    VStack(alignment: .trailing, spacing: 4) {
                        Button {
                            withAnimation(.easeOut(duration: 0.2)) { inRoom = true }
                        } label: {
                            PixelLabel(text: "ROOM", scale: 2, color: Palette.highlight)
                        }
                        .buttonStyle(.plain)
                        Button(action: session.toggleAlwaysDay) {
                            PixelLabel(
                                text: session.alwaysDay ? "ALWAYS DAY" : "DAY/NIGHT",
                                scale: 2
                            )
                        }
                        .buttonStyle(.plain)
                        HStack(spacing: 4) {
                            Button { session.nudgeBaseTemp(-1) } label: {
                                PixelLabel(text: "T-", scale: 2)
                            }
                            .buttonStyle(.plain)
                            PixelLabel(text: String(format: "BASE %.0f", session.tBase), scale: 2)
                            Button { session.nudgeBaseTemp(1) } label: {
                                PixelLabel(text: "T+", scale: 2)
                            }
                            .buttonStyle(.plain)
                        }
                        HStack(spacing: 4) {
                            Button { session.nudgeLight(-0.1) } label: {
                                PixelLabel(text: "L-", scale: 2)
                            }
                            .buttonStyle(.plain)
                            PixelLabel(text: String(format: "LIGHT %.0f%%", session.lightSlider * 100), scale: 2)
                            Button { session.nudgeLight(0.1) } label: {
                                PixelLabel(text: "L+", scale: 2)
                            }
                            .buttonStyle(.plain)
                        }
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
                    encloseOp: $session.encloseOp,
                    onUndo: session.undo,
                    onTopUp: session.topUp
                )
                .onChange(of: session.fillLevel) { _ in
                    if session.tool == .fill {
                        session.applyFillSlider()
                    }
                }
                if session.carriedCells > 0 {
                    HStack(spacing: 8) {
                        PixelPanel {
                            PixelLabel(text: "BUCKET \(session.carriedCells) CELLS  TAP TO POUR", scale: 2, color: Palette.highlight)
                        }
                        Button(action: session.discardBucket) {
                            PixelPanel { PixelLabel(text: "DUMP", scale: 2, color: Palette.dimPaper) }
                        }
                        .buttonStyle(.plain)
                    }
                }
                if let state = session.inspected {
                    InspectCard(
                        state: state,
                        onArchive: session.archiveInspected,
                        onClose: session.closeInspect
                    )
                }
                Spacer()
            }
            .padding(10)
            if let hint = session.hint {
                VStack {
                    Spacer()
                    HintToast(text: hint)
                    Spacer().frame(height: 80)
                }
                .animation(.easeOut(duration: 0.2), value: hint)
            }
        }
        .simultaneousGesture(zoomGesture(toRoom: true))
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
                                ? Palette.highlight
                                : Color(red: 0.7, green: 0.68, blue: 0.6)
                        )
                    }
                }
                .buttonStyle(.plain)
            }
            Button { session.newTank() } label: {
                PixelPanel {
                    PixelLabel(text: "NEW", scale: 2)
                }
            }
            .buttonStyle(.plain)
        }
    }
}
