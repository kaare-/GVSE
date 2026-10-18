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
                    VStack(alignment: .leading, spacing: 0) {
                        InspectCard(
                            state: state,
                            onArchive: session.archiveInspected,
                            onClose: session.closeInspect
                        )
                        .padding(6)
                        .offset(x: cardHome.width + cardDrag.width, y: cardHome.height + cardDrag.height)
                        .gesture(
                            DragGesture(minimumDistance: 12)
                                .onChanged { cardDrag = $0.translation }
                                .onEnded { value in
                                    cardHome.width = min(max(cardHome.width + value.translation.width, -24), 180)
                                    cardHome.height = min(max(cardHome.height + value.translation.height, -8), 520)
                                    cardDrag = .zero
                                }
                        )
                        Spacer(minLength: 0)
                    }
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
    }

    @State private var cardHome = CGSize.zero
    @State private var cardDrag = CGSize.zero

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

    private var inspecting: Bool {
        session.tool == .needle || session.tool == .sample
    }

    private var bottomBar: some View {
        VStack(spacing: 0) {
            if inspecting {
                HStack(spacing: 3) {
                    modeChip("INSPECT", on: session.tool == .needle) { session.tool = .needle }
                    modeChip("SCOOP", on: session.tool == .sample) { session.tool = .sample }
                }
                .padding(.horizontal, 6)
                .padding(.top, 4)
            }
            HStack(spacing: 3) {
                barButton(
                    session.playing ? "PAUSE" : "PLAY",
                    icon: session.playing ? BarIcon.pause : BarIcon.play,
                    on: session.playing
                ) {
                    if session.playing { session.pause() } else { session.unpauseFromEvent() }
                }
                barButton("INSPECT", icon: BuildTool.needle.icon, on: inspecting) {
                    session.tool = inspecting ? .none : .needle
                }
                barButton("ROOM", icon: BarIcon.room, on: false) {
                    withAnimation(.easeOut(duration: 0.2)) { inRoom = true }
                }
                barButton("PHOTO", icon: BarIcon.photo, on: false) {
                    showPhoto = true
                }
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 4)
        }
        .frame(maxWidth: .infinity)
        .background(Palette.panel)
        .overlay(alignment: .top) {
            Rectangle().fill(Color(white: 0.35)).frame(height: 1)
        }
    }

    private func modeChip(_ title: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            PixelLabel(text: title, scale: 1, color: on ? Palette.highlight : Palette.dimPaper)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 4)
                .background(on ? Color(white: 0.18) : Color(white: 0.08))
                .overlay(Rectangle().stroke(on ? Palette.highlight : Color(white: 0.28), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .a11yButton(title, selected: on)
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
