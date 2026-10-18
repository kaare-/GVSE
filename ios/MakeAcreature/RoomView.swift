import Combine
import SwiftUI

/// The study. The picture is a low-res bitmap; the buttons sit on the
/// painted objects and only the live bits (tree, fuse lamps, counts,
/// portrait, door name) are drawn on top.
struct RoomView: View {
    @ObservedObject var session: TankSession
    var onBack: () -> Void
    var onOpenBook: () -> Void
    var onOpenTree: () -> Void
    var onOpenSettings: () -> Void
    var onOpenLog: () -> Void

    @State private var tree: [SpeciesNode] = []
    @State private var editingSign = false
    @State private var signDraft = ""
    @State private var confirmEmpty: Int?
    private let refresh = Timer.publish(every: 2, on: .main, in: .common).autoconnect()

    var body: some View {
        GeometryReader { geo in
            let layout = RoomLayout(size: geo.size)
            ZStack {
                Color(red: 0.07, green: 0.05, blue: 0.04)
                Image(uiImage: RoomArt.image)
                    .resizable()
                    .interpolation(.none)
                    .frame(
                        width: CGFloat(RoomArt.w) * layout.s,
                        height: CGFloat(RoomArt.h) * layout.s
                    )
                    .position(x: geo.size.width / 2, y: geo.size.height / 2)

                place(RoomArt.treeHit, layout) { treeButton(layout) }
                place(RoomArt.fuseHit, layout) { fuseButton(layout) }
                place(RoomArt.tank, layout) { tankButton }
                ForEach(0..<3, id: \.self) { i in
                    place(RoomArt.vials[i], layout) { vialButton(i) }
                }
                place(RoomArt.bookHit, layout) { bookButton(layout) }
                place(RoomArt.logHit, layout) { logButton }
                place(RoomArt.crateHit, layout) { crateButton(layout) }
                place(RoomArt.signHit, layout) { signButton(layout) }
            }
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .ignoresSafeArea()
        .onAppear {
            tree = session.speciesTree()
            signDraft = session.doorName
        }
        .onReceive(refresh) { _ in tree = session.speciesTree() }
        .sheet(isPresented: $editingSign) {
            doorSignEditor
        }
        .confirmationDialog(
            "Empty sample tank \((confirmEmpty ?? 0) + 1)? It goes back to a clean glass.",
            isPresented: Binding(
                get: { confirmEmpty != nil },
                set: { if !$0 { confirmEmpty = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Empty tank", role: .destructive) {
                if let i = confirmEmpty {
                    session.emptySample(i)
                }
                confirmEmpty = nil
            }
        }
    }

    private func place<Content: View>(_ spot: RoomArt.Spot, _ layout: RoomLayout, @ViewBuilder _ content: () -> Content) -> some View {
        let r = layout.rect(spot)
        return content()
            .frame(width: r.width, height: r.height)
            .contentShape(Rectangle())
            .position(x: r.midX, y: r.midY)
    }

    private func treeButton(_ layout: RoomLayout) -> some View {
        let hit = layout.rect(RoomArt.treeHit)
        let inner = layout.rect(RoomArt.treeInner)
        let fit = min(inner.width / 180, inner.height / 120)
        return Button(action: onOpenTree) {
            ZStack(alignment: .topLeading) {
                TreeOfLifeView(nodes: tree, compact: true)
                    .scaleEffect(fit)
                    .frame(width: inner.width, height: inner.height)
                    .clipped()
                    .offset(x: inner.minX - hit.minX, y: inner.minY - hit.minY)
                VStack {
                    Spacer(minLength: 0)
                    PixelLabel(text: "TREE OF LIFE", scale: 1, color: Palette.dimPaper)
                }
                .frame(width: hit.width, height: hit.height)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Tree of life")
    }

    private func fuseButton(_ layout: RoomLayout) -> some View {
        let hit = layout.rect(RoomArt.fuseHit)
        return Button(action: onOpenSettings) {
            ZStack(alignment: .topLeading) {
                Color.clear
                ForEach(0..<3, id: \.self) { i in
                    let lamp = RoomArt.fuseLamps[i]
                    Circle()
                        .fill(breakerOn(i) ? Color(red: 0.78, green: 1, blue: 0.35) : Color(white: 0.12))
                        .overlay(Circle().stroke(Color(white: 0.75), lineWidth: 1))
                        .frame(width: CGFloat(lamp.w) * layout.s, height: CGFloat(lamp.h) * layout.s)
                        .position(
                            x: (CGFloat(lamp.x - RoomArt.fuseHit.x) + CGFloat(lamp.w) / 2) * layout.s,
                            y: (CGFloat(lamp.y - RoomArt.fuseHit.y) + CGFloat(lamp.h) / 2) * layout.s
                        )
                }
                Color.clear.frame(width: hit.width, height: hit.height)
            }
        }
        .buttonStyle(FullHit())
        .a11yButton("Settings")
    }

    /// Sound, clock, and swirl. Off is a dark well; on is a bright lamp.
    private func breakerOn(_ index: Int) -> Bool {
        switch index {
        case 0: return session.soundOn
        case 1: return session.alwaysDay
        default: return session.effectiveSwirl
        }
    }

    private var tankButton: some View {
        Button {
            session.openMain()
            onBack()
        } label: {
            VStack(spacing: 3) {
                PixelLabel(text: "TANK", scale: 2, color: Palette.highlight)
                if session.container.sampleIndex == nil {
                    PixelLabel(
                        text: "\(session.isDay ? "DAY" : "NIGHT") \(session.dayNumber)",
                        scale: 1,
                        color: Palette.dimPaper
                    )
                    PixelLabel(text: "\(session.liveCells) CELLS", scale: 1)
                } else {
                    PixelLabel(text: "MAIN", scale: 1, color: Palette.dimPaper)
                }
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Main tank, \(session.liveCells) cells", hint: "Look into the main tank")
    }

    private func vialButton(_ index: Int) -> some View {
        let open = session.container.sampleIndex == index
        return Button {
            session.openSample(index)
            onBack()
        } label: {
            VStack {
                Spacer(minLength: 0)
                PixelLabel(
                    text: "\(index + 1)",
                    scale: 1,
                    color: open ? Palette.highlight : Palette.paper
                )
            }
        }
        .buttonStyle(FullHit())
        .a11yButton("Sample tank \(index + 1)", hint: "Open this sample, separate from the main tank", selected: open)
        .contextMenu {
            Button("Empty tank \(index + 1)", role: .destructive) {
                confirmEmpty = index
            }
        }
    }

    private func bookButton(_ layout: RoomLayout) -> some View {
        let cover = layout.rect(RoomArt.book)
        let hit = layout.rect(RoomArt.bookHit)
        return Button(action: onOpenBook) {
            ZStack {
                PixelLabel(
                    text: "\(session.archive.specimens.count)",
                    scale: 2,
                    color: Palette.highlight
                )
                .offset(y: cover.midY - hit.midY)
                VStack {
                    Spacer(minLength: 0)
                    PixelLabel(text: "THE BOOK", scale: 1, color: Palette.dimPaper)
                }
            }
        }
        .buttonStyle(.plain)
        .a11yButton("The book", hint: "Archive")
    }

    private var logButton: some View {
        Button(action: onOpenLog) {
            PixelLabel(text: "EVENT LOG", scale: 1, color: Palette.paper)
        }
        .buttonStyle(.plain)
        .a11yButton("Event log")
    }

    private func crateButton(_ layout: RoomLayout) -> some View {
        let inner = layout.rect(RoomArt.crateInner)
        let hit = layout.rect(RoomArt.crateHit)
        return Button {
            session.cycleFavourite()
        } label: {
            ZStack {
                if let fav = session.favouriteSpecimen {
                    portrait(fav, side: min(inner.width, inner.height))
                        .offset(y: inner.midY - hit.midY)
                } else {
                    PixelLabel(text: "PICK", scale: 1, color: Palette.dimPaper)
                        .offset(y: inner.midY - hit.midY)
                }
                VStack {
                    Spacer(minLength: 0)
                    PixelLabel(text: "FAVOURITE", scale: 1, color: Palette.dimPaper)
                }
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Favourite frame", hint: "Cycle the framed creature")
        .contextMenu {
            Button("Clear favourite") { session.clearFavourite() }
        }
    }

    private func signButton(_ layout: RoomLayout) -> some View {
        let plaque = layout.rect(RoomArt.sign)
        let hit = layout.rect(RoomArt.signHit)
        return Button { editingSign = true } label: {
            ZStack {
                PixelLabel(
                    text: shortSign(session.doorName),
                    scale: 1,
                    color: Color(red: 0.16, green: 0.1, blue: 0.04)
                )
                .offset(y: plaque.midY - hit.midY)
                VStack {
                    Spacer(minLength: 0)
                    PixelLabel(text: "DOOR SIGN", scale: 1, color: Palette.dimPaper)
                }
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Door sign", hint: session.doorName.isEmpty ? "Set your name" : session.doorName)
    }

    private var doorSignEditor: some View {
        VStack(alignment: .leading, spacing: 12) {
            PixelLabel(text: "DOOR SIGN", scale: 3, color: Palette.highlight)
            PixelLabel(text: "YOUR NAME ON THE DOOR", scale: 1, color: Palette.dimPaper)
            Button {
                PixelPrompt.singleLine(title: "DOOR SIGN", text: signDraft, placeholder: "NAME") { entered in
                    signDraft = String(entered.prefix(24))
                    session.setDoorName(signDraft)
                }
            } label: {
                VStack(alignment: .leading, spacing: 4) {
                    PixelLabel(text: "NAME", scale: 1, color: Palette.dimPaper)
                    PixelLabel(
                        text: signDraft.isEmpty ? "TAP TO NAME" : signDraft.uppercased(),
                        scale: 2,
                        color: Palette.highlight,
                        maxWidth: 240
                    )
                }
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color(white: 0.14))
                .overlay(Rectangle().stroke(Palette.highlight, lineWidth: 1))
            }
            .buttonStyle(.plain)
            HStack {
                Button {
                    editingSign = false
                } label: {
                    PixelPanel { PixelLabel(text: "CANCEL", scale: 2) }
                }
                .buttonStyle(.plain)
                Spacer()
                Button {
                    session.setDoorName(signDraft)
                    editingSign = false
                } label: {
                    PixelPanel { PixelLabel(text: "HANG IT", scale: 2, color: Palette.highlight) }
                }
                .buttonStyle(.plain)
            }
            Spacer()
        }
        .padding(16)
        .background(Palette.panel.ignoresSafeArea())
        .ignoresSafeArea(.keyboard)
        .preferredColorScheme(.dark)
    }

    private func shortSign(_ name: String) -> String {
        let up = name.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        if up.isEmpty { return "????" }
        return up.count > 10 ? String(up.prefix(9)) + "." : up
    }
}

/// Hits the whole frame, not only the opaque pixels of the label.
private struct FullHit: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .contentShape(Rectangle())
    }
}

/// Maps the 120×260 picture onto the phone, letterboxed, nearest pixel.
private struct RoomLayout {
    var s: CGFloat
    var origin: CGPoint

    init(size: CGSize) {
        s = min(size.width / CGFloat(RoomArt.w), size.height / CGFloat(RoomArt.h))
        origin = CGPoint(
            x: (size.width - CGFloat(RoomArt.w) * s) / 2,
            y: (size.height - CGFloat(RoomArt.h) * s) / 2
        )
    }

    func rect(_ spot: RoomArt.Spot) -> CGRect {
        CGRect(
            x: origin.x + CGFloat(spot.x) * s,
            y: origin.y + CGFloat(spot.y) * s,
            width: CGFloat(spot.w) * s,
            height: CGFloat(spot.h) * s
        )
    }
}

/// Full-screen tree-of-life with names.
struct TreeScreen: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void

    @State private var tree: [SpeciesNode] = []

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Palette.panel.ignoresSafeArea()
            TreeOfLifeView(nodes: tree)
                .padding(.top, 44)
            HStack {
                PixelLabel(text: "TREE OF LIFE  \(tree.count) SPECIES", scale: 2, color: Palette.highlight)
                Spacer()
                Button(action: onClose) {
                    PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                }
                .buttonStyle(.plain)
            }
            .padding(10)
        }
        .onAppear { tree = session.speciesTree() }
        .preferredColorScheme(.dark)
    }
}
