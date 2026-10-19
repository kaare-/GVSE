import AVFoundation
import Foundation

/// Generative room tone. Eight tapes of soft sines: a drone, a chord
/// that turns over, and a line that walks, rests, and changes register.
/// Each tape has its own scale, pace, and habit. Original material.
final class AmbientAudio {
    static let shared = AmbientAudio()

    static let tapeNames = ["SLOW", "BRIGHT", "NIGHT", "TIDE", "DRIFT", "REED", "GLASS", "HEARTH"]
    /// How long each voice plays before the playlist fades into the next.
    static let tapeDurations: [TimeInterval] = [48, 36, 72, 40, 56, 34, 80, 44]

    struct PlaySlot {
        var voice: Int
        var duration: Double
    }

    private let gate = NSLock()
    private var slots: [PlaySlot] = []
    private var slot = 0
    private var into: Double = 0
    private var deckOn = false

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
    private var tape = 0
    /// Melody walk, advanced once per note so the line can step.
    private var heldStep: UInt64 = UInt64.max
    private var heldIndex = 0
    private var heldOctave: Float = 1

    func setTape(_ index: Int) {
        tape = min(Self.tapeNames.count - 1, max(0, index))
    }

    /// Left to right is the playlist. `resetHead` restarts the fade-in.
    func setDeck(on: Bool, slots: [PlaySlot], start: Int, resetHead: Bool) {
        gate.lock()
        deckOn = on
        self.slots = slots
        if slots.isEmpty {
            slot = 0
            into = 0
        } else {
            let next = min(max(0, start), slots.count - 1)
            if resetHead { into = 0 }
            slot = next
            tape = min(max(0, slots[next].voice), Self.tapes.count - 1)
        }
        gate.unlock()
    }

    var playingSlot: Int {
        gate.lock()
        defer { gate.unlock() }
        return slot
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
            let rate: Float = 22_050
            let sting: Float = now < self.stingUntil ? self.stingFreq : 0
            self.gate.lock()
            let playing = self.deckOn && !self.slots.isEmpty
            var slot = self.slot
            var into = self.into
            var voiceIndex = self.tape
            let slots = self.slots
            for i in 0..<n {
                self.tick &+= 1
                let t = Float(self.tick) / rate
                var sample: Float = 0
                if playing, !slots.isEmpty {
                    let safeSlot = min(slot, slots.count - 1)
                    voiceIndex = min(max(0, slots[safeSlot].voice), Self.tapes.count - 1)
                    let part = Self.tapes[voiceIndex]
                    let dur = max(slots[safeSlot].duration, 1)
                    let fade = min(1.6, dur / 3)
                    var gain = 1.0
                    if into < fade { gain = into / fade }
                    let left = dur - into
                    if left < fade { gain = min(gain, max(0, left) / fade) }
                    sample = self.voice(t: t, tape: voiceIndex, part: part, sting: 0) * Float(max(0, gain))
                    into += 1 / Double(rate)
                    if into >= dur {
                        into = 0
                        slot = (safeSlot + 1) % slots.count
                    }
                }
                if sting > 0 {
                    sample += sin(2 * .pi * sting * t) * 0.08
                }
                self.warm += (sample - self.warm) * 0.18
                data[i] = max(-1, min(1, self.warm))
            }
            if playing {
                self.slot = slot
                self.into = into
                self.tape = voiceIndex
            }
            self.gate.unlock()
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

    /// Drone, a two-note chord, and a line that steps. Envelopes sit on
    /// zero at each note edge, so a tape change does not click.
    private func voice(t: Float, tape: Int, part: Tape, sting: Float) -> Float {
        let scale = part.scale
        let root = scale[0]
        let fifth = scale[min(2, scale.count - 1)]
        let breath = 0.72 + 0.28 * sin(2 * .pi * t / part.breath)
        let beat = 1 + 0.004 * sin(2 * .pi * t / (part.breath * 3))
        var sample = sin(2 * .pi * root * 0.5 * t) * 0.30
        sample += sin(2 * .pi * root * t) * 0.12
        sample += sin(2 * .pi * fifth * beat * t) * 0.07

        let phrase = UInt64(t / (part.noteSeconds * 8))
        let phraseInto = (t / (part.noteSeconds * 8)).truncatingRemainder(dividingBy: 1)
        let phraseEnv = sin(phraseInto * .pi)
        let partial = scale[min(3, scale.count - 1)]
        sample += sin(2 * .pi * partial * 2 * t) * phraseEnv * (tape == 6 ? 0.09 : 0.035)
        if tape == 6 {
            let shimmer = 0.45 + 0.55 * sin(2 * .pi * t / 22)
            sample += sin(2 * .pi * scale[min(4, scale.count - 1)] * 1.002 * t) * shimmer * 0.11
        }

        let chordMul: Float = (tape == 1 || tape == 7) ? 2 : 4
        let chordSeconds = part.noteSeconds * chordMul
        let chordStep = UInt64(t / chordSeconds)
        let chordInto = (t / chordSeconds).truncatingRemainder(dividingBy: 1)
        let chordEnv = sin(chordInto * .pi)
        let chordIndex = Self.pick(seed, chordStep &+ phrase, 11, scale.count)
        let chord = scale[chordIndex]
        let colour = scale[(chordIndex + 2) % scale.count]
        sample += sin(2 * .pi * chord * t) * chordEnv * part.chord
        sample += sin(2 * .pi * colour * t) * chordEnv * part.chord * 0.55

        let step = UInt64(t / part.noteSeconds)
        let into = (t / part.noteSeconds).truncatingRemainder(dividingBy: 1)
        let env = sin(into * .pi)
        if step != heldStep {
            heldStep = step
            advanceLine(tape: tape, step: step, phrase: phrase, count: scale.count)
        }
        let hush: Float = Self.pick(seed, phrase, 53, 4) == 0 ? 0.35 : 1
        let rest = Self.pick(seed, step, 3, part.restEvery) == 0
        if !rest {
            let note = scale[heldIndex % scale.count]
            sample += sin(2 * .pi * note * heldOctave * t) * env * part.melody * hush
        }
        if part.answer {
            let other = part.noteSeconds * (tape == 4 ? 1.5 : 2)
            let step2 = UInt64(t / other)
            let into2 = (t / other).truncatingRemainder(dividingBy: 1)
            let env2 = sin(into2 * .pi)
            let rest2 = Self.pick(seed, step2, 41, part.restEvery + 1) == 0
            if !rest2 {
                let note2 = scale[Self.pick(seed, step2 &+ phrase, 43, scale.count)]
                let low: Float = tape == 7 ? 0.5 : 1
                sample += sin(2 * .pi * note2 * low * t) * env2 * part.melody * 0.5
            }
        }

        sample *= intensity * breath * part.level
        if sting > 0 {
            sample += sin(2 * .pi * sting * t) * 0.08
        }
        return sample
    }

    /// Step, leap, or follow the tape's habit. Called once per note.
    private func advanceLine(tape: Int, step: UInt64, phrase: UInt64, count: Int) {
        let n = max(count, 1)
        switch tape {
        case 3:
            let place = Int(step % 8)
            heldIndex = place == 7 ? 0 : min(n - 1, heldIndex + 1)
        case 5:
            let generation = step / 10
            heldIndex = Self.pick(seed, generation &* 5 &+ (step % 5), 23, n)
        case 6:
            heldIndex = Self.pick(seed, step / 2, 23, n)
        default:
            if Self.pick(seed, step, 19, tape == 4 ? 4 : 7) == 0 {
                heldIndex = Self.pick(seed, step, 23, n)
            } else {
                let interval = Self.pick(seed, step, 17, 5) - 2
                heldIndex = (heldIndex + interval + n) % n
            }
        }
        let up = Self.pick(seed, phrase, 29, tape == 6 ? 3 : 5) == 0
        heldOctave = up ? 2 : 1
        if tape == 7 && Self.pick(seed, phrase, 37, 2) == 0 {
            heldOctave = 0.5
        }
        if tape == 2 {
            heldOctave = 1
        }
    }

    func stop() {
        engine?.stop()
        engine = nil
        source = nil
        warm = 0
    }

    /// Pace, scale, and how busy the line is. Hertz, original sets.
    private struct Tape {
        var scale: [Float]
        var noteSeconds: Float
        var restEvery: Int
        var level: Float
        var breath: Float
        var melody: Float
        var chord: Float
        var answer: Bool
    }

    private static let tapes: [Tape] = [
        Tape(scale: [110, 130.81, 146.83, 164.81, 196, 220], noteSeconds: 6.5, restEvery: 5, level: 0.70, breath: 16, melody: 0.16, chord: 0.14, answer: false),
        Tape(scale: [196, 220, 261.63, 293.66, 329.63, 392], noteSeconds: 3.4, restEvery: 7, level: 0.85, breath: 12, melody: 0.20, chord: 0.15, answer: true),
        Tape(scale: [82.41, 98, 110, 123.47, 146.83, 164.81], noteSeconds: 9.5, restEvery: 3, level: 0.48, breath: 30, melody: 0.12, chord: 0.10, answer: false),
        Tape(scale: [146.83, 164.81, 174.61, 196, 220, 246.94], noteSeconds: 5.2, restEvery: 5, level: 0.75, breath: 20, melody: 0.17, chord: 0.13, answer: false),
        Tape(scale: [98, 110, 130.81, 146.83, 174.61, 196], noteSeconds: 7.5, restEvery: 3, level: 0.62, breath: 24, melody: 0.14, chord: 0.11, answer: true),
        Tape(scale: [130.81, 155.56, 174.61, 196, 233.08, 261.63], noteSeconds: 4.0, restEvery: 6, level: 0.72, breath: 14, melody: 0.18, chord: 0.13, answer: false),
        Tape(scale: [246.94, 293.66, 329.63, 369.99, 440, 493.88], noteSeconds: 11, restEvery: 4, level: 0.40, breath: 36, melody: 0.08, chord: 0.12, answer: false),
        Tape(scale: [174.61, 196, 220, 261.63, 293.66, 349.23], noteSeconds: 4.6, restEvery: 5, level: 0.78, breath: 15, melody: 0.17, chord: 0.16, answer: true),
    ]

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
