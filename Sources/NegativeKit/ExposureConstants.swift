import Foundation

/// Verbatim port of NegPy's EXPOSURE_CONSTANTS (negpy/features/exposure/models.py).
/// C-41-only subset: E6/B&W, flat-master, paper-profile and dodge/burn constants
/// that SwiftInvert doesn't use are omitted.
public enum K {
    // CMY white-balance sliders: slider ±1 → ±this absolute density.
    public static let cmyMaxDensity = 0.2
    // Density slider → exposure pivot scale.
    public static let densityMultiplier = 0.2
    // Target print density for the reference tone.
    public static let anchorTargetDensity = 0.75
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
    // 4.0 → 6.0 in NegPy 8532dd92 (0.58.0): the earlier toe dropped system
    // gamma below Buhr's preferred-reproduction envelope by scene D 1.4.
    public static let toeSharpnessBase = 6.0
    public static let shoulderSharpnessBase = 3.0
    public static let toeShoulderWidthRef = 2.5
    // Density shift per toe/shoulder slider unit. toeHeight is larger than
    // shoulderHeight (NegPy 0.36 recalibration): density is log10, so a ΔD
    // near d_max reads perceptually far smaller than the same ΔD near d_min —
    // 0.90 evens out toe vs shoulder slider strength in L*.
    public static let toeHeight = 0.90
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
    // Above this median corrected chroma (pass 2) the set isn't trustworthy →
    // fall back to the shadow-only tie (NegPy 0.43: 0.35 → 0.29, coupled to the
    // two-pass estimator).
    public static let neutralAxisChromaCap = 0.29
    // Pass-1 (pre-correction) chroma ceiling: admits strong correctable casts,
    // rejects saturated content.
    public static let neutralAxisFirstPassCap = 0.55
    public static let neutralAxisMinPixels = 64
    // Confidence sample-size half-point: the size term is n / (n + this).
    public static let neutralAxisConfidenceN0 = 256.0
    // Mid↔shadow deviation-difference dead zone (plausible crossover passes
    // free) and roll-off width of the confidence agreement term beyond it.
    public static let neutralAxisAgreementDeadzone = 0.10
    public static let neutralAxisAgreementScale = 0.20
    // Width (percentile points) of the luma-extreme band the same-pixel colour
    // floor refs read; the colour clip sets the band's depth.
    public static let colorBoundsBandWidth = 4.0
    // Anchor metering (NegPy 8532dd92: the P50-of-every-cell meter became the
    // mean+midpoint of the trimmed textured window — Boyack & Juenger,
    // US 5,724,456 — so rebate, sky and flat walls stop setting exposure).
    // Activity gate shared by the anchor, textural-range and reach/hold
    // meters: the grid is tiled into sectors of 2×2 blocks of activityBlock
    // cells; a sector votes only when its four block means span more than
    // activityGateDensity (in the units of the luma passed — log D for the
    // textural meter, normalized luma for the anchor/points, upstream's own
    // behaviour). Below activityMinFraction of sectors passing, every cell
    // votes.
    public static let activityBlock = 8
    public static let activityGateDensity = 0.05
    public static let activityMinFraction = 0.05
    // Per-tail percentile trim of the textured-cell window the anchor reads.
    public static let anchorTrimClip = 5.0
    public static let anchorMeterBand = 0.12
    public static let anchorMeterStrength = 0.2
    // Grade-coupled knees. toeGradeStrength is rescaled by 0.35/0.90 so the
    // grade-coupled baseline ΔD (strength · toeHeight) keeps its calibrated
    // value after the perceptual toeHeight change — default output unchanged
    // (mirrors NegPy 0.36).
    public static let toeGradeStrength = 0.15 * 0.35 / 0.90
    public static let shoulderGradeStrength = 0.12
    // Auto Grade (NegPy 8532dd92, Alkofer US 4,731,671): the paper gamma
    // follows the negative's textural density scale, shrunk toward a normal
    // negative's — effective = K · floor_ceil · min((1−s) + s·n/t, c·n/t).
    public static let autoGradeTarget = 0.85
    public static let autoGradeStrength = 0.4
    // Sizes the unmetered fallback only (× nominalRange × target).
    public static let autoGradeNominalRatio = 1.5
    // Textural (P10–P90) density range of a normal negative, log10 D.
    public static let autoGradeNominalRange = 0.9
    // Cap on the textural range's print span, as a multiple of the norm's.
    public static let autoGradeMaxOverfill = 1.2
    // Textural-range percentile margin.
    public static let texturalRangeClip = 10.0
    // Auto Grade shadow reach (Gindele US 7,113,649): the textured dark tail
    // (this percentile of gated normalized luma) must print at least this
    // straight-line density; the grade only ever goes harder for it.
    public static let shadowReachPercentile = 99.0
    public static let shadowReachDensity = 1.9
    // Auto Grade highlight hold, the soft-exposure half of a split-grade
    // print (Agfa US 4,104,069 / 3,839,036): the textured bright tail (this
    // percentile) must print at least this density, met by an automatic
    // highlight-zone burn (never a lift) capped at highlightHoldMax. 0 = off.
    public static let highlightHoldPercentile = 2.0
    public static let highlightHoldDensity = 0.10
    public static let highlightHoldMax = 0.5
    // Zone Density actuator constants (NegPy models.py zone_density_*): the
    // hold burn rides upstream's Zone Density highlight term
    // v += burn · (1 − σ(sharpness · (v − (anchorTarget + highlightOffset)))).
    // We ship no user Zone Density sliders — the term exists for the auto
    // burn only. Mirrored as literals in NegPipeline.metal and
    // NegPipeline.comp (4.0 / 0.40) — keep all three in sync.
    public static let zoneDensitySharpness = 4.0
    public static let zoneDensityHighlightOffset = -0.40
    // Variable-gamma paper S-curve (0.15 → 0.05 in 8532dd92: the Snap bell
    // pushed system gamma over Buhr's envelope near scene D 0.8).
    public static let paperMidtoneGamma = 0.05
    public static let paperGammaWidth = 0.6

    // ── Regional tone controls (SwiftInvert addition, no NegPy equivalent) ──────
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
    // Dark Shadows: a second, deeper lift band — the Shadows anchor (1.40)
    // covers broad darks; this targets the deepest printable tones near d_max.
    // Same sharpness/amplitude, so the same monotone bounds apply.
    public static let darkShadowToneAnchor = 1.85
    // Density swing of the Shadows slider at ±1 (positive slider = lift).
    // Slider ranges ±2: still monotone alone (2 · 0.5 · sharpness/4 = 0.875 < 1).
    public static let shadowsMaxLift = 0.5
    // Density swing of the Highlights slider at ±1 (negative slider = recover).
    public static let highlightsMaxShift = 0.4
    // Max extra slope within each region at contrast slider ±1. The shadow
    // contrast slider ranges −3…+3: positive values only steepen the curve, so
    // headroom there cannot break monotonicity; the negative side is remapped
    // in deriveRenderParams so slider −3 lands exactly on the monotone floor,
    // and the kernels clamp at that floor as a hard invariant.
    public static let shadowContrastMax = 0.5
    public static let highlightContrastMax = 0.5
    // Most negative effective shadow-contrast gain that keeps the transfer
    // monotone (1 − 0.8·max|d/dv[(v−c)·w]| > 0 at sharpness 3.5).
    public static let shadowContrastNegFloor = -0.8
    // Overall contrast at slider ±1: the print curve rotates around the
    // reference tone, v → v + k·(v − v*), folded into slopes/pivots/curvatures
    // (no shader involvement). Slider range −1…+2 → k ∈ [−0.5, +1].
    public static let overallContrastMax = 0.5
    // Print Saturation's matrix-coefficient clamp (NegPy per_channel_dye_separation
    // — their surviving density-saturation control, renamed in 3fb5ca8).
    public static let printSaturationMax = 3.0
    // Separation Damping's reference spread (NegPy separation_damping_ref_spread,
    // d86a5aa): the per-pixel chroma left at exactly gain 1.0 when damping is
    // full — above it the push reverses. Inlined as an MSL literal — change both.
    public static let separationDampingRefSpread = 0.35
    // One photographic stop in log10 density.
    public static let log10Two = 0.3010299956639812

    // Rec.709 luma weights (negpy/domain/types.py).
    public static let lumaR = 0.2126
    public static let lumaG = 0.7152
    public static let lumaB = 0.0722
}

/// Working-space OETF: Adobe RGB (1998) TRC — a pure 563/256 power, no
/// linear segment (negpy/kernel/image/logic.py working_oetf_encode/decode,
/// since b3490eb: the working space moved ProPhoto→Adobe RGB because the
/// pipeline ASSIGNS primaries to sensor-native data at output, and
/// ProPhoto's imaginary primaries inflated chroma and skewed hues).
public enum WorkingOETF {
    /// 563/256 = 2.19921875 — Adobe RGB's exact rational gamma.
    public static let gamma: Float = 563.0 / 256.0

    @inlinable public static func encode(_ x: Float) -> Float {
        pow(max(x, 0), 1.0 / 2.19921875)
    }

    @inlinable public static func decode(_ e: Float) -> Float {
        pow(max(e, 0), 2.19921875)
    }
}
