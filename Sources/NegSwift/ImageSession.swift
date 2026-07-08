import CoreGraphics
import Foundation
import Metal
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

    /// GPU source textures, uploaded once per (image, crop) — slider changes
    /// only re-run the compute passes on the cached texture.
    private var fullTexture: MTLTexture?
    private var croppedTexture: MTLTexture?
    private var croppedTextureRect: NormalizedRect?

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

    /// The cached GPU texture for this frame's current crop state.
    private func sourceTexture(image: RGBImage, settings: ExposureSettings, uncropped: Bool) throws -> MTLTexture {
        if uncropped || settings.cropRect == nil {
            if fullTexture == nil { fullTexture = try pipeline.upload(image) }
            return fullTexture!
        }
        let crop = settings.cropRect!
        if croppedTexture == nil || croppedTextureRect != crop {
            croppedTexture = try pipeline.upload(image.cropped(to: crop))
            croppedTextureRect = crop
        }
        return croppedTexture!
    }

    /// `uncropped` shows the full frame (used while a selection tool is active
    /// so the user can drag on the whole image, like NegPy's crop_preview_full).
    func render(settings: ExposureSettings, uncropped: Bool = false) throws -> RenderOutput {
        let (image, analysis) = try prepare(settings: settings)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let source = try sourceTexture(image: image, settings: settings, uncropped: uncropped)
        let result = try pipeline.render(source: source, params: params)
        let encoded = pipeline.readback(result.encoded)
        guard let cg = ImageConversion.cgImage(fromEncoded: encoded) else {
            throw RenderError.resource("CGImage conversion")
        }
        return RenderOutput(image: cg, histogram: result.histogram)
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
