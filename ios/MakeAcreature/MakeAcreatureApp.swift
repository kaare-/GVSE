import SwiftUI

@main
struct MakeAcreatureApp: App {
    @StateObject private var session = TankSession()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ZStack(alignment: .top) {
                Color.black.ignoresSafeArea()
                TankScreen(session: session)
                    .ignoresSafeArea()
                HStack(alignment: .top) {
                    PixelPanel {
                        PixelLabel(text: session.isDay ? "DAY" : "NIGHT", scale: 3)
                    }
                    Spacer()
                    Button(action: session.toggleConsent) {
                        PixelLabel(
                            text: session.consent ? "STATS ON" : "STATS OFF",
                            scale: 2
                        )
                    }
                    .buttonStyle(.plain)
                }
                .padding(10)
            }
            .preferredColorScheme(.dark)
            .statusBarHidden(true)
            .onAppear { session.resume() }
            .onChange(of: scenePhase) { phase in
                if phase == .active {
                    session.resume()
                } else {
                    session.pauseAndSave()
                }
            }
        }
    }
}
