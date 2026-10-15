import CoreMotion
import Foundation
import QuartzCore
import UIKit

/// Owns the tank handle, the 30 Hz step, tools, slots, and save/resume.
/// Backgrounding saves and drops leftover frame time so the sim does
/// not catch up the wall clock.
final class TankSession: ObservableObject {
    @Published private(set) var isDay = true
    @Published private(set) var consent = false
    @Published var tool: BuildTool = .sand
    @Published var brush = 3
    @Published var variant = 0
    @Published var fillLevel = 0.55
    @Published var slot = 0
    @Published var swirl = false
    @Published var magnifierPinned = false
    @Published var magnifierPoint: CGPoint?
    @Published var cameraTiltDegrees: Double = 0

    private var handle: OpaquePointer?
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

    private var docs: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    }

    private func saveURL(for slot: Int) -> URL {
        docs.appendingPathComponent("tank-\(slot).postcard")
    }

    init() {
        TelemetryClient.installCrashHook()
        slot = UserDefaults.standard.integer(forKey: "activeSlot")
        swirl = UserDefaults.standard.bool(forKey: "swirlEnabled")
        loadSlot(slot)
        consent = UserDefaults.standard.bool(forKey: "telemetryConsent")
        if let handle {
            tank_set_consent(handle, consent ? 1 : 0)
            if UserDefaults.standard.bool(forKey: "telemetryKill") {
                tank_telemetry_kill(handle, 1)
            }
            TelemetryClient.flushPendingCrash(handle: handle, consent: consent)
        }
    }

    deinit {
        link?.invalidate()
        motion.stopDeviceMotionUpdates()
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
        startMotion()
        guard link == nil else { return }
        let pump = DisplayPump()
        pump.onFrame = { [weak self] dt in self?.advance(dt) }
        self.pump = pump
        let link = CADisplayLink(target: pump, selector: #selector(DisplayPump.step(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 60, preferred: 60)
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    func pauseAndSave() {
        running = false
        accumulator = 0
        link?.invalidate()
        link = nil
        pump = nil
        motion.stopDeviceMotionUpdates()
        saveCurrent()
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

    func selectSlot(_ index: Int) {
        saveCurrent()
        slot = max(0, min(2, index))
        UserDefaults.standard.set(slot, forKey: "activeSlot")
        loadSlot(slot)
    }

    func newTank() {
        tank_free(handle)
        handle = tank_empty(UInt64(Date().timeIntervalSince1970))
        pendingTools.removeAll()
        saveCurrent()
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
        var tool = ToolEvent(
            kind: 5,
            material: 0,
            x: 0,
            y: level,
            r: 1,
            variant: 0,
            _pad: 0
        )
        pendingTools.append(tool)
        _ = tool
    }

    private func loadSlot(_ index: Int) {
        tank_free(handle)
        handle = nil
        let url = saveURL(for: index)
        if let data = try? Data(contentsOf: url), !data.isEmpty {
            handle = data.withUnsafeBytes { raw in
                guard let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else {
                    return nil
                }
                return tank_load(base, raw.count)
            }
        }
        if handle == nil {
            handle = index == 0 ? tank_preset(1) : tank_empty(UInt64(index + 1))
        }
    }

    private func saveCurrent() {
        guard let handle else { return }
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
        try? data.write(to: saveURL(for: slot), options: .atomic)
    }

    private func startMotion() {
        guard motion.isDeviceMotionAvailable else { return }
        motion.deviceMotionUpdateInterval = 1.0 / 30.0
        motion.startDeviceMotionUpdates(to: .main) { [weak self] data, _ in
            guard let self, let g = data?.gravity else { return }
            // Portrait: x is left/right tilt.
            self.lastGravX = Float(g.x)
            self.lastGravY = Float(g.y)
            if self.swirl {
                let deg = Double(g.x) * 4.0
                self.cameraTiltDegrees = max(-4, min(4, deg))
                self.metal?.cameraTiltDegrees = self.cameraTiltDegrees
            }
        }
    }

    private func handleTouch(point: CGPoint?, ended: Bool) {
        guard let point else {
            if ended && !magnifierPinned {
                magnifierPoint = nil
                metal?.magnifier = nil
            }
            return
        }
        if tool == .fill {
            // Vertical position sets the fill level.
            if let metal {
                let frac = 1.0 - Double(point.y / max(metal.bounds.height, 1))
                fillLevel = max(0, min(1, frac))
                applyFillSlider()
            }
            return
        }
        magnifierPoint = point
        metal?.magnifier = MagnifierState(center: point, pinned: magnifierPinned)
        queueStroke(at: point)
        if ended && !magnifierPinned {
            // Keep the last stroke; clear the lens after a short beat.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { [weak self] in
                guard let self, !self.magnifierPinned else { return }
                self.magnifierPoint = nil
                self.metal?.magnifier = nil
            }
        }
    }

    private func queueStroke(at point: CGPoint) {
        guard let metal, let handle else { return }
        var w: UInt32 = 0
        var h: UInt32 = 0
        _ = tank_pixels(handle, &w, &h)
        guard w > 0, h > 0 else { return }
        let scale = metal.contentScale
        let drawableW = metal.bounds.width * scale
        let drawableH = metal.bounds.height * scale
        let k = max(1, floor(min(drawableW / CGFloat(w), drawableH / CGFloat(h))))
        let tw = CGFloat(w) * k
        let th = CGFloat(h) * k
        let ox = (drawableW - tw) * 0.5
        let oy = (drawableH - th) * 0.5
        let px = (point.x * scale - ox) / k
        let py = (point.y * scale - oy) / k
        guard px >= 0, py >= 0, px < CGFloat(w), py < CGFloat(h) else { return }
        let cellX = UInt16(px / 3)
        let cellYFromTop = UInt16(py / 3)
        let cellsH = UInt16(h / 3)
        let cellY = cellsH > 0 ? cellsH - 1 - cellYFromTop : 0
        pendingTools.append(
            ToolEvent(
                kind: tool.kind,
                material: tool.material,
                x: cellX,
                y: cellY,
                r: UInt16(brush),
                variant: UInt8(variant),
                _pad: 0
            )
        )
    }

    private func advance(_ dt: CFTimeInterval) {
        guard running, let handle else { return }
        accumulator += dt
        var steps = 0
        while accumulator >= step && steps < 4 {
            var tools = pendingTools
            pendingTools.removeAll(keepingCapacity: true)
            tools.withUnsafeBufferPointer { buf in
                var input = TankInput(
                    grav_x: lastGravX,
                    grav_y: lastGravY,
                    paused: 0,
                    always_day: 0,
                    swirl: swirl ? 1 : 0,
                    _pad1: 0,
                    light_slider: 1,
                    t_base: 15,
                    tools: buf.baseAddress,
                    tool_count: UInt32(buf.count)
                )
                tank_tick(handle, &input)
            }
            accumulator -= step
            steps += 1
        }
        if steps == 4 {
            accumulator = 0
        }
        var w: UInt32 = 0
        var h: UInt32 = 0
        if let pixels = tank_pixels(handle, &w, &h) {
            metal?.present(pixels: pixels, width: Int(w), height: Int(h))
        }
        let day = tank_is_day(handle) != 0
        if day != isDay {
            isDay = day
        }
    }
}

struct MagnifierState {
    var center: CGPoint
    var pinned: Bool
}

private final class DisplayPump: NSObject {
    var onFrame: ((CFTimeInterval) -> Void)?

    @objc func step(_ link: CADisplayLink) {
        onFrame?(link.duration)
    }
}
