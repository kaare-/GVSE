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
    /// False while the scissors hang on the hook.
    @State private var shearHeld = false
    /// View point at the bottom edge of the scissors. That edge is the cut.
    @State private var shearPoint: CGPoint = .zero
    /// 1 open, 0 shut. Springs shut when the blades meet a vine.
    @State private var shearSpread: CGFloat = 1
    @State private var shearSnipped = false
    @State private var thumbs: [UIImage?] = [nil, nil, nil, nil]
    @State private var pinchEntered = false
    @State private var skyPhase = RoomMood.phase()
    @State private var lampOn = RoomMood.storedLamp
    @State private var shelfOn = RoomMood.storedShelf
    @State private var candleLit = RoomMood.storedCandle
    private let cast = LobbyScript.cast
    private let refresh = Timer.publish(every: 2, on: .main, in: .common).autoconnect()
    private let skyClock = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

    private var roomMood: RoomMood {
        RoomMood(phase: skyPhase, lampOn: lampOn, shelfOn: shelfOn, candleLit: candleLit)
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
                }
                place(RoomArt.deckHit, layout) { deckButton }
                place(RoomArt.deckPower, layout) { deckPowerButton }
                place(RoomArt.favHit, layout) { crateButton(layout) }
                place(RoomArt.deskLampHit, layout) { deskLampControl }
                place(RoomArt.shelfLamp, layout) { shelfLampControl }
                place(RoomArt.candleHit, layout) { candleControl }
                if cast.openingGlass {
                    place(RoomArt.openingGlass, layout) { openingButton }
                }
                scissorsTool(layout)
            }
            .coordinateSpace(name: "room")
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
            let mood = RoomMood(phase: skyPhase, lampOn: lampOn, candleLit: candleLit)
            AmbientAudio.shared.setCv([
                Float(mood.sunPower) / 30,
                mood.raining ? 1 : 0,
                mood.lampOn ? 1 : 0,
                mood.candleLit ? 1 : 0,
                min(1, Float(session.liveCells) / 2_000),
            ])
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
                    PixelLabel(text: "TREE OF LIFE", scale: 1, color: Palette.dimPaper)
                }
                .frame(width: hit.width, height: hit.height)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Tree of life")
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
                    if scale > 1.18 { enterGlass { session.openGlass() } }
                }
            )
            .accessibilityAction(named: "Look in") { enterGlass { session.openGlass() } }
            .a11yButton("Glass of water", hint: "Pinch in to look into the glass")
    }

    /// A mote in the opening glass. It is not baked into the picture.
    /// Coordinates match the centred picture, the same way the vines do.
    private func openingMote(_ layout: RoomLayout) -> some View {
        let g = RoomArt.openingWater
        let s = layout.s
        let waterX = CGFloat(g.x)
        let waterY = CGFloat(g.y)
        let waterW = CGFloat(g.w)
        let waterH = CGFloat(g.h)
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

    private func bookButton(_: RoomLayout) -> some View {
        Button(action: onOpenBook) {
            PixelLabel(text: "BOOK", scale: 1, color: Palette.highlight)
        }
        .buttonStyle(.plain)
        .a11yButton("Book", hint: "Archive, \(session.archive.specimens.count)")
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
            if let sheet = UIImage(named: "power") {
                PixelStrip(image: sheet, frames: 2, frame: session.tapePlaying ? 1 : 0)
            } else {
                Rectangle()
                    .fill(session.tapePlaying
                        ? Color(red: 0.22, green: 0.72, blue: 0.28)
                        : Color(red: 0.78, green: 0.16, blue: 0.14))
            }
        }
        .buttonStyle(FullHit())
        .a11yButton(session.tapePlaying ? "Deck power, on" : "Deck power, off")
    }

    private var logButton: some View {
        Button(action: onOpenLog) {
            PixelLabel(text: "LOG", scale: 1, color: Palette.paper)
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
                    if !creatureName.isEmpty {
                        PixelLabel(
                            text: creatureName,
                            scale: 1,
                            color: Color(red: 0.16, green: 0.10, blue: 0.04)
                        )
                        .fixedSize()
                    }
                }
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Framed creature", hint: creatureName.isEmpty ? "Cycle the framed creature" : creatureName)
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

    private func scissorsTool(_ layout: RoomLayout) -> some View {
        let s = layout.s
        let h = scissorsHeight(s)
        let hook = layout.rect(RoomArt.shearHook)
        let x = shearHeld ? shearPoint.x : hook.midX
        let y = shearHeld ? shearPoint.y - h / 2 : hook.midY
        return scissorsGraphic(spread: shearSpread, s: s)
            .frame(width: 13 * s, height: h)
            .contentShape(Rectangle())
            .position(x: x, y: y)
            .gesture(
                DragGesture(minimumDistance: 0, coordinateSpace: .named("room"))
                    .onChanged { value in
                        shearHeld = true
                        shearPoint = value.location
                        let over = vineAt(value.location, layout: layout) != nil
                        if over && !shearSnipped {
                            withAnimation(.spring(response: 0.16, dampingFraction: 0.55)) {
                                shearSpread = 0
                            }
                            _ = cutVines(at: value.location, layout: layout)
                            shearSnipped = true
                        } else if !over {
                            shearSnipped = false
                            withAnimation(.spring(response: 0.18, dampingFraction: 0.7)) {
                                shearSpread = 1
                            }
                        }
                    }
                    .onEnded { value in
                        let hookHit = layout.rect(RoomArt.shearHook).insetBy(dx: -18, dy: -18)
                        if hookHit.contains(value.location) {
                            shearHeld = false
                            shearSnipped = false
                            withAnimation(.spring(response: 0.2, dampingFraction: 0.7)) {
                                shearSpread = 1
                            }
                            return
                        }
                        if vineAt(value.location, layout: layout) != nil, !shearSnipped {
                            withAnimation(.spring(response: 0.16, dampingFraction: 0.55)) {
                                shearSpread = 0
                            }
                            _ = cutVines(at: value.location, layout: layout)
                        }
                        shearPoint = value.location
                        shearHeld = true
                    }
            )
            .a11yButton(shearHeld ? "Scissors" : "Scissors on the hook", hint: "Drag them over a vine to cut. Drop them on the hook to hang them up")
            .accessibilityAction(named: "Hang up") {
                shearHeld = false
                shearSpread = 1
            }
    }

    /// The strip is 18 room pixels tall. The last pixel is the cut.
    private func scissorsHeight(_ s: CGFloat) -> CGFloat { 18 * s }

    /// `spread` is 1 open and 0 shut. The frames live in `art/sprites/scissors.aseprite`.
    private func scissorsGraphic(spread: CGFloat, s: CGFloat) -> some View {
        let frames = 4
        let index = Int(((1 - min(max(spread, 0), 1)) * CGFloat(frames - 1)).rounded())
        return Group {
            if let sheet = UIImage(named: "scissors") {
                PixelStrip(image: sheet, frames: frames, frame: index)
            }
        }
        .frame(width: 13 * s, height: scissorsHeight(s))
    }

    /// Index of the vine whose stem the bottom edge crosses.
    private func vineAt(_ bottom: CGPoint, layout: RoomLayout) -> Int? {
        let s = layout.s
        let x = (bottom.x - layout.origin.x) / s
        let y = Int(((bottom.y - layout.origin.y) / s).rounded())
        for i in RoomArt.vines.indices {
            let vine = RoomArt.vines[i]
            let len = vines.indices.contains(i) ? Int(vines[i]) : 6
            if vine.hangs {
                guard x >= CGFloat(vine.x) - 8, x <= CGFloat(vine.x) + 8 else { continue }
                guard y > vine.y, y < vine.y + len else { continue }
                return i
            }
            guard x >= CGFloat(vine.x) - 6, x <= CGFloat(vine.x) + 6 else { continue }
            guard y < vine.y, y > vine.y - len else { continue }
            return i
        }
        return nil
    }

    /// The bottom edge of the scissors is the cut. The stub that remains
    /// is however much vine is above that edge.
    private func cutVines(at bottom: CGPoint, layout: RoomLayout) -> Bool {
        guard let i = vineAt(bottom, layout: layout) else { return false }
        let s = layout.s
        let cutY = Int(((bottom.y - layout.origin.y) / s).rounded())
        let vine = RoomArt.vines[i]
        let keep: Int
        if vine.hangs {
            keep = max(4, cutY - vine.y)
        } else {
            keep = max(4, vine.y - cutY)
        }
        vines[i] = LobbyGrowth.setLength(i, CGFloat(keep))
        return true
    }

    private var deskLampControl: some View {
        Color.clear
            .contentShape(Rectangle())
            .onTapGesture(count: 2) { toggleLamp() }
            .accessibilityAction(named: lampOn ? "Turn off" : "Turn on") { toggleLamp() }
            .a11yButton(lampOn ? "Desk lamp, on" : "Desk lamp, off", hint: "Double tap to turn the light on or off")
    }

    private var shelfLampControl: some View {
        Color.clear
            .contentShape(Rectangle())
            .onTapGesture(count: 2) { toggleShelf() }
            .accessibilityAction(named: shelfOn ? "Turn off" : "Turn on") { toggleShelf() }
            .a11yButton(shelfOn ? "Shelf lamp, on" : "Shelf lamp, off", hint: "Double tap to turn the light on or off")
    }

    private var candleControl: some View {
        Color.clear
            .contentShape(Rectangle())
            .onTapGesture(count: 2) { toggleCandle() }
            .simultaneousGesture(
                DragGesture(minimumDistance: 8).onEnded { value in
                    if abs(value.translation.width) > 12 { blowOutCandle() }
                }
            )
            .accessibilityAction(named: candleLit ? "Put out" : "Light") { toggleCandle() }
            .accessibilityAction(named: "Blow out") { blowOutCandle() }
            .a11yButton(candleLit ? "Candle, lit" : "Candle", hint: "Double tap to light or put out. Swipe to blow it out")
    }

    private func toggleLamp() {
        lampOn.toggle()
        UserDefaults.standard.set(lampOn, forKey: "lobbyLamp")
    }

    private func toggleShelf() {
        shelfOn.toggle()
        UserDefaults.standard.set(shelfOn, forKey: "lobbyShelfLamp")
    }

    private func toggleCandle() {
        candleLit.toggle()
        UserDefaults.standard.set(candleLit, forKey: "lobbyCandle")
    }

    private func blowOutCandle() {
        guard candleLit else { return }
        candleLit = false
        UserDefaults.standard.set(false, forKey: "lobbyCandle")
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
        let bright = Color(red: 0.635, green: 0.839, blue: 0.435)
        let leaf = Color(red: 0.424, green: 0.722, blue: 0.290)
        let dark = Color(red: 0.18, green: 0.42, blue: 0.16)
        return Canvas { ctx, _ in
            for (i, vine) in RoomArt.vines.enumerated() {
                let len = vines.indices.contains(i) ? vines[i] : 6
                if vine.hangs {
                    let x = CGFloat(vine.x)
                    let n = max(1, Int(len))
                    for step in 0..<n {
                        let y = CGFloat(vine.y + step)
                        let stem = CGRect(
                            x: (x - 1) * s,
                            y: y * s,
                            width: max(s * 3, 1),
                            height: max(s, 1)
                        )
                        ctx.fill(Path(stem), with: .color(bright))
                        if step > 8 && step % 4 == 0 {
                            let side: CGFloat = step % 8 == 0 ? -3 : 2
                            let leafRect = CGRect(
                                x: (x + side) * s,
                                y: y * s,
                                width: max(s * 2, 1),
                                height: max(s, 1)
                            )
                            ctx.fill(Path(leafRect), with: .color(dark))
                        }
                    }
                    continue
                }
                var x = CGFloat(vine.x)
                var y = CGFloat(vine.y) + (vine.hangs ? min(len, 10) : 0)
                let limit = vine.hangs
                    ? min(CGFloat(RoomArt.floorY - 8), CGFloat(vine.y) + len)
                    : max(2, CGFloat(vine.y) - len)
                var step = 0
                while vine.hangs ? y < limit : y > limit {
                    let ink = step % 3 == 0 ? bright : leaf
                    let rect = CGRect(x: x * s, y: y * s, width: max(s, 1), height: max(s * 2, 1))
                    ctx.fill(Path(rect), with: .color(ink))
                    if step % 3 == 0 {
                        let leafX = vine.hangs ? x - 1 : x + 1
                        let leafRect = CGRect(x: leafX * s, y: y * s, width: max(s, 1), height: max(s, 1))
                        ctx.fill(Path(leafRect), with: .color(dark))
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

    static func setLength(_ index: Int, _ length: CGFloat) -> CGFloat {
        let hangs = RoomArt.vines.indices.contains(index) && RoomArt.vines[index].hangs
        let cap = hangs ? 56.0 : 14.0
        let len = min(cap, max(4, Double(length)))
        UserDefaults.standard.set(len, forKey: "lobbyVine\(index)")
        UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: "lobbySeen\(index)")
        return CGFloat(len)
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
    @State private var recording = false
    @State private var blankOnPlayer = false
    @State private var started = Date()
    @State private var pitch = 0.42
    @State private var level = 0.65
    @State private var bright = false
    @State private var gateOpen = true
    @State private var pitchCv = -1
    @State private var levelCv = -1
    @State private var armed: Int?
    private let cvNames = ["DAY", "RAIN", "LAMP", "CANDLE", "ROOM"]
    private let clock = Timer.publish(every: 0.5, on: .main, in: .common).autoconnect()

    var body: some View {
        ZStack {
            Color.black.opacity(0.28)
                .ignoresSafeArea()
                .onTapGesture(perform: closeDesk)
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
                    Button(action: closeDesk) {
                        PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                    }
                    .buttonStyle(.plain)
                }
                shelf
                pile
                player
                if recording { rack }
                Spacer(minLength: 0)
            }
            .padding(16)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(Color(red: 0.16, green: 0.11, blue: 0.08).opacity(0.94))
            .padding(.horizontal, 28)
            .padding(.vertical, 48)
        }
        .preferredColorScheme(.dark)
        .onReceive(clock) { _ in
            guard recording else { return }
            if Date().timeIntervalSince(started) >= 36 {
                finishRecord()
            } else {
                session.soloTake(currentTake())
            }
        }
    }

    private var shelf: some View {
        HStack(alignment: .bottom, spacing: 3) {
            ForEach(Array(session.shelf.enumerated()), id: \.element.id) { index, tape in
                shelfTape(tape, index: index)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(height: 86)
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
                            .stroke(playing ? Color(red: 0.22, green: 0.72, blue: 0.28) : Color(white: 0.55), lineWidth: 1)
                    )
                PixelLabel(text: tape.name, scale: 1, color: Palette.paper)
                    .frame(width: 58, height: 12)
                    .rotationEffect(.degrees(-90))
                    .frame(width: 14, height: 60)
                    .allowsHitTesting(false)
            }
        }
        .frame(width: 18, height: 72)
        .contentShape(Rectangle())
        .gesture(
            DragGesture(minimumDistance: 0).onEnded { value in
                let dx = value.translation.width
                let dy = value.translation.height
                if abs(dx) < 12, abs(dy) < 12 {
                    if out { restore(tape.id) } else { pull(tape.id) }
                } else if abs(dx) > abs(dy) {
                    let shift = Int((dx / 28).rounded())
                    if shift != 0 {
                        session.moveTape(from: index, to: index + shift)
                    }
                }
            }
        )
        .a11yButton(out ? "Empty slot" : tape.name, hint: out ? "Put the tape back. Drag sideways to reorder" : "Pull it out. Drag sideways to reorder")
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

    /// The player under the shelf. Record pulls a blank off the rack.
    private var player: some View {
        HStack(alignment: .center, spacing: 14) {
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 3)
                    .fill(Color(red: 0.08, green: 0.07, blue: 0.06))
                    .frame(width: 168, height: 64)
                PixelLabel(text: "RACK", scale: 1, color: Palette.dimPaper)
                    .offset(x: 176)
                blankCassette
                    .offset(x: blankOnPlayer ? 18 : 168)
                    .animation(.easeInOut(duration: 0.35), value: blankOnPlayer)
            }
            .frame(width: 230, height: 64, alignment: .leading)
            Button(action: toggleRecord) {
                ZStack {
                    Circle()
                        .fill(Color(red: 0.82, green: 0.12, blue: 0.1))
                        .frame(width: 28, height: 28)
                    if recording {
                        RoundedRectangle(cornerRadius: 1)
                            .fill(Color.white)
                            .frame(width: 10, height: 10)
                    }
                }
            }
            .buttonStyle(.plain)
            .a11yButton(recording ? "Stop recording" : "Record", hint: "Record onto a blank cassette")
        }
    }

    private var blankCassette: some View {
        RoundedRectangle(cornerRadius: 2)
            .fill(Color(red: 0.55, green: 0.5, blue: 0.42))
            .frame(width: 46, height: 30)
            .overlay(PixelLabel(text: "EMPTY", scale: 1, color: Color(red: 0.12, green: 0.08, blue: 0.04)))
    }

    /// Buttons, sliders, and patch cables. Environmental jacks feed the sliders.
    private var rack: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                ForEach(cvNames.indices, id: \.self) { index in
                    Button {
                        armed = armed == index ? nil : index
                    } label: {
                        VStack(spacing: 3) {
                            Circle()
                                .fill(armed == index ? Palette.highlight : Color(white: 0.45))
                                .frame(width: 12, height: 12)
                            PixelLabel(text: cvNames[index], scale: 1, color: Palette.dimPaper)
                        }
                    }
                    .buttonStyle(.plain)
                    .gesture(
                        DragGesture(minimumDistance: 12, coordinateSpace: .named("rack")).onEnded { value in
                            plug(index, at: value.location.y)
                        }
                    )
                    .a11yButton(cvNames[index], hint: "Patch this into a slider")
                }
            }
            sliderRow("PITCH", value: $pitch, cv: $pitchCv)
            sliderRow("LEVEL", value: $level, cv: $levelCv)
            HStack(spacing: 8) {
                rackButton(bright ? "TONE" : "SOFT") { bright.toggle(); hear() }
                rackButton(gateOpen ? "OPEN" : "SHUT") { gateOpen.toggle(); hear() }
            }
            if pitchCv >= 0 || levelCv >= 0 {
                PixelLabel(text: cableCaption, scale: 1, color: Palette.highlight)
            }
        }
        .padding(8)
        .background(Color(red: 0.05, green: 0.06, blue: 0.05))
        .coordinateSpace(name: "rack")
        .overlay(cableLayer)
    }

    private var cableCaption: String {
        var parts: [String] = []
        if pitchCv >= 0, cvNames.indices.contains(pitchCv) { parts.append("\(cvNames[pitchCv]) PITCH") }
        if levelCv >= 0, cvNames.indices.contains(levelCv) { parts.append("\(cvNames[levelCv]) LEVEL") }
        return parts.joined(separator: "  ")
    }

    private var cableLayer: some View {
        GeometryReader { geo in
            Canvas { ctx, size in
                let ink = Color(red: 0.9, green: 0.72, blue: 0.28)
                func line(_ from: Int, _ y: CGFloat) {
                    guard cvNames.indices.contains(from) else { return }
                    var path = Path()
                    let x = size.width * (CGFloat(from) + 0.5) / CGFloat(cvNames.count)
                    path.move(to: CGPoint(x: x, y: 18))
                    path.addLine(to: CGPoint(x: size.width - 18, y: y))
                    ctx.stroke(path, with: .color(ink), lineWidth: 2)
                }
                if pitchCv >= 0 { line(pitchCv, 58) }
                if levelCv >= 0 { line(levelCv, 96) }
            }
            .allowsHitTesting(false)
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .allowsHitTesting(false)
    }

    private func sliderRow(_ title: String, value: Binding<Double>, cv: Binding<Int>) -> some View {
        HStack(spacing: 8) {
            PixelLabel(text: title, scale: 1, color: Palette.paper)
                .frame(width: 48, alignment: .leading)
            Slider(value: value, in: 0...1)
                .onChange(of: value.wrappedValue) { _ in hear() }
            Button {
                if let armed {
                    cv.wrappedValue = cv.wrappedValue == armed ? -1 : armed
                    self.armed = nil
                } else {
                    cv.wrappedValue = -1
                }
                hear()
            } label: {
                Circle()
                    .stroke(cv.wrappedValue >= 0 ? Color(red: 0.9, green: 0.72, blue: 0.28) : Color(white: 0.4), lineWidth: 2)
                    .background(Circle().fill(Color(white: 0.12)))
                    .frame(width: 16, height: 16)
            }
            .buttonStyle(.plain)
            .a11yButton("\(title) jack", hint: cv.wrappedValue >= 0 ? "Unplug" : "Plug a cable here")
        }
    }

    private func rackButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            PixelPanel { PixelLabel(text: title, scale: 1, color: Palette.paper) }
        }
        .buttonStyle(.plain)
    }

    private func plug(_ source: Int, at y: CGFloat) {
        if y < 70 {
            pitchCv = pitchCv == source ? -1 : source
        } else {
            levelCv = levelCv == source ? -1 : source
        }
        armed = nil
        hear()
    }

    private func toggleRecord() {
        if recording { finishRecord() } else { beginRecord() }
    }

    private func beginRecord() {
        recording = true
        blankOnPlayer = true
        started = Date()
        session.soloTake(currentTake())
    }

    private func finishRecord() {
        let dur = min(36, max(4, Date().timeIntervalSince(started)))
        session.keepTake(currentTake(), duration: dur)
        recording = false
        blankOnPlayer = false
        armed = nil
    }

    private func currentTake() -> AmbientAudio.Take {
        AmbientAudio.Take(
            pitch: Float(pitch),
            level: Float(gateOpen ? level : 0.04),
            bright: bright,
            pitchCv: pitchCv,
            levelCv: levelCv
        )
    }

    private func hear() {
        guard recording else { return }
        session.soloTake(currentTake())
    }

    private func closeDesk() {
        if recording { finishRecord() }
        onClose()
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
            Button {
                restore(tape.id)
            } label: {
                PixelPanel {
                    PixelLabel(text: "SHELF", scale: 1, color: Palette.paper)
                }
            }
            .buttonStyle(.plain)
            .a11yButton("Put \(tape.name) back")
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
