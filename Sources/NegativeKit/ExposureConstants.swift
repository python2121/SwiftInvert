import Foundation

/// Verbatim port of NegPy's EXPOSURE_CONSTANTS (negpy/features/exposure/models.py).
/// C-41-only subset: E6/B&W, flat-master, paper-profile and dodge/burn constants
/// that NegSwift doesn't use are omitted.
public enum K {
    // CMY white-balance sliders: slider ±1 → ±this absolute density.
    public static let cmyMaxDensity = 0.2
    // Density slider → exposure pivot scale.
    public static let densityMultiplier = 0.2
    // Target print density for the reference tone.
    public static let anchorTargetDensity = 0.74
    // Default normalized midtone reference (auto-exposure off).
    public static let assumedAnchor = 0.46
    // ISO R paper-grade slider range (hard → soft).
    public static let isoRMin = 50.0
    public static let isoRMax = 180.0
    // Straight-line slope clamps.
    public static let slopeMin = 2.0
    public static let slopeMax = 10.0
    // Paper black / paper white densities.
    public static let dMax = 2.3
    public static let dMin = 0.06
    // Toe/shoulder slider pre-scale.
    public static let toeShoulderStrength = 0.85
    // Softplus knee sharpness: a = base * widthRef / width.
    public static let toeSharpnessBase = 4.0
    public static let shoulderSharpnessBase = 3.0
    public static let toeShoulderWidthRef = 2.5
    // Density shift per toe/shoulder slider unit.
    public static let toeHeight = 0.35
    public static let shoulderHeight = 0.35
    // Grade → slope calibration: k = scale * densityRange / (ISO R / 100).
    public static let gradeContrastScale = 2.9
    // Block-median prefilter target grid side.
    public static let analysisGrid = 1024
    // Baseline percentile clips for the two bounds axes.
    public static let baseLumaClip = 0.01
    public static let baseColorClip = 1.0
    // Shadow-reference percentile for cast detection.
    public static let shadowNeutralPercentile = 98.0
    // Cast removal clamps.
    public static let castRemovalMaxOffset = 0.1
    public static let midtoneCastMaxOffset = 0.2
    public static let neutralAxisCurvMaxRatio = 0.45
    // Neutral-axis luma bands (normalized; low = bright print).
    public static let neutralAxisHighlightBand = (0.10, 0.30)
    public static let neutralAxisMidBand = (0.40, 0.60)
    public static let neutralAxisShadowBand = (0.72, 0.92)
    public static let neutralAxisChromaQuantile = 0.30
    public static let neutralAxisChromaCap = 0.35
    public static let neutralAxisMinPixels = 64
    // Anchor metering.
    public static let anchorMeterPercentile = 50.0
    public static let anchorMeterBand = 0.12
    public static let anchorMeterStrength = 0.2
    // Grade-coupled knees.
    public static let toeGradeStrength = 0.15
    public static let shoulderGradeStrength = 0.12
    // Auto grade.
    public static let autoGradeTarget = 0.5
    public static let autoGradeStrength = 0.4
    public static let autoGradeNominalRatio = 2.0
    // Dim-surround gamma / veiling flare (optional toggles).
    public static let targetSystemGamma = 1.10
    public static let flareFraction = 0.005
    // Textural-range percentile margin.
    public static let texturalRangeClip = 10.0
    // Variable-gamma paper S-curve.
    public static let paperMidtoneGamma = 0.15
    public static let paperGammaWidth = 0.6

    // ── Regional tone controls (NegSwift addition, no NegPy equivalent) ──────
    // Shadow/highlight lift and per-region contrast operate on print density v
    // (after the curve core + midtone gamma, before regional CMY and the
    // toe/shoulder bounds) with smooth sigmoid region masks:
    //   w_shadow    = σ(sharpness · (v − shadowToneAnchor))
    //   w_highlight = σ(sharpness · (highlightToneAnchor − v))
    // Amplitudes are bounded so each control alone keeps the transfer monotone
    // (sharpness · maxAmount / 4 < 1). Mirrored in NegPipeline.metal — keep in sync.
    public static let toneRegionSharpness = 3.5
    // Density anchor of the "medium darks" (shadow region centre).
    public static let shadowToneAnchor = 1.40
    // Density anchor of the highlight region centre.
    public static let highlightToneAnchor = 0.30
    // Density swing of the Shadows slider at ±1 (positive slider = lift).
    public static let shadowsMaxLift = 0.5
    // Density swing of the Highlights slider at ±1 (negative slider = recover).
    public static let highlightsMaxShift = 0.4
    // Max extra slope within each region at contrast slider ±1.
    public static let shadowContrastMax = 0.5
    public static let highlightContrastMax = 0.5
    // One photographic stop in log10 density.
    public static let log10Two = 0.3010299956639812

    // Rec.709 luma weights (negpy/domain/types.py).
    public static let lumaR = 0.2126
    public static let lumaG = 0.7152
    public static let lumaB = 0.0722
}

/// Working-space OETF: ProPhoto RGB (ROMM) TRC — gamma 1.8 with a linear toe
/// below 1/512 (negpy/kernel/image/logic.py working_oetf_encode/decode).
public enum WorkingOETF {
    @inlinable public static func encode(_ x: Float) -> Float {
        if x < 1.0 / 512.0 { return max(x, 0) * 16.0 }
        return pow(x, 1.0 / 1.8)
    }

    @inlinable public static func decode(_ e: Float) -> Float {
        if e < 1.0 / 32.0 { return max(e, 0) / 16.0 }
        return pow(e, 1.8)
    }
}
