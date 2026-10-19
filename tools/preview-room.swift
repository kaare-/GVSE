import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// Renders the opening study from `RoomArt` so a one-pixel change can be
/// checked on the Mac, without building or installing the phone app.
@main
struct PreviewRoom {
    static func main() throws {
        let outDir = URL(
            fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "art/preview",
            isDirectory: true
        )
        try FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

        let mood = RoomMood(phase: 0.12, lampOn: true, shelfOn: true, candleLit: true)
        let room = RoomArt.cgImage(cast: LobbyScript.cast, mood: mood)
        let bytes = pixels(of: room)
        let w = room.width
        let glass = RoomArt.glassView
        try save(image(from: bytes, width: w, height: room.height), scale: 4, name: "room@4.png", to: outDir)
        try save(crop(bytes, width: w, spot: glass), scale: 8, name: "glass@8.png", to: outDir)
        reportContact(bytes, width: w)
    }
}

func pixels(of image: CGImage) -> [UInt8] {
    let w = image.width
    let h = image.height
    var bytes = [UInt8](repeating: 0, count: w * h * 4)
    let ctx = CGContext(
        data: &bytes,
        width: w,
        height: h,
        bitsPerComponent: 8,
        bytesPerRow: w * 4,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!
    ctx.interpolationQuality = .none
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: CGFloat(w), height: CGFloat(h)))
    return bytes
}

func crop(_ bytes: [UInt8], width: Int, spot: RoomArt.Spot) -> CGImage {
    var out = [UInt8]()
    out.reserveCapacity(spot.w * spot.h * 4)
    for y in spot.y..<(spot.y + spot.h) {
        let start = (y * width + spot.x) * 4
        out.append(contentsOf: bytes[start..<(start + spot.w * 4)])
    }
    return image(from: out, width: spot.w, height: spot.h)
}

func image(from bytes: [UInt8], width: Int, height: Int) -> CGImage {
    let data = Data(bytes) as CFData
    let provider = CGDataProvider(data: data)!
    return CGImage(
        width: width,
        height: height,
        bitsPerComponent: 8,
        bitsPerPixel: 32,
        bytesPerRow: width * 4,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
        provider: provider,
        decode: nil,
        shouldInterpolate: false,
        intent: .defaultIntent
    )!
}

func save(_ image: CGImage, scale: Int, name: String, to outDir: URL) throws {
    let scaled = scale == 1 ? image : zoom(image, scale)
    let url = outDir.appendingPathComponent(name)
    guard let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        fputs("could not write \(url.path)\n", stderr)
        exit(1)
    }
    CGImageDestinationAddImage(dest, scaled, nil)
    guard CGImageDestinationFinalize(dest) else {
        fputs("could not finish \(url.path)\n", stderr)
        exit(1)
    }
    print(url.path)
}

func zoom(_ image: CGImage, _ scale: Int) -> CGImage {
    let w = image.width * scale
    let h = image.height * scale
    let ctx = CGContext(
        data: nil,
        width: w,
        height: h,
        bitsPerComponent: 8,
        bytesPerRow: w * 4,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!
    ctx.interpolationQuality = .none
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
    return ctx.makeImage()!
}

/// The tumbler foot should sit on the table highlight, with no empty row between.
func reportContact(_ bytes: [UInt8], width: Int) {
    let x = RoomArt.openingGlass.x + RoomArt.openingGlass.w / 2
    print("tumbler column x=\(x)")
    for y in 152...158 {
        let i = (y * width + x) * 4
        let rgb = (Int(bytes[i]) << 16) | (Int(bytes[i + 1]) << 8) | Int(bytes[i + 2])
        print(String(format: "  y=%d #%06X", y, rgb))
    }
}
