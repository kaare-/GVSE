import CoreGraphics
import Foundation
#if canImport(UIKit)
import UIKit
#endif
import AVFoundation

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

    /// Breaker panel under the tank table, at the right.
    static let fuseBox = Spot(x: 84, y: 174, w: 14, h: 16)
    static let fuseHit = Spot(x: 82, y: 172, w: 18, h: 20)
    static let fuseLamps = [
        Spot(x: 87, y: 176, w: 3, h: 3),
        Spot(x: 87, y: 181, w: 3, h: 3),
        Spot(x: 87, y: 186, w: 3, h: 3),
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

    /// Hook on the right wall. The scissors hang here until they are picked up.
    static let shearHook = Spot(x: 104, y: 46, w: 12, h: 18)

    /// Where a plant's live growth starts. The window plant hangs and
    /// lengthens downward. The other still climbs.
    struct Vine {
        var x, y, sway: Int
        var hangs: Bool
    }

    static let vines = [
        Vine(x: 9, y: 36, sway: 4, hangs: true),
        Vine(x: 99, y: 20, sway: 3, hangs: false),
    ]

    /// A taller upright plant in the bottom right. The shelf pots stay.
    static let ficus = Spot(x: 100, y: 172, w: 18, h: 50)

    /// The desk lamp. A double tap turns it on or off.
    static let deskLampHit = Spot(x: 1, y: 110, w: 14, h: 46)

    /// The lantern on the top shelf. A double tap turns it on or off.
    static let shelfLamp = Spot(x: 60, y: 14, w: 12, h: 20)

    /// The candle. A double tap lights or puts it out. A swipe blows it out.
    static let candleHit = Spot(x: 104, y: 118, w: 16, h: 40)

    /// Small frame on the wall under the scissor hook.
    static let favFrame = Spot(x: 98, y: 68, w: 18, h: 24)
    static let favInner = Spot(x: 100, y: 70, w: 14, h: 16)
    static let favHit = Spot(x: 96, y: 66, w: 22, h: 28)

    /// One book under the tank table. The archive and the event log share it.
    static let book = Spot(x: 42, y: 172, w: 16, h: 18)
    static let bookHit = Spot(x: 40, y: 170, w: 20, h: 12)
    static let logHit = Spot(x: 40, y: 182, w: 20, h: 8)

    /// The gray box under the table. Charts live here.
    static let crate = Spot(x: 20, y: 174, w: 20, h: 14)
    static let crateInner = Spot(x: 24, y: 178, w: 8, h: 6)
    static let crateHit = Spot(x: 18, y: 172, w: 24, h: 18)

    /// Cassette deck under the table, between the book and the fuse box.
    static let deck = Spot(x: 64, y: 174, w: 16, h: 16)
    static let deckHit = Spot(x: 62, y: 172, w: 20, h: 14)
    /// Power key on the deck. Red while off, green while a tape plays.
    static let deckPower = Spot(x: 66, y: 184, w: 5, h: 5)

    /// Plaque on the bench, directly under the main tank.
    static let sign = Spot(x: 36, y: 159, w: 48, h: 7)
    static let signHit = Spot(x: 28, y: 156, w: 64, h: 14)

    /// The opening glass. One tumbler, not the big tank.
    /// Sits on the aquarium stand, in the space the big tank leaves open.
    static let openingGlass = Spot(x: 52, y: 132, w: 16, h: 26)

    static func cgImage(cast: RoomCast = RoomCast(), mood: RoomMood = RoomMood()) -> CGImage {
        let pix = Pix(w: w, h: h, cast: cast, mood: mood)
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
    static func picture(_ cast: RoomCast, mood: RoomMood = RoomMood()) -> UIImage {
        UIImage(cgImage: cgImage(cast: cast, mood: mood))
    }
    #endif
}

/// What a beat has put in the room. Later beats fill these in.
struct RoomCast {
    var tank = true
    var jars = true
    var bench = true
    var openingGlass = false
}

/// Ordered beats. A beat can show or hide room objects, choose which glass
/// is on screen, and play a short system sound. The cassette is separate.
/// Only the opening beat is played. The rest are empty hooks.
struct LobbyBeat {
    var name: String
    var tank: Bool?
    var jars: Bool?
    var bench: Bool?
    var glass: String?
    var sound: String?
}

enum LobbyScript {
    static let beats: [LobbyBeat] = [
        LobbyBeat(name: "opening", tank: false, jars: false, bench: true, glass: "glass", sound: "move"),
        LobbyBeat(name: "bench", tank: nil, jars: nil, bench: nil, glass: nil, sound: nil),
        LobbyBeat(name: "tank", tank: nil, jars: nil, bench: nil, glass: nil, sound: nil),
        LobbyBeat(name: "jars", tank: nil, jars: nil, bench: nil, glass: nil, sound: nil),
    ]

    /// The beat on screen. Hooks past this are not played.
    static let opening = beats[0]

    static var cast: RoomCast {
        RoomCast(
            tank: opening.tank ?? false,
            jars: opening.jars ?? false,
            bench: opening.bench ?? false,
            openingGlass: opening.glass != nil
        )
    }
}

/// A short cue for a beat. It does not start or stop the cassette.
/// Day, night, and weather for the left window, plus the two lights.
/// `phase` runs 0...1 across `period` so the cycle is visible in the room.
struct RoomMood: Equatable {
    var phase: Double = 0.12
    var lampOn: Bool = true
    var shelfOn: Bool = true
    var candleLit: Bool = false

    static let period: TimeInterval = 180

    enum DayPart { case day, dusk, night }

    static func phase(at date: Date = Date()) -> Double {
        let t = date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: period)
        let wrapped = t < 0 ? t + period : t
        return wrapped / period
    }

    static var storedLamp: Bool {
        if UserDefaults.standard.object(forKey: "lobbyLamp") == nil { return true }
        return UserDefaults.standard.bool(forKey: "lobbyLamp")
    }

    static var storedShelf: Bool {
        if UserDefaults.standard.object(forKey: "lobbyShelfLamp") == nil { return true }
        return UserDefaults.standard.bool(forKey: "lobbyShelfLamp")
    }

    static var storedCandle: Bool {
        UserDefaults.standard.bool(forKey: "lobbyCandle")
    }

    var dayPart: DayPart {
        switch phase {
        case 0.58..<0.72, 0.92..<1: return .dusk
        case 0.72..<0.92: return .night
        default: return .day
        }
    }

    var cloudy: Bool { phase >= 0.28 && phase < 0.58 }
    var raining: Bool { phase >= 0.40 && phase < 0.58 }

    /// How hard the window light pushes into the room.
    var sunPower: Int {
        switch dayPart {
        case .night: return 0
        case .dusk: return raining ? 6 : 12
        case .day: return raining ? 8 : (cloudy ? 16 : 30)
        }
    }
}

enum SystemCue {
    private static var played = Set<String>()
    private static var player: AVAudioPlayer?

    static func play(_ name: String?) {
        guard let name, played.insert(name).inserted else { return }
        let rate = 22_050
        let n = rate / 6
        var samples = [Int16](repeating: 0, count: n)
        for i in 0..<n {
            let t = Float(i) / Float(rate)
            let env = sin(Float.pi * Float(i) / Float(n))
            let s = sin(2 * Float.pi * 520 * t) * env * 0.35
            samples[i] = Int16(max(-1, min(1, s)) * 32000)
        }
        var data = Data(capacity: 44 + n * 2)
        func append(_ s: String) { data.append(contentsOf: s.utf8) }
        func append32(_ v: UInt32) { var le = v.littleEndian; withUnsafeBytes(of: &le) { data.append(contentsOf: $0) } }
        func append16(_ v: UInt16) { var le = v.littleEndian; withUnsafeBytes(of: &le) { data.append(contentsOf: $0) } }
        append("RIFF")
        append32(UInt32(36 + n * 2))
        append("WAVE")
        append("fmt ")
        append32(16)
        append16(1)
        append16(1)
        append32(UInt32(rate))
        append32(UInt32(rate * 2))
        append16(2)
        append16(16)
        append("data")
        append32(UInt32(n * 2))
        samples.withUnsafeBytes { data.append(contentsOf: $0) }
        player = try? AVAudioPlayer(data: data)
        player?.volume = 0.5
        player?.play()
    }
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
    let cast: RoomCast
    let mood: RoomMood
    var bytes: [UInt8]

    init(w: Int, h: Int, cast: RoomCast = RoomCast(), mood: RoomMood = RoomMood()) {
        self.w = w
        self.h = h
        self.cast = cast
        self.mood = mood
        bytes = [UInt8](repeating: 0, count: w * h * 4)
    }

    func paint() {
        planks()
        window()
        shelf()
        tree()
        lamps()
        if cast.tank { tank() }
        favouriteFrame()
        shearHook()
        if cast.bench {
            bench()
        }
        floor()
        if cast.bench {
            archiveBook()
            filingBox()
            cassette()
            tankPlaque()
        }
        if cast.bench { fuse() }
        if cast.openingGlass { tumbler() }
        ficus()
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
        let top = skyTop()
        let bot = skyBot()
        for y in 47..<101 {
            let t = y < 78 ? 0 : (y - 78)
            fill(1, y, 13, 1, t == 0 ? top : blend(top, bot, t, 23))
        }
        if mood.cloudy {
            let cloud = mood.raining ? 0xD8DEE4 : 0xF2F6F8
            fill(2, 52, 8, 2, cloud)
            fill(6, 58, 7, 2, cloud)
            fill(2, 64, 6, 1, cloud)
        }
        if mood.raining {
            let shift = Int(mood.phase * 180) % 4
            for x in stride(from: 2, to: 13, by: 3) {
                fill(x, 60 + (x + shift) % 5, 1, 4, 0xD6E6F0)
            }
        }
        if mood.dayPart == .night {
            fill(10, 52, 2, 2, 0xF4F0E4)
        }
        fill(7, 47, 1, 54, Ink.wood)
        fill(1, 73, 13, 1, Ink.wood)
        for x in 1..<14 {
            let crown = 10 + (x * 5) % 12
            let pine = mood.dayPart == .night ? Ink.pineDk : (x % 2 == 0 ? Ink.pine : Ink.pineDk)
            fill(x, 101 - crown, 1, crown, pine)
        }
        fill(0, 46, 15, 1, Ink.woodLt)
        fill(0, 100, 16, 2, Ink.woodLt)
        fill(0, 102, 16, 2, Ink.woodDk)
    }

    private func skyTop() -> Int {
        switch mood.dayPart {
        case .night: return 0x1A2438
        case .dusk: return 0xC47858
        case .day: return mood.raining ? 0x6E8494 : (mood.cloudy ? 0xB7C6D2 : Ink.sky)
        }
    }

    private func skyBot() -> Int {
        switch mood.dayPart {
        case .night: return 0x101820
        case .dusk: return 0x3A3458
        case .day: return mood.raining ? 0x4A6270 : (mood.cloudy ? 0x8AA0AE : Ink.skyDeep)
        }
    }

    private func blend(_ a: Int, _ b: Int, _ t: Int, _ n: Int) -> Int {
        func mix(_ shift: Int) -> Int {
            let av = (a >> shift) & 255
            let bv = (b >> shift) & 255
            return av + (bv - av) * t / max(n, 1)
        }
        return (mix(16) << 16) | (mix(8) << 8) | mix(0)
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
                var sunAmt = mood.sunPower - dx / 3 - vertical
                if y >= RoomArt.floorY && x < 78 && mood.dayPart == .day && !mood.raining {
                    sunAmt = max(sunAmt, 14 - dx / 6 - (y - RoomArt.floorY) / 5)
                }
                if sunAmt > 0 { sun(x, y, sunAmt) }
                if mood.dayPart == .night {
                    if dx < 36 { cool(x, y, (36 - dx) / 5) }
                    shade(x, y, 6)
                } else if mood.dayPart == .dusk {
                    shade(x, y, 3)
                } else if mood.raining {
                    if dx < 28 { cool(x, y, (28 - dx) / 6) }
                    shade(x, y, 3)
                }
                if dx > 52 {
                    let sh = min(18, (dx - 52) / 3 + (inBeam ? 0 : 3))
                    shade(x, y, sh)
                }
            }
        }
        // Hard rims: light from the left leaves a dark edge on the right.
        if cast.tank { rightRim(16, 100, 88, 60) }
        if cast.bench {
            rightRim(6, 156, 108, 12)
            rightRim(RoomArt.crate.x, RoomArt.crate.y, RoomArt.crate.w, RoomArt.crate.h)
            rightRim(RoomArt.book.x, RoomArt.book.y, RoomArt.book.w, RoomArt.book.h)
            rightRim(RoomArt.deck.x, RoomArt.deck.y, RoomArt.deck.w, RoomArt.deck.h)
            rightRim(RoomArt.fuseBox.x, RoomArt.fuseBox.y, RoomArt.fuseBox.w, RoomArt.fuseBox.h)
        }
        for x in 2..<118 {
            shade(x, 40, 5 + x / 24)
        }
        if mood.shelfOn { cone(cx: 66, cy: 30, reach: 18, power: 22) }
        if mood.lampOn { cone(cx: 8, cy: 118, reach: 36, power: 24) }
        if mood.candleLit { cone(cx: 112, cy: 124, reach: 22, power: 18) }
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

        // The window plant. A bright stem runs from the soil, through the
        // shelf, and into the live vine so the pot and the hanging part meet.
        fill(4, 26, 12, 8, Ink.pot)
        fill(3, 25, 14, 2, Ink.potLt)
        fill(7, 28, 6, 3, 0x3E2818)
        sprite(4, 21, [
            " gGGg ",
            "gG  Gg",
        ], [
            "g": Ink.plantDk, "G": Ink.plant, " ": 0,
        ])
        fill(8, 29, 3, 22, Ink.plant)
        fill(9, 29, 1, 22, Ink.plantLt)

        for spot in RoomArt.vials where cast.jars {
            vial(spot.x, spot.y)
        }

        // Lantern. A double tap on the shelf light turns this on or off.
        fill(64, 16, 2, 3, Ink.brassDk)
        fill(62, 19, 8, 2, Ink.brass)
        fill(63, 21, 6, 10, Ink.brassDk)
        if mood.shelfOn {
            fill(64, 23, 4, 6, Ink.glow)
            fill(65, 24, 2, 3, Ink.glowHot)
        } else {
            fill(64, 23, 4, 6, Ink.lampOff)
        }
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
        fill(f.x - 1, f.y - 1, f.w + 2, f.h + 2, Ink.woodDk)
        fill(f.x, f.y, f.w, f.h, Ink.brassDk)
        fill(f.x + 2, f.y + 2, f.w - 4, f.h - 4, Ink.wood)
        fill(f.x + 3, f.y + 3, f.w - 6, 1, Ink.woodLt)
        fill(f.x + 3, f.y + f.h - 4, f.w - 6, 1, Ink.brass)
        fill(inner.x - 1, inner.y - 1, inner.w + 2, inner.h + 2, Ink.black)
        fill(inner.x, inner.y, inner.w, inner.h, Ink.night)
        for (x, y) in [
            (f.x + 1, f.y + 1), (f.x + f.w - 2, f.y + 1),
            (f.x + 1, f.y + f.h - 2), (f.x + f.w - 2, f.y + f.h - 2),
        ] {
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
        if mood.candleLit {
            fill(x + 5, 126, 2, 4, Ink.glow)
            set(x + 5, 124, Ink.glowHot)
            set(x + 6, 127, Ink.glowHot)
        } else {
            set(x + 5, 128, 0x2A241C)
        }
        fill(x + 2, 152, 8, 2, Ink.brass)
        fill(x + 3, 154, 6, 2, Ink.brassDk)
    }

    private func deskLamp(_ x: Int) {
        fill(x, 112, 12, 2, Ink.brass)
        fill(x + 1, 114, 10, 8, Ink.brassDk)
        fill(x + 3, 116, 6, 4, mood.lampOn ? Ink.glow : Ink.lampOff)
        if mood.lampOn {
            fill(x + 4, 117, 4, 2, Ink.glowHot)
        }
        fill(x + 5, 122, 2, 30, Ink.metalDk)
        fill(x + 2, 152, 8, 2, Ink.brass)
        fill(x + 1, 154, 10, 2, Ink.brassDk)
    }

    /// Taller than the shelf pots. Crown first, then the pot, then the
    /// trunk last so it runs from inside the soil up into the leaves.
    private func ficus() {
        let p = RoomArt.ficus
        let potTop = p.y + p.h - 12
        let cx = p.x + p.w / 2
        fill(p.x, p.y + 16, p.w, 16, Ink.plant)
        fill(p.x + 1, p.y + 8, p.w - 2, 14, Ink.plantLt)
        fill(p.x + 4, p.y + 2, p.w - 8, 10, Ink.plant)
        fill(p.x - 1, p.y + 20, 6, 6, Ink.plantDk)
        fill(p.x + p.w - 5, p.y + 14, 6, 6, Ink.plant)
        fill(p.x + 1, potTop + 3, p.w - 2, 9, Ink.pot)
        fill(p.x, potTop, p.w, 4, Ink.potLt)
        fill(p.x + 3, potTop + 1, p.w - 6, 2, 0x3E2818)
        let trunkTop = p.y + 18
        let trunkBot = potTop + 5
        guard trunkBot > trunkTop else { return }
        fill(cx - 1, trunkTop, 3, trunkBot - trunkTop, Ink.woodDk)
        fill(cx, trunkTop + 2, 1, max(1, trunkBot - trunkTop - 6), Ink.wood)
        fill(cx - 5, trunkTop + 6, 5, 2, Ink.woodDk)
        fill(cx + 2, trunkTop + 12, 5, 2, Ink.woodDk)
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

    /// One tumbler of water. The mote that moves in it is drawn on top.
    private func tumbler() {
        let g = RoomArt.openingGlass
        fill(g.x + 3, g.y, g.w - 6, 2, Ink.brass)
        fill(g.x + 1, g.y + 2, 1, g.h - 4, Ink.glass)
        fill(g.x + g.w - 2, g.y + 2, 1, g.h - 4, Ink.glass)
        fill(g.x + 2, g.y + 4, g.w - 4, g.h - 8, Ink.waterL)
        fill(g.x + 2, g.y + 6, g.w - 4, 4, Ink.waterM)
        fill(g.x, g.y + g.h - 2, g.w, 2, Ink.glass)
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

    /// One closed book under the table. The archive and the log share it.
    private func archiveBook() {
        let b = RoomArt.book
        fill(b.x + 3, b.y + 1, max(1, b.w - 4), b.h - 2, Ink.page)
        fill(b.x, b.y, max(1, b.w - 5), b.h, Ink.red)
        fill(b.x, b.y, max(1, b.w - 5), 2, Ink.redLt)
        fill(b.x, b.y, 3, b.h, Ink.redDk)
        fill(b.x + 2, b.y + b.h, max(1, b.w - 3), 2, 0x120C0E)
    }

    /// The gray box under the table. Charts live here.
    private func filingBox() {
        let c = RoomArt.crate
        fill(c.x, c.y, c.w, c.h, Ink.metalDk)
        fill(c.x, c.y, c.w, 2, Ink.metal)
        fill(c.x, c.y + c.h - 2, c.w, 2, 0x2A3034)
        fill(c.x + 2, c.y + 5, 4, 4, Ink.brass)
        fill(c.x + 1, c.y + c.h, max(1, c.w - 2), 2, 0x120C0E)
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

    /// An empty hook. The scissors themselves are drawn on top.
    private func shearHook() {
        let h = RoomArt.shearHook
        let x = h.x + 5
        let y = h.y + 1
        fill(x, y, 2, 2, Ink.metalLt)
        fill(x - 1, y + 2, 4, 2, Ink.metal)
        fill(x - 1, y + 3, 2, 5, Ink.metalDk)
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

    /// Night and rain. A little blue near the window, red falls off.
    private func cool(_ x: Int, _ y: Int, _ amt: Int) {
        guard x >= 0, y >= 0, x < w, y < h, amt > 0, !holds(x, y) else { return }
        let i = (y * w + x) * 4
        bytes[i] = UInt8(max(Int(bytes[i]) - amt / 2, 0))
        bytes[i + 2] = UInt8(min(Int(bytes[i + 2]) + amt, 255))
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
