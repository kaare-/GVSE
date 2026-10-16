import SwiftUI

/// Archive-side event log (Phase 6): milestones this session, newest first.
struct EventLogEntry: Identifiable, Equatable {
    let id: UInt64
    let tick: UInt64
    let kind: UInt32
    let message: String
}

struct EventLogView: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void

    var body: some View {
        ZStack {
            Palette.panel.ignoresSafeArea()
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    PixelLabel(text: "EVENT LOG", scale: 3, color: Palette.highlight)
                    Spacer()
                    Button(action: onClose) {
                        PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                    }
                    .buttonStyle(.plain)
                }
                PixelLabel(
                    text: "\(session.eventLog.count) ENTRIES  THIS SESSION",
                    scale: 1,
                    color: Palette.dimPaper
                )
                if session.eventLog.isEmpty {
                    Spacer()
                    PixelLabel(text: "NOTHING YET. LIFE WILL WRITE HERE.", scale: 2, color: Palette.dimPaper)
                        .frame(maxWidth: .infinity)
                    Spacer()
                } else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 6) {
                            ForEach(session.eventLog) { e in
                                HStack {
                                    PixelLabel(text: String(format: "T%06d", e.tick), scale: 1, color: Palette.dimPaper)
                                    PixelLabel(text: e.message, scale: 2)
                                    Spacer()
                                }
                                .padding(4)
                                .background(Color(white: 0.1))
                            }
                        }
                    }
                }
            }
            .padding(12)
        }
        .preferredColorScheme(.dark)
    }
}
