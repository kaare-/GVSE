import SwiftUI

/// The cassette desk, over the lobby. The room stays visible around it.
/// Left to right on the shelf is the playlist. Record opens the eurorack page.
struct CassetteDeck: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void

    @State private var pulled: [UUID] = []
    @State private var showRack = false
    @State private var dragID: UUID?
    @State private var dragX: CGFloat = 0
    /// Tape currently seated in the player for preview.
    @State private var seated: UUID?

    private let slotW: CGFloat = 32

    private var seatedTape: ShelfTape? {
        guard let seated else { return nil }
        return session.shelf.first { $0.id == seated }
    }

    var body: some View {
        ZStack {
            Color.black.opacity(0.28)
                .ignoresSafeArea()
                .onTapGesture(perform: closeDesk)
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    PixelLabel(text: "SHELF", scale: 2, color: Palette.highlight)
                    Spacer()
                    Button(action: session.toggleDeck) {
                        PixelPanel {
                            PixelLabel(
                                text: session.tapePlaying ? "ON" : "OFF",
                                scale: 2,
                                color: session.tapePlaying
                                    ? Color(red: 0.22, green: 0.72, blue: 0.28)
                                    : Color(red: 0.78, green: 0.16, blue: 0.14)
                            )
                        }
                    }
                    .buttonStyle(.plain)
                    .a11yButton(session.tapePlaying ? "Deck power, on" : "Deck power, off")
                    Button(action: closeDesk) {
                        PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                    }
                    .buttonStyle(.plain)
                }
                PixelLabel(text: "TAP TO PULL  DRAG TO REORDER", scale: 1, color: Palette.dimPaper)
                shelf
                pile
                player
                Spacer(minLength: 0)
            }
            .padding(16)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(Color(red: 0.16, green: 0.11, blue: 0.08).opacity(0.94))
            .padding(.horizontal, 28)
            .padding(.vertical, 48)
        }
        .preferredColorScheme(.dark)
        .onAppear {
            // Clear any stuck rack sine from a previous session.
            session.endSolo()
            if seated == nil, session.shelf.indices.contains(session.tape) {
                seated = session.shelf[session.tape].id
            }
        }
        .fullScreenCover(isPresented: $showRack) {
            EurorackStudio(session: session) {
                showRack = false
                session.endSolo()
            }
        }
        .onChange(of: showRack) { open in
            // Only restore playlist when the rack closes. Starting solo is
            // EurorackStudio.onAppear's job — never leave a bare sine running here.
            if !open { session.endSolo() }
        }
    }

    private var shelf: some View {
        GeometryReader { geo in
            let tapes = session.shelf
            ZStack {
                ForEach(Array(tapes.enumerated()), id: \.element.id) { index, tape in
                    let baseX = CGFloat(index) * slotW + slotW / 2
                    let x = dragID == tape.id ? dragX : baseX
                    shelfTape(tape, index: index)
                        .position(x: x, y: 43)
                        .zIndex(dragID == tape.id ? 10 : Double(index))
                        .onTapGesture { pull(tape.id) }
                        .gesture(shelfDrag(tape: tape, width: geo.size.width))
                }
            }
            .frame(width: geo.size.width, height: 86, alignment: .leading)
            .coordinateSpace(name: "shelf")
        }
        .frame(height: 86)
    }

    private func shelfDrag(tape: ShelfTape, width: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 16, coordinateSpace: .named("shelf"))
            .onChanged { value in
                if dragID == nil { dragID = tape.id }
                guard dragID == tape.id else { return }
                let minX = slotW / 2
                let maxX = max(minX, width - slotW / 2)
                dragX = min(max(minX, value.location.x), maxX)
            }
            .onEnded { _ in
                let was = dragID
                let dropX = dragX
                dragID = nil
                dragX = 0
                guard was == tape.id else { return }
                let dest = Int((dropX / slotW).rounded())
                let clamped = min(max(0, dest), max(0, session.shelf.count - 1))
                if let from = session.shelf.firstIndex(where: { $0.id == tape.id }) {
                    session.moveTape(from: from, to: clamped)
                }
            }
    }

    private func shelfTape(_ tape: ShelfTape, index: Int) -> some View {
        let playing = session.tape == index && session.tapePlaying && !showRack
        let out = pulled.contains(tape.id)
        let onPlayer = seated == tape.id
        return ZStack {
            RoundedRectangle(cornerRadius: 1)
                .fill(Color(red: 0.08, green: 0.06, blue: 0.05))
                .frame(width: 20, height: 68)
            if !out {
                RoundedRectangle(cornerRadius: 1)
                    .fill(spineColor(index))
                    .frame(width: 16, height: 64)
                    .overlay(
                        RoundedRectangle(cornerRadius: 1)
                            .stroke(
                                playing || onPlayer
                                    ? Color(red: 0.22, green: 0.72, blue: 0.28)
                                    : Color(white: 0.55),
                                lineWidth: 1
                            )
                    )
                PixelLabel(text: tape.name, scale: 1, color: Palette.paper)
                    .frame(width: 58, height: 12)
                    .rotationEffect(.degrees(-90))
                    .frame(width: 16, height: 60)
                    .allowsHitTesting(false)
            }
        }
        .frame(width: slotW, height: 72, alignment: .leading)
        .contentShape(Rectangle())
        .a11yButton(
            out ? "Empty slot \(tape.name)" : tape.name,
            hint: "Tap to pull out. Drag to reorder"
        )
        .accessibilityAction(named: out ? "Put back" : "Pull out") {
            if out { restore(tape.id) } else { pull(tape.id) }
        }
    }

    private func spineColor(_ index: Int) -> Color {
        let inks: [Color] = [
            Color(red: 0.55, green: 0.22, blue: 0.18),
            Color(red: 0.18, green: 0.32, blue: 0.48),
            Color(red: 0.20, green: 0.38, blue: 0.22),
            Color(red: 0.45, green: 0.32, blue: 0.16),
            Color(red: 0.32, green: 0.22, blue: 0.40),
            Color(red: 0.15, green: 0.28, blue: 0.32),
            Color(red: 0.48, green: 0.28, blue: 0.18),
            Color(red: 0.28, green: 0.22, blue: 0.16),
            Color(red: 0.40, green: 0.18, blue: 0.22),
            Color(red: 0.22, green: 0.36, blue: 0.30),
        ]
        return inks[index % inks.count]
    }

    private var pile: some View {
        VStack(alignment: .leading, spacing: 8) {
            PixelLabel(
                text: pulled.isEmpty ? "PULL A TAPE FROM THE SHELF" : "PULLED",
                scale: 1,
                color: Palette.dimPaper
            )
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(alignment: .top, spacing: 10) {
                    ForEach(pulled, id: \.self) { id in
                        if let tape = session.shelf.first(where: { $0.id == id }) {
                            frontTape(tape)
                        }
                    }
                }
            }
            .frame(minHeight: pulled.isEmpty ? 24 : 120)
        }
    }

    /// Seated tape for preview. Record opens the eurorack (playlist stops).
    private var player: some View {
        VStack(alignment: .leading, spacing: 8) {
            PixelLabel(text: "PLAYER", scale: 1, color: Palette.dimPaper)
            HStack(alignment: .center, spacing: 12) {
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 3)
                        .fill(Color(red: 0.08, green: 0.07, blue: 0.06))
                        .frame(width: 180, height: 64)
                    if let tape = seatedTape {
                        RoundedRectangle(cornerRadius: 2)
                            .fill(Color(red: 0.55, green: 0.5, blue: 0.42))
                            .frame(width: 92, height: 40)
                            .overlay(
                                VStack(spacing: 2) {
                                    PixelLabel(text: tape.name, scale: 1, color: Color(red: 0.12, green: 0.08, blue: 0.04))
                                    PixelLabel(text: clock(tape.duration), scale: 1, color: Color(red: 0.25, green: 0.18, blue: 0.1))
                                }
                            )
                            .offset(x: 16)
                    } else {
                        PixelLabel(text: "NO TAPE", scale: 1, color: Palette.dimPaper)
                            .offset(x: 16)
                    }
                }
                .frame(width: 180, height: 64, alignment: .leading)
                .onTapGesture { previewSeated() }

                Button(action: previewSeated) {
                    PixelPanel {
                        PixelLabel(
                            text: session.tapePlaying && seated != nil ? "STOP" : "PLAY",
                            scale: 2,
                            color: session.tapePlaying
                                ? Color(red: 0.22, green: 0.72, blue: 0.28)
                                : Palette.highlight
                        )
                    }
                }
                .buttonStyle(.plain)
                .a11yButton("Preview tape", hint: "Play the seated cassette")

                Button { showRack = true } label: {
                    ZStack {
                        Circle()
                            .fill(Color(red: 0.82, green: 0.12, blue: 0.1))
                            .frame(width: 28, height: 28)
                        Circle()
                            .fill(Color(red: 0.55, green: 0.08, blue: 0.08))
                            .frame(width: 12, height: 12)
                    }
                }
                .buttonStyle(.plain)
                .a11yButton("Record", hint: "Open the eurorack. Ambient playlist stops")
            }
        }
    }

    private func frontTape(_ tape: ShelfTape) -> some View {
        let onPlayer = seated == tape.id
        return VStack(spacing: 6) {
            HStack(spacing: 16) {
                Circle().stroke(Palette.dimPaper, lineWidth: 2).frame(width: 22, height: 22)
                Circle().stroke(Palette.dimPaper, lineWidth: 2).frame(width: 22, height: 22)
            }
            Button {
                PixelPrompt.singleLine(title: "TAPE", text: tape.name, placeholder: "NAME") { entered in
                    session.renameTape(tape.id, to: entered)
                }
            } label: {
                PixelPanel {
                    PixelLabel(text: tape.name, scale: 2, color: Palette.highlight)
                }
            }
            .buttonStyle(.plain)
            .a11yButton("Name tag \(tape.name)", hint: "Rename")
            PixelLabel(text: clock(tape.duration), scale: 1, color: Palette.paper)
            HStack(spacing: 6) {
                Button { seatAndPreview(tape) } label: {
                    PixelPanel {
                        PixelLabel(
                            text: onPlayer ? "PLAYING" : "PLAY",
                            scale: 1,
                            color: onPlayer ? Palette.highlight : Palette.paper
                        )
                    }
                }
                .buttonStyle(.plain)
                .a11yButton("Play \(tape.name)", hint: "Seat it in the player and preview")
                Button { restore(tape.id) } label: {
                    PixelPanel { PixelLabel(text: "SHELF", scale: 1, color: Palette.paper) }
                }
                .buttonStyle(.plain)
                .a11yButton("Put \(tape.name) back")
            }
        }
        .padding(10)
        .background(Color(red: 0.42, green: 0.28, blue: 0.16))
        .overlay(
            Rectangle().stroke(
                onPlayer ? Color(red: 0.22, green: 0.72, blue: 0.28) : Color(red: 0.85, green: 0.7, blue: 0.4),
                lineWidth: 2
            )
        )
    }

    private func pull(_ id: UUID) {
        if pulled.contains(id) {
            // Second tap on an empty slot puts it back.
            restore(id)
            return
        }
        pulled.append(id)
        // Seat it so PLAY is ready.
        if let tape = session.shelf.first(where: { $0.id == id }) {
            seated = tape.id
        }
    }

    private func restore(_ id: UUID) {
        pulled.removeAll { $0 == id }
        if seated == id { seated = nil }
    }

    private func seatAndPreview(_ tape: ShelfTape) {
        seated = tape.id
        session.previewTape(id: tape.id)
    }

    private func previewSeated() {
        guard let tape = seatedTape else {
            // Fall back to the playlist head.
            if session.shelf.indices.contains(session.tape) {
                seated = session.shelf[session.tape].id
                session.previewTape(id: session.shelf[session.tape].id)
            }
            return
        }
        if session.tapePlaying,
           let i = session.shelf.firstIndex(where: { $0.id == tape.id }),
           session.tape == i {
            session.toggleDeck()
        } else {
            session.previewTape(id: tape.id)
        }
    }

    private func closeDesk() {
        session.endSolo()
        onClose()
    }

    private func clock(_ secs: Double) -> String {
        let s = max(0, Int(secs.rounded()))
        return String(format: "%d:%02d", s / 60, s % 60)
    }
}
