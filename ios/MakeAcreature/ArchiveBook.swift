import SwiftUI
import UIKit

/// The archive book: a grid of portraits, and a page per creature with
/// notes, rename, text + image export, and "new tank from this".
struct ArchiveBook: View {
    @ObservedObject var session: TankSession
    @ObservedObject var store: ArchiveStore
    var onClose: () -> Void

    @State private var open: ArchivedSpecimen?

    init(session: TankSession, onClose: @escaping () -> Void) {
        _session = ObservedObject(wrappedValue: session)
        _store = ObservedObject(wrappedValue: session.archive)
        self.onClose = onClose
    }

    private let columns = [GridItem(.adaptive(minimum: 96), spacing: 10)]

    var body: some View {
        ZStack {
            Palette.panel.ignoresSafeArea()
            if let open {
                SpecimenPage(
                    specimen: open,
                    session: session,
                    onBack: { self.open = nil },
                    onDelete: {
                        store.remove(open)
                        self.open = nil
                    }
                )
            } else {
                grid
            }
        }
        .preferredColorScheme(.dark)
    }

    private var grid: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                PixelLabel(text: "THE BOOK", scale: 3, color: Palette.highlight)
                Spacer()
                PixelLabel(text: "\(store.specimens.count)", scale: 2, color: Palette.dimPaper)
                Button(action: onClose) {
                    PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                }
                .buttonStyle(.plain)
            }
            HStack(spacing: 6) {
                PixelLabel(text: "STARTERS", scale: 1, color: Palette.dimPaper)
                ForEach([(0, "MOSS"), (1, "PLANT"), (2, "CROWN")], id: \.0) { kind, label in
                    Button {
                        _ = session.addStarter(kind: UInt8(kind))
                    } label: {
                        PixelPanel { PixelLabel(text: label, scale: 1) }
                    }
                    .buttonStyle(.plain)
                }
            }
            if store.specimens.isEmpty {
                Spacer()
                PixelLabel(text: "EMPTY. LOOK AT A CREATURE AND ARCHIVE IT.", scale: 2, color: Palette.dimPaper)
                    .frame(maxWidth: .infinity)
                Spacer()
            } else {
                ScrollView {
                    LazyVGrid(columns: columns, spacing: 10) {
                        ForEach(store.specimens) { s in
                            Button {
                                open = s
                            } label: {
                                PixelPanel {
                                    VStack(spacing: 4) {
                                        portrait(s, side: 72)
                                        PixelLabel(text: short(s.displayName), scale: 1)
                                        PixelLabel(text: "FORM \(roman(s.info.species_depth))", scale: 1, color: Palette.dimPaper)
                                    }
                                }
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
        }
        .padding(12)
    }

    private func short(_ name: String) -> String {
        let up = name.uppercased()
        return up.count > 14 ? String(up.prefix(13)) + "." : up
    }
}

/// Pixel-exact portrait fitted into a square, nearest-neighbour.
func portrait(_ s: ArchivedSpecimen, side: CGFloat) -> some View {
    Group {
        if let img = s.portrait {
            Image(uiImage: img)
                .interpolation(.none)
                .resizable()
                .aspectRatio(contentMode: .fit)
        } else {
            Rectangle().fill(s.colour)
        }
    }
    .frame(width: side, height: side)
    .background(Color(white: 0.12))
}

/// One page of the book.
struct SpecimenPage: View {
    let specimen: ArchivedSpecimen
    @ObservedObject var session: TankSession
    var onBack: () -> Void
    var onDelete: () -> Void

    @State private var name = ""
    @State private var notes = ""
    @State private var share = false
    @State private var confirmDelete = false
    @State private var confirmNewTank = false

    private var dirty: Bool {
        name != specimen.displayName || notes != specimen.notes
    }

    var body: some View {
        NavigationStack {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Button(action: onBack) {
                        PixelPanel { PixelLabel(text: "< BOOK", scale: 2) }
                    }
                    .buttonStyle(.plain)
                    Spacer()
                    Button { share = true } label: {
                        PixelPanel { PixelLabel(text: "SHARE", scale: 2) }
                    }
                    .buttonStyle(.plain)
                }
                HStack(alignment: .top, spacing: 12) {
                    portrait(specimen, side: 160)
                    VStack(alignment: .leading, spacing: 4) {
                        PixelLabel(text: specimen.displayName.uppercased(), scale: 2, color: Palette.highlight)
                        if specimen.displayName != specimen.speciesName {
                            PixelLabel(text: specimen.speciesName.uppercased(), scale: 1, color: Palette.dimPaper)
                        }
                        PixelLabel(text: "FORM \(roman(specimen.info.species_depth))  GEN \(specimen.info.generation)", scale: 1)
                        PixelLabel(text: "CELLS \(specimen.info.cells)  MUT \(specimen.info.mutations_since_seed)", scale: 1)
                        PixelLabel(text: "AGE \(specimen.info.age_ticks / 30)S OF \(specimen.info.max_age_ticks / 30)S", scale: 1)
                        PixelLabel(text: "CHILDREN \(specimen.info.children)  KIN \(specimen.info.kin_alive)", scale: 1)
                        PixelLabel(text: "TANK \(String(specimen.info.tank_seed, radix: 16).uppercased().prefix(8))  T \(specimen.info.tick)", scale: 1, color: Palette.dimPaper)
                    }
                }
                if !specimen.lineage.isEmpty {
                    PixelLabel(text: specimen.lineage.uppercased(), scale: 1, color: Palette.dimPaper)
                        .fixedSize(horizontal: false, vertical: true)
                }
                PixelLabel(text: "NAME", scale: 2, color: Palette.dimPaper)
                PixelTextField(text: $name)
                PixelLabel(text: "NOTES", scale: 2, color: Palette.dimPaper)
                PixelTextEditor(text: $notes)
                if dirty {
                    Button {
                        if let edited = specimen.edited(name: name, notes: notes) {
                            session.archive.update(edited)
                        }
                        onBack()
                    } label: {
                        PixelPanel { PixelLabel(text: "SAVE PAGE", scale: 2, color: Palette.highlight) }
                    }
                    .buttonStyle(.plain)
                }
                HStack(spacing: 10) {
                    Button { confirmNewTank = true } label: {
                        PixelPanel { PixelLabel(text: "NEW TANK FROM THIS", scale: 2) }
                    }
                    .buttonStyle(.plain)
                    Spacer()
                    Button { confirmDelete = true } label: {
                        PixelPanel { PixelLabel(text: "DELETE", scale: 2, color: Palette.dimPaper) }
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(12)
        }
        .onAppear {
            name = specimen.displayName
            notes = specimen.notes
        }
        .sheet(isPresented: $share) {
            ShareSheet(items: [specimen.pageImage(), specimen.page])
        }
        .confirmationDialog("Replace tank \(session.slot + 1) with a new tank seeded from this creature?", isPresented: $confirmNewTank, titleVisibility: .visible) {
            Button("New tank", role: .destructive) {
                session.newTank(from: specimen)
                onBack()
            }
        }
        .confirmationDialog("Remove this page from the book?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete", role: .destructive, action: onDelete)
        }
        .toolbar(.hidden, for: .navigationBar)
        }
    }
}

struct ShareSheet: UIViewControllerRepresentable {
    let items: [Any]

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ uiViewController: UIActivityViewController, context: Context) {}
}
