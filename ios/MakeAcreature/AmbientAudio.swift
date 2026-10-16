import AVFoundation
import Foundation

/// Phase 6 ambient: a tiny generative 8-bit placeholder. Real synth
/// tuning is open-ended; this ships a muteable engine that reacts to
/// population and plays event stings through the same interface.
final class AmbientAudio {
    static let shared = AmbientAudio()

    private var engine: AVAudioEngine?
    private var source: AVAudioSourceNode?
    private var muted = false
    private var intensity: Float = 0.15
    private var phase: Float = 0
    private var stingUntil: CFAbsoluteTime = 0
    private var stingFreq: Float = 0
    private var seed: UInt64 = 1
    private var tick: UInt64 = 0

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
            for i in 0..<n {
                self.tick &+= 1
                // Square-ish drone keyed off the tank seed.
                let base: Float = 55 + Float(self.seed & 31) * 2
                self.phase += base * 2 * .pi / 22_050
                if self.phase > 2 * .pi { self.phase -= 2 * .pi }
                let square: Float = sin(self.phase) >= 0 ? 1 : -1
                var sample = square * self.intensity * 0.12
                if now < self.stingUntil, self.stingFreq > 0 {
                    let t = Float(self.tick) / 22_050
                    sample += sin(2 * .pi * self.stingFreq * t) * 0.25
                }
                // Sparse noise ticks for "life".
                if (self.tick &+ self.seed) % 2048 < 3 {
                    let noise = Float((self.tick &* 1_103_515_245) & 255) / 255 - 0.5
                    sample += noise * 0.05
                }
                data[i] = max(-1, min(1, sample))
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

    func stop() {
        engine?.stop()
        engine = nil
        source = nil
    }
}
