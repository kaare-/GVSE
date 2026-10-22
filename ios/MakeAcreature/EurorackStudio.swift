import SwiftUI

/// Full-screen modular rack. Modelled loosely on Plaits, Maths, a Moog
/// ladder, Doepfer noise, and an 8-step sequencer. Patch outs into ins
/// with coloured cables (many at once), record, then save onto the shelf.
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
    @State private var anchors: [Int: CGPoint] = [:]
    @State private var dragFrom: AmbientAudio.Take.Jack?
    @State private var dragPoint: CGPoint?
    private let tick = Timer.publish(every: 0.25, on: .main, in: .common).autoconnect()

    private let panel = Color(red: 0.10, green: 0.11, blue: 0.10)
    private let face = Color(red: 0.16, green: 0.17, blue: 0.16)
    private let screw = Color(red: 0.55, green: 0.52, blue: 0.42)

    var body: some View {
        ZStack {
            Color(red: 0.05, green: 0.05, blue: 0.05).ignoresSafeArea()
            VStack(spacing: 0) {
                header
                ScrollView {
                    ZStack {
                        rackModules
                            .padding(12)
                            .coordinateSpace(name: "rack")
                            .onPreferenceChange(JackAnchorKey.self) { anchors = $0 }

                        cableLayer
                            .allowsHitTesting(false)
                            .padding(12)
                    }
                }
                transport
            }
        }
        .preferredColorScheme(.dark)
        .onAppear {
            // Exclusive rack voice — ambient playlist is already cut by the desk.
            session.beginRackSolo(take)
        }
        .onDisappear { session.endSolo() }
        .onReceive(tick) { _ in
            guard recording else { return }
            elapsed = Date().timeIntervalSince(started)
            if elapsed >= 48 { saveTape() }
            else { hear() }
        }
        .onChange(of: take) { _ in hear() }
    }

    private var rackModules: some View {
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
            legend
            PixelLabel(
                text: hintText,
                scale: 1,
                color: Palette.dimPaper
            )
        }
    }

    private var hintText: String {
        if dragFrom != nil {
            return "DROP ON AN IN JACK"
        }
        if let armed {
            return "ARMED \(armed.label)  TAP AN IN  OR DRAG FROM AN OUT"
        }
        return "DRAG OUT TO IN  OR TAP OUT THEN IN  MANY CABLES OK"
    }

    private var legend: some View {
        VStack(alignment: .leading, spacing: 4) {
            PixelLabel(text: "CABLE COLOURS", scale: 1, color: Palette.dimPaper)
            HStack(spacing: 8) {
                ForEach(sourceJacks, id: \.rawValue) { jack in
                    HStack(spacing: 3) {
                        Circle().fill(RackCable.color(for: jack)).frame(width: 8, height: 8)
                        PixelLabel(text: jack.label, scale: 1, color: Palette.paper)
                    }
                }
            }
            if !take.cables.isEmpty {
                PixelLabel(text: cableCaption, scale: 1, color: Palette.paper)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var sourceJacks: [AmbientAudio.Take.Jack] {
        [.lfo, .env, .noiseOut, .seq, .day, .rain, .lamp, .candle, .room]
    }

    private var cableLayer: some View {
        GeometryReader { _ in
            Canvas { ctx, _ in
                for (i, cable) in take.cables.enumerated() {
                    guard let from = anchors[cable.from], let to = anchors[cable.to] else { continue }
                    let ink = RackCable.color(for: cable.from)
                    var path = Path()
                    path.move(to: from)
                    let midY = (from.y + to.y) / 2 + CGFloat((i % 5) - 2) * 10
                    let c1 = CGPoint(x: from.x + (to.x - from.x) * 0.25, y: midY)
                    let c2 = CGPoint(x: from.x + (to.x - from.x) * 0.75, y: midY)
                    path.addCurve(to: to, control1: c1, control2: c2)
                    ctx.stroke(path, with: .color(ink.opacity(0.95)), style: StrokeStyle(lineWidth: 3, lineCap: .round))
                    // Tip plugs.
                    ctx.fill(Path(ellipseIn: CGRect(x: from.x - 3, y: from.y - 3, width: 6, height: 6)), with: .color(ink))
                    ctx.fill(Path(ellipseIn: CGRect(x: to.x - 3, y: to.y - 3, width: 6, height: 6)), with: .color(ink))
                }
                if let fromJack = dragFrom, let from = anchors[fromJack.rawValue], let tip = dragPoint {
                    let ink = RackCable.color(for: fromJack)
                    var path = Path()
                    path.move(to: from)
                    let mid = CGPoint(x: (from.x + tip.x) / 2, y: min(from.y, tip.y) - 24)
                    path.addQuadCurve(to: tip, control: mid)
                    ctx.stroke(
                        path,
                        with: .color(ink.opacity(0.85)),
                        style: StrokeStyle(lineWidth: 3, lineCap: .round, dash: [5, 4])
                    )
                }
            }
        }
    }

    private var header: some View {
        HStack {
            PixelLabel(text: "EURORACK", scale: 2, color: Palette.highlight)
            Spacer()
            if !take.cables.isEmpty {
                Button {
                    take.cables.removeAll()
                    armed = nil
                } label: {
                    PixelPanel { PixelLabel(text: "CLEAR", scale: 2) }
                }
                .buttonStyle(.plain)
                .a11yButton("Clear all cables")
            }
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

    private var plaits: some View {
        module("PLAITS", width: 160) {
            knob("PITCH", value: $take.pitch)
            knob("WAVE", value: $take.wave)
            HStack(spacing: 10) {
                jackIn(.pitch)
                jackIn(.wave)
            }
            toggle("BRIGHT", isOn: $take.bright)
        }
    }

    private var maths: some View {
        module("MATHS", width: 160) {
            knob("ATK", value: $take.attack)
            knob("DEC", value: $take.decay)
            knob("SUS", value: $take.sustain)
            knob("REL", value: $take.release)
            HStack(spacing: 10) {
                jackOut(.env)
            }
        }
    }

    private var ladder: some View {
        module("LADDER", width: 160) {
            knob("CUT", value: $take.cutoff)
            knobsRow
            HStack(spacing: 10) {
                jackIn(.cutoff)
                jackIn(.reso)
            }
        }
    }

    private var knobsRow: some View {
        knob("RES", value: $take.reso)
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
                        PixelLabel(text: jack.label, scale: 1, color: RackCable.color(for: jack))
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
        let ink = RackCable.color(for: jack)
        let lit = armed == jack || dragFrom == jack
        return VStack(spacing: 2) {
            Circle()
                .strokeBorder(lit ? Color.white : ink, lineWidth: lit ? 3 : 2)
                .background(Circle().fill(ink.opacity(0.35)))
                .frame(width: 20, height: 20)
                .background(anchorProbe(jack))
            PixelLabel(text: jack.label, scale: 1, color: ink)
        }
        .contentShape(Rectangle())
        .gesture(outGesture(jack))
        .a11yButton("\(jack.label) out", hint: "Drag to an in, or tap then tap an in")
    }

    private func jackIn(_ jack: AmbientAudio.Take.Jack) -> some View {
        let cable = take.cables.first { $0.to == jack.rawValue }
        let ink = cable.map { RackCable.color(for: $0.from) } ?? Color(white: 0.4)
        let plugged = cable != nil
        return VStack(spacing: 2) {
            Circle()
                .strokeBorder(plugged ? ink : Color(white: 0.4), lineWidth: 2)
                .background(Circle().fill(plugged ? ink.opacity(0.45) : Color(white: 0.08)))
                .frame(width: 20, height: 20)
                .background(anchorProbe(jack))
            PixelLabel(text: jack.label, scale: 1, color: plugged ? ink : Palette.dimPaper)
        }
        .contentShape(Rectangle())
        .onTapGesture { tapJack(jack) }
        .a11yButton("\(jack.label) in", hint: plugged ? "Tap to unplug, or patch another source" : "Drop a cable here")
    }

    private func anchorProbe(_ jack: AmbientAudio.Take.Jack) -> some View {
        GeometryReader { geo in
            let frame = geo.frame(in: .named("rack"))
            Color.clear.preference(
                key: JackAnchorKey.self,
                value: [jack.rawValue: CGPoint(x: frame.midX, y: frame.midY)]
            )
        }
    }

    private func outGesture(_ jack: AmbientAudio.Take.Jack) -> some Gesture {
        DragGesture(minimumDistance: 0, coordinateSpace: .named("rack"))
            .onChanged { value in
                if hypot(value.translation.width, value.translation.height) < 6 {
                    return
                }
                dragFrom = jack
                dragPoint = value.location
                armed = jack
            }
            .onEnded { value in
                let dragged = hypot(value.translation.width, value.translation.height) >= 6
                defer {
                    dragFrom = nil
                    dragPoint = nil
                }
                if dragged, let dest = nearestIn(to: value.location, maxDist: 40) {
                    connect(from: jack, to: dest)
                    armed = nil
                } else if !dragged {
                    tapJack(jack)
                }
            }
    }

    private func nearestIn(to point: CGPoint, maxDist: CGFloat) -> AmbientAudio.Take.Jack? {
        let inputs: [AmbientAudio.Take.Jack] = [.pitch, .wave, .cutoff, .level, .clock, .reso]
        var best: AmbientAudio.Take.Jack?
        var bestDist = maxDist
        for jack in inputs {
            guard let p = anchors[jack.rawValue] else { continue }
            let d = hypot(p.x - point.x, p.y - point.y)
            if d < bestDist {
                bestDist = d
                best = jack
            }
        }
        return best
    }

    private func tapJack(_ jack: AmbientAudio.Take.Jack) {
        if jack.isSource {
            armed = armed == jack ? nil : jack
            return
        }
        guard let src = armed else {
            take.cables.removeAll { $0.to == jack.rawValue }
            return
        }
        connect(from: src, to: jack)
        armed = nil
    }

    /// One cable per input; many cables overall. Same pair toggles off.
    private func connect(from: AmbientAudio.Take.Jack, to: AmbientAudio.Take.Jack) {
        guard from.isSource, !to.isSource else { return }
        if let i = take.cables.firstIndex(where: { $0.from == from.rawValue && $0.to == to.rawValue }) {
            take.cables.remove(at: i)
            return
        }
        take.cables.removeAll { $0.to == to.rawValue }
        take.cables.append(.init(from: from.rawValue, to: to.rawValue))
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
        // Keep a single rack slot — never layer over the shelf playlist.
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

/// Stable colour per source jack so several cables stay readable.
private enum RackCable {
    static func color(for jack: AmbientAudio.Take.Jack) -> Color {
        color(for: jack.rawValue)
    }

    static func color(for jack: Int) -> Color {
        switch jack {
        case AmbientAudio.Take.Jack.lfo.rawValue:
            return Color(red: 0.90, green: 0.22, blue: 0.20)
        case AmbientAudio.Take.Jack.env.rawValue:
            return Color(red: 0.95, green: 0.55, blue: 0.12)
        case AmbientAudio.Take.Jack.noiseOut.rawValue:
            return Color(red: 0.82, green: 0.82, blue: 0.86)
        case AmbientAudio.Take.Jack.seq.rawValue:
            return Color(red: 0.20, green: 0.78, blue: 0.38)
        case AmbientAudio.Take.Jack.day.rawValue:
            return Color(red: 0.35, green: 0.70, blue: 0.98)
        case AmbientAudio.Take.Jack.rain.rawValue:
            return Color(red: 0.28, green: 0.52, blue: 0.78)
        case AmbientAudio.Take.Jack.lamp.rawValue:
            return Color(red: 0.96, green: 0.86, blue: 0.28)
        case AmbientAudio.Take.Jack.candle.rawValue:
            return Color(red: 0.95, green: 0.48, blue: 0.22)
        case AmbientAudio.Take.Jack.room.rawValue:
            return Color(red: 0.72, green: 0.38, blue: 0.90)
        default:
            return Color(red: 0.92, green: 0.72, blue: 0.22)
        }
    }
}

private struct JackAnchorKey: PreferenceKey {
    static var defaultValue: [Int: CGPoint] = [:]
    static func reduce(value: inout [Int: CGPoint], nextValue: () -> [Int: CGPoint]) {
        value.merge(nextValue(), uniquingKeysWith: { $1 })
    }
}

/// Circular knob. Drag up/down to turn.
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
