import AVFoundation
import Foundation

/// Generative room tone. Three tapes — slow, bright, night — of soft
/// sines: a drone, a chord that turns over, and a pentatonic line with
/// long notes and rests. Original material, not a chip tune.
final class AmbientAudio {
    static let shared = AmbientAudio()

    private var engine: AVAudioEngine?
    private var source: AVAudioSourceNode?
    private var muted = false
    private var intensity: Float = 0.15
    private var stingUntil: CFAbsoluteTime = 0
    private var stingFreq: Float = 0
    private var seed: UInt64 = 1
    private var tick: UInt64 = 0
    /// One-pole warmth so a new note does not click.
    private var warm: Float = 0
    /// 0 slow, 1 bright, 2 night.
    private var tape = 0

    func setTape(_ index: Int) {
        tape = min(2, max(0, index))
    }

    private init() {}

    var isMuted: Bool { muted }

    func setMuted(_ on: Bool) {
        muted = on
        if on {
            stop()
        } else {
            startIfNeeded()
        }
    }

    /// Population / activity → drone loudness (0…1).
    func setIntensity(liveCells: UInt32) {
        let n = Float(liveCells)
        intensity = min(0.55, 0.08 + n / 8_000)
    }

    func setTankSeed(_ s: UInt64) {
        seed = s == 0 ? 1 : s
    }

    /// Event sting from `EventToast.soundTrigger`.
    func playSting(_ name: String) {
        guard !muted else { return }
        startIfNeeded()
        switch name {
        case "moved", "locomotion": stingFreq = 220
        case "predation", "escape": stingFreq = 110
        case "eye", "chamber": stingFreq = 330
        default: stingFreq = 165
        }
        stingUntil = CFAbsoluteTimeGetCurrent() + 0.18
    }

    func startIfNeeded() {
        guard !muted, engine == nil else { return }
        let eng = AVAudioEngine()
        let format = AVAudioFormat(standardFormatWithSampleRate: 22_050, channels: 1)!
        let node = AVAudioSourceNode { [weak self] _, _, frameCount, abl in
            guard let self else { return noErr }
            let buf = UnsafeMutableAudioBufferListPointer(abl)
            guard let data = buf[0].mData?.assumingMemoryBound(to: Float.self) else { return noErr }
            let n = Int(frameCount)
            let now = CFAbsoluteTimeGetCurrent()
            let tape = self.tape
            let scale = Self.scale(tape)
            let noteSeconds: Float = tape == 1 ? 3.6 : (tape == 2 ? 9 : 6.5)
            let chordSeconds = noteSeconds * 2
            let rate: Float = 22_050
            for i in 0..<n {
                self.tick &+= 1
                let t = Float(self.tick) / rate
                let sample = self.voice(
                    t: t,
                    tape: tape,
                    scale: scale,
                    noteSeconds: noteSeconds,
                    chordSeconds: chordSeconds,
                    sting: now < self.stingUntil ? self.stingFreq : 0
                )
                self.warm += (sample - self.warm) * 0.18
                data[i] = max(-1, min(1, self.warm))
            }
            return noErr
        }
        eng.attach(node)
        eng.connect(node, to: eng.mainMixerNode, format: format)
        eng.mainMixerNode.outputVolume = 0.7
        do {
            try AVAudioSession.sharedInstance().setCategory(.ambient, mode: .default, options: [.mixWithOthers])
            try AVAudioSession.sharedInstance().setActive(true)
            try eng.start()
            engine = eng
            source = node
        } catch {
            engine = nil
            source = nil
        }
    }

    /// Drone, turning chord, and a slow pentatonic step. Envelopes sit
    /// on zero at each note edge, so a tape change does not click.
    private func voice(
        t: Float,
        tape: Int,
        scale: [Float],
        noteSeconds: Float,
        chordSeconds: Float,
        sting: Float
    ) -> Float {
        let root = scale[0]
        let fifth = scale[min(2, scale.count - 1)]
        let breath = 0.72 + 0.28 * sin(2 * .pi * t / (tape == 2 ? 28 : 16))
        var sample = sin(2 * .pi * root * 0.5 * t) * 0.34
        sample += sin(2 * .pi * root * t) * 0.16
        sample += sin(2 * .pi * fifth * 1.002 * t) * 0.08

        let chordStep = UInt64(t / chordSeconds)
        let chordInto = (t / chordSeconds).truncatingRemainder(dividingBy: 1)
        let chordEnv = sin(chordInto * .pi)
        let chord = scale[Self.pick(seed, chordStep, 11, scale.count)]
        sample += sin(2 * .pi * chord * t) * chordEnv * 0.16

        let step = UInt64(t / noteSeconds)
        let into = (t / noteSeconds).truncatingRemainder(dividingBy: 1)
        let env = sin(into * .pi)
        let rest = Self.pick(seed, step, 3, tape == 1 ? 6 : 4) == 0
        if !rest {
            let note = scale[Self.pick(seed, step, 9, scale.count)]
            let octave: Float = Self.pick(seed, step, 5, 5) == 0 ? 2 : 1
            sample += sin(2 * .pi * note * octave * t) * env * (tape == 1 ? 0.22 : 0.16)
        }

        let level: Float = tape == 2 ? 0.5 : (tape == 1 ? 0.85 : 0.7)
        sample *= intensity * breath * level
        if sting > 0 {
            sample += sin(2 * .pi * sting * t) * 0.08
        }
        return sample
    }

    func stop() {
        engine?.stop()
        engine = nil
        source = nil
        warm = 0
    }

    /// Pentatonic-ish sets, in hertz. Slow sits low, bright opens up,
    /// night drops and thins out.
    private static func scale(_ tape: Int) -> [Float] {
        switch tape {
        case 1: return [220, 261.6, 329.6, 392, 440]
        case 2: return [98, 110, 130.8, 146.8, 196]
        default: return [110, 130.8, 164.8, 196, 220]
        }
    }

    /// Stable pick in `0..<count` from the tank seed and a phrase step.
    private static func pick(_ seed: UInt64, _ step: UInt64, _ salt: UInt64, _ count: Int) -> Int {
        var x = seed ^ salt ^ (step &* 0x9E37_79B9_7F4A_7C15)
        x ^= x >> 30
        x &*= 0xBF58_476D_1CE4_E5B9
        x ^= x >> 27
        let n = UInt64(max(count, 1))
        return Int(x % n)
    }
}
