import CoreGraphics
import Foundation
import MetalRenderKit
import NegativeKit
import RawDecodeKit

/// Per-image state: preview decode, one-time analysis, and settings → render.
/// Sliders call `update(settings:)`; only the microsecond parameter derivation
/// and the GPU passes re-run — never the decode or the analysis.
actor ImageSession {
    let url: URL
    private let pipeline: RenderPipeline
    private var preview: RGBImage?
    private var analysis: ExposureAnalysis?
    private var offsetsUsedForAnalysis: (wp: Double, bp: Double) = (0, 0)

    struct RenderOutput: Sendable {
        let image: CGImage
        let histogram: [UInt32]
    }

    init(url: URL, pipeline: RenderPipeline) {
        self.url = url
        self.pipeline = pipeline
    }

    /// Decode + analyze once (or re-analyze when wp/bp offsets changed — the
    /// neutral axis is measured against offset bounds, matching NegPy).
    private func prepare(settings: ExposureSettings) throws -> (RGBImage, ExposureAnalysis) {
        if preview == nil {
            preview = try RawDecoder().decode(url: url, quality: .preview, maxLongEdge: 1536)
        }
        let offsets = (settings.whitePointOffset, settings.blackPointOffset)
        if analysis == nil || offsets != offsetsUsedForAnalysis {
            analysis = ExposureKernel.analyze(
                linearImage: preview!,
                whitePointOffset: offsets.0,
                blackPointOffset: offsets.1)
            offsetsUsedForAnalysis = offsets
        }
        return (preview!, analysis!)
    }

    func render(settings: ExposureSettings) throws -> RenderOutput {
        let (image, analysis) = try prepare(settings: settings)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let (encoded, histogram) = try pipeline.render(image: image, params: params)
        guard let cg = ImageConversion.cgImage(fromEncoded: encoded) else {
            throw RenderError.resource("CGImage conversion")
        }
        return RenderOutput(image: cg, histogram: histogram)
    }

    /// Full-resolution export render (fresh decode, same analysis).
    func exportRender(settings: ExposureSettings) throws -> RGBImage {
        let (_, analysis) = try prepare(settings: settings)
        let full = try RawDecoder().decode(url: url, quality: .full)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let (encoded, _) = try pipeline.render(image: full, params: params)
        return encoded
    }
}
