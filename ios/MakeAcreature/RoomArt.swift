import CoreGraphics
import Foundation
#if canImport(UIKit)
import UIKit
#endif

/// The study, drawn at 120×260 and shown with nearest-neighbour scaling
/// so the pixels stay big. Hotspots are in the same grid as the picture.
enum RoomArt {
    static let w = 120
    static let h = 260
    static let floorY = 192

    struct Spot {
        var x, y, w, h: Int
    }

    static let treeFrame = Spot(x: 8, y: 46, w: 52, h: 42)
    static let treeInner = Spot(x: 12, y: 50, w: 44, h: 30)
    static let treeHit = Spot(x: 8, y: 46, w: 52, h: 52)

    /// Small breaker panel, low on the right.
    static let fuseBox = Spot(x: 98, y: 214, w: 18, h: 24)
    static let fuseHit = Spot(x: 96, y: 212, w: 22, h: 28)
    static let fuseLamps = [
        Spot(x: 102, y: 218, w: 4, h: 4),
        Spot(x: 102, y: 224, w: 4, h: 4),
        Spot(x: 102, y: 230, w: 4, h: 4),
    ]

    static let tank = Spot(x: 16, y: 100, w: 88, h: 60)
    /// The glass inside the frame. The live tank is drawn here.
    static let tankGlass = Spot(x: 19, y: 103, w: 82, h: 54)

    /// Three sample vials on the shelf. Tap one to look in; empty it from
    /// the long-press menu.
    static let vials = [
        Spot(x: 20, y: 17, w: 12, h: 17),
        Spot(x: 34, y: 17, w: 12, h: 17),
        Spot(x: 48, y: 17, w: 12, h: 17),
    ]

    static func vialGlass(_ index: Int) -> Spot {
        let v = vials[index]
        return Spot(x: v.x + 1, y: v.y + 2, w: 10, h: 13)
    }

    /// A jar of odd tools under the right end of the shelf.
    static let toolJar = Spot(x: 104, y: 44, w: 14, h: 18)

    /// Where a vine leaves its pot. Length is not painted; it grows later.
    struct Vine {
        var x, y, sway: Int
    }

    static let vines = [
        Vine(x: 5, y: 40, sway: 5),
        Vine(x: 88, y: 42, sway: 4),
    ]

    static let book = Spot(x: 6, y: 200, w: 28, h: 30)
    static let bookHit = Spot(x: 4, y: 198, w: 36, h: 40)
    static let logHit = Spot(x: 4, y: 240, w: 40, h: 14)

    static let crate = Spot(x: 48, y: 206, w: 30, h: 24)
    static let crateInner = Spot(x: 52, y: 210, w: 22, h: 14)
    static let crateHit = Spot(x: 46, y: 204, w: 34, h: 40)

    /// Plaque on the bench, directly under the main tank.
    static let sign = Spot(x: 36, y: 159, w: 48, h: 7)
    static let signHit = Spot(x: 28, y: 156, w: 64, h: 14)

    static func cgImage() -> CGImage {
        let pix = Pix(w: w, h: h)
        pix.paint()
        let provider = CGDataProvider(data: Data(pix.bytes) as CFData)!
        return CGImage(
            width: w,
            height: h,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: w * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
            provider: provider,
            decode: nil,
            shouldInterpolate: false,
            intent: .defaultIntent
        )!
    }

    #if canImport(UIKit)
    static let image: UIImage = UIImage(cgImage: cgImage())
    #endif
}

private enum Ink {
    static let gap = 0x100C0A
    static let wallA = 0x2C221A
    static let wallB = 0x241C14
    static let floorA = 0x3A2A1E
    static let floorB = 0x2E2218
    static let wood = 0x6B4A30
    static let woodLt = 0x8A6240
    static let woodDk = 0x3E2818
    static let shelf = 0x5A3C26
    static let plant = 0x2E7A38
    static let plantDk = 0x184C22
    static let plantLt = 0x6CB84A
    static let pot = 0x6A4030
    static let potLt = 0x8A5840
    static let bookR = 0x7A2830
    static let bookB = 0x243858
    static let bookG = 0x3A4030
    static let bookT = 0x8A5A28
    static let glass = 0xC5D8DC
    static let jar = 0x142022
    static let liquid = 0x2A6844
    static let brass = 0xC8A45A
    static let brassDk = 0x8A682E
    static let glow = 0xFFE2A0
    static let glowHot = 0xFFF8DC
    static let night = 0x12100E
    static let metal = 0x8A9298
    static let metalDk = 0x3E464C
    static let metalLt = 0xC8CED0
    static let lampOff = 0x1A1E18
    static let waterD = 0x0C2034
    static let waterM = 0x14304C
    static let waterL = 0x2A6494
    static let sand = 0xA07848
    static let sandDk = 0x6E4C30
    static let algae = 0x3CB44A
    static let algaeLt = 0x9AE868
    static let black = 0x101014
    static let red = 0x8C2C28
    static let redLt = 0xB4443C
    static let redDk = 0x5C1C1C
    static let page = 0xE6D8C0
    static let gold = 0xE8C868
    static let ivory = 0xE4DCC8
}

private final class Pix {
    let w: Int
    let h: Int
    var bytes: [UInt8]

    init(w: Int, h: Int) {
        self.w = w
        self.h = h
        bytes = [UInt8](repeating: 0, count: w * h * 4)
    }

    func paint() {
        planks()
        shafts()
        shelf()
        tree()
        lamps()
        tank()
        bench()
        clutter()
        floor()
        archiveBook()
        favouriteCrate()
        tankPlaque()
        fuse()
        toolJar()
    }

    private func planks() {
        for x in 0..<w {
            let gap = x % 7 == 0
            let alt = (x / 7) % 2 == 0
            for y in 0..<RoomArt.floorY {
                var c = gap ? Ink.gap : (alt ? Ink.wallA : Ink.wallB)
                if !gap && (x * 3 + y) % 53 == 0 { c = 0x1A140E }
                set(x, y, c)
            }
        }
        for (x, y) in [(20, 70), (44, 130), (96, 90), (70, 160), (14, 150)] {
            fill(x, y, 2, 2, 0x140E0C)
            set(x + 2, y + 1, 0x403022)
        }
    }

    private func shafts() {
        // Wide stepped beams, brighter at the ceiling, so the planks
        // are not one flat brown.
        for y in 0..<170 {
            let add = max(18, (170 - y) / 3)
            for base in [0, 16, 40, 74, 102] {
                let x = base + y / 20
                lighten(x, y, add)
                lighten(x + 1, y, add)
                if y % 2 == 0 { lighten(x + 2, y, add / 2) }
            }
        }
    }

    private func shelf() {
        fill(2, 34, 116, 1, Ink.woodLt)
        fill(2, 35, 116, 3, Ink.shelf)
        fill(2, 38, 116, 2, Ink.woodDk)
        fill(18, 38, 3, 5, Ink.woodDk)
        fill(100, 38, 3, 5, Ink.woodDk)

        // Trailing plant, left. Foliage stays clear of the island.
        fill(4, 26, 12, 8, Ink.pot)
        fill(3, 25, 14, 2, Ink.potLt)
        sprite(2, 16, [
            "  ggGG  ",
            " gGGGgg ",
            "gGG  gGg",
            " gG  Gg ",
        ], [
            "g": Ink.plantDk, "G": Ink.plant, " ": 0,
        ])

        for spot in RoomArt.vials {
            vial(spot.x, spot.y)
        }

        // Lantern.
        fill(64, 16, 2, 3, Ink.brassDk)
        fill(62, 19, 8, 2, Ink.brass)
        fill(63, 21, 6, 10, Ink.brassDk)
        fill(64, 23, 4, 6, Ink.glow)
        fill(65, 24, 2, 3, Ink.glowHot)
        fill(62, 31, 8, 2, Ink.brass)

        // Little picture and a second trailing plant.
        fill(76, 20, 14, 14, Ink.brassDk)
        fill(78, 22, 10, 10, 0x163040)
        fill(78, 29, 10, 3, 0x2A6840)
        fill(80, 24, 2, 6, Ink.plantDk)
        fill(83, 23, 2, 7, Ink.plant)
        fill(86, 25, 2, 5, Ink.plantLt)

        fill(96, 28, 10, 6, Ink.pot)
        fill(95, 27, 12, 2, Ink.potLt)
        sprite(94, 20, [
            " gGGg ",
            "gGG Gg",
            " gG g ",
        ], ["g": Ink.plantDk, "G": Ink.plant, " ": 0])
    }

    private func tree() {
        let f = RoomArt.treeFrame
        let inner = RoomArt.treeInner
        fill(f.x, f.y, f.w, f.h, Ink.wood)
        fill(f.x + 1, f.y + 1, f.w - 2, 1, Ink.woodLt)
        fill(inner.x - 1, inner.y - 1, inner.w + 2, inner.h + 2, Ink.woodDk)
        fill(inner.x, inner.y, inner.w, inner.h, Ink.night)
        for (x, y) in [(f.x + 2, f.y + 2), (f.x + f.w - 3, f.y + 2), (f.x + 2, f.y + f.h - 3), (f.x + f.w - 3, f.y + f.h - 3)] {
            set(x, y, Ink.gold)
        }
    }

    private func fuse() {
        let b = RoomArt.fuseBox
        fill(b.x, b.y, b.w, b.h, Ink.metal)
        fill(b.x + 1, b.y + 1, b.w - 2, b.h - 2, Ink.metalDk)
        fill(b.x + 2, b.y + 2, b.w - 4, b.h - 4, 0x5A6268)
        for lamp in RoomArt.fuseLamps {
            disc(lamp.x + lamp.w / 2, lamp.y + lamp.h / 2, 2, Ink.lampOff)
        }
        set(b.x + 2, b.y + 2, Ink.metalLt)
        set(b.x + b.w - 3, b.y + 2, Ink.metalLt)
    }

    private func lamps() {
        deskLamp(2)
        deskLamp(106)
    }

    private func deskLamp(_ x: Int) {
        fill(x, 112, 12, 2, Ink.brass)
        fill(x + 1, 114, 10, 8, Ink.brassDk)
        fill(x + 3, 116, 6, 4, Ink.glow)
        fill(x + 4, 117, 4, 2, Ink.glowHot)
        fill(x + 5, 122, 2, 30, Ink.metalDk)
        fill(x + 2, 152, 8, 2, Ink.brass)
        fill(x + 1, 154, 10, 2, Ink.brassDk)
        // Warm spill on the wall behind the shade.
        for dy in 0..<10 {
            lighten(x + 12, 110 + dy, 18 - dy)
            lighten(x - 1, 110 + dy, 12)
        }
    }

    private func tank() {
        let t = RoomArt.tank
        let g = RoomArt.tankGlass
        fill(t.x, t.y, t.w, t.h, Ink.black)
        fill(g.x, g.y, g.w, g.h, 0x101820)
        fill(t.x, t.y, t.w, 1, 0x9EC8D4)
        fill(t.x, t.y + t.h - 1, t.w, 1, 0x203040)
    }

    private func bench() {
        fill(6, 156, 108, 2, Ink.woodLt)
        fill(6, 158, 108, 8, Ink.wood)
        fill(6, 166, 108, 2, Ink.woodDk)
        for x in stride(from: 10, to: 110, by: 8) {
            fill(x, 158, 1, 8, Ink.woodDk)
        }
        fill(12, 168, 6, 24, Ink.woodDk)
        fill(13, 168, 1, 24, Ink.wood)
        fill(102, 168, 6, 24, Ink.woodDk)
        fill(103, 168, 1, 24, Ink.wood)
        fill(18, 168, 84, 6, 0x140E0C)
    }

    private func clutter() {
        fill(26, 176, 22, 14, Ink.metalDk)
        fill(26, 176, 22, 2, Ink.metal)
        fill(26, 188, 22, 2, 0x2A3034)
        fill(35, 181, 4, 4, Ink.brass)
        fill(52, 182, 14, 3, Ink.bookR)
        fill(51, 185, 15, 3, Ink.bookB)
        fill(53, 188, 13, 3, Ink.bookT)
        fill(76, 178, 14, 12, Ink.metal)
        fill(77, 179, 12, 4, 0x2A3438)
        // Handle.
        for dx in 0..<10 { set(78 + dx, 176, Ink.metalLt) }
        set(78, 177, Ink.metalLt)
        set(87, 177, Ink.metalLt)
        fill(86, 168, 2, 12, Ink.woodLt)
        fill(83, 172, 7, 3, Ink.page)
        set(84, 173, Ink.bookR)
    }

    private func floor() {
        for x in 0..<w {
            let gap = x % 8 == 0
            let alt = (x / 8) % 2 == 0
            for y in RoomArt.floorY..<h {
                var c = gap ? Ink.gap : (alt ? Ink.floorA : Ink.floorB)
                if !gap && (x + y * 2) % 29 == 0 { c = 0x4A3828 }
                set(x, y, c)
            }
        }
        fill(0, RoomArt.floorY, w, 2, 0x16100C)
        // Rug under the three floor objects.
        fill(2, 246, 116, 14, 0x1A1216)
        for x in stride(from: 4, to: 116, by: 6) {
            set(x, 252, 0x2A2030)
        }
    }

    private func archiveBook() {
        let b = RoomArt.book
        fill(b.x + 4, b.y + 2, b.w - 2, b.h - 2, Ink.page)
        fill(b.x, b.y, b.w - 4, b.h, Ink.red)
        fill(b.x, b.y, b.w - 4, 2, Ink.redLt)
        fill(b.x, b.y, 3, b.h, Ink.redDk)
        fill(b.x + 8, b.y + 8, 12, 14, Ink.redDk)
        // Contact shadow.
        fill(b.x + 2, b.y + b.h, b.w - 2, 2, 0x120C0E)
    }

    private func favouriteCrate() {
        let c = RoomArt.crate
        let inner = RoomArt.crateInner
        fill(c.x, c.y + c.h - 4, c.w, 4, Ink.woodDk)
        fill(c.x, c.y, c.w, c.h - 4, Ink.wood)
        fill(c.x, c.y, c.w, 2, Ink.woodLt)
        fill(c.x, c.y, 3, c.h, Ink.woodDk)
        fill(c.x + c.w - 3, c.y, 3, c.h, Ink.woodDk)
        fill(inner.x, inner.y, inner.w, inner.h, 0x2A1C12)
        fill(c.x + 1, c.y + c.h, c.w - 2, 2, 0x120C0E)
    }

    private func tankPlaque() {
        let s = RoomArt.sign
        fill(s.x, s.y, s.w, s.h, Ink.brass)
        fill(s.x + 1, s.y + 1, s.w - 2, s.h - 2, Ink.brassDk)
    }

    private func toolJar() {
        let j = RoomArt.toolJar
        fill(j.x + 3, j.y + 4, 8, 2, Ink.brass)
        fill(j.x + 1, j.y + 6, 1, 11, Ink.glass)
        fill(j.x + 12, j.y + 6, 1, 11, Ink.glass)
        fill(j.x + 2, j.y + 6, 10, 11, 0x101820)
        fill(j.x + 1, j.y + 16, 12, 1, Ink.glass)
        // Scissors and a couple of other handles, sticking out of the mouth.
        set(j.x + 4, j.y, Ink.metalLt)
        set(j.x + 4, j.y + 1, Ink.metalLt)
        set(j.x + 5, j.y + 2, Ink.metal)
        set(j.x + 8, j.y, Ink.metalLt)
        set(j.x + 8, j.y + 1, Ink.metal)
        set(j.x + 7, j.y + 2, Ink.metal)
        set(j.x + 6, j.y + 3, Ink.brass)
        set(j.x + 9, j.y + 1, Ink.woodLt)
        set(j.x + 10, j.y + 2, Ink.wood)
        set(j.x + 3, j.y + 2, 0xC8D0D4)
    }

    private func vial(_ x: Int, _ y: Int) {
        fill(x + 2, y, 8, 2, Ink.brass)
        fill(x, y + 2, 1, 14, Ink.glass)
        fill(x + 11, y + 2, 1, 14, Ink.glass)
        fill(x + 1, y + 2, 10, 14, 0x101820)
        set(x + 2, y + 4, Ink.glow)
        fill(x, y + 16, 12, 1, Ink.glass)
    }

    private func disc(_ cx: Int, _ cy: Int, _ r: Int, _ c: Int) {
        for y in (cy - r)...(cy + r) {
            for x in (cx - r)...(cx + r) {
                let dx = x - cx
                let dy = y - cy
                if dx * dx + dy * dy <= r * r + 1 { set(x, y, c) }
            }
        }
    }

    private func sprite(_ x: Int, _ y: Int, _ rows: [String], _ map: [Character: Int]) {
        for (dy, row) in rows.enumerated() {
            for (dx, ch) in row.enumerated() where ch != " " {
                if let c = map[ch] { set(x + dx, y + dy, c) }
            }
        }
    }

    private func lighten(_ x: Int, _ y: Int, _ add: Int) {
        guard x >= 0, y >= 0, x < w, y < h, add > 0 else { return }
        let i = (y * w + x) * 4
        for k in 0..<3 {
            bytes[i + k] = UInt8(min(Int(bytes[i + k]) + add, 255))
        }
    }

    private func fill(_ x: Int, _ y: Int, _ bw: Int, _ bh: Int, _ rgb: Int) {
        for yy in y..<(y + bh) {
            for xx in x..<(x + bw) { set(xx, yy, rgb) }
        }
    }

    private func set(_ x: Int, _ y: Int, _ rgb: Int) {
        guard x >= 0, y >= 0, x < w, y < h, rgb != 0 else { return }
        let i = (y * w + x) * 4
        bytes[i] = UInt8((rgb >> 16) & 255)
        bytes[i + 1] = UInt8((rgb >> 8) & 255)
        bytes[i + 2] = UInt8(rgb & 255)
        bytes[i + 3] = 255
    }
}
