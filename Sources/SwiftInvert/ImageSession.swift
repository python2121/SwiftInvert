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

    /// Whether the render input may simply ALIAS `meterPreview` instead of
    /// being oriented out of `basePreview` again — the COW that makes the
    /// common case cost no copy at all.
    ///
    /// It is not enough to ask whether the straighten angle is zero.
    /// `meterPreview` is built at `meterAngle`, which is the angle an analysis
    /// region was DRAWN at, not the angle the image is currently straightened
    /// to — the two are pinned apart on purpose so the meters keep reading the
    /// content the user pointed at. Aliasing on `fineRotation` alone therefore
    /// hands the renderer the region's angle whenever the two disagree:
    /// straighten, draw a region, then return the slider to 0 (or clear crop &
    /// straighten, which zeroes the angle and keeps the region) and the canvas
    /// would show a picture tilted and inscribed at an angle every control
    /// reads as zero — laid out, via `displayAspect`/`contentWindow`, for the
    /// untilted frame it isn't.
    ///
    /// So BOTH angles have to be flat. The threshold mirrors
    /// `RGBImage.oriented`, which is what actually decides whether a fine
    /// rotation gets applied.
    static func aliasesMeterPreview(fineRotation: Double, meterAngle: Double) -> Bool {
        abs(fineRotation) <= 0.005 && abs(meterAngle) <= 0.005
    }

    /// Where the produced bitmap actually sits inside the IDEAL display
    /// rectangle, in units of that rectangle — nominally (0, 0, 1, 1), and off
    /// it by well under a pixel.
    ///
    /// Two stages quantize, and they quantize differently per tier:
    /// `fineRotated` FLOORS the inscribed rect (so the bitmap covers slightly
    /// less than the ideal, centered), and the crop `pixelROI` floors onto that
    /// bitmap's grid. Anchoring the HQ window to the proxy's got the two tiers
    /// to within half a full-resolution pixel of each other, but half a pixel
    /// is still ~1 pt of visible slide at 4× zoom — pixel-aligned cropping
    /// cannot do better. So stop pretending each bitmap fills the ideal rect:
    /// report where it really lands and let the canvas place it there. Both
    /// tiers then put the same content at the same point to float precision.
    private func contentWindow(
        _ settings: ExposureSettings, uncropped: Bool, tierBase: RGBImage, oriented: RGBImage,
        roi: (x0: Int, y0: Int, x1: Int, y1: Int)?
    ) -> NormalizedRect {
        let base = orientedFrameSize(of: tierBase, settings)
        let radians = abs(settings.fineRotation) > 0.005 ? settings.fineRotation * .pi / 180 : 0
        return CropGeometry.contentWindow(
            frame: SIMD2(Double(base.width), Double(base.height)),
            radians: radians,
            crop: uncropped ? nil : settings.cropRect,
            orientedPixels: SIMD2(Double(oriented.width), Double(oriented.height)),
            roi: roi.map { SIMD4(Double($0.x0), Double($0.y0), Double($0.x1), Double($0.y1)) })
    }

    /// The displayed rectangle's width÷height in CONTINUOUS space: the
    /// orientation-only frame, through the straighten inscribed rect, through
    /// the crop — none of it rounded to a pixel grid. `orientedFrameSize` reads
    /// the proxy's dims in both tiers, so this is identical for the proxy and
    /// the full-resolution render by construction. See `RenderOutput`.
    private func displayAspect(_ settings: ExposureSettings, uncropped: Bool) -> Double {
        let base = orientedFrameSize(settings)
        // `oriented` applies the fine rotation only past this threshold — the
        // conditions here must mirror it and `sourceTexture` exactly, or the
        // layout would describe a rectangle the render didn't produce.
        let radians = abs(settings.fineRotation) > 0.005 ? settings.fineRotation * .pi / 180 : 0
        return CropGeometry.displayAspect(
            frame: SIMD2(Double(base.width), Double(base.height)),
            radians: radians,
            crop: uncropped ? nil : settings.cropRect)
    }

    /// Unrotated (orientation-only) frame dims — meterPreview can itself be
    /// rotated when an analysis region pins an angle, so derive from the
    /// decode dims + the 90° step instead.
    private func orientedFrameSize(_ settings: ExposureSettings) -> CGSize {
        orientedFrameSize(of: basePreview!, settings)
    }

    /// Same, for an explicit base — `contentWindow` must measure a tier's
    /// bitmap against ITS OWN frame. Measuring the full-resolution bitmap
    /// against the proxy's frame yields a window ~3.9x the ideal rect, not the
    /// fraction-of-a-pixel correction it is meant to be.
    private func orientedFrameSize(of base: RGBImage, _ settings: ExposureSettings) -> CGSize {
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

    /// Which source a display render should read from. Analysis ALWAYS runs on
    /// the 1536px proxy whatever this says — the tiers differ only in pixels on
    /// screen, so switching between them can never move the conversion.
    enum RenderTier: Sendable {
        /// The 1536px analysis proxy.
        case proxy
        /// The best tier that costs no decode: the retained half-size buffer
        /// when this body has one, else `full`.
        case mediumIfFree
        /// True full resolution with the export demosaic — what export produces.
        case full
    }

    /// A display tier above the analysis proxy: decoded base → oriented →
    /// GPU textures for the current crop, keyed exactly like the proxy tower.
    /// One instance per tier, so adding the medium tier didn't mean a third
    /// copy of this bookkeeping.
    private struct DisplayTier {
        var base: RGBImage?
        var oriented: RGBImage?
        var orientKey: OrientKey?
        var fullTexture: MTLTexture?
        var croppedTexture: MTLTexture?
        var croppedRect: NormalizedRect?

        /// Drop everything derived from the orientation (textures live in
        /// fine-oriented space) while keeping the decoded base.
        mutating func invalidateOrientation() {
            oriented = nil
            orientKey = nil
            fullTexture = nil
            croppedTexture = nil
            croppedRect = nil
        }

        mutating func clear() {
            base = nil
            invalidateOrientation()
        }
    }

    /// The half-size buffer the PREVIEW decode already produced and used to
    /// throw away: 3012×2010 on a 24MP Bayer body, i.e. 4× the proxy's pixels
    /// for 0 ms of extra work and ~73 MB. It is the same linear half-size
    /// demosaic as the proxy — more resolution, not better colour — so it is
    /// not export-truth; `.full` remains that.
    private var medium = DisplayTier()

    /// Full-resolution decode with the export demosaic. Hundreds of MB, and it
    /// costs a ~500–700 ms decode the first time.
    private var full = DisplayTier()

    /// Above this the retained half-size buffer costs more than it is worth
    /// (12 bytes/px): 20 MP ≈ 240 MB. A 24 MP Bayer body's half-size decode is
    /// 6 MP and a 61 MP body's is ~15 MP, so both qualify — but an X-Trans
    /// preview decodes FULL size (half_size aliases the 6×6 CFA), and 24 MP of
    /// that does not. Those bodies simply get no medium tier and `.mediumIfFree`
    /// falls through to `.full`, which is exactly today's behaviour.
    private static let mediumTierPixelBudget = 20_000_000

    struct RenderOutput: Sendable {
        let image: CGImage
        let histogram: [UInt32]
        /// Unrotated (orientation-only) frame dimensions in pixels — the
        /// coordinate base for CropGeometry's rotated-space math.
        let frameSize: CGSize
        /// Width÷height the canvas should LAY OUT at, computed in continuous
        /// space rather than read off `image.width/height`.
        ///
        /// The bitmap's own pixel ratio is resolution-dependent: `pixelROI`
        /// truncates the crop to whole pixels and `fineRotated` floors the
        /// inscribed rect, so the 1536px proxy and the full-resolution tier
        /// land on aspects that differ by up to ~0.2%. Laying out from the
        /// bitmap made the picture jump when HQ swapped in — and `scaleEffect`
        /// multiplies that error by the zoom, which is exactly when the swap
        /// happens. Both tiers describe the same continuous rectangle, so lay
        /// out from THAT and let the bitmap fill it (the ≤0.2% difference
        /// between a tier's true ratio and the ideal is sub-pixel).
        let displayAspect: Double
        /// Where this bitmap actually lands inside the ideal display rect, in
        /// units of that rect (nominally (0,0,1,1)). The canvas places the image
        /// by THIS rather than stretching it to fill, which is what makes the
        /// proxy and full-resolution tiers put identical content at identical
        /// screen positions. See `ImageSession.contentWindow`.
        let contentWindow: NormalizedRect
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
            // Decode at the sensor's native preview size and downsample HERE,
            // rather than asking the decoder to cap it, so the buffer it was
            // already producing can be kept as the medium tier. `basePreview`
            // is still a single downsample from that same buffer, so the
            // analysis input is byte-identical to capping inside the decoder.
            let native = try RawDecoder().decode(url: url, quality: .preview)
            basePreview = native.downsampled(maxLongEdge: 1536)
            if native.width * native.height <= Self.mediumTierPixelBudget,
                native.width > basePreview!.width
            {
                medium.base = native
            }
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
            // COW where both angles are flat (see aliasesMeterPreview) — the
            // meter image is only the render input when it was itself built
            // unrotated, which a pinned analysis region breaks.
            preview = Self.aliasesMeterPreview(
                fineRotation: settings.fineRotation, meterAngle: mKey.meterAngle)
                ? meterPreview
                : basePreview!.oriented(
                    rotationCW: settings.rotation, flipHorizontal: settings.flipHorizontal,
                    fineRotation: settings.fineRotation)
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

    /// The cached GPU texture for this frame's current crop state, plus the
    /// pixel window it covers (nil = the whole oriented image) — `contentWindow`
    /// needs the window that was actually used, not the one that was asked for.
    private func sourceTexture(image: RGBImage, settings: ExposureSettings, uncropped: Bool) throws
        -> (texture: MTLTexture, roi: (x0: Int, y0: Int, x1: Int, y1: Int)?)
    {
        if uncropped || settings.cropRect == nil {
            if fullTexture == nil { fullTexture = try pipeline.upload(image) }
            return (fullTexture!, nil)
        }
        let crop = settings.cropRect!
        let roi = crop.pixelROI(width: image.width, height: image.height)
        if croppedTexture == nil || croppedTextureRect != crop {
            croppedTexture = try pipeline.upload(image.cropped(to: crop))
            croppedTextureRect = crop
        }
        return (croppedTexture!, roi)
    }

    /// Source texture for a display tier, with the same orient → crop keying as
    /// the proxy path. `tier.base` must already be decoded.
    ///
    /// Crops per tier without trying to match the proxy's pixel window:
    /// `contentWindow` reports where each bitmap really lands and the canvas
    /// places it accordingly, which corrects the difference exactly.
    private func tierSource(
        _ tier: inout DisplayTier, settings: ExposureSettings, uncropped: Bool
    ) throws -> (texture: MTLTexture, roi: (x0: Int, y0: Int, x1: Int, y1: Int)?, oriented: RGBImage) {
        let oKey = OrientKey(
            rotation: settings.rotation, flipHorizontal: settings.flipHorizontal,
            fineRotation: settings.fineRotation)
        if tier.oriented == nil || oKey != tier.orientKey {
            tier.invalidateOrientation()
            tier.oriented = tier.base!.oriented(
                rotationCW: settings.rotation, flipHorizontal: settings.flipHorizontal,
                fineRotation: settings.fineRotation)
            tier.orientKey = oKey
        }
        let oriented = tier.oriented!
        if uncropped || settings.cropRect == nil {
            if tier.fullTexture == nil { tier.fullTexture = try pipeline.upload(oriented) }
            return (tier.fullTexture!, nil, oriented)
        }
        let crop = settings.cropRect!
        if tier.croppedTexture == nil || tier.croppedRect != crop {
            tier.croppedTexture = try pipeline.upload(oriented.cropped(to: crop))
            tier.croppedRect = crop
        }
        let roi = crop.pixelROI(width: oriented.width, height: oriented.height)
        return (tier.croppedTexture!, roi, oriented)
    }

    /// Resolve `.mediumIfFree` against what this body actually gave us.
    private func resolvedTier(_ requested: RenderTier) -> RenderTier {
        guard requested == .mediumIfFree else { return requested }
        return medium.base != nil ? .mediumIfFree : .full
    }

    private func clearFull() {
        full.clear()
    }

    /// Drop the full-resolution tier while keeping the proxy tower warm.
    /// Called on sessions the LRU retains but isn't showing: the proxy caches
    /// are tens of MB (worth holding for an instant return), the HQ decode is
    /// hundreds (not worth holding for a frame nobody is looking at).
    func releaseEnhancedTiers() {
        full.clear()
        medium.clear()
    }

    /// True when the next render must decode or run the heavy prepared stage
    /// (drives the "Analyzing…" indicator; offset-only finalizes are fast and
    /// don't flash it).
    func needsPreparation(settings: ExposureSettings, tier: RenderTier = .proxy) -> Bool {
        // Only the full tier can stall: its decode is ~500-700 ms. The medium
        // tier was decoded alongside the proxy, so reaching for it costs an
        // orient and an upload and must not flash the indicator.
        if resolvedTier(tier) == .full {
            let oKey = OrientKey(
                rotation: settings.rotation, flipHorizontal: settings.flipHorizontal,
                fineRotation: settings.fineRotation)
            if full.base == nil || full.oriented == nil || oKey != full.orientKey { return true }
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
        let source = try sourceTexture(image: image, settings: settings, uncropped: false).texture
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
        let detachedROI = settings.cropRect.flatMap {
            $0.pixelROI(width: oriented.width, height: oriented.height)
        }
        let source = try pipeline.upload(image)
        let result = try pipeline.renderDisplay(source: source, params: params)
        guard let cg = ImageConversion.cgImage(rgba8: result.rgba, width: result.width, height: result.height)
        else { throw RenderError.resource("CGImage conversion") }
        return RenderOutput(
            image: cg, histogram: result.histogram, frameSize: orientedFrameSize(settings),
            displayAspect: displayAspect(settings, uncropped: false),
            contentWindow: contentWindow(
                settings, uncropped: false, tierBase: basePreview!, oriented: oriented,
                roi: detachedROI),
            densityRange: analysis.baseBounds.luminanceDensityRange)
    }

    /// `uncropped` shows the full frame (used while a selection tool is active
    /// so the user can drag on the whole image, like NegPy's crop_preview_full).
    ///
    /// `retainFull` keeps the full-resolution tier alive across a proxy render.
    /// Auto mode needs it: zooming back out below the threshold is a proxy
    /// render, and dropping the tier there would make the next zoom-in pay the
    /// whole ~500-700 ms decode again. It is still freed when HQ is switched
    /// off, and by `releaseEnhancedTiers()` on every session the LRU isn't
    /// showing — so at most the on-screen frame holds it. The MEDIUM tier is
    /// never dropped here: it was free, and re-deriving it means re-decoding.
    func render(
        settings: ExposureSettings, uncropped: Bool = false,
        tier requested: RenderTier = .proxy, retainFull: Bool = false
    ) throws -> RenderOutput {
        let (image, analysis) = try prepare(settings: settings)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let source: MTLTexture
        let roi: (x0: Int, y0: Int, x1: Int, y1: Int)?
        let orientedSource: RGBImage
        let tierBase: RGBImage
        switch resolvedTier(requested) {
        case .full:
            if full.base == nil {
                full.base = try RawDecoder().decode(url: url, quality: .full)
            }
            let s = try tierSource(&full, settings: settings, uncropped: uncropped)
            source = s.texture
            roi = s.roi
            orientedSource = s.oriented
            tierBase = full.base!
        case .mediumIfFree:
            if !retainFull { clearFull() }
            let s = try tierSource(&medium, settings: settings, uncropped: uncropped)
            source = s.texture
            roi = s.roi
            orientedSource = s.oriented
            tierBase = medium.base!
        case .proxy:
            if !retainFull { clearFull() }
            let proxySource = try sourceTexture(
                image: image, settings: settings, uncropped: uncropped)
            source = proxySource.texture
            roi = proxySource.roi
            orientedSource = image
            tierBase = basePreview!
        }
        let result = try pipeline.renderDisplay(source: source, params: params)
        guard let cg = ImageConversion.cgImage(rgba8: result.rgba, width: result.width, height: result.height)
        else { throw RenderError.resource("CGImage conversion") }
        return RenderOutput(
            image: cg, histogram: result.histogram, frameSize: orientedFrameSize(settings),
            displayAspect: displayAspect(settings, uncropped: uncropped),
            contentWindow: contentWindow(
                settings, uncropped: uncropped, tierBase: tierBase, oriented: orientedSource,
                roi: roi),
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
