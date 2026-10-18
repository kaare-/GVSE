import CoreMotion
import Foundation
import QuartzCore
import UIKit

/// What the inspect card shows for the creature under the finger. Numbers
/// come from `tank_inspect_cell`/`tank_lineage`; the rectangle is refreshed
/// every frame from `tank_org_bbox` so the card follows the body. Never
/// the genome.
struct InspectState: Equatable {
    var cell: UInt32
    var org: UInt32
    var species: UInt16
    var speciesName: String
    var parentName: String?
    var generation: UInt32
    var orgCells: UInt32
    var gluedKin: UInt32
    var ageTicks: UInt32
    var maxAgeTicks: UInt32
    var children: UInt32
    var energyFrac: UInt8
    var stress: UInt8
    var lineage: LineageRec
    /// Tracking rectangle in view points (nil when the body is gone).
    var rect: CGRect?
    var alive = true

    static func == (a: InspectState, b: InspectState) -> Bool {
        a.cell == b.cell && a.org == b.org && a.rect == b.rect && a.alive == b.alive
            && a.energyFrac == b.energyFrac && a.ageTicks == b.ageTicks
            && a.children == b.children && a.orgCells == b.orgCells
    }
}

/// One species of the tree-of-life view with its name resolved.
struct SpeciesNode: Identifiable {
    let rec: SpeciesNodeRec
    let name: String
    var id: UInt16 { rec.id }
    var parent: UInt16? { rec.parent == UInt16(TANK_NO_SPECIES) ? nil : rec.parent }
}

/// The big tank, or one of the three shelf vials. The vials are their own saves.
enum TankContainer: Equatable {
    case main
    case sample(Int)

    var sampleIndex: Int? {
        if case .sample(let index) = self { return index }
        return nil
    }

    var key: String {
        switch self {
        case .main: return "main"
        case .sample(let index): return "sample-\(index)"
        }
    }

    static func stored() -> TankContainer {
        let raw = UserDefaults.standard.string(forKey: "openContainer") ?? "main"
        if raw.hasPrefix("sample-"), let n = Int(raw.dropFirst("sample-".count)), (0..<3).contains(n) {
            return .sample(n)
        }
        return .main
    }
}

/// Owns the tank handle, the 30 Hz step, tools, and save/resume.
/// Backgrounding saves and drops leftover frame time so the sim does
/// not catch up the wall clock.
final class TankSession: ObservableObject {
    @Published private(set) var isDay = true
    /// Day 1 covers that day's night as well. The counter advances at dawn.
    @Published private(set) var dayNumber = 1
    @Published private(set) var playing = true
    @Published private(set) var consent = false
    @Published private(set) var temperatureC: Float = 15
    @Published var tool: BuildTool = .sand {
        didSet {
            if tool != .needle {
                needleTip = nil
                if tool != oldValue { inspected = nil }
            }
        }
    }
    /// Add lays the selected material down. Remove lifts only that material.
    @Published var strokeMode: StrokeMode = .add
    @Published var brush = 3
    @Published var variant = 0
    @Published var fillLevel = 0.55
    /// Main tank, or a shelf sample. Jar 1 is not the main tank.
    @Published private(set) var container: TankContainer = .main
    /// Needle hot spot, above the finger so the tip stays visible.
    @Published var needleTip: CGPoint?
    @Published var swirl = false
    /// System Reduce Motion. Swirl stays off while this is set.
    @Published private(set) var reduceMotion = UIAccessibility.isReduceMotionEnabled
    var effectiveSwirl: Bool { swirl && !reduceMotion }
    /// Pause the sim when a big evolution milestone fires (Phase 6).
    @Published var pauseOnEvent = false
    @Published var soundOn = true
    @Published var hapticsOn = true
    /// Freeze the tank clock at midday (Phase 4 setting).
    @Published var alwaysDay = false
    /// Base temperature °C (default 15).
    @Published var tBase: Double = 15
    /// Global light multiplier 0.25…1.5 (also warms via T_direct).
    @Published var lightSlider: Double = 1
    /// Enclosure op when BOX is selected: 0 walls · 1 drain · 2 fill · 3 release.
    @Published var encloseOp = 0
    @Published var magnifierPinned = false
    @Published var magnifierPoint: CGPoint?
    @Published var cameraTiltDegrees: Double = 0
    @Published private(set) var inspected: InspectState?
    /// Cells in the scoop bucket; 0 = empty.
    @Published private(set) var carriedCells = 0
    /// Short pixel-font toast ("NO ROOM", "ARCHIVED").
    @Published private(set) var hint: String?
    @Published private(set) var liveCells: UInt32 = 0
    /// Door sign text (Phase 6 room).
    @Published private(set) var doorName: String = ""
    /// Favourite archive specimen id for the room frame.
    @Published private(set) var favouriteId: UInt64 = 0
    /// Session event log (newest first).
    @Published private(set) var eventLog: [EventLogEntry] = []
    @Published private(set) var showStartGuide = false

    let archive = ArchiveStore()
    private var eventLogSerial: UInt64 = 0
    private let haptic = UIImpactFeedbackGenerator(style: .light)

    private var handle: OpaquePointer?
    private var carried: OpaquePointer?
    private var link: CADisplayLink?
    private var pump: DisplayPump?
    private weak var metal: TankMetalView?
    private var accumulator: CFTimeInterval = 0
    private var running = false
    private let step = 1.0 / 30.0
    private var pendingTools: [ToolEvent] = []
    private let motion = CMMotionManager()
    private var lastGravX: Float = 0
    private var lastGravY: Float = -1
    private var frame = 0
    private var hintToken = 0
    private var reduceMotionObserver: NSObjectProtocol?

    private var docs: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    }

    private func saveURL(for container: TankContainer) -> URL {
        switch container {
        case .main:
            return docs.appendingPathComponent("tank-0.postcard")
        case .sample(let index):
            return docs.appendingPathComponent("sample-\(index).postcard")
        }
    }

    init() {
        TelemetryClient.installCrashHook()
        container = TankContainer.stored()
        swirl = UserDefaults.standard.bool(forKey: "swirlEnabled")
        pauseOnEvent = UserDefaults.standard.bool(forKey: "pauseOnEvent")
        soundOn = UserDefaults.standard.object(forKey: "soundOn") as? Bool ?? true
        hapticsOn = UserDefaults.standard.object(forKey: "hapticsOn") as? Bool ?? true
        alwaysDay = UserDefaults.standard.bool(forKey: "alwaysDay")
        let savedBase = UserDefaults.standard.double(forKey: "tBase")
        tBase = savedBase == 0 ? 15 : savedBase
        let savedLight = UserDefaults.standard.double(forKey: "lightSlider")
        lightSlider = savedLight == 0 ? 1 : min(1.5, max(0.25, savedLight))
        doorName = UserDefaults.standard.string(forKey: "doorName") ?? ""
        favouriteId = UInt64(UserDefaults.standard.integer(forKey: "favouriteId"))
        showStartGuide = !UserDefaults.standard.bool(forKey: "startGuideDone")
        PixelKeyboard.applyAppearance()
        reduceMotionObserver = NotificationCenter.default.addObserver(
            forName: UIAccessibility.reduceMotionStatusDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.reduceMotion = UIAccessibility.isReduceMotionEnabled
        }
        loadContainer()
        consent = UserDefaults.standard.bool(forKey: "telemetryConsent")
        if let handle {
            tank_set_consent(handle, consent ? 1 : 0)
            if UserDefaults.standard.bool(forKey: "telemetryKill") {
                tank_telemetry_kill(handle, 1)
            }
            TelemetryClient.flushPendingCrash(handle: handle, consent: consent)
            AmbientAudio.shared.setTankSeed(tank_seed(handle))
        }
        AmbientAudio.shared.setMuted(!soundOn)
        if soundOn { AmbientAudio.shared.startIfNeeded() }
        haptic.prepare()
    }

    deinit {
        link?.invalidate()
        motion.stopDeviceMotionUpdates()
        dropBucket()
        tank_free(handle)
    }

    func attach(view: TankMetalView) {
        metal = view
        view.onTouch = { [weak self] point, ended in
            self?.handleTouch(point: point, ended: ended)
        }
    }

    func resume() {
        accumulator = 0
        running = true
        playing = true
        startMotion()
        if soundOn { AmbientAudio.shared.startIfNeeded() }
        guard link == nil else { return }
        let pump = DisplayPump()
        pump.onFrame = { [weak self] dt in self?.advance(dt) }
        self.pump = pump
        let link = CADisplayLink(target: pump, selector: #selector(DisplayPump.step(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 60, preferred: 60)
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    /// The bucket is not part of the save: whatever is carried goes back
    /// into the tank where it was scooped, or is lost if that no longer
    /// fits, before the file is written.
    func pauseAndSave() {
        running = false
        playing = false
        accumulator = 0
        link?.invalidate()
        link = nil
        pump = nil
        motion.stopDeviceMotionUpdates()
        AmbientAudio.shared.stop()
        dropBucket()
        saveCurrent()
    }

    /// Freeze the sim without dropping a sample or writing the save.
    func pause() {
        guard running else { return }
        running = false
        playing = false
        AmbientAudio.shared.stop()
        showHint("PAUSED")
    }

    /// Resume after pause-on-event (also the PLAY button).
    func unpauseFromEvent() {
        guard !running else { return }
        running = true
        playing = true
        if link == nil {
            resume()
        } else if soundOn {
            AmbientAudio.shared.startIfNeeded()
        }
        showHint("RUNNING")
    }

    func toggleConsent() {
        consent.toggle()
        UserDefaults.standard.set(consent, forKey: "telemetryConsent")
        if let handle {
            tank_set_consent(handle, consent ? 1 : 0)
            if consent {
                tank_telemetry_push(handle, 1, 0)
                TelemetryClient.flush(handle: handle, consent: true)
                TelemetryClient.refreshKillSwitch { [weak self] kill in
                    guard kill, let handle = self?.handle else { return }
                    tank_telemetry_kill(handle, 1)
                }
            }
        }
    }

    func toggleSwirl() {
        swirl.toggle()
        UserDefaults.standard.set(swirl, forKey: "swirlEnabled")
        if !swirl {
            cameraTiltDegrees = 0
            metal?.cameraTiltDegrees = 0
        }
    }

    func openMain() {
        switchTo(.main)
    }

    func openSample(_ index: Int) {
        switchTo(.sample(max(0, min(2, index))))
    }

    /// A carried creature stays in the bucket so it can be poured into another vial.
    private func switchTo(_ next: TankContainer) {
        guard next != container else { return }
        saveCurrent()
        container = next
        UserDefaults.standard.set(next.key, forKey: "openContainer")
        loadContainer()
    }

    func newTank() {
        dropBucket()
        inspected = nil
        tank_free(handle)
        handle = tank_empty(UInt64(Date().timeIntervalSince1970))
        pendingTools.removeAll()
        saveCurrent()
        refreshClock()
    }

    /// Wipe one shelf vial. The open sample is replaced in place. Another
    /// vial is rewritten on disk and the sample bucket is left alone.
    /// The main tank is not one of these vials.
    func emptySample(_ index: Int) {
        let index = max(0, min(2, index))
        if container == .sample(index) {
            newTank()
            showHint("EMPTY")
            return
        }
        guard let fresh = tank_empty(UInt64(Date().timeIntervalSince1970)) else { return }
        write(handle: fresh, to: .sample(index))
        tank_free(fresh)
        showHint("EMPTY")
    }

    /// Three empty tanks and the room's default breakers. The book stays.
    func resetGame() {
        dropBucket()
        inspected = nil
        alwaysDay = false
        swirl = false
        pauseOnEvent = false
        soundOn = true
        hapticsOn = true
        tBase = 15
        lightSlider = 1
        doorName = ""
        favouriteId = 0
        cameraTiltDegrees = 0
        metal?.cameraTiltDegrees = 0
        UserDefaults.standard.set(false, forKey: "swirlEnabled")
        UserDefaults.standard.set(false, forKey: "pauseOnEvent")
        UserDefaults.standard.set(true, forKey: "soundOn")
        UserDefaults.standard.set(true, forKey: "hapticsOn")
        UserDefaults.standard.set(false, forKey: "alwaysDay")
        UserDefaults.standard.set(15.0, forKey: "tBase")
        UserDefaults.standard.set(1.0, forKey: "lightSlider")
        UserDefaults.standard.set("", forKey: "doorName")
        UserDefaults.standard.set(0, forKey: "favouriteId")
        UserDefaults.standard.set(false, forKey: "startGuideDone")
        AmbientAudio.shared.setMuted(false)
        let targets: [TankContainer] = [.main, .sample(0), .sample(1), .sample(2)]
        for (i, target) in targets.enumerated() {
            guard let fresh = tank_empty(UInt64(Date().timeIntervalSince1970) &+ UInt64(i)) else { continue }
            write(handle: fresh, to: target)
            tank_free(fresh)
        }
        container = .main
        UserDefaults.standard.set(TankContainer.main.key, forKey: "openContainer")
        loadContainer()
        showStartGuide = true
        showHint("RESET")
    }

    /// Replace the current slot with a fresh half-full tank seeded with a
    /// few copies of an archived creature. Returns how many were placed.
    @discardableResult
    func newTank(from specimen: ArchivedSpecimen, copies: UInt32 = 3) -> UInt32 {
        if container != .main {
            switchTo(.main)
        }
        dropBucket()
        inspected = nil
        tank_free(handle)
        let fresh = tank_empty(UInt64(Date().timeIntervalSince1970))
        handle = fresh
        pendingTools.removeAll()
        guard let fresh else { return 0 }
        // Fill to the default level before the first tick so the copies
        // land in standing water.
        var w: UInt32 = 0
        var h: UInt32 = 0
        _ = tank_pixels(fresh, &w, &h)
        let cells = max(Int(h) / 3, 1)
        var fill = ToolEvent(kind: 5, material: 0, x: 0, y: UInt16(Double(cells) * fillLevel), r: 1, variant: 0, _pad: 0)
        withUnsafePointer(to: &fill) { ptr in
            var input = TankInput(
                grav_x: 0, grav_y: -1, paused: 0, always_day: alwaysDay ? 1 : 0, swirl: 0, _pad1: 0,
                light_slider: Float(lightSlider), t_base: Float(tBase), flash_x: 0, flash_y: 0, flash_r: 0,
                tools: ptr, tool_count: 1
            )
            tank_tick(fresh, &input)
        }
        let placed = specimen.bytes.withUnsafeBytes { raw -> UInt32 in
            let base = raw.baseAddress!.assumingMemoryBound(to: UInt8.self)
            return tank_seed_specimen_copies(fresh, base, raw.count, copies)
        }
        saveCurrent()
        refreshClock()
        showHint(placed > 0 ? "\(placed) PLACED" : "NO ROOM")
        return placed
    }

    func undo() {
        guard let handle else { return }
        _ = tank_undo(handle)
    }

    func applyFillSlider() {
        guard let handle else { return }
        var h: UInt32 = 0
        var w: UInt32 = 0
        _ = tank_pixels(handle, &w, &h)
        // pixel height / 3 = visible physics height
        let cells = max(Int(h) / 3, 1)
        let level = UInt16(Double(cells) * fillLevel)
        pendingTools.append(ToolEvent(kind: 5, material: 0, x: 0, y: level, r: 1, variant: 0, _pad: 0))
    }

    /// Pour pure water onto every free surface (tool 14).
    func topUp() {
        pendingTools.append(ToolEvent(kind: 14, material: 0, x: 0, y: 0, r: 1, variant: 0, _pad: 0))
        showHint("TOPPED UP")
    }

    func toggleAlwaysDay() {
        alwaysDay.toggle()
        UserDefaults.standard.set(alwaysDay, forKey: "alwaysDay")
    }

    func togglePauseOnEvent() {
        pauseOnEvent.toggle()
        UserDefaults.standard.set(pauseOnEvent, forKey: "pauseOnEvent")
    }

    func toggleSound() {
        soundOn.toggle()
        UserDefaults.standard.set(soundOn, forKey: "soundOn")
        AmbientAudio.shared.setMuted(!soundOn)
    }

    func toggleHaptics() {
        hapticsOn.toggle()
        UserDefaults.standard.set(hapticsOn, forKey: "hapticsOn")
    }

    func setDoorName(_ name: String) {
        doorName = String(name.prefix(24))
        UserDefaults.standard.set(doorName, forKey: "doorName")
    }

    var favouriteSpecimen: ArchivedSpecimen? {
        archive.specimens.first { $0.id == favouriteId }
    }

    func cycleFavourite() {
        let list = archive.specimens
        guard !list.isEmpty else {
            showHint("ARCHIVE FIRST")
            return
        }
        if let i = list.firstIndex(where: { $0.id == favouriteId }) {
            favouriteId = list[(i + 1) % list.count].id
        } else {
            favouriteId = list[0].id
        }
        UserDefaults.standard.set(Int(favouriteId), forKey: "favouriteId")
        showHint("FAVOURITE SET")
    }

    func clearFavourite() {
        favouriteId = 0
        UserDefaults.standard.set(0, forKey: "favouriteId")
    }

    func dismissStartGuide(skipped: Bool) {
        showStartGuide = false
        UserDefaults.standard.set(true, forKey: "startGuideDone")
        if let handle {
            tank_telemetry_push(handle, skipped ? 60 : 61, 0)
        }
    }

    func showPhotoHint(_ text: String) {
        showHint(text)
    }

    /// RGBA tank buffer → nearest-neighbour UIImage.
    func tankSnapshot(scale: Int = 2) -> UIImage? {
        guard let handle else { return nil }
        var w: UInt32 = 0
        var h: UInt32 = 0
        guard let ptr = tank_pixels(handle, &w, &h), w > 0, h > 0 else { return nil }
        return rgbaImage(ptr: ptr, width: Int(w), height: Int(h), scale: max(1, scale))
    }

    /// Crop around the inspected organism bbox.
    func creatureSnapshot(scale: Int = 4) -> UIImage? {
        guard let handle, let inspected, inspected.alive else { return nil }
        var w: UInt32 = 0
        var h: UInt32 = 0
        guard let ptr = tank_pixels(handle, &w, &h), w > 0, h > 0 else { return nil }
        let pw = Int(w)
        let ph = Int(h)
        var x0u: UInt16 = 0, y0u: UInt16 = 0, x1u: UInt16 = 0, y1u: UInt16 = 0
        guard tank_org_bbox(handle, inspected.org, &x0u, &y0u, &x1u, &y1u) != 0, x1u > x0u, y1u > y0u else {
            return tankSnapshot(scale: scale)
        }
        let pad = 4
        let x0 = max(0, Int(x0u) - pad)
        let y0 = max(0, Int(y0u) - pad)
        let x1 = min(pw, Int(x1u) + pad)
        let y1 = min(ph, Int(y1u) + pad)
        let cw = max(1, x1 - x0)
        let ch = max(1, y1 - y0)
        var crop = [UInt8](repeating: 0, count: cw * ch * 4)
        for row in 0..<ch {
            let src = ptr.advanced(by: ((y0 + row) * pw + x0) * 4)
            _ = crop.withUnsafeMutableBytes { dst in
                memcpy(dst.baseAddress!.advanced(by: row * cw * 4), src, cw * 4)
            }
        }
        return crop.withUnsafeBytes { raw in
            rgbaImage(
                ptr: raw.baseAddress!.assumingMemoryBound(to: UInt8.self),
                width: cw,
                height: ch,
                scale: max(1, scale)
            )
        }
    }

    private func rgbaImage(ptr: UnsafePointer<UInt8>, width: Int, height: Int, scale: Int) -> UIImage? {
        let data = Data(bytes: ptr, count: width * height * 4)
        guard let provider = CGDataProvider(data: data as CFData),
              let cg = CGImage(
                  width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                  bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                  bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                  provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent
              )
        else { return nil }
        let size = CGSize(width: width * scale, height: height * scale)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        return UIGraphicsImageRenderer(size: size, format: format).image { ctx in
            ctx.cgContext.interpolationQuality = .none
            ctx.cgContext.translateBy(x: 0, y: size.height)
            ctx.cgContext.scaleBy(x: 1, y: -1)
            ctx.cgContext.draw(cg, in: CGRect(origin: .zero, size: size))
        }
    }

    func nudgeBaseTemp(_ delta: Double) {
        tBase = min(35, max(5, tBase + delta))
        UserDefaults.standard.set(tBase, forKey: "tBase")
    }

    func nudgeLight(_ delta: Double) {
        lightSlider = min(1.5, max(0.25, lightSlider + delta))
        UserDefaults.standard.set(lightSlider, forKey: "lightSlider")
    }

    /// Seed a built-in Phase 4 starter into the archive book.
    @discardableResult
    func addStarter(kind: UInt8) -> Bool {
        let need = tank_starter_specimen(kind, nil, 0)
        guard need > 0 else { return false }
        var buf = [UInt8](repeating: 0, count: need)
        let wrote = tank_starter_specimen(kind, &buf, need)
        guard wrote == need, let specimen = archive.add(bytes: Data(buf)) else { return false }
        showHint("GOT \(specimen.displayName.uppercased())")
        return true
    }

    // MARK: inspect / archive

    func closeInspect() {
        inspected = nil
    }

    /// Archive the inspected creature. The species is pinned in the tank
    /// (continues as a true root when it reappears from the book).
    func archiveInspected() {
        guard let handle, let inspected, inspected.alive else { return }
        let need = tank_specimen(handle, inspected.cell, nil, 0)
        guard need > 0 else {
            showHint("GONE")
            return
        }
        var buf = [UInt8](repeating: 0, count: need)
        let wrote = tank_specimen(handle, inspected.cell, &buf, need)
        guard wrote == need, archive.add(bytes: Data(buf)) != nil else {
            showHint("GONE")
            return
        }
        showHint("ARCHIVED")
        refreshInspect(force: true)
    }

    /// The whole tree-of-life, roots first, with names.
    func speciesTree() -> [SpeciesNode] {
        guard let handle else { return [] }
        let count = Int(tank_species_tree(handle, nil, 0))
        guard count > 0 else { return [] }
        var recs = [SpeciesNodeRec](repeating: SpeciesNodeRec(), count: count)
        let got = Int(tank_species_tree(handle, &recs, UInt32(count)))
        return recs.prefix(got).map { SpeciesNode(rec: $0, name: speciesName($0.id)) }
    }

    func speciesName(_ species: UInt16) -> String {
        guard let handle, species != UInt16(TANK_NO_SPECIES) else { return "" }
        let need = tank_species_name(handle, species, nil, 0)
        var buf = [UInt8](repeating: 0, count: Int(need) + 1)
        _ = tank_species_name(handle, species, &buf, UInt32(buf.count))
        return String(decoding: buf.prefix(Int(need)), as: UTF8.self)
    }

    // MARK: scoop

    /// Throw the bucket's contents away (the "DUMP" button).
    func discardBucket() {
        guard let handle, let carried else { return }
        tank_discard(handle, carried)
        self.carried = nil
        carriedCells = 0
        showHint("DUMPED")
    }

    /// Put the bucket back where it came from; what no longer fits is lost.
    private func dropBucket() {
        guard let carried else { return }
        if let handle {
            tank_discard(handle, carried)
        } else {
            tank_carried_free(carried)
        }
        self.carried = nil
        carriedCells = 0
    }

    private func scoop(at point: CGPoint) {
        guard let handle, carried == nil, let px = pixel(at: point) else { return }
        let r = Int32(max(brush, 1) * 6)
        guard let bucket = tank_scoop(handle, Int32(px.x), Int32(px.y), r) else {
            showHint("EMPTY")
            return
        }
        carried = bucket
        carriedCells = Int(tank_carried_cells(bucket))
        inspected = nil
    }

    private func release(at point: CGPoint) {
        guard let handle, let carried, let px = pixel(at: point) else { return }
        if tank_release(handle, carried, Int32(px.x), Int32(px.y)) != 0 {
            self.carried = nil
            carriedCells = 0
        } else {
            showHint("NO ROOM")
        }
    }

    // MARK: persistence

    private func loadContainer() {
        inspected = nil
        tank_free(handle)
        handle = nil
        let url = saveURL(for: container)
        if let data = try? Data(contentsOf: url), !data.isEmpty {
            handle = data.withUnsafeBytes { raw in
                guard let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else {
                    return nil
                }
                return tank_load(base, raw.count)
            }
        }
        if handle == nil {
            switch container {
            case .main:
                handle = tank_preset(1)
            case .sample(let index):
                handle = tank_empty(UInt64(index + 11))
            }
        }
        refreshClock()
    }

    private func saveCurrent() {
        guard let handle else { return }
        write(handle: handle, to: container)
    }

    private func write(handle: OpaquePointer, to container: TankContainer) {
        let needed = tank_save(handle, nil, 0)
        guard needed > 0 else { return }
        var data = Data(count: needed)
        let wrote = data.withUnsafeMutableBytes { raw -> Int in
            guard let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else {
                return 0
            }
            return tank_save(handle, base, needed)
        }
        guard wrote == needed else { return }
        try? data.write(to: saveURL(for: container), options: .atomic)
    }

    private func refreshClock() {
        guard let handle else { return }
        let tick = tank_tick_count(handle)
        dayNumber = Int(tick / 72_000) + 1
        isDay = tank_is_day(handle) != 0
        temperatureC = tank_temperature(handle)
        liveCells = tank_live_cells(handle)
    }

    private func startMotion() {
        guard motion.isDeviceMotionAvailable else { return }
        motion.deviceMotionUpdateInterval = 1.0 / 30.0
        motion.startDeviceMotionUpdates(to: .main) { [weak self] data, _ in
            guard let self, let g = data?.gravity else { return }
            // Portrait: x is left/right tilt.
            self.lastGravX = Float(g.x)
            self.lastGravY = Float(g.y)
            if self.effectiveSwirl {
                let deg = Double(g.x) * 4.0
                self.cameraTiltDegrees = max(-4, min(4, deg))
                self.metal?.cameraTiltDegrees = self.cameraTiltDegrees
            }
        }
    }

    // MARK: touch

    private func handleTouch(point: CGPoint?, ended: Bool) {
        guard let point else {
            if ended && !magnifierPinned {
                magnifierPoint = nil
                metal?.magnifier = nil
            }
            return
        }
        switch tool {
        case .look:
            magnifierPoint = point
            metal?.magnifier = magnifier(at: point)
            if ended { clearLensSoon() }
            return
        case .needle:
            let tip = needleTipPoint(point)
            needleTip = tip
            // A pinned glass stays where Look left it.
            if !magnifierPinned {
                magnifierPoint = tip
                metal?.magnifier = magnifier(at: tip)
            }
            inspect(at: tip, ended: ended)
            if ended { clearLensSoon() }
            return
        case .sample:
            if ended {
                if carried == nil { scoop(at: point) } else { release(at: point) }
            }
            magnifierPoint = point
            metal?.magnifier = magnifier(at: point)
            if ended { clearLensSoon() }
            return
        default:
            break
        }
        magnifierPoint = point
        metal?.magnifier = magnifier(at: point)
        queueStroke(at: point, ended: ended)
        if ended { clearLensSoon() }
    }

    /// The hot spot sits above the finger so the tip is not covered.
    private func needleTipPoint(_ touch: CGPoint) -> CGPoint {
        CGPoint(x: touch.x, y: max(12, touch.y - 78))
    }

    /// Round glass under the finger. Radius follows the size slider.
    private func magnifier(at point: CGPoint) -> MagnifierState {
        MagnifierState(
            center: point,
            pinned: magnifierPinned,
            radius: 28 + CGFloat(max(brush, 1)) * 12
        )
    }

    private func clearLensSoon() {
        guard !magnifierPinned else { return }
        // Keep the last stroke; clear the lens after a short beat.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { [weak self] in
            guard let self, !self.magnifierPinned else { return }
            self.magnifierPoint = nil
            self.metal?.magnifier = nil
        }
    }

    private func inspect(at point: CGPoint, ended: Bool) {
        guard let handle, let px = pixel(at: point) else { return }
        let rec = tank_inspect_cell(handle, UInt32(px.x), UInt32(px.y))
        if rec.cell == UInt32(TANK_NO_CELL) {
            // A tap on open water closes the card; a drag keeps the last hit.
            guard ended else { return }
            if let r = inspected?.rect, r.insetBy(dx: -24, dy: -24).contains(point) {
                return
            }
            inspected = nil
            return
        }
        apply(rec)
    }

    private func apply(_ rec: CellInspectRec) {
        guard let handle else { return }
        let lineage = tank_lineage(handle, rec.cell)
        let parent = lineage.parent_species == UInt16(TANK_NO_SPECIES) ? nil : speciesName(lineage.parent_species)
        inspected = InspectState(
            cell: rec.cell,
            org: rec.org,
            species: rec.species,
            speciesName: speciesName(rec.species),
            parentName: parent,
            generation: rec.generation,
            orgCells: rec.org_cells,
            gluedKin: rec.glued_kin,
            ageTicks: rec.age_ticks,
            maxAgeTicks: rec.max_age_ticks,
            children: rec.children,
            energyFrac: rec.energy_frac,
            stress: rec.stress,
            lineage: lineage,
            rect: viewRect(x0: rec.bbox_x0, y0: rec.bbox_y0, x1: rec.bbox_x1, y1: rec.bbox_y1)
        )
    }

    /// Every frame: move the rectangle with the body. Every 10th frame:
    /// re-read the numbers by finding the body again inside its rectangle.
    private func refreshInspect(force: Bool = false) {
        guard let handle, var state = inspected else { return }
        var x0: UInt16 = 0, y0: UInt16 = 0, x1: UInt16 = 0, y1: UInt16 = 0
        if tank_org_bbox(handle, state.org, &x0, &y0, &x1, &y1) == 0 {
            if state.alive {
                state.alive = false
                state.rect = nil
                inspected = state
                showHint("GONE")
            }
            return
        }
        state.rect = viewRect(x0: x0, y0: y0, x1: x1, y1: y1)
        if force || frame % 10 == 0 {
            var probes = 0
            search: for py in y0...y1 {
                for px in x0...x1 {
                    probes += 1
                    if probes > 2048 { break search }
                    let rec = tank_inspect_cell(handle, UInt32(px), UInt32(py))
                    if rec.cell != UInt32(TANK_NO_CELL) && rec.org == state.org {
                        apply(rec)
                        return
                    }
                }
            }
        }
        inspected = state
    }

    private func queueStroke(at point: CGPoint, ended: Bool) {
        guard let cell = physicsCell(at: point) else { return }
        let stroke = tool.stroke(strokeMode)
        if stroke.kind == 0 {
            if ended, let hint = stroke.hint { showHint(hint) }
            return
        }
        pendingTools.append(
            ToolEvent(
                kind: stroke.kind,
                material: stroke.material,
                x: cell.0,
                y: cell.1,
                r: UInt16(brush),
                variant: UInt8(variant),
                _pad: 0
            )
        )
    }

    // MARK: coordinates

    private struct Blit {
        var w: CGFloat
        var h: CGFloat
        var k: CGFloat
        var ox: CGFloat
        var oy: CGFloat
        var scale: CGFloat
    }

    /// The same letterbox the Metal view uses: integer scale `k`, centred.
    private func blit() -> Blit? {
        guard let metal, let handle else { return nil }
        var w: UInt32 = 0
        var h: UInt32 = 0
        _ = tank_pixels(handle, &w, &h)
        guard w > 0, h > 0 else { return nil }
        let scale = metal.contentScale
        let drawableW = metal.bounds.width * scale
        let drawableH = metal.bounds.height * scale
        let k = tankFitScale(drawableW: drawableW, drawableH: drawableH, width: CGFloat(w), height: CGFloat(h))
        let tw = CGFloat(w) * k
        let th = CGFloat(h) * k
        return Blit(w: CGFloat(w), h: CGFloat(h), k: k, ox: (drawableW - tw) * 0.5, oy: (drawableH - th) * 0.5, scale: scale)
    }

    /// View point → tank pixel (top-left origin, as `tank_pixels`).
    private func pixel(at point: CGPoint) -> (x: Int, y: Int)? {
        guard let b = blit() else { return nil }
        let px = (point.x * b.scale - b.ox) / b.k
        let py = (point.y * b.scale - b.oy) / b.k
        guard px >= 0, py >= 0, px < b.w, py < b.h else { return nil }
        return (Int(px), Int(py))
    }

    /// Inclusive pixel box → view rectangle in points.
    private func viewRect(x0: UInt16, y0: UInt16, x1: UInt16, y1: UInt16) -> CGRect? {
        guard let b = blit() else { return nil }
        let left = (CGFloat(x0) * b.k + b.ox) / b.scale
        let top = (CGFloat(y0) * b.k + b.oy) / b.scale
        let right = (CGFloat(Int(x1) + 1) * b.k + b.ox) / b.scale
        let bottom = (CGFloat(Int(y1) + 1) * b.k + b.oy) / b.scale
        return CGRect(x: left, y: top, width: right - left, height: bottom - top)
    }

    /// Map a view touch to a visible physics cell (origin bottom-left).
    private func physicsCell(at point: CGPoint) -> (UInt16, UInt16)? {
        guard let b = blit(), let px = pixel(at: point) else { return nil }
        let cellX = UInt16(px.x / 3)
        let cellYFromTop = UInt16(px.y / 3)
        let cellsH = UInt16(Int(b.h) / 3)
        let cellY = cellsH > 0 ? cellsH - 1 - cellYFromTop : 0
        return (cellX, cellY)
    }

    /// Hold finger → flashlight cone (×2 light, no heat). Off while feeding.
    private func flashlightCell() -> (Int32, Int32, Int32) {
        guard let point = magnifierPoint, let cell = physicsCell(at: point) else {
            return (0, 0, 0)
        }
        return (Int32(cell.0), Int32(cell.1), 8)
    }

    private func showHint(_ text: String) {
        hint = text
        hintToken += 1
        let token = hintToken
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.4) { [weak self] in
            guard let self, self.hintToken == token else { return }
            self.hint = nil
        }
    }

    /// Drain the sim event queue into toasts, log, audio, haptics.
    private func drainEvents(_ handle: OpaquePointer) {
        var pause = false
        let tick = tank_tick_count(handle)
        while true {
            let ev = tank_poll_event(handle)
            if ev.kind == 0 { break }
            if let msg = EventToast.message(kind: ev.kind) {
                showHint(msg)
                eventLogSerial &+= 1
                let entry = EventLogEntry(id: eventLogSerial, tick: tick, kind: ev.kind, message: msg)
                eventLog.insert(entry, at: 0)
                if eventLog.count > 200 { eventLog.removeLast(eventLog.count - 200) }
                if let tip = GuideTip.contextual(kind: ev.kind), showStartGuide == false {
                    // Soft tip after first milestones once the guide is done.
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
                        self?.showHint(tip)
                    }
                }
            }
            if let sting = EventToast.soundTrigger(kind: ev.kind) {
                AmbientAudio.shared.playSting(sting)
            }
            if hapticsOn, EventToast.shouldPause(kind: ev.kind) {
                haptic.impactOccurred()
            }
            if pauseOnEvent && EventToast.shouldPause(kind: ev.kind) {
                pause = true
            }
        }
        if pause {
            running = false
            playing = false
            showHint(hint.map { "\($0) · PAUSED" } ?? "PAUSED")
        }
    }

    // MARK: frame

    private func advance(_ dt: CFTimeInterval) {
        guard let handle else { return }
        var steps = 0
        if running {
        accumulator += dt
        while accumulator >= step && steps < 4 {
            let tools = pendingTools
            pendingTools.removeAll(keepingCapacity: true)
            let flash = tool.isFeed ? (0, 0, 0) : flashlightCell()
            tools.withUnsafeBufferPointer { buf in
                var input = TankInput(
                    grav_x: lastGravX,
                    grav_y: lastGravY,
                    paused: 0,
                    always_day: alwaysDay ? 1 : 0,
                    swirl: effectiveSwirl ? 1 : 0,
                    _pad1: 0,
                    light_slider: Float(lightSlider),
                    t_base: Float(tBase),
                    flash_x: flash.0,
                    flash_y: flash.1,
                    flash_r: flash.2,
                    tools: buf.baseAddress,
                    tool_count: UInt32(buf.count)
                )
                tank_tick(handle, &input)
            }
            drainEvents(handle)
            accumulator -= step
            steps += 1
        }
        if steps == 4 {
            accumulator = 0
        }
        }
        // Enclosure release fills a pending scoop bucket.
        if carried == nil, let bucket = tank_take_enclosure_bucket(handle) {
            carried = bucket
            carriedCells = Int(tank_carried_cells(bucket))
            showHint("BOX FREED")
        }
        frame &+= 1
        var w: UInt32 = 0
        var h: UInt32 = 0
        if let pixels = tank_pixels(handle, &w, &h) {
            metal?.present(pixels: pixels, width: Int(w), height: Int(h))
        }
        if steps > 0 || (!running && (tool == .needle || tool == .look)) {
            if tool == .needle { refreshInspect() } else if steps > 0 { refreshInspect() }
            if frame % 30 == 0 {
                liveCells = tank_live_cells(handle)
                AmbientAudio.shared.setIntensity(liveCells: liveCells)
            }
            temperatureC = tank_temperature(handle)
        }
        let tick = tank_tick_count(handle)
        let day = Int(tick / 72_000) + 1
        if day != dayNumber {
            dayNumber = day
        }
        let sun = tank_is_day(handle) != 0
        if sun != isDay {
            isDay = sun
        }
    }
}

struct MagnifierState {
    var center: CGPoint
    var pinned: Bool
    /// Glass radius in view points.
    var radius: CGFloat
}

private final class DisplayPump: NSObject {
    var onFrame: ((CFTimeInterval) -> Void)?

    @objc func step(_ link: CADisplayLink) {
        onFrame?(link.duration)
    }
}
