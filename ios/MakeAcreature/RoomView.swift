import Combine
import SwiftUI

/// The dressed room (Phase 6): aquarium, tree picture, archive book,
/// fuse-box settings, favourite-creature frame, and a door sign with
/// the player's name. Post-MVP spots stay dashed. The sim keeps running.
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
    private let refresh = Timer.publish(every: 2, on: .main, in: .common).autoconnect()

    var body: some View {
        GeometryReader { geo in
            ZStack {
                wall
                shelves
                VStack(spacing: 0) {
                    Spacer(minLength: 20)
                    HStack(alignment: .bottom, spacing: 14) {
                        treePicture
                        Spacer(minLength: 0)
                        fuseBox
                    }
                    .padding(.horizontal, 16)
                    Spacer(minLength: 10)
                    HStack(alignment: .center, spacing: 10) {
                        lamp
                        tankOnStand
                        lamp
                    }
                    Spacer(minLength: 10)
                    HStack(alignment: .bottom, spacing: 14) {
                        book
                        Spacer(minLength: 0)
                        favouriteFrame
                        doorSign
                    }
                    .padding(.horizontal, 16)
                    HStack {
                        Button(action: onOpenLog) {
                            PixelLabel(text: "EVENT LOG", scale: 1, color: Palette.paper)
                        }
                        .buttonStyle(.plain)
                        .a11yButton("Event log")
                        Spacer()
                        PixelLabel(text: "PINCH IN · TAP TANK", scale: 1, color: Palette.paper)
                    }
                    .padding(.horizontal, 18)
                    .padding(.top, 8)
                    Spacer(minLength: geo.size.height * 0.08)
                }
            }
        }
        .onAppear {
            tree = session.speciesTree()
            signDraft = session.doorName
        }
        .onReceive(refresh) { _ in tree = session.speciesTree() }
        .sheet(isPresented: $editingSign) {
            NavigationStack {
                doorSignEditor
                    .toolbar(.hidden, for: .navigationBar)
            }
        }
    }

    private var wall: some View {
        Canvas { ctx, size in
            ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(Palette.wall))
            // Soft vertical lamp wash.
            for i in 0..<6 {
                let x = size.width * (0.12 + CGFloat(i) * 0.15)
                ctx.fill(
                    Path(CGRect(x: x - 20, y: 0, width: 40, height: size.height * 0.55)),
                    with: .color(Color(red: 1, green: 0.9, blue: 0.6).opacity(0.04))
                )
            }
            let floorY = size.height * 0.82
            ctx.fill(Path(CGRect(x: 0, y: floorY, width: size.width, height: size.height - floorY)), with: .color(Palette.floor))
            ctx.fill(Path(CGRect(x: 0, y: floorY, width: size.width, height: 3)), with: .color(Color(white: 0.08)))
            var x: CGFloat = 0
            while x < size.width {
                ctx.fill(Path(CGRect(x: x, y: floorY + 3, width: 2, height: size.height - floorY)), with: .color(Color(white: 0.06).opacity(0.5)))
                x += 48
            }
        }
        .ignoresSafeArea()
    }

    private var shelves: some View {
        VStack {
            HStack(spacing: 0) {
                Rectangle().fill(Color(red: 0.28, green: 0.2, blue: 0.12)).frame(height: 8)
            }
            .padding(.horizontal, 24)
            .padding(.top, 36)
            Spacer()
        }
        .allowsHitTesting(false)
    }

    private var lamp: some View {
        VStack(spacing: 2) {
            Circle().fill(Color(red: 1, green: 0.92, blue: 0.7).opacity(0.7)).frame(width: 10, height: 10)
            Rectangle().fill(Color(white: 0.25)).frame(width: 2, height: 28)
        }
        .allowsHitTesting(false)
    }

    private var tankOnStand: some View {
        Button(action: onBack) {
            VStack(spacing: 0) {
                ZStack {
                    Rectangle().fill(Color(white: 0.2))
                    VStack(spacing: 6) {
                        PixelLabel(text: "TANK \(session.slot + 1)", scale: 3, color: Palette.highlight)
                        PixelLabel(text: session.isDay ? "DAY" : "NIGHT", scale: 2, color: Palette.dimPaper)
                        PixelLabel(text: "\(session.liveCells) CELLS", scale: 2)
                        PixelLabel(text: "TAP TO LOOK IN", scale: 1, color: Palette.dimPaper)
                    }
                }
                .frame(width: 220, height: 150)
                .overlay(Rectangle().stroke(Color(white: 0.45), lineWidth: 3))
                Rectangle().fill(Color(red: 0.3, green: 0.22, blue: 0.14)).frame(width: 240, height: 14)
                HStack {
                    Rectangle().fill(Color(red: 0.26, green: 0.18, blue: 0.11)).frame(width: 12, height: 44)
                    Spacer()
                    Rectangle().fill(Color(red: 0.26, green: 0.18, blue: 0.11)).frame(width: 12, height: 44)
                }
                .frame(width: 220)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Tank \(session.slot + 1), \(session.liveCells) cells", hint: "Look into the tank")
    }

    private var treePicture: some View {
        Button(action: onOpenTree) {
            VStack(spacing: 4) {
                TreeOfLifeView(nodes: tree, compact: true)
                    .overlay(Rectangle().stroke(Color(red: 0.5, green: 0.38, blue: 0.2), lineWidth: 4))
                PixelLabel(text: "TREE OF LIFE", scale: 1, color: Palette.dimPaper)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Tree of life")
    }

    private var fuseBox: some View {
        Button(action: onOpenSettings) {
            VStack(spacing: 4) {
                ZStack {
                    Rectangle().fill(Color(red: 0.07, green: 0.08, blue: 0.07))
                    VStack(spacing: 4) {
                        ForEach(0..<3, id: \.self) { i in
                            HStack(spacing: 4) {
                                Circle()
                                    .fill(breakerOn(i) ? Color(red: 0.78, green: 1, blue: 0.35) : Color(white: 0.22))
                                    .frame(width: 8, height: 8)
                                    .overlay(Circle().stroke(Color(white: 0.85), lineWidth: 1))
                                Rectangle()
                                    .fill(Color(red: 0.96, green: 0.9, blue: 0.72))
                                    .frame(width: 28, height: 5)
                            }
                        }
                    }
                }
                .frame(width: 56, height: 56)
                .overlay(Rectangle().stroke(Color(red: 0.93, green: 0.86, blue: 0.62), lineWidth: 2))
                PixelLabel(text: "FUSE BOX", scale: 1, color: Palette.paper)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Fuse box", hint: "Settings")
    }

    /// Sound, clock, and swirl lamps. Off is a dark well; on is a bright lamp.
    private func breakerOn(_ index: Int) -> Bool {
        switch index {
        case 0: return session.soundOn
        case 1: return session.alwaysDay
        default: return session.effectiveSwirl
        }
    }

    private var book: some View {
        Button(action: onOpenBook) {
            VStack(spacing: 4) {
                ZStack {
                    Rectangle().fill(Color(red: 0.45, green: 0.2, blue: 0.14))
                    Rectangle().fill(Color(red: 0.55, green: 0.26, blue: 0.18)).frame(width: 54, height: 72).offset(x: 4)
                    PixelLabel(text: "\(session.archive.specimens.count)", scale: 2, color: Palette.highlight).offset(x: 4)
                }
                .frame(width: 64, height: 80)
                PixelLabel(text: "THE BOOK", scale: 1, color: Palette.dimPaper)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("The book", hint: "Archive")
    }

    private var favouriteFrame: some View {
        Button {
            session.cycleFavourite()
        } label: {
            VStack(spacing: 4) {
                ZStack {
                    Rectangle().fill(Color(red: 0.35, green: 0.28, blue: 0.18))
                    if let fav = session.favouriteSpecimen {
                        portrait(fav, side: 48)
                    } else {
                        PixelLabel(text: "PICK", scale: 1, color: Color(white: 0.4))
                    }
                }
                .frame(width: 56, height: 56)
                .overlay(Rectangle().stroke(Color(red: 0.55, green: 0.42, blue: 0.22), lineWidth: 3))
                PixelLabel(text: "FAVOURITE", scale: 1, color: Palette.dimPaper)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Favourite frame", hint: "Cycle the framed creature")
        .contextMenu {
            Button("Clear favourite") { session.clearFavourite() }
        }
    }

    private var doorSign: some View {
        Button { editingSign = true } label: {
            VStack(spacing: 4) {
                ZStack {
                    RoundedRectangle(cornerRadius: 2).fill(Color(red: 0.55, green: 0.45, blue: 0.28))
                    PixelLabel(
                        text: shortSign(session.doorName),
                        scale: 1,
                        color: Color(red: 0.15, green: 0.1, blue: 0.05)
                    )
                }
                .frame(width: 72, height: 36)
                .overlay(Rectangle().stroke(Color(red: 0.35, green: 0.28, blue: 0.15), lineWidth: 2))
                PixelLabel(text: "DOOR SIGN", scale: 1, color: Palette.dimPaper)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("Door sign", hint: session.doorName.isEmpty ? "Set your name" : session.doorName)
    }

    private var doorSignEditor: some View {
        VStack(alignment: .leading, spacing: 12) {
            PixelLabel(text: "DOOR SIGN", scale: 3, color: Palette.highlight)
            PixelLabel(text: "YOUR NAME ON THE DOOR", scale: 1, color: Palette.dimPaper)
            PixelTextField(text: $signDraft, placeholder: "NAME") {
                session.setDoorName(signDraft)
                editingSign = false
            }
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
        .preferredColorScheme(.dark)
    }

    private func shortSign(_ name: String) -> String {
        let up = name.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        if up.isEmpty { return "????" }
        return up.count > 10 ? String(up.prefix(9)) + "." : up
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
