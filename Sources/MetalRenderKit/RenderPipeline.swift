import Foundation
import Metal
import NegativeKit

public enum RenderError: Error, CustomStringConvertible {
    case noDevice
    case shaderCompile(String)
    case function(String)
    case resource(String)

    public var description: String {
        switch self {
        case .noDevice: return "No Metal device available"
        case .shaderCompile(let m): return "Metal shader compilation failed: \(m)"
        case .function(let n): return "Missing Metal function \(n)"
        case .resource(let m): return "Metal resource allocation failed: \(m)"
        }
    }
}

/// The 3-pass render chain (normalize → print curve → encode) + histogram,
/// mirroring NegPy's GPU pipeline for the C-41 path. Shaders are compiled at
/// runtime from the bundled source (no build-time metal compiler under CLT).
/// Thread-safety: Metal objects are internally synchronized; NegSwift serializes
/// renders through one session actor anyway.
public final class RenderPipeline: @unchecked Sendable {
    public let device: MTLDevice
    let queue: MTLCommandQueue
    let normalizePSO: MTLComputePipelineState
    let curvePSO: MTLComputePipelineState
    let encodePSO: MTLComputePipelineState
    let histogramPSO: MTLComputePipelineState

    /// Intermediate textures reused across renders of the same size (slider
    /// drags re-render constantly; allocation was the dominant per-frame cost).
    /// Export-size textures (>4 MP ≈ 3×64 MB+) are not retained.
    private struct SizeKey: Hashable {
        let w: Int
        let h: Int
    }
    private var intermediates: [SizeKey: (normalized: MTLTexture, linear: MTLTexture, encoded: MTLTexture)] = [:]
    private var histBuffer: MTLBuffer?
    private let maxCachedPixels = 4_000_000
    /// Serializes render(): the cached intermediates and histogram buffer are
    /// shared mutable state (the app renders from one actor, but tests run
    /// concurrently against one pipeline).
    private let renderLock = NSLock()

    public init() throws {
        guard let device = MTLCreateSystemDefaultDevice() else { throw RenderError.noDevice }
        self.device = device
        guard let queue = device.makeCommandQueue() else { throw RenderError.resource("command queue") }
        self.queue = queue

        guard
            let url = Bundle.module.url(forResource: "NegPipeline", withExtension: "metal", subdirectory: "Shaders")
                ?? Bundle.module.url(forResource: "NegPipeline", withExtension: "metal"),
            let source = try? String(contentsOf: url, encoding: .utf8)
        else { throw RenderError.resource("NegPipeline.metal not found in bundle") }

        let library: MTLLibrary
        do {
            library = try device.makeLibrary(source: source, options: MTLCompileOptions())
        } catch {
            throw RenderError.shaderCompile("\(error)")
        }
        func pso(_ name: String) throws -> MTLComputePipelineState {
            guard let fn = library.makeFunction(name: name) else { throw RenderError.function(name) }
            return try device.makeComputePipelineState(function: fn)
        }
        normalizePSO = try pso("normalizeLog")
        curvePSO = try pso("printCurve")
        encodePSO = try pso("outputEncode")
        histogramPSO = try pso("histogram256")
    }

    // MARK: - Textures

    public func makeTexture(width: Int, height: Int, usage: MTLTextureUsage = [.shaderRead, .shaderWrite]) throws -> MTLTexture {
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .rgba32Float, width: width, height: height, mipmapped: false)
        desc.usage = usage
        desc.storageMode = .shared
        guard let tex = device.makeTexture(descriptor: desc) else {
            throw RenderError.resource("texture \(width)x\(height)")
        }
        return tex
    }

    /// Upload an interleaved RGB float image as rgba32float.
    public func upload(_ image: RGBImage) throws -> MTLTexture {
        let tex = try makeTexture(width: image.width, height: image.height, usage: [.shaderRead])
        var rgba = [Float](repeating: 1, count: image.width * image.height * 4)
        image.pixels.withUnsafeBufferPointer { src in
            rgba.withUnsafeMutableBufferPointer { dst in
                for i in 0..<(image.width * image.height) {
                    dst[i * 4] = src[i * 3]
                    dst[i * 4 + 1] = src[i * 3 + 1]
                    dst[i * 4 + 2] = src[i * 3 + 2]
                }
            }
        }
        rgba.withUnsafeBufferPointer { buf in
            tex.replace(
                region: MTLRegionMake2D(0, 0, image.width, image.height), mipmapLevel: 0,
                withBytes: buf.baseAddress!, bytesPerRow: image.width * 16)
        }
        return tex
    }

    public func readback(_ texture: MTLTexture) -> RGBImage {
        let w = texture.width, h = texture.height
        var rgba = [Float](repeating: 0, count: w * h * 4)
        rgba.withUnsafeMutableBufferPointer { buf in
            texture.getBytes(
                buf.baseAddress!, bytesPerRow: w * 16, from: MTLRegionMake2D(0, 0, w, h), mipmapLevel: 0)
        }
        var out = RGBImage(width: w, height: h)
        out.pixels.withUnsafeMutableBufferPointer { dst in
            rgba.withUnsafeBufferPointer { src in
                for i in 0..<(w * h) {
                    dst[i * 3] = src[i * 4]
                    dst[i * 3 + 1] = src[i * 4 + 1]
                    dst[i * 3 + 2] = src[i * 4 + 2]
                }
            }
        }
        return out
    }

    // MARK: - Encoding

    private func dispatch(
        _ encoder: MTLComputeCommandEncoder, _ pso: MTLComputePipelineState, width: Int, height: Int
    ) {
        encoder.setComputePipelineState(pso)
        let tg = MTLSizeMake(8, 8, 1)
        let grid = MTLSizeMake((width + 7) / 8, (height + 7) / 8, 1)
        encoder.dispatchThreadgroups(grid, threadsPerThreadgroup: tg)
    }

    /// One render's read-back results: the encoded output (display/export), the
    /// linear print when requested (parity tests), and the 4×256 histogram.
    public struct Result: Sendable {
        public let encoded: RGBImage
        public let linear: RGBImage?
        public let histogram: [UInt32]  // R,G,B,Luma × 256
    }

    private func intermediatesFor(width w: Int, height h: Int) throws
        -> (normalized: MTLTexture, linear: MTLTexture, encoded: MTLTexture)
    {
        let key = SizeKey(w: w, h: h)
        if let cached = intermediates[key] { return cached }
        let set = (
            normalized: try makeTexture(width: w, height: h),
            linear: try makeTexture(width: w, height: h),
            encoded: try makeTexture(width: w, height: h)
        )
        if w * h <= maxCachedPixels {
            if intermediates.count >= 4 { intermediates.removeAll() }  // crop-size churn cap
            intermediates[key] = set
        }
        return set
    }

    /// Full chain on one command buffer: normalize → curve → [histogram] → encode.
    /// Serialized by `renderLock` and returns read-back buffers: the reused
    /// intermediate textures must never escape to a caller (a subsequent render
    /// would overwrite them — concurrent test runs segfaulted on exactly that).
    public func render(
        source: MTLTexture, params: RenderParams, computeHistogram: Bool = true, wantLinear: Bool = false
    ) throws -> Result {
        renderLock.lock()
        defer { renderLock.unlock() }
        let w = source.width, h = source.height
        let (normalized, linear, encoded) = try intermediatesFor(width: w, height: h)

        var normU = UniformsBuilder.normUniforms(params)
        var curveU = UniformsBuilder.curveUniforms(params)
        if histBuffer == nil {
            histBuffer = device.makeBuffer(length: 1024 * 4, options: .storageModeShared)
        }
        guard let histBuffer else { throw RenderError.resource("histogram buffer") }
        memset(histBuffer.contents(), 0, 1024 * 4)

        guard let cmd = queue.makeCommandBuffer(), let enc = cmd.makeComputeCommandEncoder() else {
            throw RenderError.resource("command buffer")
        }
        enc.setTexture(source, index: 0)
        enc.setTexture(normalized, index: 1)
        enc.setBytes(&normU, length: MemoryLayout<NormUniforms>.stride, index: 0)
        dispatch(enc, normalizePSO, width: w, height: h)

        enc.setTexture(normalized, index: 0)
        enc.setTexture(linear, index: 1)
        enc.setBytes(&curveU, length: MemoryLayout<CurveUniforms>.stride, index: 0)
        dispatch(enc, curvePSO, width: w, height: h)

        if computeHistogram {
            enc.setTexture(linear, index: 0)
            enc.setBuffer(histBuffer, offset: 0, index: 0)
            enc.setComputePipelineState(histogramPSO)
            let tg = MTLSizeMake(16, 16, 1)
            let grid = MTLSizeMake((w + 15) / 16, (h + 15) / 16, 1)
            enc.dispatchThreadgroups(grid, threadsPerThreadgroup: tg)
        }

        enc.setTexture(linear, index: 0)
        enc.setTexture(encoded, index: 1)
        dispatch(enc, encodePSO, width: w, height: h)
        enc.endEncoding()

        cmd.commit()
        cmd.waitUntilCompleted()
        if let error = cmd.error { throw RenderError.resource("command buffer failed: \(error)") }

        let histogram = histBuffer.contents().withMemoryRebound(to: UInt32.self, capacity: 1024) {
            Array(UnsafeBufferPointer(start: $0, count: 1024))
        }
        return Result(
            encoded: readback(encoded),
            linear: wantLinear ? readback(linear) : nil,
            histogram: histogram)
    }

    /// Convenience: full CPU-image → CPU-image render.
    public func render(image: RGBImage, params: RenderParams) throws -> (encoded: RGBImage, histogram: [UInt32]) {
        let source = try upload(image)
        let result = try render(source: source, params: params)
        return (result.encoded, result.histogram)
    }
}
