import Photos
import SwiftUI
import UIKit

/// Pixel-exact PNG of the live tank (or a tracked creature crop), saved
/// to Photos when permitted. Archive page export already ships in Phase 3.
enum PhotoMode {
    /// Snapshot the current RGBA tank buffer as a nearest-neighbour PNG.
    static func captureTank(session: TankSession, scale: Int = 2) -> UIImage? {
        session.tankSnapshot(scale: scale)
    }

    /// Crop around the inspected body's bbox when available.
    static func captureCreature(session: TankSession, scale: Int = 4) -> UIImage? {
        session.creatureSnapshot(scale: scale) ?? session.tankSnapshot(scale: scale)
    }

    static func saveToPhotos(_ image: UIImage, completion: @escaping (Bool) -> Void) {
        PHPhotoLibrary.requestAuthorization(for: .addOnly) { status in
            guard status == .authorized || status == .limited else {
                DispatchQueue.main.async { completion(false) }
                return
            }
            PHPhotoLibrary.shared().performChanges({
                PHAssetChangeRequest.creationRequestForAsset(from: image)
            }) { ok, _ in
                DispatchQueue.main.async { completion(ok) }
            }
        }
    }
}

/// Overlay with SNAP / CREATURE / CLOSE.
struct PhotoModeOverlay: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void
    @State private var preview: UIImage?
    @State private var status: String?

    var body: some View {
        ZStack {
            Color.black.opacity(0.4).ignoresSafeArea()
            VStack(spacing: 12) {
                PixelLabel(text: "PHOTO", scale: 3, color: Palette.highlight)
                if let preview {
                    Image(uiImage: preview)
                        .interpolation(.none)
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                        .frame(maxHeight: 280)
                        .overlay(Rectangle().stroke(Color(white: 0.4), lineWidth: 2))
                } else {
                    PixelLabel(text: "SNAP THE TANK OR A CREATURE", scale: 2, color: Palette.dimPaper)
                }
                HStack(spacing: 8) {
                    Button {
                        preview = PhotoMode.captureTank(session: session)
                        status = nil
                    } label: {
                        PixelPanel { PixelLabel(text: "TANK", scale: 2) }
                    }
                    .buttonStyle(.plain)
                    .a11yButton("Snap the tank")
                    Button {
                        preview = PhotoMode.captureCreature(session: session)
                        status = nil
                    } label: {
                        PixelPanel { PixelLabel(text: "CREATURE", scale: 2) }
                    }
                    .buttonStyle(.plain)
                    .a11yButton("Snap the creature")
                    Button {
                        guard let preview else { return }
                        PhotoMode.saveToPhotos(preview) { ok in
                            status = ok ? "SAVED" : "NO ACCESS"
                            if ok { session.showPhotoHint("PHOTO SAVED") }
                        }
                    } label: {
                        PixelPanel { PixelLabel(text: "SAVE", scale: 2, color: Palette.highlight) }
                    }
                    .buttonStyle(.plain)
                    .a11yButton("Save photo to library")
                    Button(action: onClose) {
                        PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
                    }
                    .buttonStyle(.plain)
                    .a11yButton("Close photo")
                }
                if let status {
                    PixelLabel(text: status, scale: 2, color: Palette.highlight)
                }
            }
            .padding(14)
            .background(Palette.panel)
            .overlay(Rectangle().stroke(Color(white: 0.35), lineWidth: 3))
        }
        .preferredColorScheme(.dark)
    }
}
