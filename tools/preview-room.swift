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
        var bytes = pixels(of: room)
        let w = room.width
        stampPlant(&bytes, width: w, length: 42)
        let glass = RoomArt.glassView
        try save(image(from: bytes, width: w, height: room.height), scale: 4, name: "room@4.png", to: outDir)
        try save(crop(bytes, width: w, spot: glass), scale: 8, name: "glass@8.png", to: outDir)
        reportContact(bytes, width: w)
        reportPlant(bytes, width: w)
        reportShelf(bytes, width: w)
    }
}

func stampPlant(_ bytes: inout [UInt8], width: Int, length: Int) {
    for dot in RoomArt.hangingPlant(length: length) {
        let i = (dot.y * width + dot.x) * 4
        bytes[i] = UInt8((dot.rgb >> 16) & 255)
        bytes[i + 1] = UInt8((dot.rgb >> 8) & 255)
        bytes[i + 2] = UInt8(dot.rgb & 255)
        bytes[i + 3] = 255
    }
}

func reportPlant(_ bytes: [UInt8], width: Int) {
    print("hanging plant")
    var broke = false
    var seen = false
    for y in 24..<90 {
        var row = ""
        var green = false
        for x in 0..<16 {
            let i = (y * width + x) * 4
            let r = Int(bytes[i])
            let g = Int(bytes[i + 1])
            if g > r + 12 && g > 70 {
                row.append("g")
                green = true
                seen = true
            } else {
                row.append(".")
            }
        }
        if seen && !green && !broke && y < 70 {
            print(String(format: "%3d GAP", y))
            broke = true
        }
        if green { print(String(format: "%3d %@", y, row as NSString)) }
    }
}

func reportShelf(_ bytes: [UInt8], width: Int) {
    print("shelf x=40..60")
    for y in 168..<194 {
        var row = ""
        for x in 40..<62 {
            let i = (y * width + x) * 4
            let r = Int(bytes[i])
            let g = Int(bytes[i + 1])
            let b = Int(bytes[i + 2])
            if r > 140 && r > g + 30 { row.append("R") }
            else if g > 70 && g > r + 10 && b < 90 { row.append("G") }
            else if r > 200 && g > 180 { row.append("p") }
            else if r < 30 && g < 30 { row.append(".") }
            else { row.append(" ") }
        }
        print(String(format: "%3d %@", y, row as NSString))
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
