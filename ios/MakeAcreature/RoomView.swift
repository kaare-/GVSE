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
    @State private var pinchEntered = false
    @State private var skyPhase = RoomMood.phase()
    @State private var lampOn = RoomMood.storedLamp
    @State private var candleLit = RoomMood.storedCandle
    private let cast = LobbyScript.cast
    private let refresh = Timer.publish(every: 2, on: .main, in: .common).autoconnect()
    private let skyClock = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

    private var roomMood: RoomMood {
        RoomMood(phase: skyPhase, lampOn: lampOn, candleLit: candleLit)
    }

    var body: some View {
        GeometryReader { geo in
            let layout = RoomLayout(size: geo.size)
            ZStack {
                Color(red: 0.20, green: 0.14, blue: 0.10)
                Image(uiImage: RoomArt.picture(cast, mood: roomMood))
                    .resizable()
                    .interpolation(.none)
                    .frame(
                        width: CGFloat(RoomArt.w) * layout.s,
                        height: CGFloat(RoomArt.h) * layout.s
                    )
                    .position(x: geo.size.width / 2, y: geo.size.height / 2)

                if cast.tank {
                    glassThumb(thumbs[0], RoomArt.tankGlass, layout)
                }
                if cast.jars {
                    ForEach(0..<3, id: \.self) { i in
                        glassThumb(thumbs[i + 1], RoomArt.vialGlass(i), layout)
                    }
                }
                vineLayer(layout)
                    .position(x: geo.size.width / 2, y: geo.size.height / 2)
                if cast.openingGlass {
                    openingMote(layout)
                        .position(x: geo.size.width / 2, y: geo.size.height / 2)
                }

                place(RoomArt.treeHit, layout) { treeButton(layout) }
                place(RoomArt.fuseHit, layout) { fuseButton(layout) }
                if cast.tank {
                    place(RoomArt.tank, layout) { tankButton }
                }
                if cast.jars {
                    ForEach(0..<3, id: \.self) { i in
                        place(RoomArt.vials[i], layout) { vialButton(i) }
                    }
                }
                if cast.bench {
                    place(RoomArt.bookHit, layout) { bookButton(layout) }
                    place(RoomArt.logHit, layout) { logButton }
                    place(RoomArt.crateHit, layout) { drawerButton }
                    place(RoomArt.signHit, layout) { signButton }
                    place(RoomArt.toolJar, layout) { toolJarButton }
                    if shears {
                        ForEach(0..<RoomArt.vines.count, id: \.self) { i in
                            place(vineHit(i), layout) { vineButton(i) }
                        }
                    }
                }
                place(RoomArt.deckHit, layout) { deckButton }
                place(RoomArt.deckPower, layout) { deckPowerButton }
                place(RoomArt.favHit, layout) { crateButton(layout) }
                place(RoomArt.lampSwitch, layout) { lampSwitchButton }
                place(RoomArt.matchbox, layout) { matchboxButton }
                if candleLit {
                    place(RoomArt.flame, layout) { flameControl }
                }
                if cast.openingGlass {
                    place(RoomArt.openingGlass, layout) { openingButton }
                }
            }
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .ignoresSafeArea()
        .onAppear {
            tree = session.speciesTree()
            vines = LobbyGrowth.catchUp()
            refreshThumbs(others: true)
            SystemCue.play(LobbyScript.opening.sound)
        }
        .onReceive(skyClock) { date in
            skyPhase = RoomMood.phase(at: date)
            session.followPlaylist()
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
                    if !creatureName.isEmpty {
                        PixelLabel(
                            text: creatureName,
                            scale: 1,
                            color: Color(red: 0.16, green: 0.10, blue: 0.04)
                        )
                    }
                }
                .padding(.bottom, layout.s * CGFloat(RoomArt.treeHit.h - RoomArt.treeFrame.h))
                .frame(width: hit.width, height: hit.height)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Tree of life", hint: creatureName.isEmpty ? "Open the tree" : creatureName)
    }

    /// The name on the bottom edge of the tree frame.
    private var creatureName: String {
        guard let fav = session.favouriteSpecimen else { return "" }
        let raw = fav.displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        let fallback = fav.speciesName.trimmingCharacters(in: .whitespacesAndNewlines)
        let up = (raw.isEmpty ? fallback : raw).uppercased()
        if up.isEmpty { return "" }
        return up.count > 12 ? String(up.prefix(11)) + "." : up
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

    private func enterGlass(_ open: () -> Void) {
        guard !pinchEntered else { return }
        pinchEntered = true
        open()
        onBack()
    }

    private var openingButton: some View {
        Color.clear
            .contentShape(Rectangle())
            .gesture(
                MagnificationGesture().onChanged { scale in
                    if scale > 1.18 { enterGlass { session.openMain() } }
                }
            )
            .accessibilityAction(named: "Look in") { enterGlass { session.openMain() } }
            .a11yButton("Glass of water", hint: "Pinch in to look into the glass")
    }

    /// A mote in the opening glass. It is not baked into the picture.
    /// Coordinates match the centred picture, the same way the vines do.
    private func openingMote(_ layout: RoomLayout) -> some View {
        let g = RoomArt.openingGlass
        let s = layout.s
        let waterX = CGFloat(g.x + 3)
        let waterY = CGFloat(g.y + 6)
        let waterW = CGFloat(max(4, g.w - 6))
        let waterH = CGFloat(max(6, g.h - 12))
        return TimelineView(.animation) { timeline in
            let t = timeline.date.timeIntervalSinceReferenceDate
            let x = (waterX + waterW * (0.5 + 0.22 * CGFloat(sin(t * 1.4)))) * s
            let y = (waterY + waterH * (0.48 + 0.18 * CGFloat(cos(t * 0.9)))) * s
            Rectangle()
                .fill(Color(red: 0.42, green: 0.72, blue: 0.29))
                .frame(width: max(s * 2, 2), height: max(s * 2, 2))
                .position(x: x, y: y)
        }
        .frame(width: CGFloat(RoomArt.w) * s, height: CGFloat(RoomArt.h) * s)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private var tankButton: some View {
        Color.clear
            .contentShape(Rectangle())
            .gesture(
                MagnificationGesture().onChanged { scale in
                    if scale > 1.18 { enterGlass { session.openMain() } }
                }
            )
            .accessibilityAction(named: "Look in") { enterGlass { session.openMain() } }
            .a11yButton("Main tank, \(session.liveCells) cells", hint: "Pinch in to look into the main tank")
    }

    private func vialButton(_ index: Int) -> some View {
        let open = session.container.sampleIndex == index
        return VStack {
            Spacer(minLength: 0)
            PixelLabel(
                text: "\(index + 1)",
                scale: 1,
                color: open ? Palette.highlight : Palette.paper
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .contentShape(Rectangle())
        .gesture(
            MagnificationGesture().onChanged { scale in
                if scale > 1.18 { enterGlass { session.openSample(index) } }
            }
        )
        .accessibilityAction(named: "Look in") { enterGlass { session.openSample(index) } }
        .a11yButton("Sample tank \(index + 1)", hint: "Pinch in to open this sample", selected: open)
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
        .a11yButton("Cassette", hint: "Open the desk")
    }

    private var deckPowerButton: some View {
        Button(action: session.toggleDeck) {
            Rectangle()
                .fill(session.tapePlaying
                    ? Color(red: 0.22, green: 0.72, blue: 0.28)
                    : Color(red: 0.78, green: 0.16, blue: 0.14))
        }
        .buttonStyle(FullHit())
        .a11yButton(session.tapePlaying ? "Deck power, on" : "Deck power, off")
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
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Framed creature", hint: "Cycle the framed creature")
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

    private var lampSwitchButton: some View {
        Button {
            lampOn.toggle()
            UserDefaults.standard.set(lampOn, forKey: "lobbyLamp")
        } label: {
            Color.clear
        }
        .buttonStyle(FullHit())
        .a11yButton(lampOn ? "Light switch, on" : "Light switch, off", hint: "Electrical light")
    }

    private var matchboxButton: some View {
        Button {
            guard !candleLit else { return }
            candleLit = true
            UserDefaults.standard.set(true, forKey: "lobbyCandle")
        } label: {
            Color.clear
        }
        .buttonStyle(FullHit())
        .a11yButton("Matchbox", hint: "Light the candle")
    }

    private var flameControl: some View {
        Color.clear
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 8).onEnded { value in
                    if abs(value.translation.width) > 12 { blowOutCandle() }
                }
            )
            .accessibilityAction(named: "Blow out") { blowOutCandle() }
            .a11yButton("Candle flame", hint: "Swipe across the flame to blow it out")
    }

    private func blowOutCandle() {
        guard candleLit else { return }
        candleLit = false
        UserDefaults.standard.set(false, forKey: "lobbyCandle")
    }

    private func vineHit(_ index: Int) -> RoomArt.Spot {
        let vine = RoomArt.vines[index]
        let len = max(4, Int(vines[index]))
        if vine.hangs {
            return RoomArt.Spot(x: vine.x - 4, y: vine.y, w: 12, h: len + 2)
        }
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
                let limit = vine.hangs
                    ? min(CGFloat(RoomArt.floorY - 8), y + len)
                    : max(2, y - len)
                var step = 0
                while vine.hangs ? y < limit : y > limit {
                    let ink: Color = step % 3 == 0
                        ? Color(red: 0.635, green: 0.839, blue: 0.435)
                        : Color(red: 0.424, green: 0.722, blue: 0.290)
                    let rect = CGRect(x: x * s, y: y * s, width: max(s, 1), height: max(s, 1))
                    ctx.fill(Path(rect), with: .color(ink))
                    if step % 3 == 0 {
                        let leafX = vine.hangs ? x - 1 : x + 1
                        let leaf = CGRect(x: leafX * s, y: y * s, width: max(s, 1), height: max(s, 1))
                        ctx.fill(Path(leaf), with: .color(Color(red: 0.18, green: 0.42, blue: 0.16)))
                    }
                    y += vine.hangs ? 2 : -2
                    let sway = max(vine.sway, 1)
                    x += (Int(y) / sway) % 2 == 0 ? 1 : -1
                    if vine.hangs {
                        x = min(CGFloat(14), max(CGFloat(2), x))
                    } else {
                        x = min(CGFloat(RoomArt.w - 2), max(1, x))
                    }
                    step += 1
                }
            }
        }
        .frame(width: CGFloat(RoomArt.w) * s, height: CGFloat(RoomArt.h) * s)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private func refreshThumbs(others: Bool) {
        guard cast.tank || cast.jars else { return }
        if cast.tank {
            let glass = RoomArt.tankGlass
            thumbs[0] = session.lobbyPreview(.main, width: glass.w, height: glass.h)
        }
        guard cast.jars else { return }
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
            let cap = RoomArt.vines[i].hangs ? 56.0 : 14.0
            if seen <= 0, len <= 0 {
                len = RoomArt.vines[i].hangs ? 10 : 6
            } else if seen > 0 {
                len += max(0, now - seen) / 3600 * 2
            }
            len = min(cap, max(4, len))
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

/// The cassette desk, over the lobby. The room stays visible around it.
/// Left to right on the shelf is the playlist.
struct CassetteDeck: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void

    @State private var pulled: [UUID] = []

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
                shelf
                pile
                Spacer(minLength: 0)
            }
            .padding(16)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(Color(red: 0.16, green: 0.11, blue: 0.08).opacity(0.94))
            .padding(.horizontal, 28)
            .padding(.vertical, 48)
        }
        .preferredColorScheme(.dark)
    }

    private var shelf: some View {
        HStack(alignment: .bottom, spacing: 4) {
            ForEach(Array(session.shelf.enumerated()), id: \.element.id) { index, tape in
                shelfTape(tape, index: index)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(height: 78)
    }

    private func shelfTape(_ tape: ShelfTape, index: Int) -> some View {
        let playing = session.tape == index && session.tapePlaying
        let edge = index % 2 == 1
        return VStack(spacing: 4) {
            ZStack {
                RoundedRectangle(cornerRadius: 2)
                    .fill(Color(red: 0.12, green: 0.1, blue: 0.08))
                    .overlay(
                        RoundedRectangle(cornerRadius: 2)
                            .stroke(playing ? Color(red: 0.22, green: 0.72, blue: 0.28) : Color(white: 0.35), lineWidth: 1)
                    )
                if edge {
                    Rectangle()
                        .fill(Color(white: 0.22))
                        .frame(width: 4)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
                PixelLabel(text: tape.name, scale: 1, color: Palette.paper)
                    .padding(4)
            }
            .frame(width: edge ? 28 : 36, height: 48)
        }
        .contentShape(Rectangle())
        .gesture(
            DragGesture(minimumDistance: 0).onEnded { value in
                let dx = value.translation.width
                let dy = value.translation.height
                if abs(dx) < 12, abs(dy) < 12 {
                    pull(tape.id)
                } else if abs(dx) > abs(dy) {
                    let shift = Int((dx / 48).rounded())
                    if shift != 0 {
                        session.moveTape(from: index, to: index + shift)
                    }
                }
            }
        )
        .a11yButton(tape.name, hint: "Pull it out. Drag sideways to reorder")
        .accessibilityAction(named: "Pull out") { pull(tape.id) }
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
        .frame(maxWidth: .infinity, minHeight: 120, alignment: .topLeading)
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
        }
        .padding(10)
        .background(Color(red: 0.42, green: 0.28, blue: 0.16))
        .overlay(Rectangle().stroke(Color(red: 0.85, green: 0.7, blue: 0.4), lineWidth: 2))
    }

    private func pull(_ id: UUID) {
        guard !pulled.contains(id) else { return }
        pulled.append(id)
    }
}
