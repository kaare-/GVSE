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

    /// Centered on the aquarium, larger than the old left-hand frame.
    static let treeFrame = Spot(x: 24, y: 44, w: 72, h: 48)
    static let treeInner = Spot(x: 28, y: 48, w: 64, h: 36)
    static let treeHit = Spot(x: 24, y: 44, w: 72, h: 54)

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
    static let tankGlass = Spot(x: 20, y: 104, w: 80, h: 52)

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

    /// Where a pot's foliage starts. Growth climbs toward the ceiling.
    struct Vine {
        var x, y, sway: Int
    }

    static let vines = [
        Vine(x: 8, y: 16, sway: 4),
        Vine(x: 99, y: 20, sway: 3),
    ]

    /// Small frame on the wall under the scissor jar.
    static let favFrame = Spot(x: 98, y: 68, w: 18, h: 24)
    static let favInner = Spot(x: 100, y: 70, w: 14, h: 16)
    static let favHit = Spot(x: 96, y: 66, w: 22, h: 28)

    static let book = Spot(x: 6, y: 200, w: 28, h: 30)
    static let bookHit = Spot(x: 4, y: 198, w: 36, h: 40)
    static let logHit = Spot(x: 4, y: 240, w: 40, h: 14)

    static let crate = Spot(x: 48, y: 206, w: 30, h: 24)
    static let crateInner = Spot(x: 52, y: 210, w: 22, h: 14)
    static let crateHit = Spot(x: 46, y: 204, w: 34, h: 40)

    /// Cassette deck on the floor, between the cabinet and the fuse box.
    static let deck = Spot(x: 80, y: 208, w: 16, h: 22)
    static let deckHit = Spot(x: 78, y: 206, w: 20, h: 28)

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
    static let gap = 0x3A2C22
    static let wallA = 0x6A5344
    static let wallB = 0x56483C
    static let floorA = 0xC99B61
    static let floorB = 0xA67C49
    static let wood = 0xBB8553
    static let woodLt = 0xF0C776
    static let woodDk = 0x63432F
    static let shelf = 0x8F6844
    static let plant = 0x6CB84A
    static let plantDk = 0x2E6A32
    static let plantLt = 0xA2D66F
    static let pot = 0x8A5840
    static let potLt = 0xC69E68
    static let bookR = 0xA84848
    static let bookB = 0x3A6090
    static let bookG = 0x4A6840
    static let bookT = 0xC69E68
    static let glass = 0xE4F2F6
    static let jar = 0x6A90A4
    static let liquid = 0x519FBE
    static let brass = 0xE0C070
    static let brassDk = 0xA88440
    static let glow = 0xF1D27B
    static let glowHot = 0xFFF4D0
    static let night = 0x243038
    static let metal = 0x787E79
    static let metalDk = 0x4A524C
    static let metalLt = 0xB7C0B4
    static let lampOff = 0x2A3028
    static let waterD = 0x316F96
    static let waterM = 0x3E8AAD
    static let waterL = 0x4DABC7
    static let sand = 0xC99B61
    static let sandDk = 0x8A6844
    static let algae = 0x6CB84A
    static let algaeLt = 0xA2D66F
    static let black = 0x292E30
    static let red = 0xA84848
    static let redLt = 0xD07068
    static let redDk = 0x703028
    static let page = 0xF5E8CB
    static let gold = 0xF1D27B
    static let ivory = 0xF5E8CB
    static let sky = 0xC5E6F4
    static let skyDeep = 0x7EB6D4
    static let pine = 0x2F6A3A
    static let pineDk = 0x1E4A28
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
        window()
        shelf()
        tree()
        lamps()
        tank()
        favouriteFrame()
        bench()
        clutter()
        floor()
        archiveBook()
        favouriteCrate()
        cassette()
        tankPlaque()
        fuse()
        toolJar()
        lighting()
    }

    private func planks() {
        for x in 0..<w {
            let gap = x % 7 == 0
            let alt = (x / 7) % 2 == 0
            for y in 0..<RoomArt.floorY {
                var c = gap ? Ink.gap : (alt ? Ink.wallA : Ink.wallB)
                if !gap && (x * 3 + y) % 53 == 0 { c = 0x4A3428 }
                set(x, y, c)
            }
        }
        for (x, y) in [(20, 70), (44, 130), (96, 90), (70, 160), (14, 150)] {
            fill(x, y, 2, 2, 0x4A3428)
            set(x + 2, y + 1, 0x8A6848)
        }
    }

    private func window() {
        fill(0, 46, 15, 56, Ink.woodDk)
        fill(1, 47, 13, 54, Ink.sky)
        fill(1, 78, 13, 23, Ink.skyDeep)
        fill(7, 47, 1, 54, Ink.wood)
        fill(1, 73, 13, 1, Ink.wood)
        for x in 1..<14 {
            let crown = 10 + (x * 5) % 12
            fill(x, 101 - crown, 1, crown, x % 2 == 0 ? Ink.pine : Ink.pineDk)
        }
        fill(0, 46, 15, 1, Ink.woodLt)
        fill(0, 100, 16, 2, Ink.woodLt)
        fill(0, 102, 16, 2, Ink.woodDk)
    }

    /// Window from the left, lamps from their shades. Painted last so
    /// the light falls on the wall and the furniture, and it falls off
    /// away from each source. Sun stays woody; lamps go gold.
    private func lighting() {
        let winMid = 74
        for y in 0..<h {
            let besideSky = y >= 46 && y < 102
            for x in 0..<w {
                if besideSky && x < 15 { continue }
                let dx = max(0, x - 14)
                let dy = abs(y - winMid)
                let inBeam = y >= 40 && y <= 130
                let vertical = inBeam ? dy / 12 : dy / 5
                var sunAmt = 30 - dx / 3 - vertical
                if y >= RoomArt.floorY && x < 78 {
                    sunAmt = max(sunAmt, 14 - dx / 6 - (y - RoomArt.floorY) / 5)
                }
                if sunAmt > 0 { sun(x, y, sunAmt) }
                if dx > 52 {
                    let sh = min(18, (dx - 52) / 3 + (inBeam ? 0 : 3))
                    shade(x, y, sh)
                }
            }
        }
        // Hard rims: light from the left leaves a dark edge on the right.
        rightRim(16, 100, 88, 60)
        rightRim(6, 156, 108, 12)
        rightRim(6, 200, 28, 30)
        rightRim(48, 206, 30, 24)
        rightRim(80, 208, 16, 22)
        for x in 2..<118 {
            shade(x, 40, 5 + x / 24)
        }
        cone(cx: 66, cy: 30, reach: 18, power: 22)
        cone(cx: 8, cy: 118, reach: 36, power: 24)
        cone(cx: 112, cy: 124, reach: 22, power: 18)
    }

    /// Light on the wall around a shade, then a widening pool beneath it.
    private func cone(cx: Int, cy: Int, reach: Int, power: Int) {
        for dy in -16...18 {
            for dx in -14...14 where abs(dx) + abs(dy) / 2 < 18 {
                lamp(cx + dx, cy + dy, 34 - abs(dx) - abs(dy) / 2)
            }
        }
        for dy in -28...6 {
            let spread = 9 + abs(min(dy, 0)) / 2
            let along = power - abs(dy) * 2 / 3
            for dx in -spread...spread {
                lamp(cx + dx, cy + dy, max(0, along - abs(dx)))
            }
        }
        for dy in 0..<reach {
            let half = 4 + dy / 2
            let along = power - dy * power / reach
            for dx in -half...half {
                lamp(cx + dx, cy + dy, max(0, along - abs(dx) * 2 / 3))
            }
        }
    }

    private func rightRim(_ x: Int, _ y: Int, _ bw: Int, _ bh: Int) {
        for yy in y..<(y + bh) {
            shade(x + bw, yy, 16)
            shade(x + bw + 1, yy, 9)
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

        // A small landscape, sky over a far shore. No loose foliage.
        fill(76, 20, 14, 14, Ink.brassDk)
        fill(78, 22, 10, 6, 0x163040)
        fill(78, 28, 10, 4, 0x8A6844)

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
        candle(106)
    }

    /// The right-hand light. A short wax column and a flame, not a second lamp.
    private func candle(_ x: Int) {
        fill(x + 4, 130, 4, 22, Ink.ivory)
        fill(x + 5, 132, 1, 16, 0xE7D3A4)
        fill(x + 5, 126, 2, 4, Ink.glow)
        set(x + 5, 124, Ink.glowHot)
        set(x + 6, 127, Ink.glowHot)
        fill(x + 2, 152, 8, 2, Ink.brass)
        fill(x + 3, 154, 6, 2, Ink.brassDk)
    }

    private func deskLamp(_ x: Int) {
        fill(x, 112, 12, 2, Ink.brass)
        fill(x + 1, 114, 10, 8, Ink.brassDk)
        fill(x + 3, 116, 6, 4, Ink.glow)
        fill(x + 4, 117, 4, 2, Ink.glowHot)
        fill(x + 5, 122, 2, 30, Ink.metalDk)
        fill(x + 2, 152, 8, 2, Ink.brass)
        fill(x + 1, 154, 10, 2, Ink.brassDk)
    }

    private func tank() {
        let t = RoomArt.tank
        let g = RoomArt.tankGlass
        fill(t.x, t.y, t.w, t.h, Ink.ivory)
        fill(t.x + 2, t.y + 2, t.w - 4, t.h - 4, Ink.woodDk)
        fill(t.x + 3, t.y + 3, t.w - 6, t.h - 6, Ink.brassDk)
        fill(g.x, g.y, g.w, g.h, Ink.waterL)
        fill(t.x, t.y, t.w, 2, Ink.ivory)
        fill(t.x, t.y + t.h - 2, t.w, 2, Ink.wood)
        fill(t.x, t.y, 2, t.h, Ink.woodLt)
        fill(t.x + t.w - 2, t.y, 2, t.h, Ink.woodDk)
    }

    private func favouriteFrame() {
        let f = RoomArt.favFrame
        let inner = RoomArt.favInner
        fill(f.x, f.y, f.w, f.h, Ink.wood)
        fill(f.x, f.y, f.w, 1, Ink.woodLt)
        fill(inner.x, inner.y, inner.w, inner.h, Ink.night)
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
                if !gap && (x + y * 2) % 29 == 0 { c = 0xD4AE78 }
                set(x, y, c)
            }
        }
        fill(0, RoomArt.floorY, w, 2, 0x16100C)
        // Rug under the three floor objects.
        fill(2, 246, 116, 14, 0x2A1814)
        for x in stride(from: 4, to: 116, by: 6) {
            set(x, 252, 0x4A3028)
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

    /// A small deck. Window, two reels, a tape band, two keys.
    private func cassette() {
        let d = RoomArt.deck
        fill(d.x, d.y + 2, d.w, d.h - 2, Ink.metalDk)
        fill(d.x, d.y, d.w, 2, Ink.metal)
        fill(d.x, d.y + 2, 1, d.h - 4, 0x2A241C)
        fill(d.x + 2, d.y + 4, d.w - 4, 8, 0x1A1410)
        fill(d.x + 3, d.y + 5, d.w - 6, 6, Ink.page)
        fill(d.x + 6, d.y + 7, d.w - 12, 2, 0x3A2418)
        disc(d.x + 5, d.y + 8, 2, Ink.metal)
        disc(d.x + d.w - 6, d.y + 8, 2, Ink.metal)
        set(d.x + 5, d.y + 8, 0x1A1410)
        set(d.x + d.w - 6, d.y + 8, 0x1A1410)
        fill(d.x + 3, d.y + d.h - 5, 3, 2, Ink.metal)
        fill(d.x + d.w - 6, d.y + d.h - 5, 3, 2, Ink.brass)
        fill(d.x + 1, d.y + d.h, d.w - 1, 2, 0x120C0E)
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
        fill(j.x + 2, j.y + 6, 10, 11, Ink.waterL)
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
        fill(x + 1, y + 2, 10, 14, Ink.waterL)
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

    /// Sky and the tree screen stay as painted. Lighting a blue-black
    /// pixel just turns it gray.
    private func holds(_ x: Int, _ y: Int) -> Bool {
        if x < 15 && y >= 46 && y < 102 { return true }
        let inner = RoomArt.treeInner
        return x >= inner.x && y >= inner.y && x < inner.x + inner.w && y < inner.y + inner.h
    }

    /// Daylight. Extra red, a little green, almost no blue, so the
    /// planks go sunlit wood instead of chalk. Stops short of white.
    private func sun(_ x: Int, _ y: Int, _ amt: Int) {
        warmAdd(x, y, amt, green: 2, blue: 5)
    }

    /// Lamp gold. A touch more green than daylight, still short on blue.
    private func lamp(_ x: Int, _ y: Int, _ amt: Int) {
        warmAdd(x, y, amt, green: 3, blue: 6)
    }

    /// `green` and `blue` are divisors: daylight adds 2/3 of the red to
    /// green and 1/5 to blue; a lamp adds 3/4 and 1/6.
    private func warmAdd(_ x: Int, _ y: Int, _ amt: Int, green: Int, blue: Int) {
        guard x >= 0, y >= 0, x < w, y < h, amt > 0, !holds(x, y) else { return }
        let i = (y * w + x) * 4
        let room = 232 - Int(bytes[i])
        if room <= 0 { return }
        let a = min(amt, min(room, 40))
        bytes[i] = UInt8(Int(bytes[i]) + a)
        bytes[i + 1] = UInt8(min(Int(bytes[i + 1]) + a * green / (green + 1), 255))
        bytes[i + 2] = UInt8(min(Int(bytes[i + 2]) + a / blue, 255))
    }

    /// Shadow away from the window. Blue falls fastest, so the dark
    /// stays brown.
    private func shade(_ x: Int, _ y: Int, _ amt: Int) {
        guard x >= 0, y >= 0, x < w, y < h, amt > 0, !holds(x, y) else { return }
        let i = (y * w + x) * 4
        let a = min(amt, 24)
        bytes[i] = UInt8(max(Int(bytes[i]) - a, 0))
        bytes[i + 1] = UInt8(max(Int(bytes[i + 1]) - a - a / 4, 0))
        bytes[i + 2] = UInt8(max(Int(bytes[i + 2]) - a - a / 2, 0))
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
