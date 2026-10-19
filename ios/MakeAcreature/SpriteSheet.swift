import SwiftUI

/// One horizontal strip exported from an Aseprite file.
struct SpriteSheet {
    var image: UIImage
    var frames: Int
    var frameSize: CGSize

    static func named(_ name: String, frameSize: CGSize, frames: Int) -> SpriteSheet? {
        guard let image = UIImage(named: name) else { return nil }
        return SpriteSheet(image: image, frames: frames, frameSize: frameSize)
    }
}

/// Shows one frame of a strip. Nearest-neighbour, so the pixels stay square.
struct PixelStrip: View {
    var image: UIImage
    var frames: Int
    var frame: Int

    var body: some View {
        GeometryReader { geo in
            let fw = geo.size.width
            let fh = geo.size.height
            let index = min(max(frame, 0), max(frames - 1, 0))
            ZStack(alignment: .topLeading) {
                Image(uiImage: image)
                    .resizable()
                    .interpolation(.none)
                    .frame(width: fw * CGFloat(frames), height: fh)
                    .offset(x: -fw * CGFloat(index))
            }
            .frame(width: fw, height: fh, alignment: .topLeading)
            .clipped()
        }
    }
}
