import SwiftUI

@main
struct MakeAcreatureApp: App {
    @StateObject private var session = TankSession()
    @Environment(\.scenePhase) private var scenePhase
    @State private var inRoom = false
    @State private var showBook = false
    @State private var showTree = false
    @State private var showSettings = false
    @State private var showPhoto = false
    @State private var showLog = false

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
                        onOpenTree: { showTree = true },
                        onOpenSettings: { showSettings = true },
                        onOpenLog: { showLog = true }
                    )
                    .transition(.opacity)
                    .simultaneousGesture(zoomGesture(toRoom: false))
                }
                if session.showStartGuide {
                    StartGuide(
                        onSkip: { session.dismissStartGuide(skipped: true) },
                        onDone: { session.dismissStartGuide(skipped: false) }
                    )
                    .zIndex(20)
                }
                if showPhoto {
                    PhotoModeOverlay(session: session) { showPhoto = false }
                        .zIndex(15)
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
            .fullScreenCover(isPresented: $showSettings) {
                FuseBoxSettings(session: session) { showSettings = false }
            }
            .fullScreenCover(isPresented: $showLog) {
                EventLogView(session: session) { showLog = false }
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
        VStack(spacing: 0) {
            topMenu
            ZStack {
                TankScreen(session: session)
                if let rect = session.inspected?.rect {
                    TrackingBox(rect: rect)
                }
                if let rect = session.selectionRect {
                    SelectionBox(rect: rect)
                }
                if let state = session.inspected {
                    VStack {
                        InspectCard(
                            state: state,
                            onArchive: session.archiveInspected,
                            onClose: session.closeInspect
                        )
                        Spacer(minLength: 0)
                    }
                    .padding(6)
                }
                if session.carriedCells > 0 {
                    VStack {
                        Spacer(minLength: 0)
                        HStack(spacing: 6) {
                            PixelPanel {
                                PixelLabel(text: "SAMPLE \(session.carriedCells)", scale: 1, color: Palette.highlight)
                            }
                            Button(action: session.discardBucket) {
                                PixelPanel { PixelLabel(text: "DUMP", scale: 1, color: Palette.dimPaper) }
                            }
                            .buttonStyle(.plain)
                            .a11yButton("Dump sample", hint: "Throw away what you sampled")
                        }
                        .padding(.bottom, 6)
                    }
                }
                if let hint = session.hint {
                    VStack {
                        Spacer()
                        HintToast(text: hint)
                        Spacer().frame(height: 24)
                    }
                    .animation(.easeOut(duration: 0.2), value: hint)
                    .accessibilityElement(children: .combine)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .clipped()
            .background(Palette.panel)
            bottomBar
        }
        .background(Palette.panel)
        .simultaneousGesture(zoomGesture(toRoom: true))
    }

    private var clockLabel: String {
        "\(session.isDay ? "DAY" : "NIGHT") \(session.dayNumber)"
    }

    private var topMenu: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .center, spacing: 6) {
                PixelLabel(text: clockLabel, scale: 1)
                    .accessibilityLabel("\(session.isDay ? "Day" : "Night") \(session.dayNumber)")
                PixelLabel(
                    text: String(format: "%.0f C", session.temperatureC),
                    scale: 1,
                    color: Palette.highlight
                )
                .accessibilityLabel(String(format: "Tank temperature %.0f degrees", session.temperatureC))
                Spacer(minLength: 4)
                textButton("ROOM", color: Palette.highlight) {
                    withAnimation(.easeOut(duration: 0.2)) { inRoom = true }
                }
                .a11yButton("Room", hint: "Leave the tank and open the room")
                textButton("PHOTO") { showPhoto = true }
                    .a11yButton("Photo", hint: "Save a picture of the tank")
            }
            ToolWheel(
                tool: $session.tool,
                brush: $session.brush,
                variant: $session.variant,
                strokeMode: $session.strokeMode
            )
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Palette.panel)
        .overlay(alignment: .bottom) {
            Rectangle().fill(Color(white: 0.35)).frame(height: 1)
        }
    }

    private var bottomBar: some View {
        HStack(spacing: 3) {
            barButton("PAUSE", icon: BarIcon.pause, on: !session.playing, action: session.pause)
            barButton("PLAY", icon: BarIcon.play, on: session.playing, action: session.unpauseFromEvent)
            barButton("LOOK", icon: BuildTool.look.icon, on: session.tool == .look) { session.tool = .look }
            barButton("SAMPLE", icon: BuildTool.sample.icon, on: session.tool == .sample) { session.tool = .sample }
            barButton("NEEDLE", icon: BuildTool.needle.icon, on: session.tool == .needle) { session.tool = .needle }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity)
        .background(Palette.panel)
        .overlay(alignment: .top) {
            Rectangle().fill(Color(white: 0.35)).frame(height: 1)
        }
    }

    private func textButton(_ title: String, color: Color = Palette.paper, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            PixelLabel(text: title, scale: 1, color: color)
                .padding(.horizontal, 4)
                .padding(.vertical, 3)
                .background(Color(white: 0.1))
                .overlay(Rectangle().stroke(Color(white: 0.35), lineWidth: 1))
        }
        .buttonStyle(.plain)
    }

    private func barButton(_ title: String, icon: [String], on: Bool, action: @escaping () -> Void) -> some View {
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
            .padding(.vertical, 2)
            .background(on ? Color(white: 0.2) : Color(white: 0.08))
            .overlay(Rectangle().stroke(on ? Palette.highlight : Color(white: 0.28), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .a11yButton(title, selected: on)
    }
}
