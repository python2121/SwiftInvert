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

    /// Everything the analysis depends on besides the pixels; a change here
    /// re-runs it (the "automatically re-run the base analysis" contract).
    private struct AnalysisKey: Equatable {
        var analysisRect: NormalizedRect?
        var cropRect: NormalizedRect?
        var whitePoint: Double
        var blackPoint: Double
    }
    private var analysisKey: AnalysisKey?

    struct RenderOutput: Sendable {
        let image: CGImage
        let histogram: [UInt32]
    }

    init(url: URL, pipeline: RenderPipeline) {
        self.url = url
        self.pipeline = pipeline
    }

    /// Decode + analyze once; re-analyze when the wp/bp offsets or the
    /// pre-process rects change (the meters are scoped by both, matching NegPy).
    private func prepare(settings: ExposureSettings) throws -> (RGBImage, ExposureAnalysis) {
        if preview == nil {
            preview = try RawDecoder().decode(url: url, quality: .preview, maxLongEdge: 1536)
        }
        let key = AnalysisKey(
            analysisRect: settings.analysisRect, cropRect: settings.cropRect,
            whitePoint: settings.whitePointOffset, blackPoint: settings.blackPointOffset)
        if analysis == nil || key != analysisKey {
            analysis = ExposureKernel.analyze(
                linearImage: preview!,
                cropRect: settings.cropRect,
                analysisRect: settings.analysisRect,
                whitePointOffset: settings.whitePointOffset,
                blackPointOffset: settings.blackPointOffset)
            analysisKey = key
        }
        return (preview!, analysis!)
    }

    /// `uncropped` shows the full frame (used while a selection tool is active
    /// so the user can drag on the whole image, like NegPy's crop_preview_full).
    func render(settings: ExposureSettings, uncropped: Bool = false) throws -> RenderOutput {
        let (image, analysis) = try prepare(settings: settings)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        var renderImage = image
        if !uncropped, let crop = settings.cropRect {
            renderImage = image.cropped(to: crop)
        }
        let (encoded, histogram) = try pipeline.render(image: renderImage, params: params)
        guard let cg = ImageConversion.cgImage(fromEncoded: encoded) else {
            throw RenderError.resource("CGImage conversion")
        }
        return RenderOutput(image: cg, histogram: histogram)
    }

    /// Full-resolution export render (fresh decode, same analysis, crop applied).
    func exportRender(settings: ExposureSettings) throws -> RGBImage {
        let (_, analysis) = try prepare(settings: settings)
        var full = try RawDecoder().decode(url: url, quality: .full)
        if let crop = settings.cropRect { full = full.cropped(to: crop) }
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let (encoded, _) = try pipeline.render(image: full, params: params)
        return encoded
    }
}
