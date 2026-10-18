import Metal
import SwiftUI
import UIKit

/// Nearest-neighbour blit of the tank's RGBA buffer, with an optional
/// circular ×3 magnifier and a small camera tilt for gyro swirl.
final class TankMetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }

    var onTouch: ((CGPoint?, Bool) -> Void)?
    var magnifier: MagnifierState? {
        didSet { setNeedsDisplay() }
    }
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
    private let shader = """
    #include <metal_stdlib>
    using namespace metal;
    struct VOut { float4 pos [[position]]; float2 uv; };
    struct Uniforms {
        float4 window;
        float2 lensCenter;
        float lensRadius;
        float lensScale;
        float aspect;
        float _pad;
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
        if (p.x < 0.0 || p.y < 0.0 || p.x > 1.0 || p.y > 1.0) {
            return float4(0.0, 0.0, 0.0, 1.0);
        }
        float2 sampleUV = p;
        if (u.lensRadius > 0.0) {
            // Distance in screen pixels, not raw UV, so a tall phone
            // does not stretch the glass into an oval.
            float2 d = in.uv - u.lensCenter;
            d.x *= max(u.aspect, 0.01);
            float dist = length(d);
            if (dist < u.lensRadius) {
                float2 center = (u.lensCenter - u.window.xy) / u.window.zw;
                // Zoom about the point under the finger. The centre of the
                // glass shows that pixel; the rim shows the neighbourhood.
                sampleUV = center + (p - center) / max(u.lensScale, 1.0);
                sampleUV = clamp(sampleUV, float2(0.0), float2(1.0));
                // Nearest sample, no frosted mix: the glass should read
                // clearer than the tank around it.
                return tex.sample(smp, sampleUV);
            }
        }
        return tex.sample(smp, sampleUV);
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
        backgroundColor = .black
        isOpaque = true
        isMultipleTouchEnabled = false
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
        let scale = contentScale
        let drawableW = max(bounds.width * scale, 1)
        let drawableH = max(bounds.height * scale, 1)
        metalLayer.drawableSize = CGSize(width: drawableW, height: drawableH)
        let k = max(1, floor(min(drawableW / CGFloat(width), drawableH / CGFloat(height))))
        let tw = CGFloat(width) * k
        let th = CGFloat(height) * k
        var uniforms = Uniforms(
            window: SIMD4<Float>(
                Float((drawableW - tw) * 0.5 / drawableW),
                Float((drawableH - th) * 0.5 / drawableH),
                Float(tw / drawableW),
                Float(th / drawableH)
            ),
            lensCenter: SIMD2<Float>(0, 0),
            lensRadius: 0,
            lensScale: 6,
            aspect: Float(drawableW / drawableH),
            pad: 0
        )
        if let lens = magnifier {
            // Shader uv.y is 0 at the top of the view, same as UIKit.
            uniforms.lensCenter = SIMD2(
                Float(lens.center.x * scale / drawableW),
                Float(lens.center.y * scale / drawableH)
            )
            let radiusPx = max(lens.radius, 24) * scale
            uniforms.lensRadius = Float(radiusPx / drawableH)
            uniforms.lensScale = 6
            uniforms.aspect = Float(drawableW / drawableH)
        }
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
        if let p = touches.first?.location(in: self) { onTouch?(p, false) }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let p = touches.first?.location(in: self) { onTouch?(p, false) }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let p = touches.first?.location(in: self) { onTouch?(p, true) }
        else { onTouch?(nil, true) }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        onTouch?(nil, true)
    }

    private struct Uniforms {
        var window: SIMD4<Float>
        var lensCenter: SIMD2<Float>
        var lensRadius: Float
        var lensScale: Float
        var aspect: Float
        var pad: Float
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
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 1)
        return pass
    }
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
