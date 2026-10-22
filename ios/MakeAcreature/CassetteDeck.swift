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
    @State private var shelfWidth: CGFloat = 1

    private let slotW: CGFloat = 28

    var body: some View {
        ZStack {
            Color.black.opacity(0.28)
                .ignoresSafeArea()
                .onTapGesture(perform: onClose)
            VStack(alignment: .leading, spacing: 16) {
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
                    Button(action: onClose) {
                        PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                    }
                    .buttonStyle(.plain)
                }
                PixelLabel(text: "DRAG TO REORDER  TAP TO PULL", scale: 1, color: Palette.dimPaper)
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
        .fullScreenCover(isPresented: $showRack) {
            EurorackStudio(session: session) { showRack = false }
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
                        .gesture(shelfDrag(tape: tape, width: geo.size.width))
                }
            }
            .frame(width: geo.size.width, height: 86, alignment: .leading)
            .coordinateSpace(name: "shelf")
            .onAppear { shelfWidth = geo.size.width }
            .onChange(of: geo.size.width) { w in shelfWidth = w }
        }
        .frame(height: 86)
    }

    private func shelfDrag(tape: ShelfTape, width: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 8, coordinateSpace: .named("shelf"))
            .onChanged { value in
                if dragID == nil { dragID = tape.id }
                guard dragID == tape.id else { return }
                let minX = slotW / 2
                let maxX = max(minX, width - slotW / 2)
                dragX = min(max(minX, value.location.x), maxX)
            }
            .onEnded { value in
                let was = dragID
                let dropX = dragX
                dragID = nil
                dragX = 0
                guard was == tape.id else { return }
                let dx = value.translation.width
                let dy = value.translation.height
                if abs(dx) < 14, abs(dy) < 14 {
                    if pulled.contains(tape.id) { restore(tape.id) } else { pull(tape.id) }
                    return
                }
                let dest = Int((dropX / slotW).rounded())
                let clamped = min(max(0, dest), max(0, session.shelf.count - 1))
                if let from = session.shelf.firstIndex(where: { $0.id == tape.id }) {
                    session.moveTape(from: from, to: clamped)
                }
            }
    }

    /// A cassette box seen edge-on. Pulling it out leaves the slot empty.
    private func shelfTape(_ tape: ShelfTape, index: Int) -> some View {
        let playing = session.tape == index && session.tapePlaying
        let out = pulled.contains(tape.id)
        return ZStack {
            RoundedRectangle(cornerRadius: 1)
                .fill(Color(red: 0.08, green: 0.06, blue: 0.05))
                .frame(width: 18, height: 68)
            if !out {
                RoundedRectangle(cornerRadius: 1)
                    .fill(spineColor(index))
                    .frame(width: 14, height: 64)
                    .overlay(
                        RoundedRectangle(cornerRadius: 1)
                            .stroke(
                                playing
                                    ? Color(red: 0.22, green: 0.72, blue: 0.28)
                                    : Color(white: 0.55),
                                lineWidth: 1
                            )
                    )
                PixelLabel(text: tape.name, scale: 1, color: Palette.paper)
                    .frame(width: 58, height: 12)
                    .rotationEffect(.degrees(-90))
                    .frame(width: 14, height: 60)
                    .allowsHitTesting(false)
            }
        }
        .frame(width: slotW, height: 72, alignment: .leading)
        .contentShape(Rectangle())
        .a11yButton(
            out ? "Empty slot" : tape.name,
            hint: "Drag sideways to reorder. Tap to pull out"
        )
        .accessibilityAction(named: "Move left") {
            guard let from = session.shelf.firstIndex(where: { $0.id == tape.id }), from > 0 else { return }
            session.moveTape(from: from, to: from - 1)
        }
        .accessibilityAction(named: "Move right") {
            guard let from = session.shelf.firstIndex(where: { $0.id == tape.id }) else { return }
            session.moveTape(from: from, to: from + 1)
        }
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
        ZStack(alignment: .topLeading) {
            if pulled.isEmpty {
                PixelLabel(text: "PULL A TAPE", scale: 1, color: Palette.dimPaper)
            }
            ForEach(Array(pulled.enumerated()), id: \.element) { i, id in
                if let tape = session.shelf.first(where: { $0.id == id }) {
                    frontTape(tape)
                        .offset(x: CGFloat(i) * 14, y: CGFloat(i) * 10)
                }
            }
        }
        .frame(maxWidth: .infinity, minHeight: 88, alignment: .topLeading)
    }

    /// Record opens the eurorack page. A blank rides onto the player there.
    private var player: some View {
        HStack(alignment: .center, spacing: 14) {
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 3)
                    .fill(Color(red: 0.08, green: 0.07, blue: 0.06))
                    .frame(width: 168, height: 64)
                PixelLabel(text: "PLAYER", scale: 1, color: Palette.dimPaper)
                    .padding(.leading, 8)
                RoundedRectangle(cornerRadius: 2)
                    .fill(Color(red: 0.55, green: 0.5, blue: 0.42))
                    .frame(width: 46, height: 30)
                    .overlay(PixelLabel(text: "EMPTY", scale: 1, color: Color(red: 0.12, green: 0.08, blue: 0.04)))
                    .offset(x: 100)
            }
            .frame(width: 230, height: 64, alignment: .leading)
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
            .a11yButton("Record", hint: "Open the eurorack and record a tape")
        }
    }

    private func frontTape(_ tape: ShelfTape) -> some View {
        VStack(spacing: 6) {
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
                Button {
                    if let from = session.shelf.firstIndex(where: { $0.id == tape.id }), from > 0 {
                        session.moveTape(from: from, to: from - 1)
                    }
                } label: {
                    PixelPanel { PixelLabel(text: "<", scale: 1) }
                }
                .buttonStyle(.plain)
                Button {
                    if let from = session.shelf.firstIndex(where: { $0.id == tape.id }) {
                        session.moveTape(from: from, to: from + 1)
                    }
                } label: {
                    PixelPanel { PixelLabel(text: ">", scale: 1) }
                }
                .buttonStyle(.plain)
                Button { restore(tape.id) } label: {
                    PixelPanel { PixelLabel(text: "SHELF", scale: 1, color: Palette.paper) }
                }
                .buttonStyle(.plain)
                .a11yButton("Put \(tape.name) back")
            }
        }
        .padding(10)
        .background(Color(red: 0.42, green: 0.28, blue: 0.16))
        .overlay(Rectangle().stroke(Color(red: 0.85, green: 0.7, blue: 0.4), lineWidth: 2))
    }

    private func pull(_ id: UUID) {
        guard !pulled.contains(id) else { return }
        pulled.append(id)
    }

    private func restore(_ id: UUID) {
        pulled.removeAll { $0 == id }
    }

    private func clock(_ secs: Double) -> String {
        let s = max(0, Int(secs.rounded()))
        return String(format: "%d:%02d", s / 60, s % 60)
    }
}
