import Metal
import SwiftUI
import UIKit

/// Nearest-neighbour blit of the tank. The phone is a window: one screen
/// wide at rest, dragged with two fingers, pinched to zoom. There is no
/// looking glass.
final class TankMetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }

    var onTouch: ((CGPoint?, Bool) -> Void)?
    var onNavigate: ((TankNav) -> Void)?
    var camera = TankCamera()
    var cameraTiltDegrees: Double = 0 {
        didSet {
            let radians = cameraTiltDegrees * .pi / 180
            transform = CGAffineTransform(rotationAngle: radians)
        }
    }
    var contentScale: CGFloat { window?.windowScene?.screen.scale ?? traitCollection.displayScale }

    private var metalLayer: CAMetalLayer { layer as! CAMetalLayer }
    private var device: MTLDevice?
    private var queue: MTLCommandQueue?
    private var pipeline: MTLRenderPipelineState?
    private var sampler: MTLSamplerState?
    private var texture: MTLTexture?
    private var textureWidth = 0
    private var textureHeight = 0
    private var panOrigin = CGPoint.zero
    private var multiTouch = false
    private var smallWorld = false
    private let shader = """
    #include <metal_stdlib>
    using namespace metal;
    struct VOut { float4 pos [[position]]; float2 uv; };
    struct Uniforms {
        float4 window;
        float4 cam;
        float2 frame;
        float2 pad;
    };
    vertex VOut vs(uint vid [[vertex_id]]) {
        float2 p[3] = { float2(-1.0, -1.0), float2(3.0, -1.0), float2(-1.0, 3.0) };
        float2 u[3] = { float2(0.0, 1.0), float2(2.0, 1.0), float2(0.0, -1.0) };
        VOut o;
        o.pos = float4(p[vid], 0.0, 1.0);
        o.uv = u[vid];
        return o;
    }
    fragment float4 fs(VOut in [[stage_in]],
                       texture2d<float> tex [[texture(0)]],
                       sampler smp [[sampler(0)]],
                       constant Uniforms &u [[buffer(0)]]) {
        float2 p = (in.uv - u.window.xy) / u.window.zw;
        if (p.x >= 0.0 && p.y >= 0.0 && p.x <= 1.0 && p.y <= 1.0) {
            float2 sampleUV = u.cam.xy + p * u.cam.zw;
            return tex.sample(smp, sampleUV);
        }
        if (u.frame.x <= 0.0) {
            discard_fragment();
        }
        float2 lo = u.window.xy;
        float2 hi = u.window.xy + u.window.zw;
        float2 gap = max(max(lo - in.uv, in.uv - hi), float2(0.0));
        float2 n = gap / max(u.frame, float2(1e-5));
        if (n.x < 1.0 && n.y < 1.0) {
            float t = max(n.x, n.y);
            float3 brass = float3(0.941, 0.780, 0.463);
            float3 recess = float3(0.388, 0.263, 0.184);
            float3 wood = float3(0.733, 0.522, 0.325);
            float3 lip = t < 0.18 ? brass : (t < 0.55 ? recess : wood);
            return float4(lip, 1.0);
        }
        float3 wall = float3(0.337, 0.282, 0.235);
        float3 sun = float3(0.733, 0.522, 0.325) * saturate(1.0 - in.uv.x) * 0.35;
        float3 room = (wall + sun) * (0.88 + 0.12 * in.uv.y);
        return float4(room, 1.0);
    }
    """

    func configure() {
        guard let device = MTLCreateSystemDefaultDevice() else { return }
        self.device = device
        metalLayer.device = device
        metalLayer.pixelFormat = .bgra8Unorm
        metalLayer.framebufferOnly = true
        queue = device.makeCommandQueue()
        let library = try? device.makeLibrary(source: shader, options: nil)
        let desc = MTLRenderPipelineDescriptor()
        desc.vertexFunction = library?.makeFunction(name: "vs")
        desc.fragmentFunction = library?.makeFunction(name: "fs")
        desc.colorAttachments[0].pixelFormat = .bgra8Unorm
        pipeline = try? device.makeRenderPipelineState(descriptor: desc)
        let samp = MTLSamplerDescriptor()
        samp.minFilter = .nearest
        samp.magFilter = .nearest
        samp.sAddressMode = .clampToEdge
        samp.tAddressMode = .clampToEdge
        sampler = device.makeSamplerState(descriptor: samp)
        backgroundColor = UIColor(red: 0.337, green: 0.282, blue: 0.235, alpha: 1)
        isOpaque = true
        clipsToBounds = true
        isMultipleTouchEnabled = true
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(pinched(_:)))
        let pan = UIPanGestureRecognizer(target: self, action: #selector(panned(_:)))
        pan.minimumNumberOfTouches = 2
        pan.maximumNumberOfTouches = 2
        addGestureRecognizer(pinch)
        addGestureRecognizer(pan)
    }

    func layoutFrame(worldW: CGFloat, worldH: CGFloat) -> TankFrame {
        let viewScale = contentScale
        return tankFrame(
            camera: &camera,
            worldW: worldW,
            worldH: worldH,
            drawableW: max(bounds.width * viewScale, 1),
            drawableH: max(bounds.height * viewScale, 1),
            viewScale: viewScale
        )
    }

    func present(pixels: UnsafePointer<UInt8>, width: Int, height: Int) {
        guard let device, let queue, let pipeline, let sampler else { return }
        ensureTexture(device: device, width: width, height: height)
        guard let texture else { return }
        texture.replace(
            region: MTLRegionMake2D(0, 0, width, height),
            mipmapLevel: 0,
            withBytes: pixels,
            bytesPerRow: width * 4
        )
        let viewScale = contentScale
        let drawableW = max(bounds.width * viewScale, 1)
        let drawableH = max(bounds.height * viewScale, 1)
        smallWorld = camera.worldW < tankScreenPixels * 0.85
        metalLayer.isOpaque = !smallWorld
        isOpaque = !smallWorld
        backgroundColor = smallWorld
            ? .clear
            : UIColor(red: 0.337, green: 0.282, blue: 0.235, alpha: 1)
        metalLayer.drawableSize = CGSize(width: drawableW, height: drawableH)
        let frame = tankFrame(
            camera: &camera,
            worldW: CGFloat(width),
            worldH: CGFloat(height),
            drawableW: drawableW,
            drawableH: drawableH,
            viewScale: viewScale
        )
        var uniforms = Uniforms(
            window: SIMD4<Float>(
                Float(frame.ox / drawableW),
                Float(frame.oy / drawableH),
                Float(frame.shownW * frame.scale / drawableW),
                Float(frame.shownH * frame.scale / drawableH)
            ),
            cam: SIMD4<Float>(
                Float(frame.left / max(frame.worldW, 1)),
                Float(frame.top / max(frame.worldH, 1)),
                Float(frame.shownW / max(frame.worldW, 1)),
                Float(frame.shownH / max(frame.worldH, 1))
            ),
            frame: SIMD2<Float>(
                Float((camera.worldW < tankScreenPixels * 0.85) ? 0 : 22 * viewScale / drawableW),
                Float((camera.worldW < tankScreenPixels * 0.85) ? 0 : 22 * viewScale / drawableH)
            ),
            pad: .zero
        )
        guard let drawable = metalLayer.nextDrawable(),
              let buffer = queue.makeCommandBuffer(),
              let passDesc = descriptor(for: drawable)
        else { return }
        guard let encoder = buffer.makeRenderCommandEncoder(descriptor: passDesc) else { return }
        encoder.setRenderPipelineState(pipeline)
        encoder.setFragmentTexture(texture, index: 0)
        encoder.setFragmentSamplerState(sampler, index: 0)
        encoder.setFragmentBytes(&uniforms, length: MemoryLayout<Uniforms>.stride, index: 0)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        encoder.endEncoding()
        buffer.present(drawable)
        buffer.commit()
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        if (event?.allTouches?.count ?? touches.count) > 1 {
            multiTouch = true
            return
        }
        multiTouch = false
        if let p = touches.first?.location(in: self) { onTouch?(p, false) }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        if multiTouch || (event?.allTouches?.count ?? 0) > 1 {
            multiTouch = true
            return
        }
        if let p = touches.first?.location(in: self) { onTouch?(p, false) }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        if multiTouch {
            let stillDown = event?.allTouches?.contains { $0.phase != .ended && $0.phase != .cancelled } ?? false
            if !stillDown { multiTouch = false }
            return
        }
        if let p = touches.first?.location(in: self) { onTouch?(p, true) }
        else { onTouch?(nil, true) }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        multiTouch = false
        onTouch?(nil, true)
    }

    @objc private func pinched(_ gesture: UIPinchGestureRecognizer) {
        onNavigate?(.pinch(
            scale: gesture.scale,
            anchor: gesture.location(in: self),
            state: gesture.state
        ))
    }

    @objc private func panned(_ gesture: UIPanGestureRecognizer) {
        let translation = gesture.translation(in: self)
        if gesture.state == .began { panOrigin = .zero }
        let delta = CGPoint(x: translation.x - panOrigin.x, y: translation.y - panOrigin.y)
        panOrigin = translation
        onNavigate?(.pan(delta: delta, state: gesture.state))
    }

    private struct Uniforms {
        var window: SIMD4<Float>
        var cam: SIMD4<Float>
        var frame: SIMD2<Float>
        var pad: SIMD2<Float>
    }

    private func ensureTexture(device: MTLDevice, width: Int, height: Int) {
        if texture != nil && textureWidth == width && textureHeight == height { return }
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba8Unorm,
            width: width,
            height: height,
            mipmapped: false
        )
        desc.usage = [.shaderRead]
        texture = device.makeTexture(descriptor: desc)
        textureWidth = width
        textureHeight = height
    }

    private func descriptor(for drawable: CAMetalDrawable) -> MTLRenderPassDescriptor? {
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = drawable.texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = MTLClearColor(
            red: smallWorld ? 0 : 0.337,
            green: smallWorld ? 0 : 0.282,
            blue: smallWorld ? 0 : 0.235,
            alpha: smallWorld ? 0 : 1
        )
        return pass
    }
}

enum TankNav {
    case pinch(scale: CGFloat, anchor: CGPoint, state: UIGestureRecognizer.State)
    case pan(delta: CGPoint, state: UIGestureRecognizer.State)
}

struct TankCamera {
    var left: CGFloat = 0
    var top: CGFloat = 0
    /// Width of the window in tank pixels. One old phone screen at rest.
    var visibleW: CGFloat = tankScreenPixels
    var worldW: CGFloat = 0
    var worldH: CGFloat = 0
    var pinchStartW: CGFloat = tankScreenPixels
    var stickToFloor = true
    /// Widest view before a pinch-out leaves for the lobby.
    /// A small glass uses a wider window so the wall shows around it.
    var homeW: CGFloat = tankScreenPixels
    var centerInWall = false
}

struct TankFrame {
    var left: CGFloat
    var top: CGFloat
    var shownW: CGFloat
    var shownH: CGFloat
    /// Drawable pixels per tank pixel.
    var scale: CGFloat
    var ox: CGFloat
    var oy: CGFloat
    var viewScale: CGFloat
    var worldW: CGFloat
    var worldH: CGFloat
    var visibleW: CGFloat
}

/// One phone-width of the old tank, in tank pixels. The wide world shows
/// this much until the player pinches.
let tankScreenPixels: CGFloat = 384
let tankMinVisible: CGFloat = 48
/// Physics cells across the main tank (three screens).
let tankMainCellsWide: UInt32 = 384

func tankFrame(
    camera: inout TankCamera,
    worldW: CGFloat,
    worldH: CGFloat,
    drawableW: CGFloat,
    drawableH: CGFloat,
    viewScale: CGFloat
) -> TankFrame {
    if camera.worldW != worldW || camera.worldH != worldH {
        camera.worldW = worldW
        camera.worldH = worldH
        if worldW < tankScreenPixels * 0.85 {
            // The tumbler view is the glass. The table around it is drawn
            // outside this view, so the world fills the view with no bezel.
            camera.homeW = worldW
            camera.visibleW = worldW
            camera.centerInWall = false
            camera.stickToFloor = false
        } else {
            camera.homeW = worldW
            camera.visibleW = min(tankScreenPixels, max(worldW, 1))
            camera.centerInWall = false
            camera.stickToFloor = true
        }
        camera.pinchStartW = camera.visibleW
        camera.left = max(0, (worldW - min(camera.visibleW, worldW)) / 2)
    }
    let visW = min(max(camera.visibleW, tankMinVisible), max(camera.homeW, worldW))
    camera.visibleW = visW
    let aspect = drawableW / max(drawableH, 1)
    let visH = visW / max(aspect, 0.01)
    let shownW = min(visW, worldW)
    let shownH = min(visH, worldH)
    let scale = drawableW / max(visW, 1)
    let contentW = shownW * scale
    let contentH = shownH * scale
    let ox = (drawableW - contentW) * 0.5
    let oy: CGFloat
    if camera.centerInWall && shownH >= worldH - 0.5 {
        oy = (drawableH - contentH) * 0.5
    } else {
        oy = drawableH - contentH
    }
    if camera.stickToFloor {
        camera.top = max(0, worldH - shownH)
        camera.stickToFloor = false
    }
    camera.left = min(max(camera.left, 0), max(0, worldW - shownW))
    camera.top = min(max(camera.top, 0), max(0, worldH - shownH))
    return TankFrame(
        left: camera.left,
        top: camera.top,
        shownW: shownW,
        shownH: shownH,
        scale: scale,
        ox: ox,
        oy: oy,
        viewScale: viewScale,
        worldW: worldW,
        worldH: worldH,
        visibleW: visW
    )
}

struct TankScreen: UIViewRepresentable {
    let session: TankSession

    func makeUIView(context: Context) -> TankMetalView {
        let view = TankMetalView()
        view.configure()
        session.attach(view: view)
        return view
    }

    func updateUIView(_ uiView: TankMetalView, context: Context) {}
}
