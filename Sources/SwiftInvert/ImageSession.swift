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
    private var basePreview: RGBImage?  // as decoded (EXIF baked, no user orientation)
    private var preview: RGBImage?  // user-oriented incl. fine rotation (render input)
    /// Orientation-only preview (90° steps + flip, NO fine rotation) — the
    /// analysis input. Metering must be invariant to straightening: the
    /// inscribed auto-crop changes with the angle, and re-metering it would
    /// drift the whole conversion as the user rotates.
    private var meterPreview: RGBImage?
    private struct MeterKey: Equatable {
        var rotation: Int
        var flipHorizontal: Bool
        /// 0 unless a manual analysis region exists — then the angle it was
        /// drawn at, so the meters read the content the user pointed at.
        var meterAngle: Double
    }
    private var meterKey: MeterKey?

    private static func meterAngle(_ settings: ExposureSettings) -> Double {
        settings.analysisRect != nil ? settings.analysisRectFineRotation : 0
    }

    /// Unrotated (orientation-only) frame dims — meterPreview can itself be
    /// rotated when an analysis region pins an angle, so derive from the
    /// decode dims + the 90° step instead.
    private func orientedFrameSize(_ settings: ExposureSettings) -> CGSize {
        let base = basePreview!
        let swapped = (((settings.rotation % 360) + 360) % 360) % 180 != 0
        return swapped
            ? CGSize(width: base.height, height: base.width)
            : CGSize(width: base.width, height: base.height)
    }
    private struct OrientKey: Equatable {
        var rotation: Int
        var flipHorizontal: Bool
        var fineRotation: Double
    }
    private var orientKey: OrientKey?

    /// Analysis cache: one tier. Since the 2125a34 port the whole analysis
    /// (neutral axis included) is offset-independent — wp/bp handle drags
    /// re-run NOTHING here; offsets fold into finalBounds at derive time.
    private struct PreparedKey: Equatable {
        var analysisRect: NormalizedRect?
        var cropRect: NormalizedRect?
    }
    private var prepared: ExposureKernel.Prepared?
    private var preparedKey: PreparedKey?
    private var analysis: ExposureAnalysis?

    /// GPU source textures, uploaded once per (image, crop) — slider changes
    /// only re-run the compute passes on the cached texture.
    private var fullTexture: MTLTexture?
    private var croppedTexture: MTLTexture?
    private var croppedTextureRect: NormalizedRect?

    /// HQ preview: the full-resolution decode cached like the proxy tower
    /// (analysis still runs on the proxy, so HQ shows exactly what export
    /// produces). Hundreds of MB per image — freed by the first non-HQ render.
    private var hqBase: RGBImage?
    private var hqOriented: RGBImage?
    private var hqOrientKey: OrientKey?
    private var hqFullTexture: MTLTexture?
    private var hqCroppedTexture: MTLTexture?
    private var hqCroppedRect: NormalizedRect?

    struct RenderOutput: Sendable {
        let image: CGImage
        let histogram: [UInt32]
        /// Unrotated (orientation-only) frame dimensions in pixels — the
        /// coordinate base for CropGeometry's rotated-space math.
        let frameSize: CGSize
        /// Pre-offset luminance density range of the negative (NegPy's
        /// `norm_density_range`) — the Negative-character diagnostic's input.
        /// Carried on the render because the analysis lives in the actor.
        let densityRange: Double
    }

    init(url: URL, pipeline: RenderPipeline) {
        self.url = url
        self.pipeline = pipeline
    }

    /// Decode + prepare once per rect state; finalize (neutral axis only) when
    /// the wp/bp offsets change — matching NegPy's meter scoping at a fraction
    /// of the cost.
    private func prepare(settings: ExposureSettings) throws -> (RGBImage, ExposureAnalysis) {
        if basePreview == nil {
            basePreview = try RawDecoder().decode(url: url, quality: .preview, maxLongEdge: 1536)
        }
        // Analysis ignores the CURRENT fine rotation (see meterPreview); 90°
        // steps and flips reshuffle pixels without changing the content set.
        // A manual analysis region pins the meter to the angle it was drawn at.
        let mKey = MeterKey(
            rotation: settings.rotation, flipHorizontal: settings.flipHorizontal,
            meterAngle: Self.meterAngle(settings))
        if meterPreview == nil || mKey != meterKey {
            meterPreview = basePreview!.oriented(
                rotationCW: settings.rotation, flipHorizontal: settings.flipHorizontal,
                fineRotation: mKey.meterAngle)
            meterKey = mKey
            prepared = nil
            preparedKey = nil
            analysis = nil
        }
        let oKey = OrientKey(
            rotation: settings.rotation, flipHorizontal: settings.flipHorizontal,
            fineRotation: settings.fineRotation)
        if preview == nil || oKey != orientKey {
            // COW: at 0° the render input IS the meter image, no copy.
            preview = abs(settings.fineRotation) > 0.005
                ? basePreview!.oriented(
                    rotationCW: settings.rotation, flipHorizontal: settings.flipHorizontal,
                    fineRotation: settings.fineRotation)
                : meterPreview
            orientKey = oKey
            // Textures are in (fine-)oriented space; analysis is not.
            fullTexture = nil
            croppedTexture = nil
            croppedTextureRect = nil
        }
        let pKey = PreparedKey(analysisRect: settings.analysisRect, cropRect: settings.cropRect)
        if prepared == nil || pKey != preparedKey {
            prepared = ExposureKernel.prepare(
                linearImage: meterPreview!,
                cropRect: settings.cropRect,
                analysisRect: settings.analysisRect)
            preparedKey = pKey
            analysis = nil
        }
        if analysis == nil {
            analysis = ExposureKernel.finalize(prepared!)
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

    /// The full-resolution source texture for HQ previews, cached with the
    /// same decode → orient → crop keying as the proxy path.
    private func hqSourceTexture(settings: ExposureSettings, uncropped: Bool) throws -> MTLTexture {
        if hqBase == nil {
            hqBase = try RawDecoder().decode(url: url, quality: .full)
        }
        let oKey = OrientKey(
            rotation: settings.rotation, flipHorizontal: settings.flipHorizontal,
            fineRotation: settings.fineRotation)
        if hqOriented == nil || oKey != hqOrientKey {
            hqOriented = hqBase!.oriented(
                rotationCW: settings.rotation, flipHorizontal: settings.flipHorizontal,
                fineRotation: settings.fineRotation)
            hqOrientKey = oKey
            hqFullTexture = nil
            hqCroppedTexture = nil
            hqCroppedRect = nil
        }
        if uncropped || settings.cropRect == nil {
            if hqFullTexture == nil { hqFullTexture = try pipeline.upload(hqOriented!) }
            return hqFullTexture!
        }
        let crop = settings.cropRect!
        if hqCroppedTexture == nil || hqCroppedRect != crop {
            hqCroppedTexture = try pipeline.upload(hqOriented!.cropped(to: crop))
            hqCroppedRect = crop
        }
        return hqCroppedTexture!
    }

    private func clearHQ() {
        hqBase = nil
        hqOriented = nil
        hqOrientKey = nil
        hqFullTexture = nil
        hqCroppedTexture = nil
        hqCroppedRect = nil
    }

    /// Drop the full-resolution tier while keeping the proxy tower warm.
    /// Called on sessions the LRU retains but isn't showing: the proxy caches
    /// are tens of MB (worth holding for an instant return), the HQ decode is
    /// hundreds (not worth holding for a frame nobody is looking at).
    func releaseHQ() {
        clearHQ()
    }

    /// True when the next render must decode or run the heavy prepared stage
    /// (drives the "Analyzing…" indicator; offset-only finalizes are fast and
    /// don't flash it).
    func needsPreparation(settings: ExposureSettings, hq: Bool = false) -> Bool {
        if hq {
            let oKey = OrientKey(
                rotation: settings.rotation, flipHorizontal: settings.flipHorizontal,
                fineRotation: settings.fineRotation)
            if hqBase == nil || hqOriented == nil || oKey != hqOrientKey { return true }
        }
        guard preview != nil, prepared != nil, meterPreview != nil else { return true }
        // Fine-rotation-only changes just re-orient (fast) — no meter re-run.
        if MeterKey(
            rotation: settings.rotation, flipHorizontal: settings.flipHorizontal,
            meterAngle: Self.meterAngle(settings)) != meterKey {
            return true
        }
        return PreparedKey(analysisRect: settings.analysisRect, cropRect: settings.cropRect) != preparedKey
    }

    /// Detached render for the straighten 0° base: same pipeline as render()
    /// but touching NONE of the cache tower (orientation/prepared/textures),
    /// so precomputing the base never evicts the committed orientation's
    /// caches — which would put ~150ms back on the next slider tick.
    /// The 5×5 test-strip mosaic: one analysis, one uploaded source, then a
    /// derive+render per patch (density/grade never touch the analysis, so
    /// the warm cache tower is reused — ~25 × the slider-tick cost). Always
    /// the preview proxy, never HQ (upstream rule: 25 full-res renders would
    /// take ages and each patch shows at a fifth of the frame's width).
    /// Assembled incrementally so only mosaic + one tile are ever held.
    func renderTestStrip(settings: ExposureSettings, orientation: Int) throws -> CGImage {
        let (image, analysis) = try prepare(settings: settings)
        let source = try sourceTexture(image: image, settings: settings, uncropped: false)
        var mosaic: [UInt8] = []
        var w = 0, h = 0
        for cell in TestStrip.cells(orientation: orientation) {
            var s = settings
            s.density = cell.density
            s.grade = cell.grade
            let params = ExposureKernel.deriveRenderParams(s, analysis)
            let result = try pipeline.renderDisplay(
                source: source, params: params, computeHistogram: false)
            if mosaic.isEmpty {
                w = result.width
                h = result.height
                mosaic = [UInt8](repeating: 0, count: result.rgba.count)
            }
            TestStrip.copyPatch(
                tile: result.rgba, into: &mosaic, width: w, height: h,
                bytesPerRow: w * 4, row: cell.row, col: cell.col)
        }
        guard let cg = ImageConversion.cgImage(rgba8: mosaic, width: w, height: h) else {
            throw RenderError.resource("test strip CGImage")
        }
        return cg
    }

    // MARK: - Zone placement (NegPy 5a095f3/9dff124)

    /// Normalized-log sample at content-normalized `u`,`v` of the DISPLAYED
    /// frame (the fine-rotated preview under the committed crop): mean of the
    /// (2·radius+1)² log-normalized patch, like upstream's
    /// `_sample_normalized_log`. This is the pre-curve value a zone pin
    /// freezes — the displayed rgba8 bytes are post-curve and can't serve.
    func sampleZonePin(
        settings: ExposureSettings, u: Double, v: Double, radius: Int = 2
    ) throws -> (valRGB: SIMD3<Double>, valLuma: Double) {
        let (image, analysis) = try prepare(settings: settings)
        let bounds = analysis.baseBounds.applyingOffsets(
            whitePoint: settings.whitePointOffset, blackPoint: settings.blackPointOffset)
        // Map through the crop: the displayed frame is preview ∩ cropRect.
        var cu = u, cv = v
        if let crop = settings.cropRect {
            cu = crop.x + u * crop.width
            cv = crop.y + v * crop.height
        }
        let cx = min(max(Int(cu * Double(image.width)), 0), image.width - 1)
        let cy = min(max(Int(cv * Double(image.height)), 0), image.height - 1)
        var sum = SIMD3<Double>()
        var n = 0.0
        for y in max(cy - radius, 0)...min(cy + radius, image.height - 1) {
            for x in max(cx - radius, 0)...min(cx + radius, image.width - 1) {
                for ch in 0..<3 {
                    let lg = log10(max(Double(image[y, x, ch]), 1e-6))
                    let denom = bounds.ceils[ch] - bounds.floors[ch]
                    sum[ch] += (lg - bounds.floors[ch]) / (abs(denom) < 1e-6 ? 1e-6 : denom)
                }
                n += 1
            }
        }
        let val = sum / n
        return (val, K.lumaR * val.x + K.lumaG * val.y + K.lumaB * val.z)
    }

    /// Zone the CURRENT settings print `valLuma` on (pin labels + the default
    /// target of an unarmed pin).
    func predictedZone(settings: ExposureSettings, valLuma: Double) throws -> Double {
        let (_, analysis) = try prepare(settings: settings)
        return ZonePlacement.predictedZone(settings: settings, analysis: analysis, valLuma: valLuma)
    }

    /// Solve the placement against the warm analysis (pure CPU, in the actor
    /// so the bisections stay off the main thread).
    func solveZonePlacement(
        settings: ExposureSettings, pins: [ZonePlacement.Pin]
    ) throws -> ZonePlacement.Solution? {
        let (_, analysis) = try prepare(settings: settings)
        return ZonePlacement.solve(settings: settings, analysis: analysis, pins: pins)
    }

    func renderDetached(settings: ExposureSettings) throws -> RenderOutput {
        if basePreview == nil {
            basePreview = try RawDecoder().decode(url: url, quality: .preview, maxLongEdge: 1536)
        }
        // Analysis is fine-rotation-independent by design, so when the
        // cached analysis's keys match these settings (the common case:
        // the straighten 0°-base differs from the last render only in
        // fineRotation), reuse it read-only — byte-identical, and it turns
        // the ~175 ms background prepare into zero. Cache misses fall back
        // to a fresh detached computation that touches no cache state.
        let mKey = MeterKey(
            rotation: settings.rotation, flipHorizontal: settings.flipHorizontal,
            meterAngle: Self.meterAngle(settings))
        let pKey = PreparedKey(analysisRect: settings.analysisRect, cropRect: settings.cropRect)
        let analysis: ExposureAnalysis
        if mKey == meterKey, pKey == preparedKey, let cached = self.analysis {
            analysis = cached
        } else {
            let meterImage = basePreview!.oriented(
                rotationCW: settings.rotation, flipHorizontal: settings.flipHorizontal,
                fineRotation: mKey.meterAngle)
            let prepared = ExposureKernel.prepare(
                linearImage: meterImage, cropRect: settings.cropRect,
                analysisRect: settings.analysisRect)
            analysis = ExposureKernel.finalize(prepared)
        }
        let oriented = basePreview!.oriented(
            rotationCW: settings.rotation, flipHorizontal: settings.flipHorizontal,
            fineRotation: settings.fineRotation)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let image = settings.cropRect.map { oriented.cropped(to: $0) } ?? oriented
        let source = try pipeline.upload(image)
        let result = try pipeline.renderDisplay(source: source, params: params)
        guard let cg = ImageConversion.cgImage(rgba8: result.rgba, width: result.width, height: result.height)
        else { throw RenderError.resource("CGImage conversion") }
        return RenderOutput(
            image: cg, histogram: result.histogram, frameSize: orientedFrameSize(settings),
            densityRange: analysis.baseBounds.luminanceDensityRange)
    }

    /// `uncropped` shows the full frame (used while a selection tool is active
    /// so the user can drag on the whole image, like NegPy's crop_preview_full).
    func render(settings: ExposureSettings, uncropped: Bool = false, hq: Bool = false) throws -> RenderOutput {
        let (image, analysis) = try prepare(settings: settings)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let source: MTLTexture
        if hq {
            source = try hqSourceTexture(settings: settings, uncropped: uncropped)
        } else {
            clearHQ()
            source = try sourceTexture(image: image, settings: settings, uncropped: uncropped)
        }
        let result = try pipeline.renderDisplay(source: source, params: params)
        guard let cg = ImageConversion.cgImage(rgba8: result.rgba, width: result.width, height: result.height)
        else { throw RenderError.resource("CGImage conversion") }
        return RenderOutput(
            image: cg, histogram: result.histogram, frameSize: orientedFrameSize(settings),
            densityRange: analysis.baseBounds.luminanceDensityRange)
    }

    /// Full-resolution export render (fresh decode, same analysis, crop applied).
    func exportRender(settings: ExposureSettings) throws -> RGBImage {
        let (_, analysis) = try prepare(settings: settings)
        var full = try RawDecoder().decode(url: url, quality: .full)
            .oriented(
                rotationCW: settings.rotation, flipHorizontal: settings.flipHorizontal,
                fineRotation: settings.fineRotation)
        if let crop = settings.cropRect { full = full.cropped(to: crop) }
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let encoded = try pipeline.render(image: full, params: params, computeHistogram: false).encoded
        return encoded
    }
}
