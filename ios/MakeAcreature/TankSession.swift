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
struct ChartPoint {
    var creatures: UInt32
    var lineages: UInt32
    var predation: UInt32
    var photo: UInt32
    var water: UInt32
    var sand: UInt32
    var stone: UInt32
}

struct SpeciesNode: Identifiable {
    let rec: SpeciesNodeRec
    let name: String
    var id: UInt16 { rec.id }
    var parent: UInt16? { rec.parent == UInt16(TANK_NO_SPECIES) ? nil : rec.parent }
}

/// The opening tumbler, the big tank, or one of the three shelf vials.
enum TankContainer: Equatable {
    case glass
    case main
    case sample(Int)

    var sampleIndex: Int? {
        if case .sample(let index) = self { return index }
        return nil
    }

    var key: String {
        switch self {
        case .glass: return "glass"
        case .main: return "main"
        case .sample(let index): return "sample-\(index)"
        }
    }

    static func stored() -> TankContainer {
        let raw = UserDefaults.standard.string(forKey: "openContainer") ?? "glass"
        if raw == "glass" { return .glass }
        if raw == "main" { return .main }
        if raw.hasPrefix("sample-"), let n = Int(raw.dropFirst("sample-".count)), (0..<3).contains(n) {
            return .sample(n)
        }
        return .glass
    }
}

/// Owns the tank handle, the 30 Hz step, tools, and save/resume.
/// Backgrounding saves and drops leftover frame time so the sim does
/// not catch up the wall clock.
/// One cassette on the lobby shelf. `voice` picks the generated tape.
struct ShelfTape: Identifiable, Codable, Equatable {
    var id: UUID
    var name: String
    var voice: Int
    var duration: Double
    var take: AmbientAudio.Take? = nil
}

final class TankSession: ObservableObject {
    @Published private(set) var isDay = true
    /// Day 1 covers that day's night as well. The counter advances at dawn.
    @Published private(set) var dayNumber = 1
    @Published private(set) var playing = true
    @Published private(set) var consent = false
    @Published private(set) var temperatureC: Float = 15
    @Published var tool: BuildTool = .none {
        didSet {
            if tool != .needle {
                selectionRect = nil
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
    /// A pinch wider than the whole glass. The shell returns to the lobby.
    @Published var leaveForLobby = false
    /// Pinch scale while looking into the tumbler. 1 fits the glass on the table.
    @Published var glassZoom: CGFloat = 1
    @Published var glassPan: CGSize = .zero
    private var glassPinchZoom: CGFloat = 1
    private var glassPinchLocal = CGPoint.zero
    /// Selection box above the finger. The creature inside it is the one selected.
    @Published var selectionRect: CGRect?
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
    /// Bumps when a tank file is replaced, so the lobby glass can refresh.
    @Published private(set) var savesChanged = 0
    /// Lobby script index. 0 = opening tumbler. 1 = first jar after something moved.
    @Published private(set) var lobbyBeat = 0

    /// What the study shows for the unlocked beat.
    var roomCast: RoomCast { LobbyScript.cast(beat: lobbyBeat) }

    /// The room crop for the open desk glass (tumbler or first jar).
    var deskView: RoomArt.Spot {
        switch container {
        case .sample(0) where lobbyBeat >= 1: return RoomArt.jarView
        case .glass: return RoomArt.glassView
        default: return RoomArt.glassView
        }
    }

    /// True when the open container is drawn into a hole in the study.
    var deskGlass: Bool {
        switch container {
        case .glass: return true
        case .sample(0) where lobbyBeat >= 1: return true
        default: return false
        }
    }
    /// Samples for the filing-cabinet charts, oldest first.
    @Published private(set) var chart: [ChartPoint] = []
    /// Cassette in the deck. Index into `shelf`, left to right.
    @Published private(set) var tape = 0
    /// Red while off, green while the current tape is playing.
    @Published private(set) var deckOn = false
    @Published private(set) var shelf: [ShelfTape] = []

    var tapePlaying: Bool { deckOn && soundOn }
    private var soloing = false
    private var predationCount: UInt32 = 0

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
    private var navigating = false
    private var reduceMotionObserver: NSObjectProtocol?

    private var docs: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    }

    private func saveURL(for container: TankContainer) -> URL {
        switch container {
        case .glass:
            return docs.appendingPathComponent("glass.postcard")
        case .main:
            return docs.appendingPathComponent("tank-0.postcard")
        case .sample(let index):
            return docs.appendingPathComponent("sample-\(index).postcard")
        }
    }

    init() {
        TelemetryClient.installCrashHook()
        lobbyBeat = UserDefaults.standard.integer(forKey: "lobbyBeat")
        container = TankContainer.stored()
        // Opening beat runs the tumbler in the background so locomotion can fire.
        if lobbyBeat == 0, container == .main {
            container = .glass
            UserDefaults.standard.set(TankContainer.glass.key, forKey: "openContainer")
        }
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
        favouriteId = Self.storedFavourite()
        showStartGuide = !UserDefaults.standard.bool(forKey: "startGuideDone")
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
        loadShelf()
        deckOn = UserDefaults.standard.object(forKey: "deckOn") as? Bool ?? soundOn
        tape = min(max(0, shelf.count - 1), max(0, UserDefaults.standard.integer(forKey: "tape")))
        pushDeck(resetHead: false)
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
        view.onNavigate = { [weak self] nav in
            self?.handleNav(nav)
        }
    }

    func resume() {
        accumulator = 0
        running = true
        playing = true
        startMotion()
        if soloing {
            AmbientAudio.shared.setMuted(false)
            AmbientAudio.shared.ensureRunning()
        } else {
            pushDeck(resetHead: false)
        }
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

    /// The opening tumbler. A small glass of water, not the wide tank.
    func openGlass() {
        glassZoom = 1
        glassPan = .zero
        switchTo(.glass)
    }

    func openSample(_ index: Int) {
        glassZoom = 1
        glassPan = .zero
        switchTo(.sample(max(0, min(2, index))))
    }

    /// Beat 2: something moved. One jar on the shelf, system cue.
    func unlockMovedBeat() {
        guard lobbyBeat < 1 else { return }
        lobbyBeat = 1
        UserDefaults.standard.set(1, forKey: "lobbyBeat")
        if let jar = tank_jar(UInt64(Date().timeIntervalSince1970) &+ 17) {
            write(handle: jar, to: .sample(0))
            tank_free(jar)
        }
        savesChanged &+= 1
        SystemCue.play(LobbyScript.beats[1].sound)
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
        handle = freshGlass(for: container)
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
        guard let fresh = tank_jar(UInt64(Date().timeIntervalSince1970)) else { return }
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
        UserDefaults.standard.removeObject(forKey: "favouriteId")
        UserDefaults.standard.set(false, forKey: "startGuideDone")
        UserDefaults.standard.set(0, forKey: "lobbyBeat")
        lobbyBeat = 0
        AmbientAudio.shared.setMuted(false)
        let targets: [TankContainer] = [.glass, .main, .sample(0), .sample(1), .sample(2)]
        for (i, target) in targets.enumerated() {
            guard let fresh = freshGlass(for: target, salt: UInt64(i)) else { continue }
            write(handle: fresh, to: target)
            tank_free(fresh)
        }
        container = .glass
        UserDefaults.standard.set(TankContainer.glass.key, forKey: "openContainer")
        loadContainer()
        showStartGuide = true
        showHint("RESET")
    }

    /// Replace one glass with a fresh half-full tank seeded from the archive.
    /// The open tank is swapped in place. A shelf jar that is not open is
    /// only rewritten on disk.
    @discardableResult
    func newTank(from specimen: ArchivedSpecimen, into target: TankContainer, copies: UInt32 = 3) -> UInt32 {
        guard let fresh = freshGlass(for: target) else {
            showHint("NO ROOM")
            return 0
        }
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
        if target == container {
            dropBucket()
            inspected = nil
            tank_free(handle)
            handle = fresh
            pendingTools.removeAll()
            saveCurrent()
            refreshClock()
        } else {
            write(handle: fresh, to: target)
            tank_free(fresh)
        }
        savesChanged += 1
        showHint(placed > 0 ? "\(placed) PLACED" : "NO ROOM")
        return placed
    }

    /// Drop one copy into a glass that already has a world. The open tank
    /// is seeded in place. A closed jar is loaded, seeded, and written back.
    @discardableResult
    func addSpecimen(_ specimen: ArchivedSpecimen, into target: TankContainer) -> UInt32 {
        let placed: UInt32
        if target == container, let handle {
            placed = specimen.bytes.withUnsafeBytes { raw -> UInt32 in
                guard let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return 0 }
                return tank_seed_specimen_copies(handle, base, raw.count, 1)
            }
            saveCurrent()
            refreshClock()
        } else {
            let loaded = loadHandle(for: target)
            guard let loaded else {
                showHint("NO ROOM")
                return 0
            }
            if case .main = target {
                tank_widen(loaded, tankMainCellsWide)
            }
            placed = specimen.bytes.withUnsafeBytes { raw -> UInt32 in
                guard let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return 0 }
                return tank_seed_specimen_copies(loaded, base, raw.count, 1)
            }
            write(handle: loaded, to: target)
            tank_free(loaded)
        }
        savesChanged += 1
        showHint(placed > 0 ? "ADDED" : "NO ROOM")
        return placed
    }

    func setTape(_ index: Int) {
        guard shelf.indices.contains(index) else { return }
        tape = index
        UserDefaults.standard.set(tape, forKey: "tape")
        pushDeck(resetHead: true)
        showHint(shelf[tape].name)
    }

    func toggleDeck() {
        deckOn.toggle()
        UserDefaults.standard.set(deckOn, forKey: "deckOn")
        pushDeck(resetHead: false)
    }

    func renameTape(_ id: UUID, to name: String) {
        guard let i = shelf.firstIndex(where: { $0.id == id }) else { return }
        let up = name.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        guard !up.isEmpty else { return }
        shelf[i].name = String(up.prefix(12))
        saveShelf()
    }

    func moveTape(from: Int, to: Int) {
        guard shelf.indices.contains(from) else { return }
        let dest = min(max(0, to), shelf.count - 1)
        guard from != dest else { return }
        let item = shelf.remove(at: from)
        shelf.insert(item, at: dest)
        if tape == from {
            tape = dest
        } else if from < tape && dest >= tape {
            tape -= 1
        } else if from > tape && dest <= tape {
            tape += 1
        }
        UserDefaults.standard.set(tape, forKey: "tape")
        saveShelf()
        pushDeck(resetHead: false)
    }

    /// The playlist advanced on its own. Keep the shelf mark in step.
    func followPlaylist() {
        guard !soloing else { return }
        let i = AmbientAudio.shared.playingSlot
        guard shelf.indices.contains(i), i != tape else { return }
        tape = i
        UserDefaults.standard.set(tape, forKey: "tape")
    }

    /// Seat a shelf tape in the desk player and preview it (playlist head).
    func previewTape(id: UUID) {
        guard let index = shelf.firstIndex(where: { $0.id == id }) else { return }
        endSolo()
        tape = index
        deckOn = true
        UserDefaults.standard.set(tape, forKey: "tape")
        UserDefaults.standard.set(true, forKey: "deckOn")
        pushDeck(resetHead: true)
        showHint(shelf[index].name)
    }

    /// Open the eurorack: kill the ambient playlist, only the rack voice.
    func beginRackSolo(_ take: AmbientAudio.Take) {
        soloing = true
        let slot = AmbientAudio.PlaySlot(voice: -1, duration: 3600, take: take)
        AmbientAudio.shared.setDeck(on: true, slots: [slot], start: 0, resetHead: true)
        AmbientAudio.shared.setMuted(false)
        AmbientAudio.shared.ensureRunning()
    }

    /// Update the rack voice while patching. Playlist stays cut.
    func soloTake(_ take: AmbientAudio.Take) {
        soloing = true
        let slot = AmbientAudio.PlaySlot(voice: -1, duration: 3600, take: take)
        AmbientAudio.shared.setDeck(on: true, slots: [slot], start: 0, resetHead: false)
        AmbientAudio.shared.setMuted(false)
        AmbientAudio.shared.ensureRunning()
    }

    /// Leave the solo voice and restore the shelf playlist.
    func endSolo() {
        guard soloing else { return }
        soloing = false
        pushDeck(resetHead: true)
    }

    /// The blank cassette leaves the player and joins the right end of the shelf.
    func keepTake(_ take: AmbientAudio.Take, duration: Double, name: String? = nil) {
        let n = shelf.filter { $0.voice < 0 }.count + 1
        let label: String = {
            if let name {
                let up = name.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
                if !up.isEmpty { return String(up.prefix(12)) }
            }
            return "TAKE \(n)"
        }()
        shelf.append(
            ShelfTape(
                id: UUID(),
                name: label,
                voice: -1,
                duration: duration,
                take: take
            )
        )
        tape = shelf.count - 1
        deckOn = true
        UserDefaults.standard.set(true, forKey: "deckOn")
        UserDefaults.standard.set(tape, forKey: "tape")
        soloing = false
        saveShelf()
        pushDeck(resetHead: true)
        showHint(label)
    }

    private func loadShelf() {
        if let data = UserDefaults.standard.data(forKey: "lobbyShelf"),
           let saved = try? JSONDecoder().decode([ShelfTape].self, from: data),
           !saved.isEmpty {
            shelf = saved
            return
        }
        shelf = Self.freshShelf()
        saveShelf()
    }

    private func saveShelf() {
        if let data = try? JSONEncoder().encode(shelf) {
            UserDefaults.standard.set(data, forKey: "lobbyShelf")
        }
    }

    private func pushDeck(resetHead: Bool) {
        // While the eurorack page is auditioning, leave its solo voice alone.
        if soloing { return }
        let slots = shelf.map {
            AmbientAudio.PlaySlot(voice: $0.voice, duration: $0.duration, take: $0.take)
        }
        let on = deckOn && soundOn
        AmbientAudio.shared.setDeck(on: on, slots: slots, start: tape, resetHead: resetHead)
        AmbientAudio.shared.setMuted(!on)
        if on { AmbientAudio.shared.ensureRunning() }
    }

    private static func freshShelf() -> [ShelfTape] {
        let names = AmbientAudio.tapeNames
        let durations = AmbientAudio.tapeDurations
        return names.indices.map { i in
            ShelfTape(
                id: UUID(),
                name: names[i],
                voice: i,
                duration: i < durations.count ? durations[i] : 45
            )
        }
    }

    private func loadHandle(for target: TankContainer) -> OpaquePointer? {
        let url = saveURL(for: target)
        if let data = try? Data(contentsOf: url), !data.isEmpty {
            let loaded = data.withUnsafeBytes { raw -> OpaquePointer? in
                guard let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return nil }
                return tank_load(base, raw.count)
            }
            if loaded != nil { return loaded }
        }
        return freshGlass(for: target, salt: 9)
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
        pushDeck(resetHead: false)
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
        UserDefaults.standard.set(String(favouriteId), forKey: "favouriteId")
    }

    /// Specimen ids are full 64-bit hashes. `Int` truncates by trapping.
    private static func storedFavourite() -> UInt64 {
        let defaults = UserDefaults.standard
        if let raw = defaults.string(forKey: "favouriteId"), let id = UInt64(raw) {
            return id
        }
        let legacy = defaults.integer(forKey: "favouriteId")
        return legacy > 0 ? UInt64(legacy) : 0
    }

    func clearFavourite() {
        favouriteId = 0
        UserDefaults.standard.removeObject(forKey: "favouriteId")
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

    /// Tiny nearest-neighbour picture of a tank, for the lobby glass.
    /// The open tank is read live. The others are loaded from their saves
    /// and dropped again, so the running tank is left alone.
    func lobbyPreview(_ container: TankContainer, width: Int, height: Int) -> UIImage? {
        let dw = max(1, width)
        let dh = max(1, height)
        if container == self.container {
            return downscale(handle, width: dw, height: dh)
        }
        let url = saveURL(for: container)
        guard let data = try? Data(contentsOf: url), !data.isEmpty else { return nil }
        let loaded: OpaquePointer? = data.withUnsafeBytes { raw in
            guard let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return nil }
            return tank_load(base, raw.count)
        }
        guard let loaded else { return nil }
        defer { tank_free(loaded) }
        return downscale(loaded, width: dw, height: dh)
    }

    private func downscale(_ handle: OpaquePointer?, width: Int, height: Int) -> UIImage? {
        guard let handle else { return nil }
        var sw: UInt32 = 0
        var sh: UInt32 = 0
        guard let ptr = tank_pixels(handle, &sw, &sh), sw > 0, sh > 0 else { return nil }
        let srcW = Int(sw)
        let srcH = Int(sh)
        var rgba = [UInt8](repeating: 0, count: width * height * 4)
        for y in 0..<height {
            let sy = min(srcH - 1, y * srcH / height)
            for x in 0..<width {
                let sx = min(srcW - 1, x * srcW / width)
                let s = (sy * srcW + sx) * 4
                let d = (y * width + x) * 4
                rgba[d] = ptr[s]
                rgba[d + 1] = ptr[s + 1]
                rgba[d + 2] = ptr[s + 2]
                rgba[d + 3] = 255
            }
        }
        return rgba.withUnsafeBytes { raw in
            rgbaImage(
                ptr: raw.baseAddress!.assumingMemoryBound(to: UInt8.self),
                width: width,
                height: height,
                scale: 1
            )
        }
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
        guard tank_org_bbox(handle, inspected.org, &x0u, &y0u, &x1u, &y1u) != 0, x1u >= x0u, y1u >= y0u else {
            return tankSnapshot(scale: scale)
        }
        let pad = 4
        let x0 = max(0, Int(x0u) - pad)
        let y0 = max(0, Int(y0u) - pad)
        // x1u/y1u are inclusive. Keep a full margin past that edge.
        let x1 = min(pw, Int(x1u) + 1 + pad)
        let y1 = min(ph, Int(y1u) + 1 + pad)
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

    func speciesCard(_ species: UInt16) -> String {
        guard let handle, species != UInt16(TANK_NO_SPECIES) else { return "" }
        let need = tank_species_card(handle, species, nil, 0)
        var buf = [UInt8](repeating: 0, count: Int(need) + 1)
        _ = tank_species_card(handle, species, &buf, UInt32(buf.count))
        return String(decoding: buf.prefix(Int(need)), as: UTF8.self)
    }

    /// Grows the founder once. Call on a tap, not every frame.
    func speciesPortrait(_ species: UInt16) -> UIImage? {
        guard let handle, species != UInt16(TANK_NO_SPECIES) else { return nil }
        var w: UInt16 = 0
        var h: UInt16 = 0
        let need = tank_species_portrait(handle, species, &w, &h, nil, 0)
        guard need > 0, w > 0, h > 0 else { return nil }
        var rgba = [UInt8](repeating: 0, count: need)
        let got = tank_species_portrait(handle, species, &w, &h, &rgba, need)
        guard got == need else { return nil }
        return rgba.withUnsafeBytes { raw in
            rgbaImage(
                ptr: raw.baseAddress!.assumingMemoryBound(to: UInt8.self),
                width: Int(w),
                height: Int(h),
                scale: 4
            )
        }
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
            case .glass:
                handle = tank_glass(1)
            case .main:
                handle = tank_main_preset(1)
            case .sample(let index):
                handle = tank_jar(UInt64(index + 11))
            }
        }
        if case .main = container, let handle {
            tank_widen(handle, tankMainCellsWide)
            if !UserDefaults.standard.bool(forKey: "wideTankHint") {
                UserDefaults.standard.set(true, forKey: "wideTankHint")
                showHint("TWO FINGERS SLIDE")
            }
        }
        refreshClock()
    }

    /// The main glass is three screens wide. A jar stays the small preset.
    private func freshGlass(for target: TankContainer, salt: UInt64 = 0) -> OpaquePointer? {
        let seed = UInt64(Date().timeIntervalSince1970) &+ salt
        switch target {
        case .glass:
            return tank_glass(seed)
        case .main:
            return tank_main_empty(seed)
        case .sample:
            return tank_jar(seed)
        }
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

    /// Last point of a one-finger slide, while no brush is in hand.
    private var slidePoint: CGPoint?
    /// One pinch-out past the widest view asks to leave, once.
    private var askedToLeave = false

    private func handleTouch(point: CGPoint?, ended: Bool) {
        if navigating || point == nil {
            slidePoint = nil
            return
        }
        guard let point else { return }
        switch tool {
        case .none:
            if ended {
                slidePoint = nil
                return
            }
            if let prev = slidePoint {
                let delta = CGPoint(x: point.x - prev.x, y: point.y - prev.y)
                if metal?.waterHole != nil {
                    nudgeGlass(by: delta)
                } else {
                    slide(by: delta)
                }
            }
            slidePoint = point
        case .needle:
            slidePoint = nil
            let box = selectionBox(at: point)
            selectionRect = box
            inspect(in: box, ended: ended)
            if ended { selectionRect = nil }
        case .sample:
            slidePoint = nil
            if ended {
                if carried == nil { scoop(at: point) } else { release(at: point) }
            }
        default:
            slidePoint = nil
            queueStroke(at: point, ended: ended)
        }
    }

    /// Two fingers slide the window. A pinch zooms about the fingers.
    private func handleNav(_ nav: TankNav) {
        guard let metal else { return }
        switch nav {
        case .pinch(let scale, let anchor, let state):
            if metal.waterHole != nil {
                pinchGlassScene(scale: scale, anchor: anchor, state: state)
                return
            }
            if state == .began {
                navigating = true
                slidePoint = nil
                askedToLeave = false
                metal.camera.pinchStartW = metal.camera.visibleW
            }
            let worldW = max(metal.camera.worldW, 1)
            let worldH = max(metal.camera.worldH, 1)
            let rawW = metal.camera.pinchStartW / max(scale, 0.05)
            let homeW = max(metal.camera.homeW, worldW)
            if rawW > homeW * 1.08 {
                if !askedToLeave {
                    askedToLeave = true
                    leaveForLobby = true
                }
                if state == .ended || state == .cancelled || state == .failed {
                    navigating = false
                }
                return
            }
            let before = pixelFloat(at: anchor)
            let home = max(metal.camera.homeW, worldW)
            let visW = min(max(metal.camera.pinchStartW / max(scale, 0.05), tankMinVisible), home)
            let viewScale = metal.contentScale
            let drawableW = max(metal.bounds.width * viewScale, 1)
            let drawableH = max(metal.bounds.height * viewScale, 1)
            let aspect = drawableW / drawableH
            let visH = visW / max(aspect, 0.01)
            let shownW = min(visW, worldW)
            let shownH = min(visH, worldH)
            let zoom = drawableW / max(visW, 1)
            let ox = (drawableW - shownW * zoom) * 0.5
            let oy: CGFloat
            if metal.camera.centerInWall && shownH >= worldH - 0.5 {
                oy = (drawableH - shownH * zoom) * 0.5
            } else {
                oy = drawableH - shownH * zoom
            }
            metal.camera.visibleW = visW
            if let before {
                let dx = anchor.x * viewScale
                let dy = anchor.y * viewScale
                metal.camera.left = before.x - (dx - ox) / zoom
                metal.camera.top = before.y - (dy - oy) / zoom
            }
            _ = viewFrame()
            if state == .ended || state == .cancelled || state == .failed {
                navigating = false
            }
        case .pan(let delta, let state):
            if state == .began {
                navigating = true
                slidePoint = nil
            }
            if metal.waterHole != nil {
                nudgeGlass(by: delta)
            } else {
                slide(by: delta)
            }
            if state == .ended || state == .cancelled || state == .failed {
                navigating = false
            }
        }
    }

    /// Pinch scales the study. The tumbler water is part of that picture.
    private func pinchGlassScene(scale: CGFloat, anchor: CGPoint, state: UIGestureRecognizer.State) {
        guard let metal else { return }
        if state == .began {
            navigating = true
            slidePoint = nil
            askedToLeave = false
            glassPinchZoom = glassZoom
            let center = CGPoint(x: metal.bounds.midX, y: metal.bounds.midY)
            glassPinchLocal = CGPoint(
                x: (anchor.x - center.x - glassPan.width) / max(glassZoom, 0.01),
                y: (anchor.y - center.y - glassPan.height) / max(glassZoom, 0.01)
            )
        }
        let raw = glassPinchZoom * scale
        if raw < 0.92 {
            if !askedToLeave {
                askedToLeave = true
                leaveForLobby = true
            }
        } else {
            let next = min(max(raw, 1), 8)
            let center = CGPoint(x: metal.bounds.midX, y: metal.bounds.midY)
            let pan = CGSize(
                width: anchor.x - center.x - glassPinchLocal.x * next,
                height: anchor.y - center.y - glassPinchLocal.y * next
            )
            glassZoom = next
            glassPan = clampedGlassPan(pan, zoom: next, view: metal.bounds.size)
        }
        if state == .ended || state == .cancelled || state == .failed {
            navigating = false
        }
    }

    /// Drag the zoomed study. At the fitted zoom there is nowhere to drag.
    private func nudgeGlass(by delta: CGPoint) {
        guard let metal, glassZoom > 1 else { return }
        glassPan = clampedGlassPan(
            CGSize(width: glassPan.width + delta.x, height: glassPan.height + delta.y),
            zoom: glassZoom,
            view: metal.bounds.size
        )
    }

    private func clampedGlassPan(_ pan: CGSize, zoom: CGFloat, view: CGSize) -> CGSize {
        let seen = deskView
        let fit = min(view.width / CGFloat(seen.w), view.height / CGFloat(seen.h))
        let w = CGFloat(seen.w) * fit * zoom
        let h = CGFloat(seen.h) * fit * zoom
        let slackX = max(0, (w - view.width) / 2)
        let slackY = max(0, (h - view.height) / 2)
        return CGSize(
            width: min(max(pan.width, -slackX), slackX),
            height: min(max(pan.height, -slackY), slackY)
        )
    }

    /// Move the window by a drag measured in view points.
    private func slide(by delta: CGPoint) {
        guard let metal, let f = viewFrame() else { return }
        let tankPerPoint = f.viewScale / max(f.scale, 0.001)
        metal.camera.left -= delta.x * tankPerPoint
        metal.camera.top -= delta.y * tankPerPoint
        _ = viewFrame()
    }

    /// Box above the finger. The gap keeps the selection off the fingertip.
    private func selectionBox(at touch: CGPoint) -> CGRect {
        let side = CGFloat(34 + max(brush, 1) * 4)
        return CGRect(x: touch.x - side / 2, y: touch.y - side - 28, width: side, height: side)
    }

    /// Any living cell inside the box, centre first. Works while the sim is paused.
    private func inspect(in rect: CGRect, ended: Bool) {
        guard let handle, let f = viewFrame() else { return }
        let center = CGPoint(x: rect.midX, y: rect.midY)
        func pix(_ p: CGPoint) -> (Int, Int) {
            let seen = pixelFloat(at: p) ?? CGPoint(x: -1, y: -1)
            return (Int(seen.x.rounded()), Int(seen.y.rounded()))
        }
        let p0 = pix(CGPoint(x: rect.minX, y: rect.minY))
        let p1 = pix(CGPoint(x: rect.maxX, y: rect.maxY))
        var x0 = max(0, min(p0.0, p1.0))
        var y0 = max(0, min(p0.1, p1.1))
        var x1 = min(Int(f.worldW) - 1, max(p0.0, p1.0))
        var y1 = min(Int(f.worldH) - 1, max(p0.1, p1.1))
        let mid = pixelFloat(at: center) ?? center
        let cap = 48
        x0 = max(x0, Int(mid.x.rounded()) - cap)
        y0 = max(y0, Int(mid.y.rounded()) - cap)
        x1 = min(x1, Int(mid.x.rounded()) + cap)
        y1 = min(y1, Int(mid.y.rounded()) + cap)
        guard x0 <= x1, y0 <= y1 else {
            if ended { inspected = nil }
            return
        }
        let focus = pix(center)
        if let hit = nearestCreature(handle, x0: x0, y0: y0, x1: x1, y1: y1, cx: focus.0, cy: focus.1) {
            apply(hit)
            return
        }
        if ended { inspected = nil }
    }

    /// Every buffer pixel in the box, closest to the centre first.
    private func nearestCreature(
        _ handle: OpaquePointer,
        x0: Int, y0: Int, x1: Int, y1: Int,
        cx: Int, cy: Int
    ) -> CellInspectRec? {
        let reach = max(x1 - x0, y1 - y0) / 2 + 1
        for radius in 0...reach {
            var y = max(y0, cy - radius)
            let yEnd = min(y1, cy + radius)
            while y <= yEnd {
                var x = max(x0, cx - radius)
                let xEnd = min(x1, cx + radius)
                while x <= xEnd {
                    let onRing = radius == 0 || abs(x - cx) == radius || abs(y - cy) == radius
                    if onRing {
                        let rec = tank_inspect_cell(handle, UInt32(x), UInt32(y))
                        if rec.cell != UInt32(TANK_NO_CELL) {
                            return rec
                        }
                    }
                    x += 1
                }
                y += 1
            }
        }
        return nil
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
        let oneDrop = tool == .sugar || tool == .lime || tool == .seed
        if oneDrop && !ended { return }
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
                r: oneDrop ? 0 : UInt16(brush),
                variant: UInt8(variant),
                _pad: 0
            )
        )
    }

    /// One sim step. While paused the stroke is drawn and nothing falls.
    private func submit(_ tools: [ToolEvent], paused: Bool) {
        guard let handle else { return }
        tools.withUnsafeBufferPointer { buf in
            var input = TankInput(
                grav_x: lastGravX,
                grav_y: lastGravY,
                paused: paused ? 1 : 0,
                always_day: (alwaysDay || deskGlass) ? 1 : 0,
                swirl: effectiveSwirl ? 1 : 0,
                _pad1: 0,
                light_slider: Float(lightSlider),
                t_base: Float(tBase),
                flash_x: 0,
                flash_y: 0,
                flash_r: 0,
                tools: buf.baseAddress,
                tool_count: UInt32(buf.count)
            )
            tank_tick(handle, &input)
        }
        drainEvents(handle)
    }

    // MARK: coordinates

    /// The same window the Metal view is drawing.
    private func viewFrame() -> TankFrame? {
        guard let metal, metal.bounds.width > 1, metal.bounds.height > 1, let handle else { return nil }
        var w: UInt32 = 0
        var h: UInt32 = 0
        _ = tank_pixels(handle, &w, &h)
        guard w > 0, h > 0 else { return nil }
        return metal.layoutFrame(worldW: CGFloat(w), worldH: CGFloat(h))
    }

    /// View point → tank pixel (top-left origin, as `tank_pixels`).
    private func pixelFloat(at point: CGPoint) -> CGPoint? {
        guard let f = viewFrame() else { return nil }
        let dx = point.x * f.viewScale
        let dy = point.y * f.viewScale
        let px = f.left + (dx - f.ox) / max(f.scale, 0.001)
        let py = f.top + (dy - f.oy) / max(f.scale, 0.001)
        guard px >= 0, py >= 0, px < f.worldW, py < f.worldH else { return nil }
        guard dx >= f.ox - 0.5, dy >= f.oy - 0.5 else { return nil }
        guard dx <= f.ox + f.shownW * f.scale + 0.5, dy <= f.oy + f.shownH * f.scale + 0.5 else { return nil }
        return CGPoint(x: px, y: py)
    }

    private func pixel(at point: CGPoint) -> (x: Int, y: Int)? {
        guard let p = pixelFloat(at: point) else { return nil }
        return (Int(p.x), Int(p.y))
    }

    /// Inclusive pixel box → view rectangle in points.
    private func viewRect(x0: UInt16, y0: UInt16, x1: UInt16, y1: UInt16) -> CGRect? {
        guard let f = viewFrame() else { return nil }
        func view(_ px: CGFloat, _ py: CGFloat) -> CGPoint {
            CGPoint(
                x: (f.ox + (px - f.left) * f.scale) / f.viewScale,
                y: (f.oy + (py - f.top) * f.scale) / f.viewScale
            )
        }
        let origin = view(CGFloat(x0), CGFloat(y0))
        let far = view(CGFloat(Int(x1) + 1), CGFloat(Int(y1) + 1))
        return CGRect(x: origin.x, y: origin.y, width: far.x - origin.x, height: far.y - origin.y)
    }

    /// Map a view touch to a visible physics cell (origin bottom-left).
    private func physicsCell(at point: CGPoint) -> (UInt16, UInt16)? {
        guard let f = viewFrame(), let px = pixel(at: point) else { return nil }
        let cellX = UInt16(px.x / 3)
        let cellYFromTop = UInt16(px.y / 3)
        let cellsH = UInt16(Int(f.worldH) / 3)
        let cellY = cellsH > 0 ? cellsH - 1 - cellYFromTop : 0
        return (cellX, cellY)
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
            if ev.kind == 16 {
                predationCount &+= 1
            }
            if let sting = EventToast.soundTrigger(kind: ev.kind) {
                AmbientAudio.shared.playSting(sting)
            }
            // Beat 2: locomotion / something moved unlocks the first jar.
            if ev.kind == 43 || ev.kind == 50 || ev.kind == 21 {
                unlockMovedBeat()
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
            submit(tools, paused: false)
            accumulator -= step
            steps += 1
        }
        if steps == 4 {
            accumulator = 0
        }
        } else if !pendingTools.isEmpty {
            let tools = pendingTools
            pendingTools.removeAll(keepingCapacity: true)
            submit(tools, paused: true)
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
        if steps > 0 || (!running && tool == .needle) {
            refreshInspect()
            if frame % 30 == 0 {
                liveCells = tank_live_cells(handle)
                let census = tank_census(handle)
                chart.append(ChartPoint(
                    creatures: census.live,
                    lineages: census.species,
                    predation: predationCount,
                    photo: census.photo,
                    water: census.water,
                    sand: census.sand,
                    stone: census.stone
                ))
                if chart.count > 180 { chart.removeFirst(chart.count - 180) }
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

private final class DisplayPump: NSObject {
    var onFrame: ((CFTimeInterval) -> Void)?

    @objc func step(_ link: CADisplayLink) {
        onFrame?(link.duration)
    }
}
