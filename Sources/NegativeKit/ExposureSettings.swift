import Foundation

/// User-facing exposure settings — the sidecar payload and the input to
/// RenderParams derivation. Field defaults mirror NegPy's ExposureConfig /
/// ProcessConfig (C-41 subset).
public struct ExposureSettings: Codable, Equatable, Sendable {
    /// Print exposure in stops around the calibrated default (NegPy: 1.0 = neutral).
    public var density: Double = 1.0
    /// Contrast as ISO R paper exposure range (50 hard … 180 soft; 115 ≈ grade 2).
    public var grade: Double = 115.0
    /// Global white balance (CMY filtration, ±1 slider = ±20cc).
    public var wbCyan: Double = 0
    public var wbMagenta: Double = 0
    public var wbYellow: Double = 0
    /// Auto density: use the per-frame metered anchor.
    public var autoExposure: Bool = true
    /// Auto grade: adapt slope to the measured textural range.
    public var autoNormalizeContrast: Bool = true
    /// C-41 gray balance (orange-mask cancel).
    /// Always confidence-scaled (NegPy 0.36 removed the auto toggle).
    public var castRemovalStrength: Double = 0.5
    /// Histogram edge handles (log-density offsets on the analyzed bounds).
    public var whitePointOffset: Double = 0
    public var blackPointOffset: Double = 0
    /// Tone shaping (secondary controls).
    public var toe: Double = 0
    public var toeWidth: Double = 2.5
    public var shoulder: Double = 0
    public var shoulderWidth: Double = 2.5
    /// Paper white floor on (NegPy paper_dmin, d_min = 0.06).
    public var paperDmin: Bool = true
    /// True Black: black point compensation — paper Dmax maps to display black
    /// (relative-colorimetric style; without it paper black floats at ~0.5%).
    public var trueBlack: Bool = false

    // Regional tone controls (see K.toneRegionSharpness block). Lightroom sign
    // conventions: shadows +1 lifts shadows; highlights −1 brings highlights
    // down; contrast sliders expand separation within their region; exposure
    // is global print exposure in stops (+ = brighter, histogram right).
    public var exposureStops: Double = 0
    public var shadows: Double = 0
    public var shadowContrast: Double = 0
    public var highlights: Double = 0
    public var highlightContrast: Double = 0
    /// Overall contrast: rotates the whole curve around the reference tone
    /// (anchor brightness preserved). + steepens, − flattens.
    public var overallContrast: Double = 0

    // Color pop (NegPy lab-stage scale: 1.0 = neutral). Vibrance boosts muted
    // colors toward the strength; saturation scales all chroma in CIELAB.
    public var vibrance: Double = 1.0
    public var saturation: Double = 1.0

    // Pre-saturation (Negative Lab Pro concept): scales per-pixel density
    // deviations from neutral in normalized log space BEFORE the print curve,
    // restoring the inter-channel separation the per-channel normalization
    // equalizes away. 1.0 = off; acts on hue separation, not just chroma.
    // Default 1.15 (A/B-chosen): counters the muted default look. NegPy has
    // no equivalent — parity fixtures must pin this to 1.0.
    public var preSaturation: Double = 1.15

    // Overall white balance as Temp (blue↔yellow along the Planckian direction,
    // + = warmer) and Tint (green↔magenta, + = magenta); composes additively
    // with the CMY trim sliders in filtration_offsets space.
    public var temp: Double = 0
    public var tint: Double = 0

    // Per-band color balance (Negative Lab Pro-style): x = R↔C, y = G↔M,
    // z = B↔Y; positive = toward C/M/Y (adds print density in that channel).
    // Bands use the tone-region masks (shadow/highlight anchors + mids).
    public var colorShadows: SIMD3<Double> = .zero
    public var colorMids: SIMD3<Double> = .zero
    public var colorHighs: SIMD3<Double> = .zero

    // Orientation, baked into the pixels right after decode so analysis,
    // display-space rects, rendering and export all agree.
    public var rotation: Int = 0  // clockwise degrees: 0/90/180/270
    public var flipHorizontal: Bool = false

    // Pre-process rects (NegPy analysis_rect / manual_crop_rect semantics):
    // analysisRect scopes ONLY the metering (buffer disabled inside it);
    // cropRect crops the output AND scopes the metering (buffer applied inside).
    public var analysisRect: NormalizedRect?
    public var cropRect: NormalizedRect?

    public init() {}

    // Sidecars written before these controls existed omit the keys; default to 0.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        func d(_ k: CodingKeys, _ def: Double) -> Double { (try? c.decode(Double.self, forKey: k)) ?? def }
        func b(_ k: CodingKeys, _ def: Bool) -> Bool { (try? c.decode(Bool.self, forKey: k)) ?? def }
        density = d(.density, 1.0)
        grade = d(.grade, 115.0)
        wbCyan = d(.wbCyan, 0)
        wbMagenta = d(.wbMagenta, 0)
        wbYellow = d(.wbYellow, 0)
        autoExposure = b(.autoExposure, true)
        autoNormalizeContrast = b(.autoNormalizeContrast, true)
        castRemovalStrength = d(.castRemovalStrength, 0.5)
        whitePointOffset = d(.whitePointOffset, 0)
        blackPointOffset = d(.blackPointOffset, 0)
        toe = d(.toe, 0)
        toeWidth = d(.toeWidth, 2.5)
        shoulder = d(.shoulder, 0)
        shoulderWidth = d(.shoulderWidth, 2.5)
        paperDmin = b(.paperDmin, true)
        trueBlack = b(.trueBlack, false)
        exposureStops = d(.exposureStops, 0)
        shadows = d(.shadows, 0)
        shadowContrast = d(.shadowContrast, 0)
        highlights = d(.highlights, 0)
        highlightContrast = d(.highlightContrast, 0)
        overallContrast = d(.overallContrast, 0)
        vibrance = d(.vibrance, 1.0)
        saturation = d(.saturation, 1.0)
        preSaturation = d(.preSaturation, 1.15)
        temp = d(.temp, 0)
        tint = d(.tint, 0)
        colorShadows = (try? c.decode(SIMD3<Double>.self, forKey: .colorShadows)) ?? .zero
        colorMids = (try? c.decode(SIMD3<Double>.self, forKey: .colorMids)) ?? .zero
        colorHighs = (try? c.decode(SIMD3<Double>.self, forKey: .colorHighs)) ?? .zero
        rotation = (try? c.decode(Int.self, forKey: .rotation)) ?? 0
        flipHorizontal = b(.flipHorizontal, false)
        analysisRect = try? c.decode(NormalizedRect.self, forKey: .analysisRect)
        cropRect = try? c.decode(NormalizedRect.self, forKey: .cropRect)
    }
}

/// Per-image analysis result — computed once per image (per decode), reused
/// across all slider changes. Mirrors the metrics NormalizationProcessor stores.
public struct ExposureAnalysis: Codable, Equatable, Sendable {
    /// Analyzed per-frame bounds (before white/black-point offsets).
    public var baseBounds: LogNegativeBounds
    public var anchor: Double
    public var texturalRange: Double
    public var shadowRefs: SIMD3<Double>
    public var neutralMid: SIMD3<Double>?
    public var neutralShadow: SIMD3<Double>?
    public var neutralHighlight: SIMD3<Double>?
    public var neutralConfidence: Double?
}

/// Everything the render pass needs: normalization bounds + print-curve uniforms.
/// Derived from (settings, analysis) in microseconds — sliders re-derive this and
/// re-render; they never re-run analysis.
public struct RenderParams: Equatable, Sendable {
    public var finalBounds: LogNegativeBounds
    public var slopes: SIMD3<Double>
    public var pivots: SIMD3<Double>
    public var curvatures: SIMD3<Double>
    public var cmyOffsets: SIMD3<Double>
    public var toeEff: Double
    public var shoulderEff: Double
    public var toeWidth: Double
    public var shoulderWidth: Double
    public var dMin: Double
    public var vStar: Double
    // Regional tone controls (exposureStops is folded into cmyOffsets upstream).
    public var shadows: Double = 0
    public var shadowContrast: Double = 0
    public var highlights: Double = 0
    public var highlightContrast: Double = 0
    // CIELAB chroma ops on the linear print (1.0 = off).
    public var vibrance: Double = 1.0
    public var saturation: Double = 1.0
    /// Pre-curve density-deviation gain (1.0 = off).
    public var preSaturation: Double = 1.0
    /// Black point compensation (paper Dmax → display black).
    public var trueBlack: Bool = false
    // Per-band CMY density offsets (already scaled to density units).
    public var shadowCMY: SIMD3<Double> = .zero
    public var midCMY: SIMD3<Double> = .zero
    public var highlightCMY: SIMD3<Double> = .zero

    public init(
        finalBounds: LogNegativeBounds, slopes: SIMD3<Double>, pivots: SIMD3<Double>,
        curvatures: SIMD3<Double>, cmyOffsets: SIMD3<Double>, toeEff: Double, shoulderEff: Double,
        toeWidth: Double, shoulderWidth: Double, dMin: Double, vStar: Double,
        shadows: Double = 0, shadowContrast: Double = 0, highlights: Double = 0,
        highlightContrast: Double = 0, vibrance: Double = 1.0, saturation: Double = 1.0,
        preSaturation: Double = 1.0, trueBlack: Bool = false,
        shadowCMY: SIMD3<Double> = .zero, midCMY: SIMD3<Double> = .zero,
        highlightCMY: SIMD3<Double> = .zero
    ) {
        self.finalBounds = finalBounds
        self.slopes = slopes
        self.pivots = pivots
        self.curvatures = curvatures
        self.cmyOffsets = cmyOffsets
        self.toeEff = toeEff
        self.shoulderEff = shoulderEff
        self.toeWidth = toeWidth
        self.shoulderWidth = shoulderWidth
        self.dMin = dMin
        self.vStar = vStar
        self.shadows = shadows
        self.shadowContrast = shadowContrast
        self.highlights = highlights
        self.highlightContrast = highlightContrast
        self.vibrance = vibrance
        self.saturation = saturation
        self.preSaturation = preSaturation
        self.trueBlack = trueBlack
        self.shadowCMY = shadowCMY
        self.midCMY = midCMY
        self.highlightCMY = highlightCMY
    }
}

public enum ExposureKernel {
    /// Full per-image analysis: prefilter + bounds + all meters. The neutral axis
    /// is measured against offset bounds; SwiftInvert re-runs analysis when the
    /// white/black-point offsets or pre-process rects change (cheap at grid size).
    ///
    /// Region priority mirrors NegPy's resolve_analysis_region: a freehand
    /// analysisRect wins and disables the centered buffer inset; otherwise the
    /// output cropRect scopes the meters (so borders outside the crop can't
    /// throw off the inversion) with the buffer applied inside it.
    /// Default centered inset: 10% per side = the middle 80% of the frame
    /// (SwiftInvert default; NegPy ships 5%).
    public static let defaultAnalysisBuffer = 0.10

    /// The expensive, offset-independent half of the analysis: the prefiltered
    /// grid plus every meter that doesn't read the final (offset) bounds.
    /// Cache this per (image, cropRect, analysisRect); white/black-point drags
    /// only need `finalize`, which re-measures the neutral axis on the grid.
    public struct Prepared: Sendable {
        public let grid: RGBImage
        public let baseBounds: LogNegativeBounds
        public let anchor: Double
        public let texturalRange: Double
        public let shadowRefs: SIMD3<Double>
    }

    public static func prepare(
        linearImage: RGBImage,
        cropRect: NormalizedRect? = nil,
        analysisRect: NormalizedRect? = nil,
        analysisBuffer: Double = defaultAnalysisBuffer
    ) -> Prepared {
        var metered = linearImage
        var buffer = analysisBuffer
        if let rect = analysisRect, rect.pixelROI(width: linearImage.width, height: linearImage.height) != nil {
            metered = linearImage.cropped(to: rect)
            buffer = 0.0
        } else if let rect = cropRect, rect.pixelROI(width: linearImage.width, height: linearImage.height) != nil {
            metered = linearImage.cropped(to: rect)
        }
        let grid = Prefilter.prefilterLogGrid(metered, analysisBuffer: buffer)
        let base = BoundsAnalysis.analyze(grid: grid)
        return Prepared(
            grid: grid,
            baseBounds: base,
            // Anchor reads against the per-frame base (luma_source_bounds).
            anchor: Meters.anchor(grid: grid, bounds: base),
            texturalRange: Meters.texturalRange(grid: grid),
            shadowRefs: Meters.shadowRefs(grid: grid)
        )
    }

    /// The cheap, offset-dependent tail: only the neutral axis reads the final
    /// bounds (its luma-band membership shifts with the offsets, as in NegPy).
    public static func finalize(
        _ prepared: Prepared, whitePointOffset: Double = 0, blackPointOffset: Double = 0
    ) -> ExposureAnalysis {
        let final = prepared.baseBounds.applyingOffsets(
            whitePoint: whitePointOffset, blackPoint: blackPointOffset)
        let neutral = Meters.neutralAxis(grid: prepared.grid, bounds: final)
        return ExposureAnalysis(
            baseBounds: prepared.baseBounds,
            anchor: prepared.anchor,
            texturalRange: prepared.texturalRange,
            shadowRefs: prepared.shadowRefs,
            neutralMid: neutral?.mid,
            neutralShadow: neutral?.shadow,
            neutralHighlight: neutral?.highlight,
            neutralConfidence: neutral?.confidence
        )
    }

    public static func analyze(
        linearImage: RGBImage,
        cropRect: NormalizedRect? = nil,
        analysisRect: NormalizedRect? = nil,
        analysisBuffer: Double = defaultAnalysisBuffer,
        whitePointOffset: Double = 0,
        blackPointOffset: Double = 0
    ) -> ExposureAnalysis {
        finalize(
            prepare(
                linearImage: linearImage, cropRect: cropRect, analysisRect: analysisRect,
                analysisBuffer: analysisBuffer),
            whitePointOffset: whitePointOffset, blackPointOffset: blackPointOffset)
    }

    /// PhotometricProcessor's parameter derivation (the cheap per-slider path).
    public static func deriveRenderParams(_ settings: ExposureSettings, _ analysis: ExposureAnalysis) -> RenderParams {
        let finalBounds = analysis.baseBounds.applyingOffsets(
            whitePoint: settings.whitePointOffset, blackPoint: settings.blackPointOffset)
        let dMin = settings.paperDmin ? K.dMin : 0.0
        let anchor = settings.autoExposure ? analysis.anchor : nil
        let strength = CurveLogic.effectiveCastStrength(
            settings.castRemovalStrength, confidence: analysis.neutralConfidence)

        var neutralAxisNorm: (mid: SIMD3<Double>, shadow: SIMD3<Double>, highlight: SIMD3<Double>?)?
        if let mid = analysis.neutralMid, let shadow = analysis.neutralShadow {
            neutralAxisNorm = (
                mid: CurveLogic.normalizeRefs(mid, bounds: finalBounds),
                shadow: CurveLogic.normalizeRefs(shadow, bounds: finalBounds),
                highlight: analysis.neutralHighlight.map { CurveLogic.normalizeRefs($0, bounds: finalBounds) }
            )
        }

        var (slopes, pivots, curvatures) = CurveLogic.perChannelCurveParams(
            grade: settings.grade,
            density: settings.density,
            autoNormalizeContrast: settings.autoNormalizeContrast,
            strength: strength,
            // NegPy computes norm_density_range before the wp/bp offsets are applied.
            lumRange: analysis.baseBounds.luminanceDensityRange,
            shadowRefsNorm: CurveLogic.normalizeRefs(analysis.shadowRefs, bounds: finalBounds),
            texturalRange: analysis.texturalRange,
            dMin: dMin,
            anchor: anchor,
            neutralAxisNorm: neutralAxisNorm
        )

        // Overall contrast: v → v + k·(v − v*) folded exactly into the core:
        // slopes and curvatures scale by (1+k); the pivot shifts by k·v*/s'
        // so the reference tone's print density is unchanged (and the midtone
        // paper S stays centered, since it's anchored at v* too).
        if settings.overallContrast != 0 {
            let k = settings.overallContrast * K.overallContrastMax
            let vStar = CurveLogic.referenceLinearValue(dMin: dMin)
            for ch in 0..<3 {
                let scaled = slopes[ch] * (1 + k)
                curvatures[ch] *= (1 + k)
                pivots[ch] += k * vStar / scaled
                slopes[ch] = scaled
            }
        }

        // Temp rides the Planckian direction (yellow + coupled magenta, NegPy's
        // mired slopes km/ky), Tint the green↔magenta axis; both compose with
        // the CMY trim sliders.
        let planckianRatio = 0.0029 / 0.0057
        var cmyOffsets = CurveLogic.filtrationOffsets(
            wbCMY: SIMD3(
                settings.wbCyan,
                settings.wbMagenta + settings.tint + settings.temp * planckianRatio,
                settings.wbYellow + settings.temp),
            bounds: finalBounds)
        // Global exposure: a uniform pre-curve print-exposure offset, one stop =
        // −log10(2) over each channel's stretch range (the local_ev_scale domain,
        // like dodging the whole print). Negative scale ⇒ positive stops brighten.
        if settings.exposureStops != 0 {
            for ch in 0..<3 {
                let range = max(abs(finalBounds.ceils[ch] - finalBounds.floors[ch]), 1e-6)
                cmyOffsets[ch] += settings.exposureStops * (-K.log10Two / range)
            }
        }
        let knees = CurveLogic.gradeCoupledShape(slopeG: slopes.y, toe: settings.toe, shoulder: settings.shoulder)

        return RenderParams(
            finalBounds: finalBounds,
            slopes: slopes,
            pivots: pivots,
            curvatures: curvatures,
            cmyOffsets: cmyOffsets,
            toeEff: knees.toe,
            shoulderEff: knees.shoulder,
            toeWidth: settings.toeWidth,
            shoulderWidth: settings.shoulderWidth,
            dMin: dMin,
            vStar: CurveLogic.referenceLinearValue(dMin: dMin),
            shadows: settings.shadows,
            // Negative side remapped so slider −3 reaches exactly the monotone
            // floor (positive side passes through, scaled in the kernel).
            shadowContrast: settings.shadowContrast >= 0
                ? settings.shadowContrast
                : settings.shadowContrast * (abs(K.shadowContrastNegFloor) / (3.0 * K.shadowContrastMax)),
            highlights: settings.highlights,
            highlightContrast: settings.highlightContrast,
            vibrance: settings.vibrance,
            saturation: settings.saturation,
            preSaturation: settings.preSaturation,
            trueBlack: settings.trueBlack,
            // Band sliders ±1 → ±cmy_max_density print-density offsets
            // (NegPy's shadow/highlight CMY scale, plus a mids band).
            shadowCMY: settings.colorShadows * K.cmyMaxDensity,
            midCMY: settings.colorMids * K.cmyMaxDensity,
            highlightCMY: settings.colorHighs * K.cmyMaxDensity
        )
    }
}
