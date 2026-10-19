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
    var onOpenCharts: () -> Void
    var onOpenDeck: () -> Void

    @State private var tree: [SpeciesNode] = []
    @State private var confirmEmpty: Int?
    @State private var vines: [CGFloat] = [6, 6]
    @State private var shears = false
    @State private var thumbs: [UIImage?] = [nil, nil, nil, nil]
    private let refresh = Timer.publish(every: 2, on: .main, in: .common).autoconnect()

    var body: some View {
        GeometryReader { geo in
            let layout = RoomLayout(size: geo.size)
            ZStack {
                Color(red: 0.20, green: 0.14, blue: 0.10)
                Image(uiImage: RoomArt.image)
                    .resizable()
                    .interpolation(.none)
                    .frame(
                        width: CGFloat(RoomArt.w) * layout.s,
                        height: CGFloat(RoomArt.h) * layout.s
                    )
                    .position(x: geo.size.width / 2, y: geo.size.height / 2)

                glassThumb(thumbs[0], RoomArt.tankGlass, layout)
                ForEach(0..<3, id: \.self) { i in
                    glassThumb(thumbs[i + 1], RoomArt.vialGlass(i), layout)
                }
                vineLayer(layout)
                    .position(x: geo.size.width / 2, y: geo.size.height / 2)

                place(RoomArt.treeHit, layout) { treeButton(layout) }
                place(RoomArt.fuseHit, layout) { fuseButton(layout) }
                place(RoomArt.tank, layout) { tankButton }
                ForEach(0..<3, id: \.self) { i in
                    place(RoomArt.vials[i], layout) { vialButton(i) }
                }
                place(RoomArt.bookHit, layout) { bookButton(layout) }
                place(RoomArt.logHit, layout) { logButton }
                place(RoomArt.favHit, layout) { crateButton(layout) }
                place(RoomArt.crateHit, layout) { drawerButton }
                place(RoomArt.deckHit, layout) { deckButton }
                place(RoomArt.signHit, layout) { signButton }
                place(RoomArt.toolJar, layout) { toolJarButton }
                if shears {
                    ForEach(0..<RoomArt.vines.count, id: \.self) { i in
                        place(vineHit(i), layout) { vineButton(i) }
                    }
                }
            }
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .ignoresSafeArea()
        .onAppear {
            tree = session.speciesTree()
            vines = LobbyGrowth.catchUp()
            refreshThumbs(others: true)
        }
        .onReceive(refresh) { _ in
            tree = session.speciesTree()
            refreshThumbs(others: false)
        }
        .onChange(of: session.savesChanged) { _ in
            refreshThumbs(others: true)
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
                    let g = RoomArt.vialGlass(i)
                    thumbs[i + 1] = session.lobbyPreview(.sample(i), width: g.w, height: g.h)
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
            Color.clear
        }
        .buttonStyle(FullHit())
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

    private var drawerButton: some View {
        Button(action: onOpenCharts) {
            Color.clear
        }
        .buttonStyle(FullHit())
        .a11yButton("Filing cabinet", hint: "Charts")
    }

    private var deckButton: some View {
        Button(action: onOpenDeck) {
            Color.clear
        }
        .buttonStyle(FullHit())
        .a11yButton("Cassette deck")
    }

    private var logButton: some View {
        Button(action: onOpenLog) {
            PixelLabel(text: "EVENT LOG", scale: 1, color: Palette.paper)
        }
        .buttonStyle(.plain)
        .a11yButton("Event log")
    }

    private func crateButton(_ layout: RoomLayout) -> some View {
        let inner = layout.rect(RoomArt.favInner)
        let hit = layout.rect(RoomArt.favHit)
        return Button {
            session.cycleFavourite()
        } label: {
            ZStack {
                if let fav = session.favouriteSpecimen {
                    let side = min(inner.width, inner.height)
                    if side >= 4 {
                        portrait(fav, side: side)
                            .offset(y: inner.midY - hit.midY)
                    }
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

    private var signButton: some View {
        let name = session.doorName.trimmingCharacters(in: .whitespacesAndNewlines)
        let shown = name.isEmpty ? "MAIN TANK" : shortSign(name)
        return Button {
            PixelPrompt.singleLine(title: "TANK", text: name, placeholder: "MAIN TANK") { entered in
                session.setDoorName(entered)
            }
        } label: {
            PixelLabel(
                text: shown,
                scale: 1,
                color: Color(red: 0.16, green: 0.1, blue: 0.04)
            )
        }
        .buttonStyle(FullHit())
        .a11yButton("Tank sign", hint: shown)
    }

    private var toolJarButton: some View {
        Button {
            shears.toggle()
        } label: {
            Color.clear
        }
        .buttonStyle(FullHit())
        .a11yButton("Jar")
    }

    private func vineButton(_ index: Int) -> some View {
        Button {
            vines[index] = LobbyGrowth.trim(index)
            shears = false
        } label: {
            Color.clear
        }
        .buttonStyle(FullHit())
        .accessibilityHidden(true)
    }

    private func vineHit(_ index: Int) -> RoomArt.Spot {
        let vine = RoomArt.vines[index]
        let len = max(4, Int(vines[index]))
        return RoomArt.Spot(x: vine.x - 4, y: vine.y - len, w: 12, h: len + 2)
    }

    @ViewBuilder
    private func glassThumb(_ image: UIImage?, _ spot: RoomArt.Spot, _ layout: RoomLayout) -> some View {
        if let image {
            let r = layout.rect(spot)
            Image(uiImage: image)
                .resizable()
                .interpolation(.none)
                .frame(width: r.width, height: r.height)
                .position(x: r.midX, y: r.midY)
                .allowsHitTesting(false)
        }
    }

    private func vineLayer(_ layout: RoomLayout) -> some View {
        let s = layout.s
        return Canvas { ctx, _ in
            for (i, vine) in RoomArt.vines.enumerated() {
                let len = vines.indices.contains(i) ? vines[i] : 6
                var x = CGFloat(vine.x)
                var y = CGFloat(vine.y)
                let top = max(2, y - len)
                var step = 0
                while y > top {
                    let ink: Color = step % 3 == 0
                        ? Color(red: 0.635, green: 0.839, blue: 0.435)
                        : Color(red: 0.424, green: 0.722, blue: 0.290)
                    let rect = CGRect(x: x * s, y: y * s, width: max(s, 1), height: max(s, 1))
                    ctx.fill(Path(rect), with: .color(ink))
                    if step % 3 == 0 {
                        let leaf = CGRect(x: (x + 1) * s, y: y * s, width: max(s, 1), height: max(s, 1))
                        ctx.fill(Path(leaf), with: .color(Color(red: 0.18, green: 0.42, blue: 0.16)))
                    }
                    y -= 2
                    let sway = max(vine.sway, 1)
                    x += (Int(y) / sway) % 2 == 0 ? 1 : -1
                    x = min(CGFloat(RoomArt.w - 2), max(1, x))
                    step += 1
                }
            }
        }
        .frame(width: CGFloat(RoomArt.w) * s, height: CGFloat(RoomArt.h) * s)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private func refreshThumbs(others: Bool) {
        let glass = RoomArt.tankGlass
        thumbs[0] = session.lobbyPreview(.main, width: glass.w, height: glass.h)
        if others {
            for i in 0..<3 {
                let g = RoomArt.vialGlass(i)
                thumbs[i + 1] = session.lobbyPreview(.sample(i), width: g.w, height: g.h)
            }
        } else if let i = session.container.sampleIndex {
            let g = RoomArt.vialGlass(i)
            thumbs[i + 1] = session.lobbyPreview(.sample(i), width: g.w, height: g.h)
        }
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

/// Vines lengthen while the app is closed. Nothing in the tank cares.
private enum LobbyGrowth {
    static func catchUp() -> [CGFloat] {
        let now = Date().timeIntervalSince1970
        var lengths: [CGFloat] = []
        for i in 0..<RoomArt.vines.count {
            let key = "lobbyVine\(i)"
            let seenKey = "lobbySeen\(i)"
            let seen = UserDefaults.standard.double(forKey: seenKey)
            var len = UserDefaults.standard.double(forKey: key)
            if seen <= 0, len <= 0 {
                len = 6
            } else if seen > 0 {
                len += max(0, now - seen) / 3600 * 2
            }
            len = min(14, max(4, len))
            UserDefaults.standard.set(len, forKey: key)
            UserDefaults.standard.set(now, forKey: seenKey)
            lengths.append(CGFloat(len))
        }
        return lengths
    }

    static func trim(_ index: Int) -> CGFloat {
        UserDefaults.standard.set(4.0, forKey: "lobbyVine\(index)")
        UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: "lobbySeen\(index)")
        return 4
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

/// Full-screen tree-of-life with names. A tap opens the genetics card.
struct TreeScreen: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void

    @State private var tree: [SpeciesNode] = []
    @State private var picked: UInt16?
    @State private var card = ""
    @State private var portrait: UIImage?

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Palette.panel.ignoresSafeArea()
            TreeOfLifeView(nodes: tree, onPick: { id in
                picked = id
                card = session.speciesCard(id)
                portrait = session.speciesPortrait(id)
            })
            .padding(.top, 44)
            VStack {
                HStack {
                    PixelLabel(text: "TREE OF LIFE  \(tree.count) SPECIES", scale: 2, color: Palette.highlight)
                    Spacer()
                    Button(action: onClose) {
                        PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                    }
                    .buttonStyle(.plain)
                }
                Spacer()
                if picked != nil {
                    speciesCard
                }
            }
            .padding(10)
        }
        .onAppear { tree = session.speciesTree() }
        .preferredColorScheme(.dark)
    }

    private var speciesCard: some View {
        HStack(alignment: .top, spacing: 10) {
            Group {
                if let portrait {
                    Image(uiImage: portrait)
                        .interpolation(.none)
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                } else {
                    Rectangle().fill(Color(white: 0.12))
                }
            }
            .frame(width: 96, height: 96)
            .background(Color(white: 0.12))
            VStack(alignment: .leading, spacing: 2) {
                let name = tree.first { $0.id == picked }?.name ?? ""
                PixelLabel(text: name.uppercased(), scale: 2, color: Palette.highlight, maxWidth: 220)
                ScrollView {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(Array(card.split(separator: "\n").enumerated()), id: \.offset) { _, line in
                            PixelLabel(text: String(line), scale: 1, color: Palette.paper, maxWidth: 240)
                        }
                    }
                }
                .frame(maxHeight: 280)
            }
            Spacer(minLength: 0)
            Button {
                picked = nil
            } label: {
                PixelPanel { PixelLabel(text: "X", scale: 2) }
            }
            .buttonStyle(.plain)
        }
        .padding(8)
        .background(Palette.panel)
        .overlay(Rectangle().stroke(Palette.highlight, lineWidth: 1))
    }
}

/// Charts from the samples the open tank has been keeping.
struct ChartDrawer: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                PixelLabel(text: "DRAWER", scale: 3, color: Palette.highlight)
                Spacer()
                Button(action: onClose) {
                    PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                }
                .buttonStyle(.plain)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    chart("CREATURES", session.chart.map { Double($0.creatures) }, Palette.highlight)
                    chart("LINEAGES", session.chart.map { Double($0.lineages) }, Palette.paper)
                    chart("PREDATION", session.chart.map { Double($0.predation) }, Color(red: 0.7, green: 0.3, blue: 0.35))
                    chart("PHOTOSYNTHESIS", session.chart.map { Double($0.photo) }, Color(red: 0.35, green: 0.62, blue: 0.32))
                    chart("WATER", session.chart.map { Double($0.water) }, Color(red: 0.32, green: 0.55, blue: 0.7))
                    chart("SAND", session.chart.map { Double($0.sand) }, Color(red: 0.72, green: 0.55, blue: 0.32))
                    chart("STONE", session.chart.map { Double($0.stone) }, Color(red: 0.55, green: 0.52, blue: 0.48))
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Palette.panel.ignoresSafeArea())
        .preferredColorScheme(.dark)
    }

    private func chart(_ title: String, _ values: [Double], _ ink: Color) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            let last = values.last.map { String(Int($0)) } ?? "0"
            PixelLabel(text: "\(title)  \(last)", scale: 1, color: Palette.dimPaper)
            Canvas { ctx, size in
                let rect = CGRect(origin: .zero, size: size)
                ctx.fill(Path(rect), with: .color(Color(white: 0.08)))
                guard values.count >= 2 else { return }
                let peak = max(values.max() ?? 1, 1)
                var path = Path()
                for (i, v) in values.enumerated() {
                    let x = CGFloat(i) / CGFloat(values.count - 1) * size.width
                    let y = size.height - CGFloat(v / peak) * (size.height - 2) - 1
                    if i == 0 { path.move(to: CGPoint(x: x, y: y)) } else { path.addLine(to: CGPoint(x: x, y: y)) }
                }
                ctx.stroke(path, with: .color(ink), lineWidth: 1)
            }
            .frame(height: 48)
        }
    }
}

/// A few generative tapes. Picking one sets the room's sound.
struct CassetteDeck: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void

    private let tapes = ["SLOW", "BRIGHT", "NIGHT"]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                PixelLabel(text: "DECK", scale: 3, color: Palette.highlight)
                Spacer()
                Button(action: onClose) {
                    PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                }
                .buttonStyle(.plain)
            }
            PixelLabel(text: "A TAPE SETS THE ROOM", scale: 1, color: Palette.dimPaper)
            ForEach(0..<tapes.count, id: \.self) { i in
                Button {
                    session.setTape(i)
                } label: {
                    PixelPanel {
                        PixelLabel(
                            text: session.tape == i ? "\(tapes[i])  IN" : tapes[i],
                            scale: 2,
                            color: session.tape == i ? Palette.highlight : Palette.paper
                        )
                    }
                }
                .buttonStyle(.plain)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Palette.panel.ignoresSafeArea())
        .preferredColorScheme(.dark)
    }
}
