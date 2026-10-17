import Foundation
import SwiftUI
import UIKit

/// One archived creature as the shell sees it: the opaque core record
/// plus the numbers and strings read back through the ABI.
struct ArchivedSpecimen: Identifiable, Equatable {
    let bytes: Data
    let info: SpecimenInfoRec
    let displayName: String
    let speciesName: String
    let lineage: String
    let notes: String
    let page: String
    let portrait: UIImage?

    var id: UInt64 { info.id }
    var colour: Color { Color(red: Double(info.r) / 255, green: Double(info.g) / 255, blue: Double(info.b) / 255) }

    static func == (a: ArchivedSpecimen, b: ArchivedSpecimen) -> Bool { a.bytes == b.bytes }

    init?(bytes: Data) {
        guard !bytes.isEmpty else { return nil }
        let info = bytes.withUnsafeBytes { raw -> SpecimenInfoRec in
            let base = raw.baseAddress!.assumingMemoryBound(to: UInt8.self)
            return tank_specimen_info(base, raw.count)
        }
        guard info.portrait_w > 0 else { return nil }
        self.bytes = bytes
        self.info = info
        displayName = Self.text(bytes, 0)
        speciesName = Self.text(bytes, 1)
        lineage = Self.text(bytes, 2)
        notes = Self.text(bytes, 3)
        page = Self.text(bytes, 4)
        portrait = Self.portrait(bytes, info)
    }

    private static func text(_ bytes: Data, _ which: UInt8) -> String {
        bytes.withUnsafeBytes { raw -> String in
            let base = raw.baseAddress!.assumingMemoryBound(to: UInt8.self)
            let need = tank_specimen_text(base, raw.count, which, nil, 0)
            var buf = [UInt8](repeating: 0, count: Int(need) + 1)
            _ = tank_specimen_text(base, raw.count, which, &buf, UInt32(buf.count))
            return String(decoding: buf.prefix(Int(need)), as: UTF8.self)
        }
    }

    /// Pixel-exact portrait scaled 8× with no smoothing.
    private static func portrait(_ bytes: Data, _ info: SpecimenInfoRec) -> UIImage? {
        let w = Int(info.portrait_w)
        let h = Int(info.portrait_h)
        guard w > 0, h > 0 else { return nil }
        var rgba = [UInt8](repeating: 0, count: w * h * 4)
        let got = bytes.withUnsafeBytes { raw -> Int in
            let base = raw.baseAddress!.assumingMemoryBound(to: UInt8.self)
            return tank_specimen_portrait(base, raw.count, &rgba, rgba.count)
        }
        guard got == rgba.count else { return nil }
        let data = Data(rgba)
        guard let provider = CGDataProvider(data: data as CFData),
              let cg = CGImage(
                  width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4,
                  space: CGColorSpaceCreateDeviceRGB(),
                  bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                  provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent
              )
        else { return nil }
        let scale = 8
        let size = CGSize(width: w * scale, height: h * scale)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        return UIGraphicsImageRenderer(size: size, format: format).image { ctx in
            ctx.cgContext.interpolationQuality = .none
            ctx.cgContext.translateBy(x: 0, y: size.height)
            ctx.cgContext.scaleBy(x: 1, y: -1)
            ctx.cgContext.draw(cg, in: CGRect(origin: .zero, size: size))
        }
    }

    /// A new record with the player's name and notes (empty name = the
    /// generated one).
    func edited(name: String, notes: String) -> ArchivedSpecimen? {
        let nameBytes = Array(name.utf8)
        let noteBytes = Array(notes.utf8)
        let out: Data = bytes.withUnsafeBytes { raw -> Data in
            let base = raw.baseAddress!.assumingMemoryBound(to: UInt8.self)
            let need = tank_specimen_edit(base, raw.count, nameBytes, nameBytes.count, noteBytes, noteBytes.count, nil, 0)
            var buf = [UInt8](repeating: 0, count: need)
            _ = tank_specimen_edit(base, raw.count, nameBytes, nameBytes.count, noteBytes, noteBytes.count, &buf, need)
            return Data(buf)
        }
        return ArchivedSpecimen(bytes: out)
    }

    /// The archive page as one shareable image: portrait + text in the
    /// pixel font, 2× scale.
    func pageImage() -> UIImage {
        let lines = page.split(separator: "\n").map(String.init)
        let scale: CGFloat = 2
        let textW = CGFloat(lines.map(\.count).max() ?? 0) * 6 * scale
        let portraitH = portrait?.size.height ?? 0
        let size = CGSize(
            width: max(max(textW, portrait?.size.width ?? 0) + 32, 32),
            height: max(portraitH + CGFloat(lines.count) * 9 * scale + 48, 48)
        )
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        return UIGraphicsImageRenderer(size: size, format: format).image { ctx in
            let g = ctx.cgContext
            g.setFillColor(UIColor(red: 0.09, green: 0.08, blue: 0.07, alpha: 1).cgColor)
            g.fill(CGRect(origin: .zero, size: size))
            g.interpolationQuality = .none
            portrait?.draw(at: CGPoint(x: 16, y: 16))
            var y = portraitH + 32
            for line in lines {
                drawPixelText(line, at: CGPoint(x: 16, y: y), scale: scale, in: g)
                y += 9 * scale
            }
        }
    }

    private func drawPixelText(_ text: String, at origin: CGPoint, scale: CGFloat, in g: CGContext) {
        g.setFillColor(UIColor(red: 0.93, green: 0.89, blue: 0.78, alpha: 1).cgColor)
        var pen = origin.x
        for scalar in text.unicodeScalars {
            let bits = tank_glyph(scalar.value)
            for row in 0..<7 {
                let rowBits = UInt32((bits >> (row * 5)) & 0x1F)
                for col in 0..<5 where (rowBits >> (4 - col)) & 1 == 1 {
                    g.fill(CGRect(x: pen + CGFloat(col) * scale, y: origin.y + CGFloat(row) * scale, width: scale, height: scale))
                }
            }
            pen += 6 * scale
        }
    }
}

/// Local archive: one `.specimen` file per creature in Documents/archive.
final class ArchiveStore: ObservableObject {
    @Published private(set) var specimens: [ArchivedSpecimen] = []

    private var dir: URL {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let d = docs.appendingPathComponent("archive", isDirectory: true)
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    init() {
        reload()
    }

    func reload() {
        let files = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: [.creationDateKey])) ?? []
        specimens = files
            .filter { $0.pathExtension == "specimen" }
            .compactMap { url -> (Date, ArchivedSpecimen)? in
                guard let data = try? Data(contentsOf: url), let s = ArchivedSpecimen(bytes: data) else { return nil }
                let date = (try? url.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast
                return (date, s)
            }
            .sorted { $0.0 > $1.0 }
            .map(\.1)
    }

    @discardableResult
    func add(bytes: Data) -> ArchivedSpecimen? {
        guard let s = ArchivedSpecimen(bytes: bytes) else { return nil }
        try? bytes.write(to: url(for: s.id), options: .atomic)
        reload()
        return s
    }

    func update(_ s: ArchivedSpecimen) {
        try? s.bytes.write(to: url(for: s.id), options: .atomic)
        reload()
    }

    func remove(_ s: ArchivedSpecimen) {
        try? FileManager.default.removeItem(at: url(for: s.id))
        reload()
    }

    func contains(speciesName: String) -> Bool {
        specimens.contains { $0.speciesName == speciesName }
    }

    private func url(for id: UInt64) -> URL {
        dir.appendingPathComponent(String(format: "%016llx.specimen", id))
    }
}
