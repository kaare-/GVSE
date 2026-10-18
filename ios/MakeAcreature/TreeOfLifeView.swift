import SwiftUI

/// Tree-of-life picture from `tank_species_tree`: one column per form
/// depth, roots at the left; edge thickness grows with organisms born,
/// extinct species are dimmed, archived ones get a ring. `compact` is
/// the framed picture on the room wall (no names).
struct TreeOfLifeView: View {
    let nodes: [SpeciesNode]
    var compact = false
    /// Set on the full tree. The lobby picture leaves this nil so the
    /// whole frame stays one button.
    var onPick: ((UInt16) -> Void)? = nil

    private let rowH: CGFloat = 22
    private let colW: CGFloat = 150

    var body: some View {
        let layout = Layout(nodes: nodes)
        let size = compact
            ? CGSize(width: 180, height: 120)
            : CGSize(
                width: max(CGFloat(layout.depthCount) * colW + 40, 300),
                height: max(CGFloat(layout.rows) * rowH + 40, 200)
            )
        Group {
            if compact {
                picture(layout, size: size)
            } else {
                ScrollView([.horizontal, .vertical]) {
                    tappable(layout, size: size)
                }
            }
        }
    }

    @ViewBuilder
    private func tappable(_ layout: Layout, size: CGSize) -> some View {
        let canvas = picture(layout, size: size)
        if let onPick {
            canvas.gesture(
                SpatialTapGesture().onEnded { value in
                    if let id = hit(layout, size: size, at: value.location) {
                        onPick(id)
                    }
                }
            )
        } else {
            canvas
        }
    }

    private func hit(_ layout: Layout, size: CGSize, at point: CGPoint) -> UInt16? {
        let sx: CGFloat = compact ? (size.width - 24) / CGFloat(max(layout.depthCount, 1)) : colW
        let sy: CGFloat = compact ? (size.height - 16) / CGFloat(max(layout.rows, 1)) : rowH
        func center(_ p: Layout.Pos) -> CGPoint {
            CGPoint(x: 12 + CGFloat(p.depth) * sx + 6, y: 12 + (CGFloat(p.row) + 0.5) * sy)
        }
        for node in nodes.reversed() {
            guard let p = layout.pos[node.id] else { continue }
            let c = center(p)
            let rect = CGRect(x: c.x - 8, y: c.y - sy / 2, width: max(sx - 8, 16), height: sy)
            if rect.contains(point) { return node.id }
        }
        return nil
    }

    private func picture(_ layout: Layout, size: CGSize) -> some View {
        Canvas { ctx, _ in
            ctx.fill(
                Path(CGRect(origin: .zero, size: size)),
                with: .color(Color(red: 0.14, green: 0.09, blue: 0.06))
            )
            if nodes.isEmpty {
                if !compact {
                    ctx.pixelText("NO LIFE YET", at: CGPoint(x: 20, y: 20), scale: 2, color: Palette.dimPaper)
                }
                return
            }
            let sx = compact ? (size.width - 24) / CGFloat(max(layout.depthCount, 1)) : colW
            let sy = compact ? (size.height - 16) / CGFloat(max(layout.rows, 1)) : rowH
            func point(_ p: Layout.Pos) -> CGPoint {
                CGPoint(x: 12 + CGFloat(p.depth) * sx + 6, y: 12 + (CGFloat(p.row) + 0.5) * sy)
            }
            for node in nodes {
                guard let parent = node.parent, let a = layout.pos[parent], let b = layout.pos[node.id] else { continue }
                var path = Path()
                let pa = point(a)
                let pb = point(b)
                path.move(to: pa)
                path.addLine(to: CGPoint(x: pa.x, y: pb.y))
                path.addLine(to: pb)
                let born = CGFloat(max(node.rec.born, 1))
                let width = min(1 + log2(born) * 0.6, 6) * (compact ? 0.5 : 1)
                let alpha = node.rec.extinct != 0 ? 0.4 : 0.9
                ctx.stroke(
                    path,
                    with: .color(Color(red: 0.72, green: 0.58, blue: 0.32).opacity(alpha)),
                    lineWidth: width
                )
            }
            for node in nodes {
                guard let p = layout.pos[node.id] else { continue }
                let c = point(p)
                let s: CGFloat = compact ? 5 : 10
                let swatch = treeInk(node.rec.r, node.rec.g, node.rec.b)
                    .opacity(node.rec.extinct != 0 ? 0.45 : 1)
                let box = CGRect(x: c.x - s / 2, y: c.y - s / 2, width: s, height: s)
                ctx.fill(Path(box), with: .color(swatch))
                if node.rec.archived != 0 {
                    ctx.stroke(Path(box.insetBy(dx: -2, dy: -2)), with: .color(Palette.highlight), lineWidth: 1)
                }
                if !compact {
                    let label = node.name.uppercased()
                    let colour = node.rec.extinct != 0 ? Palette.dimPaper : Palette.paper
                    ctx.pixelText(label, at: CGPoint(x: c.x + 10, y: c.y - 7), scale: 1, color: colour)
                    let sub = node.rec.extinct != 0
                        ? "\(node.rec.forms) FORMS  GONE"
                        : "\(node.rec.forms) FORMS  \(node.rec.live_cells) CELLS"
                    ctx.pixelText(sub, at: CGPoint(x: c.x + 10, y: c.y + 2), scale: 1, color: Palette.dimPaper)
                }
            }
        }
        .frame(width: size.width, height: size.height)
    }

    /// Pull neon greens toward olive so the picture sits in the wood.
    private func treeInk(_ r: UInt8, _ g: UInt8, _ b: UInt8) -> Color {
        var red = Double(r) / 255
        var green = Double(g) / 255
        var blue = Double(b) / 255
        if green > red + 0.12 && green > blue + 0.12 {
            red = min(1, red * 0.35 + 0.42)
            green = min(1, green * 0.62 + 0.16)
            blue = min(1, blue * 0.35 + 0.12)
        } else {
            red = min(1, 0.18 + red * 0.72)
            green = min(1, 0.14 + green * 0.62)
            blue = min(1, 0.1 + blue * 0.5)
        }
        return Color(red: red, green: green, blue: blue)
    }

    /// Depth-first order gives each species a row; the column is the
    /// number of ancestors (not the species numeral, which continues
    /// across archive roots).
    struct Layout {
        struct Pos {
            var depth: Int
            var row: Int
        }

        var pos: [UInt16: Pos] = [:]
        var rows = 0
        var depthCount = 0

        init(nodes: [SpeciesNode]) {
            var children: [UInt16: [UInt16]] = [:]
            var roots: [UInt16] = []
            for n in nodes {
                if let p = n.parent {
                    children[p, default: []].append(n.id)
                } else {
                    roots.append(n.id)
                }
            }
            var stack: [(UInt16, Int)] = roots.reversed().map { ($0, 0) }
            while let (id, depth) = stack.popLast() {
                pos[id] = Pos(depth: depth, row: rows)
                rows += 1
                depthCount = max(depthCount, depth + 1)
                for child in (children[id] ?? []).reversed() {
                    stack.append((child, depth + 1))
                }
            }
        }
    }
}
