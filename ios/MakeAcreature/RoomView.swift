import SwiftUI

/// The minimal room (Phase 3): a cosy pixel wall around the aquarium
/// with the tree-of-life picture and the archive book. The fuse box,
/// favourite frame and door sign are reserved spots until Phase 6. The
/// sim keeps running; tapping the tank or pinching in goes back.
struct RoomView: View {
    @ObservedObject var session: TankSession
    var onBack: () -> Void
    var onOpenBook: () -> Void
    var onOpenTree: () -> Void

    @State private var tree: [SpeciesNode] = []
    private let refresh = Timer.publish(every: 2, on: .main, in: .common).autoconnect()

    var body: some View {
        GeometryReader { geo in
            ZStack {
                wall
                VStack(spacing: 0) {
                    Spacer(minLength: 24)
                    HStack(alignment: .bottom, spacing: 18) {
                        treePicture
                        Spacer(minLength: 0)
                        reserved("FUSE BOX")
                    }
                    .padding(.horizontal, 18)
                    Spacer(minLength: 16)
                    tankOnStand
                    Spacer(minLength: 12)
                    HStack(alignment: .bottom, spacing: 18) {
                        book
                        Spacer(minLength: 0)
                        reserved("FRAME")
                        reserved("DOOR SIGN")
                    }
                    .padding(.horizontal, 18)
                    Spacer(minLength: geo.size.height * 0.14)
                }
            }
        }
        .onAppear { tree = session.speciesTree() }
        .onReceive(refresh) { _ in tree = session.speciesTree() }
    }

    private var wall: some View {
        Canvas { ctx, size in
            ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(Palette.wall))
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
    }

    private func reserved(_ label: String) -> some View {
        VStack(spacing: 4) {
            Rectangle()
                .stroke(Color(white: 0.3), style: StrokeStyle(lineWidth: 2, dash: [4, 4]))
                .frame(width: 56, height: 56)
                .overlay(PixelLabel(text: "SOON", scale: 1, color: Color(white: 0.35)))
            PixelLabel(text: label, scale: 1, color: Color(white: 0.4))
        }
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
