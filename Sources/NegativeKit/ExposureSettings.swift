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
    public var castRemovalStrength: Double = 0.5
    public var autoCastRemoval: Bool = true
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

    // Regional tone controls (see K.toneRegionSharpness block). Lightroom sign
    // conventions: shadows +1 lifts shadows; highlights −1 brings highlights
    // down; contrast sliders expand separation within their region; exposure
    // is global print exposure in stops (+ = brighter, histogram right).
    public var exposureStops: Double = 0
    public var shadows: Double = 0
    public var shadowContrast: Double = 0
    public var highlights: Double = 0
    public var highlightContrast: Double = 0

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
        autoCastRemoval = b(.autoCastRemoval, true)
        whitePointOffset = d(.whitePointOffset, 0)
        blackPointOffset = d(.blackPointOffset, 0)
        toe = d(.toe, 0)
        toeWidth = d(.toeWidth, 2.5)
        shoulder = d(.shoulder, 0)
        shoulderWidth = d(.shoulderWidth, 2.5)
        paperDmin = b(.paperDmin, true)
        exposureStops = d(.exposureStops, 0)
        shadows = d(.shadows, 0)
        shadowContrast = d(.shadowContrast, 0)
        highlights = d(.highlights, 0)
        highlightContrast = d(.highlightContrast, 0)
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
}

public enum ExposureKernel {
    /// Full per-image analysis: prefilter + bounds + all meters. The neutral axis
    /// is measured against offset bounds; NegSwift re-runs analysis when the
    /// white/black-point offsets change (cheap at grid size, and offsets are a
    /// rarely-touched control).
    public static func analyze(
        linearImage: RGBImage,
        analysisBuffer: Double = 0.05,
        whitePointOffset: Double = 0,
        blackPointOffset: Double = 0
    ) -> ExposureAnalysis {
        let grid = Prefilter.prefilterLogGrid(linearImage, analysisBuffer: analysisBuffer)
        let base = BoundsAnalysis.analyze(grid: grid)
        let final = base.applyingOffsets(whitePoint: whitePointOffset, blackPoint: blackPointOffset)
        let neutral = Meters.neutralAxis(grid: grid, bounds: final)
        return ExposureAnalysis(
            baseBounds: base,
            // Anchor reads against the per-frame base (luma_source_bounds).
            anchor: Meters.anchor(grid: grid, bounds: base),
            texturalRange: Meters.texturalRange(grid: grid),
            shadowRefs: Meters.shadowRefs(grid: grid),
            neutralMid: neutral?.mid,
            neutralShadow: neutral?.shadow,
            neutralHighlight: neutral?.highlight,
            neutralConfidence: neutral?.confidence
        )
    }

    /// PhotometricProcessor's parameter derivation (the cheap per-slider path).
    public static func deriveRenderParams(_ settings: ExposureSettings, _ analysis: ExposureAnalysis) -> RenderParams {
        let finalBounds = analysis.baseBounds.applyingOffsets(
            whitePoint: settings.whitePointOffset, blackPoint: settings.blackPointOffset)
        let dMin = settings.paperDmin ? K.dMin : 0.0
        let anchor = settings.autoExposure ? analysis.anchor : nil
        let strength = CurveLogic.effectiveCastStrength(
            settings.castRemovalStrength, auto: settings.autoCastRemoval, confidence: analysis.neutralConfidence)

        var neutralAxisNorm: (mid: SIMD3<Double>, shadow: SIMD3<Double>, highlight: SIMD3<Double>?)?
        if let mid = analysis.neutralMid, let shadow = analysis.neutralShadow {
            neutralAxisNorm = (
                mid: CurveLogic.normalizeRefs(mid, bounds: finalBounds),
                shadow: CurveLogic.normalizeRefs(shadow, bounds: finalBounds),
                highlight: analysis.neutralHighlight.map { CurveLogic.normalizeRefs($0, bounds: finalBounds) }
            )
        }

        let (slopes, pivots, curvatures) = CurveLogic.perChannelCurveParams(
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

        var cmyOffsets = CurveLogic.filtrationOffsets(
            wbCMY: SIMD3(settings.wbCyan, settings.wbMagenta, settings.wbYellow), bounds: finalBounds)
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
            shadowContrast: settings.shadowContrast,
            highlights: settings.highlights,
            highlightContrast: settings.highlightContrast
        )
    }
}
