import SwiftUI

/// Full-screen modular rack. Modelled loosely on Plaits, Maths, a Moog
/// ladder, Doepfer noise, and an 8-step sequencer. Patch outs into ins,
/// record, then save the take onto a cassette on the shelf.
struct EurorackStudio: View {
    @ObservedObject var session: TankSession
    var onClose: () -> Void

    @State private var take = AmbientAudio.Take.fresh
    @State private var recording = false
    @State private var blankOn = false
    @State private var started = Date()
    @State private var elapsed: TimeInterval = 0
    @State private var armed: AmbientAudio.Take.Jack?
    @State private var name = ""
    private let tick = Timer.publish(every: 0.25, on: .main, in: .common).autoconnect()

    private let panel = Color(red: 0.10, green: 0.11, blue: 0.10)
    private let face = Color(red: 0.16, green: 0.17, blue: 0.16)
    private let screw = Color(red: 0.55, green: 0.52, blue: 0.42)
    private let cableInk = Color(red: 0.92, green: 0.72, blue: 0.22)

    var body: some View {
        ZStack {
            Color(red: 0.05, green: 0.05, blue: 0.05).ignoresSafeArea()
            VStack(spacing: 0) {
                header
                ScrollView {
                    VStack(spacing: 10) {
                        HStack(alignment: .top, spacing: 8) {
                            plaits
                            maths
                        }
                        HStack(alignment: .top, spacing: 8) {
                            ladder
                            noise
                        }
                        HStack(alignment: .top, spacing: 8) {
                            lfo
                            vca
                        }
                        seq
                        roomCv
                        if !take.cables.isEmpty {
                            PixelLabel(text: cableCaption, scale: 1, color: cableInk)
                        }
                        PixelLabel(
                            text: armed == nil
                                ? "TAP AN OUT, THEN AN IN, TO PATCH"
                                : "ARMED \(armed!.label)  TAP AN IN OR THE SAME OUT TO CLEAR",
                            scale: 1,
                            color: Palette.dimPaper
                        )
                    }
                    .padding(12)
                }
                transport
            }
        }
        .preferredColorScheme(.dark)
        .onAppear { hear() }
        .onDisappear { session.endSolo() }
        .onReceive(tick) { _ in
            guard recording else { return }
            elapsed = Date().timeIntervalSince(started)
            if elapsed >= 48 { saveTape() }
            else { hear() }
        }
        .onChange(of: take) { _ in hear() }
    }

    private var header: some View {
        HStack {
            PixelLabel(text: "EURORACK", scale: 2, color: Palette.highlight)
            Spacer()
            Button(action: close) {
                PixelPanel { PixelLabel(text: "CLOSE", scale: 2) }
            }
            .buttonStyle(.plain)
        }
        .padding(12)
        .background(panel)
    }

    private var transport: some View {
        HStack(spacing: 10) {
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 2)
                    .fill(Color(red: 0.08, green: 0.07, blue: 0.06))
                    .frame(width: 120, height: 36)
                RoundedRectangle(cornerRadius: 2)
                    .fill(Color(red: 0.55, green: 0.5, blue: 0.42))
                    .frame(width: 40, height: 22)
                    .overlay(PixelLabel(text: blankOn ? "REC" : "EMPTY", scale: 1, color: Color(red: 0.12, green: 0.08, blue: 0.04)))
                    .offset(x: blankOn ? 12 : 70)
                    .animation(.easeInOut(duration: 0.35), value: blankOn)
            }
            Button(action: toggleRecord) {
                ZStack {
                    Circle()
                        .fill(Color(red: 0.82, green: 0.12, blue: 0.1))
                        .frame(width: 34, height: 34)
                    if recording {
                        RoundedRectangle(cornerRadius: 1)
                            .fill(Color.white)
                            .frame(width: 12, height: 12)
                    }
                }
            }
            .buttonStyle(.plain)
            .a11yButton(recording ? "Stop" : "Record")
            PixelLabel(text: clock(elapsed), scale: 2, color: recording ? Palette.highlight : Palette.paper)
            Spacer()
            Button(action: saveTape) {
                PixelPanel {
                    PixelLabel(text: "SAVE TAPE", scale: 2, color: Palette.highlight)
                }
            }
            .buttonStyle(.plain)
            .a11yButton("Save tape", hint: "Write this take onto a cassette on the shelf")
        }
        .padding(12)
        .background(panel)
    }

    // MARK: - Modules

    /// Morphing oscillator, Plaits-flavoured.
    private var plaits: some View {
        module("PLAITS", width: 160) {
            knob("PITCH", value: $take.pitch)
            knob("WAVE", value: $take.wave)
            HStack(spacing: 10) {
                jackOut(.seq)
                jackIn(.pitch)
                jackIn(.wave)
            }
            toggle("BRIGHT", isOn: $take.bright)
        }
    }

    /// Function / envelope, Maths-flavoured.
    private var maths: some View {
        module("MATHS", width: 160) {
            knob("ATK", value: $take.attack)
            knob("DEC", value: $take.decay)
            knob("SUS", value: $take.sustain)
            knob("REL", value: $take.release)
            HStack(spacing: 10) {
                jackOut(.env)
                jackIn(.clock)
            }
        }
    }

    /// Low-pass with resonance, ladder-flavoured.
    private var ladder: some View {
        module("LADDER", width: 160) {
            knob("CUT", value: $take.cutoff)
            knob("RES", value: $take.reso)
            HStack(spacing: 10) {
                jackIn(.cutoff)
                jackIn(.reso)
            }
        }
    }

    private var noise: some View {
        module("NOISE", width: 160) {
            knob("LEVEL", value: $take.noise)
            HStack(spacing: 10) {
                jackOut(.noiseOut)
            }
        }
    }

    private var lfo: some View {
        module("LFO", width: 160) {
            knob("RATE", value: $take.lfoRate)
            knob("AMT", value: $take.lfoAmt)
            HStack(spacing: 10) {
                jackOut(.lfo)
            }
        }
    }

    private var vca: some View {
        module("VCA", width: 160) {
            knob("LEVEL", value: $take.level)
            HStack(spacing: 10) {
                jackIn(.level)
            }
        }
    }

    private var seq: some View {
        module("SEQ-8", width: 328) {
            knob("CLOCK", value: $take.clock)
            HStack(spacing: 4) {
                ForEach(0..<8, id: \.self) { i in
                    VStack(spacing: 4) {
                        PixelLabel(text: "\(i + 1)", scale: 1, color: Palette.dimPaper)
                        Slider(value: stepBinding(i), in: 0...1)
                            .frame(width: 28, height: 72)
                            .rotationEffect(.degrees(-90))
                            .frame(width: 28, height: 72)
                    }
                    .frame(width: 34, height: 90)
                }
            }
            HStack(spacing: 10) {
                jackOut(.seq)
                jackIn(.clock)
            }
        }
    }

    private var roomCv: some View {
        module("ROOM CV", width: 328) {
            HStack(spacing: 12) {
                ForEach([AmbientAudio.Take.Jack.day, .rain, .lamp, .candle, .room], id: \.rawValue) { jack in
                    VStack(spacing: 4) {
                        PixelLabel(text: jack.label, scale: 1, color: Palette.dimPaper)
                        jackOut(jack)
                    }
                }
            }
        }
    }

    // MARK: - Chrome

    private func module<Content: View>(_ title: String, width: CGFloat, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Circle().fill(screw).frame(width: 6, height: 6)
                PixelLabel(text: title, scale: 1, color: Palette.highlight)
                Spacer()
                Circle().fill(screw).frame(width: 6, height: 6)
            }
            content()
        }
        .padding(10)
        .frame(width: width, alignment: .leading)
        .background(face)
        .overlay(Rectangle().stroke(Color(white: 0.28), lineWidth: 1))
    }

    private func knob(_ title: String, value: Binding<Float>) -> some View {
        HStack(spacing: 8) {
            PixelLabel(text: title, scale: 1, color: Palette.paper)
                .frame(width: 44, alignment: .leading)
            RackKnob(value: value)
            PixelLabel(text: String(format: "%.2f", value.wrappedValue), scale: 1, color: Palette.dimPaper)
        }
    }

    private func toggle(_ title: String, isOn: Binding<Bool>) -> some View {
        Button {
            isOn.wrappedValue.toggle()
        } label: {
            PixelPanel {
                PixelLabel(
                    text: "\(title) \(isOn.wrappedValue ? "ON" : "OFF")",
                    scale: 1,
                    color: isOn.wrappedValue ? Palette.highlight : Palette.paper
                )
            }
        }
        .buttonStyle(.plain)
    }

    private func jackOut(_ jack: AmbientAudio.Take.Jack) -> some View {
        Button { tapJack(jack) } label: {
            VStack(spacing: 2) {
                Circle()
                    .strokeBorder(armed == jack ? cableInk : Color(white: 0.55), lineWidth: 2)
                    .background(Circle().fill(Color(white: 0.12)))
                    .frame(width: 18, height: 18)
                PixelLabel(text: jack.label, scale: 1, color: Palette.dimPaper)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("\(jack.label) out", hint: "Patch from here")
    }

    private func jackIn(_ jack: AmbientAudio.Take.Jack) -> some View {
        let plugged = take.cables.contains { $0.to == jack.rawValue }
        return Button { tapJack(jack) } label: {
            VStack(spacing: 2) {
                Circle()
                    .strokeBorder(plugged ? cableInk : Color(white: 0.4), lineWidth: 2)
                    .background(Circle().fill(plugged ? Color(red: 0.35, green: 0.28, blue: 0.1) : Color(white: 0.08)))
                    .frame(width: 18, height: 18)
                PixelLabel(text: jack.label, scale: 1, color: Palette.dimPaper)
            }
        }
        .buttonStyle(.plain)
        .a11yButton("\(jack.label) in", hint: plugged ? "Unplug or repatch" : "Patch into here")
    }

    private func tapJack(_ jack: AmbientAudio.Take.Jack) {
        if jack.isSource {
            if armed == jack {
                armed = nil
            } else {
                armed = jack
            }
            return
        }
        guard let src = armed else {
            // Tap an in with nothing armed: clear cables into it.
            take.cables.removeAll { $0.to == jack.rawValue }
            return
        }
        if let i = take.cables.firstIndex(where: { $0.from == src.rawValue && $0.to == jack.rawValue }) {
            take.cables.remove(at: i)
        } else {
            take.cables.removeAll { $0.to == jack.rawValue }
            take.cables.append(.init(from: src.rawValue, to: jack.rawValue))
        }
        armed = nil
    }

    private var cableCaption: String {
        take.cables.compactMap { cable -> String? in
            guard let from = AmbientAudio.Take.Jack(rawValue: cable.from),
                  let to = AmbientAudio.Take.Jack(rawValue: cable.to) else { return nil }
            return "\(from.label)>\(to.label)"
        }
        .joined(separator: "  ")
    }

    private func stepBinding(_ index: Int) -> Binding<Float> {
        Binding(
            get: {
                guard take.steps.indices.contains(index) else { return 0 }
                return take.steps[index]
            },
            set: { value in
                if take.steps.count < 8 {
                    take.steps = (0..<8).map { i in
                        take.steps.indices.contains(i) ? take.steps[i] : 0
                    }
                }
                take.steps[index] = value
            }
        )
    }

    private func hear() {
        session.soloTake(take)
    }

    private func toggleRecord() {
        if recording {
            recording = false
        } else {
            recording = true
            blankOn = true
            started = Date()
            elapsed = 0
            hear()
        }
    }

    private func saveTape() {
        let dur = min(48, max(4, recording ? elapsed : max(elapsed, 12)))
        recording = false
        blankOn = false
        armed = nil
        let n = session.shelf.filter { $0.voice < 0 }.count + 1
        let label = name.isEmpty ? "TAKE \(n)" : name
        let patch = take
        PixelPrompt.singleLine(title: "TAPE", text: label, placeholder: "NAME") { entered in
            session.keepTake(patch, duration: dur, name: entered.isEmpty ? label : entered)
            onClose()
        }
    }

    private func close() {
        recording = false
        session.endSolo()
        onClose()
    }

    private func clock(_ secs: TimeInterval) -> String {
        let s = max(0, Int(secs.rounded()))
        return String(format: "%d:%02d", s / 60, s % 60)
    }
}

/// Circular knob. Drag up/down or around to turn.
private struct RackKnob: View {
    @Binding var value: Float
    @State private var start: Float = 0

    var body: some View {
        ZStack {
            Circle()
                .fill(Color(white: 0.12))
                .overlay(Circle().stroke(Color(white: 0.4), lineWidth: 2))
            Capsule()
                .fill(Palette.highlight)
                .frame(width: 3, height: 12)
                .offset(y: -8)
                .rotationEffect(.degrees(Double(value) * 270 - 135))
        }
        .frame(width: 36, height: 36)
        .contentShape(Circle())
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { g in
                    if g.translation == .zero { start = value }
                    let delta = Float(-g.translation.height) / 90
                    value = min(1, max(0, start + delta))
                }
        )
        .accessibilityValue(Text(String(format: "%.2f", value)))
        .accessibilityAdjustableAction { direction in
            switch direction {
            case .increment: value = min(1, value + 0.05)
            case .decrement: value = max(0, value - 0.05)
            @unknown default: break
            }
        }
    }
}
