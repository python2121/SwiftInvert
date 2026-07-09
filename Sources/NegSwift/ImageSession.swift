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

    /// Two-tier analysis cache: the expensive prepared stage depends only on
    /// the pre-process rects; the cheap finalize (neutral axis) also depends on
    /// the white/black-point offsets — so wp/bp drags never re-run the grid.
    private struct PreparedKey: Equatable {
        var analysisRect: NormalizedRect?
        var cropRect: NormalizedRect?
    }
    private struct AnalysisKey: Equatable {
        var prepared: PreparedKey
        var whitePoint: Double
        var blackPoint: Double
    }
    private var prepared: ExposureKernel.Prepared?
    private var preparedKey: PreparedKey?
    private var analysis: ExposureAnalysis?
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

    /// Decode + prepare once per rect state; finalize (neutral axis only) when
    /// the wp/bp offsets change — matching NegPy's meter scoping at a fraction
    /// of the cost.
    private func prepare(settings: ExposureSettings) throws -> (RGBImage, ExposureAnalysis) {
        if preview == nil {
            preview = try RawDecoder().decode(url: url, quality: .preview, maxLongEdge: 1536)
        }
        let pKey = PreparedKey(analysisRect: settings.analysisRect, cropRect: settings.cropRect)
        if prepared == nil || pKey != preparedKey {
            prepared = ExposureKernel.prepare(
                linearImage: preview!,
                cropRect: settings.cropRect,
                analysisRect: settings.analysisRect)
            preparedKey = pKey
            analysis = nil
        }
        let aKey = AnalysisKey(
            prepared: pKey,
            whitePoint: settings.whitePointOffset, blackPoint: settings.blackPointOffset)
        if analysis == nil || aKey != analysisKey {
            analysis = ExposureKernel.finalize(
                prepared!,
                whitePointOffset: settings.whitePointOffset,
                blackPointOffset: settings.blackPointOffset)
            analysisKey = aKey
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

    /// True when the next render must decode or run the heavy prepared stage
    /// (drives the "Analyzing…" indicator; offset-only finalizes are fast and
    /// don't flash it).
    func needsPreparation(settings: ExposureSettings) -> Bool {
        guard preview != nil, prepared != nil else { return true }
        return PreparedKey(analysisRect: settings.analysisRect, cropRect: settings.cropRect) != preparedKey
    }

    /// `uncropped` shows the full frame (used while a selection tool is active
    /// so the user can drag on the whole image, like NegPy's crop_preview_full).
    func render(settings: ExposureSettings, uncropped: Bool = false) throws -> RenderOutput {
        let (image, analysis) = try prepare(settings: settings)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let source = try sourceTexture(image: image, settings: settings, uncropped: uncropped)
        let result = try pipeline.renderDisplay(source: source, params: params)
        guard let cg = ImageConversion.cgImage(rgba8: result.rgba, width: result.width, height: result.height)
        else { throw RenderError.resource("CGImage conversion") }
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
