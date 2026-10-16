//! Exposure constants and the working-space OETF — port of `ExposureConstants.swift`
//! (itself a verbatim port of NegPy's `EXPOSURE_CONSTANTS`, `negpy/features/exposure/models.py`,
//! synced with NegPy 0.58.0 `dc8ac65f` + the `8532dd92` print-tone/auto-helpers retune).
//!
//! Layout: [`K`] is a unit struct carrying every constant as an associated `pub const`, so call
//! sites read `K::D_MAX` exactly where Swift reads `K.dMax`. The GPU crate mirrors the ones its
//! kernels need as WGSL literals (tone anchors/sharpness, Lab matrices, the OETF exponent,
//! the Zone Density actuator pair) — keep them in sync; the CPU/GPU parity tests catch drift.
//! C-41-only subset: E6/B&W, flat-master, paper-profile and dodge/burn constants that
//! SwiftInvert doesn't use are omitted, as in Swift.

/// NegPy's `EXPOSURE_CONSTANTS` (C-41 subset) plus SwiftInvert's own tone-control constants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct K;

impl K {
    /// CMY white-balance sliders: slider ±1 → ±this absolute density.
    pub const CMY_MAX_DENSITY: f64 = 0.2;
    /// Density slider → exposure pivot scale.
    pub const DENSITY_MULTIPLIER: f64 = 0.2;
    /// Target print density for the reference tone.
    pub const ANCHOR_TARGET_DENSITY: f64 = 0.75;
    /// Default normalized midtone reference (auto-exposure off).
    pub const ASSUMED_ANCHOR: f64 = 0.46;
    /// ISO R paper-grade slider range (hard → soft).
    pub const ISO_R_MIN: f64 = 50.0;
    /// ISO R paper-grade slider range (hard → soft).
    pub const ISO_R_MAX: f64 = 180.0;
    /// Straight-line slope clamp (low).
    pub const SLOPE_MIN: f64 = 2.0;
    /// Straight-line slope clamp (high).
    pub const SLOPE_MAX: f64 = 10.0;
    /// Paper black density.
    pub const D_MAX: f64 = 2.3;
    /// Paper white density.
    pub const D_MIN: f64 = 0.06;
    /// Toe/shoulder slider pre-scale.
    pub const TOE_SHOULDER_STRENGTH: f64 = 0.85;
    /// Softplus knee sharpness: `a = base * widthRef / width`.
    /// 4.0 → 6.0 in NegPy 8532dd92 (0.58.0): the earlier toe dropped system gamma below
    /// Buhr's preferred-reproduction envelope by scene D 1.4.
    pub const TOE_SHARPNESS_BASE: f64 = 6.0;
    /// Shoulder softplus sharpness base.
    pub const SHOULDER_SHARPNESS_BASE: f64 = 3.0;
    /// Reference knee width the sharpness bases are calibrated at.
    pub const TOE_SHOULDER_WIDTH_REF: f64 = 2.5;
    /// Density shift per toe slider unit. Larger than `SHOULDER_HEIGHT` (NegPy 0.36
    /// recalibration): density is log10, so a ΔD near d_max reads perceptually far smaller
    /// than the same ΔD near d_min — 0.90 evens out toe vs shoulder slider strength in L*.
    pub const TOE_HEIGHT: f64 = 0.90;
    /// Density shift per shoulder slider unit.
    pub const SHOULDER_HEIGHT: f64 = 0.35;
    /// Grade → slope calibration: `k = scale * densityRange / (ISO R / 100)`.
    pub const GRADE_CONTRAST_SCALE: f64 = 2.9;
    /// Block-median prefilter target grid side.
    pub const ANALYSIS_GRID: usize = 1024;
    /// Baseline percentile clip of the luma bounds axis.
    pub const BASE_LUMA_CLIP: f64 = 0.01;
    /// Baseline percentile clip of the colour bounds axis.
    pub const BASE_COLOR_CLIP: f64 = 1.0;
    /// Shadow-reference percentile for cast detection.
    pub const SHADOW_NEUTRAL_PERCENTILE: f64 = 98.0;
    /// Cast removal clamp of the one-point shadow-ref tie.
    pub const CAST_REMOVAL_MAX_OFFSET: f64 = 0.1;
    /// Cast removal clamp of the neutral-axis midtone deviations.
    pub const MIDTONE_CAST_MAX_OFFSET: f64 = 0.2;
    /// Curvature clamp of the neutral-axis quadratic, as a ratio of the green slope.
    pub const NEUTRAL_AXIS_CURV_MAX_RATIO: f64 = 0.45;
    /// Neutral-axis luma band (normalized; low = bright print): highlights.
    pub const NEUTRAL_AXIS_HIGHLIGHT_BAND: (f64, f64) = (0.10, 0.30);
    /// Neutral-axis luma band: midtones.
    pub const NEUTRAL_AXIS_MID_BAND: (f64, f64) = (0.40, 0.60);
    /// Neutral-axis luma band: shadows.
    pub const NEUTRAL_AXIS_SHADOW_BAND: (f64, f64) = (0.72, 0.92);
    /// Fraction of each band kept (lowest RMS chroma).
    pub const NEUTRAL_AXIS_CHROMA_QUANTILE: f64 = 0.30;
    /// Above this median corrected chroma (pass 2) the set isn't trustworthy → fall back to
    /// the shadow-only tie (NegPy 0.43: 0.35 → 0.29, coupled to the two-pass estimator).
    pub const NEUTRAL_AXIS_CHROMA_CAP: f64 = 0.29;
    /// Pass-1 (pre-correction) chroma ceiling: admits strong correctable casts, rejects
    /// saturated content.
    pub const NEUTRAL_AXIS_FIRST_PASS_CAP: f64 = 0.55;
    /// Minimum pixels for a neutral-axis band to count.
    pub const NEUTRAL_AXIS_MIN_PIXELS: usize = 64;
    /// Confidence sample-size half-point: the size term is `n / (n + this)`.
    pub const NEUTRAL_AXIS_CONFIDENCE_N0: f64 = 256.0;
    /// Mid↔shadow deviation-difference dead zone (plausible crossover passes free).
    pub const NEUTRAL_AXIS_AGREEMENT_DEADZONE: f64 = 0.10;
    /// Roll-off width of the confidence agreement term beyond the dead zone.
    pub const NEUTRAL_AXIS_AGREEMENT_SCALE: f64 = 0.20;
    /// Width (percentile points) of the luma-extreme band the same-pixel colour floor refs
    /// read; the colour clip sets the band's depth.
    pub const COLOR_BOUNDS_BAND_WIDTH: f64 = 4.0;
    /// Activity gate (NegPy 8532dd92, Boyack & Juenger US 5,724,456): the grid is tiled into
    /// sectors of 2×2 blocks of this many cells; a sector votes only when its four block
    /// means span more than `ACTIVITY_GATE_DENSITY`.
    pub const ACTIVITY_BLOCK: usize = 8;
    /// Activity gate threshold (in the units of the luma passed — log D for the textural
    /// meter, normalized luma for the anchor/points, upstream's own behaviour).
    pub const ACTIVITY_GATE_DENSITY: f64 = 0.05;
    /// Below this fraction of sectors passing, every cell votes.
    pub const ACTIVITY_MIN_FRACTION: f64 = 0.05;
    /// Per-tail percentile trim of the textured-cell window the anchor reads.
    pub const ANCHOR_TRIM_CLIP: f64 = 5.0;
    /// Anchor meter clamp (±) around `ASSUMED_ANCHOR`.
    pub const ANCHOR_METER_BAND: f64 = 0.12;
    /// Anchor meter pull toward `ASSUMED_ANCHOR`.
    pub const ANCHOR_METER_STRENGTH: f64 = 0.2;
    /// Grade-coupled toe knee. Rescaled by 0.35/0.90 so the grade-coupled baseline ΔD
    /// (strength · toeHeight) keeps its calibrated value after the perceptual `TOE_HEIGHT`
    /// change — default output unchanged (mirrors NegPy 0.36).
    pub const TOE_GRADE_STRENGTH: f64 = 0.15 * 0.35 / 0.90;
    /// Grade-coupled shoulder knee.
    pub const SHOULDER_GRADE_STRENGTH: f64 = 0.12;
    /// Auto Grade (NegPy 8532dd92, Alkofer US 4,731,671): the paper gamma follows the
    /// negative's textural density scale, shrunk toward a normal negative's —
    /// `effective = K · floor_ceil · min((1−s) + s·n/t, c·n/t)`.
    pub const AUTO_GRADE_TARGET: f64 = 0.85;
    /// Auto Grade shrink strength `s`.
    pub const AUTO_GRADE_STRENGTH: f64 = 0.4;
    /// Sizes the unmetered fallback only (× nominalRange × target).
    pub const AUTO_GRADE_NOMINAL_RATIO: f64 = 1.5;
    /// Textural (P10–P90) density range of a normal negative, log10 D.
    pub const AUTO_GRADE_NOMINAL_RANGE: f64 = 0.9;
    /// Cap on the textural range's print span, as a multiple of the norm's.
    pub const AUTO_GRADE_MAX_OVERFILL: f64 = 1.2;
    /// Textural-range percentile margin.
    pub const TEXTURAL_RANGE_CLIP: f64 = 10.0;
    /// Auto Grade shadow reach (Gindele US 7,113,649): the textured dark tail (this
    /// percentile of gated normalized luma) must print at least `SHADOW_REACH_DENSITY`;
    /// the grade only ever goes harder for it.
    pub const SHADOW_REACH_PERCENTILE: f64 = 99.0;
    /// Straight-line density the shadow reach tail must print.
    pub const SHADOW_REACH_DENSITY: f64 = 1.9;
    /// Auto Grade highlight hold, the soft-exposure half of a split-grade print (Agfa
    /// US 4,104,069 / 3,839,036): the textured bright tail (this percentile) must print at
    /// least `HIGHLIGHT_HOLD_DENSITY`, met by an automatic highlight-zone burn (never a lift)
    /// capped at `HIGHLIGHT_HOLD_MAX`. 0 = off.
    pub const HIGHLIGHT_HOLD_PERCENTILE: f64 = 2.0;
    /// Density the highlight hold tail must print (0 = off).
    pub const HIGHLIGHT_HOLD_DENSITY: f64 = 0.10;
    /// Cap on the automatic highlight burn.
    pub const HIGHLIGHT_HOLD_MAX: f64 = 0.5;
    /// Zone Density actuator constants (NegPy models.py zone_density_*): the hold burn rides
    /// upstream's Zone Density highlight term
    /// `v += burn · (1 − σ(sharpness · (v − (anchorTarget + highlightOffset))))`.
    /// We ship no user Zone Density sliders — the term exists for the auto burn only.
    /// Mirrored as literals in every kernel (4.0 / 0.40) — keep them in sync.
    pub const ZONE_DENSITY_SHARPNESS: f64 = 4.0;
    /// Zone Density highlight-centre offset from the anchor target.
    pub const ZONE_DENSITY_HIGHLIGHT_OFFSET: f64 = -0.40;
    /// Variable-gamma paper S-curve (0.15 → 0.05 in 8532dd92: the Snap bell pushed system
    /// gamma over Buhr's envelope near scene D 0.8).
    pub const PAPER_MIDTONE_GAMMA: f64 = 0.05;
    /// Paper S-curve width.
    pub const PAPER_GAMMA_WIDTH: f64 = 0.6;

    // ── Regional tone controls (SwiftInvert addition, no NegPy equivalent) ──────
    // Shadow/highlight lift and per-region contrast operate on print density v (after the
    // curve core + midtone gamma, before regional CMY and the toe/shoulder bounds) with
    // smooth sigmoid region masks:
    //   w_shadow    = σ(sharpness · (v − shadowToneAnchor))
    //   w_highlight = σ(sharpness · (highlightToneAnchor − v))
    // Amplitudes are bounded so each control alone keeps the transfer monotone
    // (sharpness · maxAmount / 4 < 1). Mirrored in the kernels — keep in sync.

    /// Sigmoid sharpness of the tone-region masks.
    pub const TONE_REGION_SHARPNESS: f64 = 3.5;
    /// Density anchor of the "medium darks" (shadow region centre).
    pub const SHADOW_TONE_ANCHOR: f64 = 1.40;
    /// Density anchor of the highlight region centre.
    pub const HIGHLIGHT_TONE_ANCHOR: f64 = 0.30;
    /// Dark Shadows: a second, deeper lift band — the Shadows anchor (1.40) covers broad
    /// darks; this targets the deepest printable tones near d_max. Same sharpness/amplitude,
    /// so the same monotone bounds apply.
    pub const DARK_SHADOW_TONE_ANCHOR: f64 = 1.85;
    /// Density swing of the Shadows slider at ±1 (positive slider = lift). Slider ranges ±2:
    /// still monotone alone (2 · 0.5 · sharpness/4 = 0.875 < 1).
    pub const SHADOWS_MAX_LIFT: f64 = 0.5;
    /// Density swing of the Highlights slider at ±1 (negative slider = recover).
    pub const HIGHLIGHTS_MAX_SHIFT: f64 = 0.4;
    /// Max extra slope within the shadow region at contrast slider ±1. The shadow contrast
    /// slider ranges −3…+3: positive values only steepen the curve, so headroom there cannot
    /// break monotonicity; the negative side is remapped in `derive_render_params` so slider
    /// −3 lands exactly on the monotone floor, and the kernels clamp at that floor as a hard
    /// invariant.
    pub const SHADOW_CONTRAST_MAX: f64 = 0.5;
    /// Max extra slope within the highlight region at contrast slider ±1.
    pub const HIGHLIGHT_CONTRAST_MAX: f64 = 0.5;
    /// Most negative effective shadow-contrast gain that keeps the transfer monotone
    /// (1 − 0.8·max|d/dv[(v−c)·w]| > 0 at sharpness 3.5).
    pub const SHADOW_CONTRAST_NEG_FLOOR: f64 = -0.8;
    /// Overall contrast at slider ±1: the print curve rotates around the reference tone,
    /// `v → v + k·(v − v*)`, folded into slopes/pivots/curvatures (no shader involvement).
    /// Slider range −1…+2 → k ∈ [−0.5, +1].
    pub const OVERALL_CONTRAST_MAX: f64 = 0.5;
    /// Print Saturation's matrix-coefficient clamp (NegPy per_channel_dye_separation — their
    /// surviving density-saturation control, renamed in 3fb5ca8).
    pub const PRINT_SATURATION_MAX: f64 = 3.0;
    /// Separation Damping's reference spread (NegPy separation_damping_ref_spread, d86a5aa):
    /// the per-pixel chroma left at exactly gain 1.0 when damping is full — above it the
    /// push reverses. Inlined as a kernel literal — change both.
    pub const SEPARATION_DAMPING_REF_SPREAD: f64 = 0.35;
    /// One photographic stop in log10 density.
    pub const LOG10_TWO: f64 = std::f64::consts::LOG10_2;

    /// Rec.709 luma weight, red (negpy/domain/types.py).
    pub const LUMA_R: f64 = 0.2126;
    /// Rec.709 luma weight, green.
    pub const LUMA_G: f64 = 0.7152;
    /// Rec.709 luma weight, blue.
    pub const LUMA_B: f64 = 0.0722;
}

/// Working-space OETF: Adobe RGB (1998) TRC — a pure 563/256 power, no linear segment
/// (negpy/kernel/image/logic.py `working_oetf_encode`/`decode`, since b3490eb: the working
/// space moved ProPhoto→Adobe RGB because the pipeline ASSIGNS primaries to sensor-native
/// data at output, and ProPhoto's imaginary primaries inflated chroma and skewed hues).
///
/// The pixel path runs in `f32` like the Swift `Float` version; `f64` twins serve the
/// densitometer and the levels inverse-remap, which do their math in `Double`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkingOetf;

impl WorkingOetf {
    /// 563/256 = 2.19921875 — Adobe RGB's exact rational gamma.
    pub const GAMMA: f64 = 563.0 / 256.0;
    /// 1/GAMMA = 256/563 (the kernel literal 0.45470693).
    pub const INV_GAMMA: f64 = 256.0 / 563.0;
    /// The gamma as the `f32` the pixel path uses (exactly representable: 2 + 51/256).
    pub const GAMMA_F32: f32 = 563.0 / 256.0;

    /// Linear → encoded, `f32` pixel path (negative inputs clamp to 0).
    #[inline]
    pub fn encode(x: f32) -> f32 {
        // Boundary cast: the exponent is exact in f32 (2.19921875 = 563/256 is dyadic).
        x.max(0.0).powf(1.0 / Self::GAMMA_F32)
    }

    /// Encoded → linear, `f32` pixel path.
    #[inline]
    pub fn decode(e: f32) -> f32 {
        e.max(0.0).powf(Self::GAMMA_F32)
    }

    /// Linear → encoded in `f64`.
    #[inline]
    pub fn encode_f64(x: f64) -> f64 {
        x.max(0.0).powf(Self::INV_GAMMA)
    }

    /// Encoded → linear in `f64`.
    #[inline]
    pub fn decode_f64(e: f64) -> f64 {
        e.max(0.0).powf(Self::GAMMA)
    }
}
