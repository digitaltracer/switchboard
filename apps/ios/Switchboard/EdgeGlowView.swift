import MetalKit
import SwiftUI

/// What the glow needs each frame.
struct GlowFrame {
    var time: Float
    var level: Float
    /// 0 to 1: how far the light has swept up the edges from the bottom centre.
    var reveal: Float
    var fade: Float
}

/// The listening glow: a band of light along the screen's rounded edge, drawn by Metal. The
/// shader is compiled from source at runtime, so building the app needs no Metal toolchain.
struct EdgeGlowView: UIViewRepresentable {
    let cornerRadius: Float
    let frame: @MainActor () -> GlowFrame

    func makeUIView(context: Context) -> MTKView {
        let view = MTKView(frame: .zero, device: MTLCreateSystemDefaultDevice())
        view.isOpaque = false
        view.layer.isOpaque = false
        view.backgroundColor = .clear
        view.clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        view.colorPixelFormat = .bgra8Unorm
        view.preferredFramesPerSecond = 60
        view.isUserInteractionEnabled = false
        context.coordinator.attach(view)
        return view
    }

    func updateUIView(_ view: MTKView, context: Context) {
        context.coordinator.cornerRadius = cornerRadius
        context.coordinator.frame = frame
    }

    func makeCoordinator() -> GlowRenderer {
        GlowRenderer(cornerRadius: cornerRadius, frame: frame)
    }
}

@MainActor
final class GlowRenderer: NSObject, MTKViewDelegate {
    var cornerRadius: Float
    var frame: @MainActor () -> GlowFrame
    private var queue: MTLCommandQueue?
    private var pipeline: MTLRenderPipelineState?

    init(cornerRadius: Float, frame: @escaping @MainActor () -> GlowFrame) {
        self.cornerRadius = cornerRadius
        self.frame = frame
    }

    func attach(_ view: MTKView) {
        guard let device = view.device else { return }
        do {
            let library = try device.makeLibrary(source: Self.shader, options: nil)
            let desc = MTLRenderPipelineDescriptor()
            desc.vertexFunction = library.makeFunction(name: "glowVertex")
            desc.fragmentFunction = library.makeFunction(name: "glowFragment")
            desc.colorAttachments[0].pixelFormat = view.colorPixelFormat
            pipeline = try device.makeRenderPipelineState(descriptor: desc)
            queue = device.makeCommandQueue()
            view.delegate = self
        } catch {
            print("edge glow unavailable: \(error)")
        }
    }

    func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {}

    func draw(in view: MTKView) {
        guard let pipeline, let queue, let pass = view.currentRenderPassDescriptor, let drawable = view.currentDrawable,
              let buffer = queue.makeCommandBuffer(), let encoder = buffer.makeRenderCommandEncoder(descriptor: pass) else { return }
        let f = frame()
        let scale = Float(view.contentScaleFactor)
        var u: [Float] = [
            Float(view.drawableSize.width), Float(view.drawableSize.height), scale, cornerRadius,
            f.time, f.level, f.reveal, f.fade,
        ]
        encoder.setRenderPipelineState(pipeline)
        encoder.setFragmentBytes(&u, length: MemoryLayout<Float>.stride * u.count, index: 0)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        encoder.endEncoding()
        buffer.present(drawable)
        buffer.commit()
    }

    /// Sizes are in points: the band is the same width on every screen.
    static let shader = """
    #include <metal_stdlib>
    using namespace metal;

    struct Uniforms { float2 size; float scale; float radius; float time; float level; float reveal; float fade; };

    vertex float4 glowVertex(uint id [[vertex_id]]) {
        float2 p = float2((id << 1) & 2, id & 2);
        return float4(p * 2.0 - 1.0, 0.0, 1.0);
    }

    static float hash21(float2 p) { return fract(sin(dot(p, float2(127.1, 311.7))) * 43758.5453); }

    static float vnoise(float2 p) {
        float2 i = floor(p), f = fract(p);
        float2 u = f * f * (3.0 - 2.0 * f);
        return mix(mix(hash21(i), hash21(i + float2(1, 0)), u.x), mix(hash21(i + float2(0, 1)), hash21(i + float2(1, 1)), u.x), u.y);
    }

    // Noise around the edge, seamless where the angle wraps.
    static float ring(float u, float t, float f) {
        float a = u * 6.2831853;
        return vnoise(float2(cos(a), sin(a)) * f + float2(t, t * 0.7));
    }

    static float3 pick(float i) {
        if (i < 0.5) return float3(0.231, 0.510, 0.965);
        if (i < 1.5) return float3(0.545, 0.361, 0.965);
        if (i < 2.5) return float3(0.851, 0.275, 0.937);
        if (i < 3.5) return float3(1.000, 0.302, 0.553);
        if (i < 4.5) return float3(1.000, 0.541, 0.239);
        return float3(0.220, 0.741, 0.973);
    }

    static float3 palette(float x) {
        x = fract(x) * 6.0;
        float i = floor(x);
        return mix(pick(i), pick(fmod(i + 1.0, 6.0)), smoothstep(0.0, 1.0, fract(x)));
    }

    static float sdRoundBox(float2 p, float2 b, float r) {
        float2 q = abs(p) - b + r;
        return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
    }

    fragment float4 glowFragment(float4 pos [[position]], constant Uniforms &U [[buffer(0)]]) {
        float2 size = U.size / U.scale;
        float2 p = pos.xy / U.scale - size * 0.5;
        p.y = -p.y;
        float2 b = size * 0.5;
        float d = max(-sdRoundBox(p, b, U.radius), 0.0);
        float u = atan2(p.y / b.y, p.x / b.x) / 6.2831853 + 0.5;
        float n1 = ring(u, U.time * 0.35, 2.2);
        float n2 = ring(u + 0.37, -U.time * 0.45, 4.5);
        float w = (0.55 + 0.9 * n1) * (0.75 + 0.7 * U.level);
        float core = exp(-d / 1.2);
        float band = exp(-pow(d / (7.5 * w), 1.7));
        float soft = exp(-d / (20.0 * w));
        float sd = abs(u - 0.25);
        sd = min(sd, 1.0 - sd);
        float front = U.reveal * 0.6;
        float mask = 1.0 - smoothstep(front - 0.05, front + 0.01, sd);
        float flare = exp(-pow((sd - front) / 0.035, 2.0)) * (1.0 - smoothstep(0.7, 1.0, U.reveal)) * step(0.001, U.reveal);
        float3 col = palette(u + U.time * 0.05 + n2 * 0.3);
        float intensity = (core * 0.6 + band * 0.85 + soft * (0.05 + 0.1 * U.level)) * mask + flare * (band * 1.1 + soft * 0.45);
        col = mix(col, float3(1.0), core * 0.15 + flare * band * 0.45);
        float alpha = clamp(intensity * U.fade + (hash21(pos.xy) - 0.5) / 255.0, 0.0, 1.0);
        return float4(col * alpha, alpha);
    }
    """
}
