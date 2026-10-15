import Foundation
import QuartzCore
import UIKit

/// Owns the tank handle, the 30 Hz step, and save/resume.
/// Backgrounding saves and drops any leftover frame time so the sim
/// does not catch up the wall clock.
final class TankSession: ObservableObject {
    @Published private(set) var isDay = true
    @Published private(set) var consent = false

    private var handle: OpaquePointer?
    private var link: CADisplayLink?
    private var pump: DisplayPump?
    private weak var metal: TankMetalView?
    private var accumulator: CFTimeInterval = 0
    private var running = false
    private let step = 1.0 / 30.0

    private var saveURL: URL {
        let dir = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        return dir.appendingPathComponent("tank.postcard")
    }

    init() {
        TelemetryClient.installCrashHook()
        if let data = try? Data(contentsOf: saveURL), !data.isEmpty {
            handle = data.withUnsafeBytes { raw in
                guard let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self) else {
                    return nil
                }
                return tank_load(base, raw.count)
            }
        }
        if handle == nil {
            handle = tank_preset(1)
        }
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
        tank_free(handle)
    }

    func attach(view: TankMetalView) {
        metal = view
    }

    func resume() {
        accumulator = 0
        running = true
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
        save()
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

    private func advance(_ dt: CFTimeInterval) {
        guard running, let handle else { return }
        accumulator += dt
        var steps = 0
        while accumulator >= step && steps < 4 {
            var input = TankInput(
                grav_x: 0,
                grav_y: -1,
                paused: 0,
                always_day: 0,
                _pad0: 0,
                _pad1: 0,
                light_slider: 1,
                t_base: 15,
                tools: nil,
                tool_count: 0
            )
            tank_tick(handle, &input)
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

    private func save() {
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
        try? data.write(to: saveURL, options: .atomic)
    }
}

private final class DisplayPump: NSObject {
    var onFrame: ((CFTimeInterval) -> Void)?

    @objc func step(_ link: CADisplayLink) {
        onFrame?(link.duration)
    }
}
