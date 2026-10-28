//! Zone placement — port of `ZonePlacement.swift` (NegPy 5a095f3 + 9dff124,
//! `features/exposure/placement.py`): solve Brightness (and Grade, and one knee control) so
//! pinned tones print on their target zones. Inverts the render's own forward model by
//! bisection — the composite is monotone in each solved control but not analytically
//! invertible.
//!
//! The scalar forward model is the ACHROMATIC green-reference curve, exactly as upstream's
//! `predicted_zone` (their chart's own curve): the pin freezes its normalized-log luma once,
//! and candidate settings re-derive the curve around it. Ours runs the pixel through the real
//! kernel ([`crate::reference`] with the params collapsed to the green channel) rather than a
//! re-derived scalar copy, so the model can never fork from the render. Divergence from
//! upstream, on purpose: the green cmyOffset stays in (it carries `exposure_stops`, a
//! SwiftInvert-only global brightness control the model must see; upstream has no such control
//! and omits filtration entirely), and so does the green levels remap (same reasoning — it
//! reshapes the displayed tone the probe reads).

use crate::constants::K;
use crate::derive::derive_render_params;
use crate::kernel::ExposureAnalysis;
use crate::{ExposureSettings, RgbImage, V3, densitometry, reference};

// ───────────────────────────── Pins ─────────────────────────────

/// Density, grade, one knee control — a fourth pin has nothing left to solve.
pub const MAX_PINS: usize = 3;

/// One probed spot: content-normalized position, frozen normalized-log sample, and the zone
/// the user wants it to print on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pin {
    pub nx: f64,
    pub ny: f64,
    /// Normalized-log RGB at the spot (mean of the sampling patch).
    pub val_rgb: V3,
    /// Rec.709 luma of `val_rgb` — what the solve actually places.
    pub val_luma: f64,
    pub target_zone: f64,
    /// Set once a zone is asked for: a dragged pin keeps it instead of re-reading (upstream
    /// `retargeted`).
    pub retargeted: bool,
    /// Contrast Mask plane sample at the pin's position (0 with no mask). The plane is
    /// spatial, which a 1-px forward model cannot see; the sample is frozen with the pin (the
    /// plane doesn't depend on the mask gamma, and any other edit clears the placement) and
    /// enters the model as a constant val addend — the solve stays exact.
    pub mask_val: f64,
}

impl Pin {
    /// A pin with `retargeted = false` and no mask sample (the Swift init's defaults).
    pub fn new(nx: f64, ny: f64, val_rgb: V3, val_luma: f64, target_zone: f64) -> Self {
        Self {
            nx,
            ny,
            val_rgb,
            val_luma,
            target_zone,
            retargeted: false,
            mask_val: 0.0,
        }
    }
}

// ───────────────────────────── Solve domain (mirrors the sliders) ─────────────────────────────

/// Brightness slider −3…4 ↔ density = 2 − b (right = brighter): density's solve bracket.
pub const DENSITY_RANGE: (f64, f64) = (-2.0, 5.0);
// Half the sliders' precision: bisecting finer than the 0.01 / 1 R rounding is wasted work.
const DENSITY_RESOLUTION: f64 = 0.005;
const GRADE_RESOLUTION: f64 = 0.5;
const CLAMP_TOL: f64 = 1e-6;
const DEGENERATE_VAL_GAP: f64 = 1e-3;

/// The tone-control knee a third pin can be solved on (the Swift key paths).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KneeControl {
    ShadowContrast,
    HighlightContrast,
}

impl KneeControl {
    /// Read the control from `settings`.
    pub fn get(self, settings: &ExposureSettings) -> f64 {
        match self {
            Self::ShadowContrast => settings.shadow_contrast,
            Self::HighlightContrast => settings.highlight_contrast,
        }
    }

    /// Write the control into `settings`.
    pub fn set(self, settings: &mut ExposureSettings, value: f64) {
        match self {
            Self::ShadowContrast => settings.shadow_contrast = value,
            Self::HighlightContrast => settings.highlight_contrast = value,
        }
    }
}

/// One control a third pin can be solved on: slider bounds, bisection resolution and commit
/// rounding (mirrors ControlsSidebar).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KneeCandidate {
    pub control: KneeControl,
    /// History/readout name — matches the slider label.
    pub label: &'static str,
    pub lo: f64,
    pub hi: f64,
    resolution: f64,
    digits: i32,
}

/// Upstream solves Shadows Grade / Highlights Grade / Snap. We don't ship those controls
/// (recorded Split Grade / Zone Density skip) — the measured-purchase picker is
/// control-agnostic, so the candidates are our convergent tone-control knees instead. No
/// midtone candidate: our only midtone contrast is global (`overall_contrast`), which the
/// 2-pin grade solve already owns.
pub const KNEE_CANDIDATES: [KneeCandidate; 2] = [
    KneeCandidate {
        control: KneeControl::ShadowContrast,
        label: "Shadow contrast",
        lo: -3.0,
        hi: 6.0,
        resolution: 0.005,
        digits: 2,
    },
    KneeCandidate {
        control: KneeControl::HighlightContrast,
        label: "Highlight contrast",
        lo: -1.0,
        hi: 1.0,
        resolution: 0.005,
        digits: 2,
    },
];

/// Fraction of a candidate's range perturbed to measure its purchase.
const KNEE_PROBE: f64 = 0.2;
/// Zones; below this the control cannot place the pin.
const KNEE_SENSITIVITY_FLOOR: f64 = 0.05;
/// A third of a ⅓-zone step: on target, and off-target honestly.
pub const ON_TARGET_TOL: f64 = 1.0 / 6.0;

// ───────────────────────────── Forward model ─────────────────────────────

/// Zone the achromatic print curve puts `val_luma` on under `settings`.
///
/// The green channel's exact kernel trajectory: derive, collapse every per-channel param to
/// green, run the 1-px pixel through the real print curve + encode, read the zone ruler.
/// `mask_val` is the pin's frozen Contrast Mask plane sample (0 without a mask).
pub fn predicted_zone(
    settings: &ExposureSettings,
    analysis: &ExposureAnalysis,
    val_luma: f64,
    mask_val: f64,
) -> f64 {
    let mut p = derive_render_params(settings, analysis);
    p.slopes = [p.slopes[1]; 3];
    p.pivots = [p.pivots[1]; 3];
    p.curvatures = [p.curvatures[1]; 3];
    p.cmy_offsets = [p.cmy_offsets[1]; 3];
    p.shadow_cmy = [p.shadow_cmy[1]; 3];
    p.mid_cmy = [p.mid_cmy[1]; 3];
    p.highlight_cmy = [p.highlight_cmy[1]; 3];
    let green_levels = p.levels_points[1].clone();
    p.levels_points = [green_levels.clone(), green_levels.clone(), green_levels];
    // Contrast Mask: the pin's frozen plane sample × the green scale is a constant val
    // addend — exactly what the kernels add at this pixel. Boundary cast: the pixel is Float.
    let v = (val_luma + p.mask_val_scale[1] * mask_val) as f32;
    let mut px = RgbImage {
        width: 1,
        height: 1,
        data: vec![v, v, v],
    };
    reference::apply_print_curve_in_place(&mut px, &p, None);
    reference::encode_output_in_place(&mut px, &p.levels_points, false);
    let green = px.data.get(1).copied().unwrap_or(0.0);
    densitometry::zone_of_encoded(f64::from(green))
}

// ───────────────────────────── Solution ─────────────────────────────

/// The outcome of [`solve`].
#[derive(Clone, Debug, PartialEq)]
pub struct Solution {
    /// The post-Apply settings: solved fields rounded to slider precision, autos off. Preview
    /// renders these; Apply commits them.
    pub settings: ExposureSettings,
    /// Zone per pin at the rounded settings, pin order.
    pub achieved: Vec<f64>,
    /// A target sat outside a slider's reach, or the knee iteration settled short of every
    /// ask.
    pub clamped: bool,
    /// Label of the knee control a third pin was solved on; `None` when none was needed (or
    /// none had purchase).
    pub knee_label: Option<&'static str>,
}

/// 1 pin: Brightness. 2 pins: Brightness + Grade from the outer tones. 3 pins: those two plus
/// one knee control for the middle one. Solved against the post-Apply settings (autos off),
/// clamped to the slider ranges, rounded to the sliders' precision with achieved zones
/// recomputed at the rounded values. `None` for 0 or more than [`MAX_PINS`] pins, or when the
/// outer tones are too close to tell apart.
pub fn solve(
    settings: &ExposureSettings,
    analysis: &ExposureAnalysis,
    pins: &[Pin],
) -> Option<Solution> {
    let mut knee_label: Option<&'static str> = None;
    let solved: ExposureSettings;
    let mut clamped: bool;
    match pins {
        [pin] => {
            let mut candidate = settings.clone();
            candidate.auto_exposure = false;
            let (density, c) = solve_density(&candidate, analysis, pin);
            candidate.density = round2(density);
            solved = candidate;
            clamped = c;
        }
        [_, _] | [_, _, _] => {
            // Dark pin = highest normalized-log luma (dense side of the print).
            let mut ordered = pins.to_vec();
            ordered.sort_by(|a, b| {
                b.val_luma
                    .partial_cmp(&a.val_luma)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let (dark, light) = (*ordered.first()?, *ordered.last()?);
            if (dark.val_luma - light.val_luma).abs() < DEGENERATE_VAL_GAP {
                return None;
            }
            let mut candidate = settings.clone();
            candidate.auto_exposure = false;
            candidate.auto_normalize_contrast = false;
            let grade: f64;
            let density: f64;
            if let Some(mid) = (ordered.len() == 3)
                .then(|| ordered.get(1).copied())
                .flatten()
            {
                let r = solve_with_knee(&candidate, analysis, &dark, &light, &mid);
                candidate = r.candidate;
                grade = r.grade;
                density = r.density;
                clamped = r.clamped;
                if let Some(knee) = r.knee {
                    knee_label = Some(knee.label);
                    let v = knee.control.get(&candidate);
                    knee.control.set(&mut candidate, rounded(v, knee.digits));
                }
            } else {
                let r = solve_grade_and_density(&candidate, analysis, &dark, &light);
                grade = r.0;
                density = r.1;
                clamped = r.2;
            }
            candidate.density = round2(density);
            candidate.grade = grade.round();
            solved = candidate;
        }
        _ => return None,
    }
    let achieved: Vec<f64> = pins
        .iter()
        .map(|p| predicted_zone(&solved, analysis, p.val_luma, p.mask_val))
        .collect();
    // The knee iteration can settle short of every ask: say so rather than report the ask.
    if knee_label.is_some()
        && achieved
            .iter()
            .zip(pins)
            .any(|(a, p)| (a - p.target_zone).abs() > ON_TARGET_TOL)
    {
        clamped = true;
    }
    Some(Solution {
        settings: solved,
        achieved,
        clamped,
        knee_label,
    })
}

// ───────────────────────────── Inner solves ─────────────────────────────

fn solve_density(
    candidate: &ExposureSettings,
    analysis: &ExposureAnalysis,
    pin: &Pin,
) -> (f64, bool) {
    let residual = |density: f64| {
        let mut placed = candidate.clone();
        placed.density = density;
        predicted_zone(&placed, analysis, pin.val_luma, pin.mask_val) - pin.target_zone
    };
    bisect_decreasing(
        residual,
        DENSITY_RANGE.0,
        DENSITY_RANGE.1,
        DENSITY_RESOLUTION,
    )
}

/// Outer bisection on grade against the light pin, inner density solve pinning the dark pin
/// at every candidate grade. Softer (higher R) lowers the light pin's zone with the dark pin
/// held, so the residual is decreasing in R. Returns `(grade, density, clamped)`.
fn solve_grade_and_density(
    candidate: &ExposureSettings,
    analysis: &ExposureAnalysis,
    dark: &Pin,
    light: &Pin,
) -> (f64, f64, bool) {
    let light_residual = |grade: f64| {
        let mut graded = candidate.clone();
        graded.grade = grade;
        let (density, _) = solve_density(&graded, analysis, dark);
        graded.density = density;
        predicted_zone(&graded, analysis, light.val_luma, light.mask_val) - light.target_zone
    };
    let (grade, grade_clamped) =
        bisect_decreasing(light_residual, K::ISO_R_MIN, K::ISO_R_MAX, GRADE_RESOLUTION);
    let mut graded = candidate.clone();
    graded.grade = grade;
    let (density, density_clamped) = solve_density(&graded, analysis, dark);
    (grade, density, grade_clamped || density_clamped)
}

struct KneeSolve {
    candidate: ExposureSettings,
    grade: f64,
    density: f64,
    knee: Option<KneeCandidate>,
    clamped: bool,
}

/// Place the outer tones, then the middle one on the knee control it can actually move — by
/// bisecting the knee against the COMPOSITE residual (the 2-pin grade+density solve nested
/// inside every evaluation).
///
/// Deliberate divergence from upstream, who alternate (place the extremes, place the middle,
/// repeat) to avoid nesting cost: their knee controls pivot at zone centres OUTSIDE the span
/// between typical pins, so their alternation converges. Ours pivot at the tone anchors,
/// which sit BETWEEN typical pin positions — the knee's raw effect on the mid pin and its net
/// effect after the outer re-solve can have opposite signs, and the alternation diverges to
/// the slider corners. Nested bisection converges unconditionally, and our µs-scale forward
/// model makes the multiplied cost single-digit milliseconds. Monotonicity also buys a
/// no-harm guarantee: when the ask is out of reach, the returned endpoint is the argmax —
/// never worse than leaving the knee alone.
fn solve_with_knee(
    start: &ExposureSettings,
    analysis: &ExposureAnalysis,
    dark: &Pin,
    light: &Pin,
    mid: &Pin,
) -> KneeSolve {
    let mut candidate = start.clone();
    let (mut grade, mut density, mut clamped) =
        solve_grade_and_density(&candidate, analysis, dark, light);
    let mut placed = candidate.clone();
    placed.grade = grade;
    placed.density = density;
    let landed = predicted_zone(&placed, analysis, mid.val_luma, mid.mask_val);
    if (landed - mid.target_zone).abs() <= ON_TARGET_TOL {
        return KneeSolve {
            candidate,
            grade,
            density,
            knee: None,
            clamped,
        };
    }

    // Mid pin's zone at a knee value, with the outer tones re-placed.
    let composite_zone = |base: &ExposureSettings, knee: &KneeCandidate, value: f64| {
        let mut c = base.clone();
        knee.control.set(&mut c, value);
        let (g, d, _) = solve_grade_and_density(&c, analysis, dark, light);
        c.grade = g;
        c.density = d;
        predicted_zone(&c, analysis, mid.val_luma, mid.mask_val)
    };

    // The control that moves this pin most WITH the outers held, or none when none of them
    // can. Measured, not inferred from the pin's zone (upstream's rule): each control's weight
    // is a sigmoid window on print density, and the contrasts also lose purchase at their own
    // anchor — a zone-number rule would have to mirror those constants and the working OETF.
    let mut best: Option<KneeCandidate> = None;
    let mut best_delta = KNEE_SENSITIVITY_FLOOR;
    for cand in &KNEE_CANDIDATES {
        let current = cand.control.get(&candidate);
        let step = KNEE_PROBE * (cand.hi - cand.lo);
        let probe = if current + step <= cand.hi {
            current + step
        } else {
            current - step
        };
        let delta = (composite_zone(&candidate, cand, probe) - landed).abs();
        if delta > best_delta {
            best = Some(*cand);
            best_delta = delta;
        }
    }
    let Some(knee) = best else {
        return KneeSolve {
            candidate,
            grade,
            density,
            knee: None,
            clamped,
        };
    };

    let (value, knee_clamped) = bisect_monotone(
        |v| composite_zone(&candidate, &knee, v) - mid.target_zone,
        knee.lo,
        knee.hi,
        knee.resolution,
    );
    knee.control.set(&mut candidate, value);
    (grade, density, clamped) = solve_grade_and_density(&candidate, analysis, dark, light);
    KneeSolve {
        candidate,
        grade,
        density,
        knee: Some(knee),
        clamped: clamped || knee_clamped,
    }
}

// ───────────────────────────── Bisection ─────────────────────────────

/// Root of a monotone `f`, direction read off the endpoints. The knee controls pivot at their
/// own anchor, so the sign of their effect flips with the pin's side of it — neither direction
/// can be assumed the way density and grade can.
fn bisect_monotone(f: impl Fn(f64) -> f64, lo: f64, hi: f64, resolution: f64) -> (f64, bool) {
    if f(lo) >= f(hi) {
        bisect_decreasing(f, lo, hi, resolution)
    } else {
        bisect_decreasing(|x| -f(x), lo, hi, resolution)
    }
}

/// Root of a decreasing `f` on `[lo, hi]` to within `resolution`; `(closest endpoint, true)`
/// when the target sits outside the bracket.
fn bisect_decreasing(f: impl Fn(f64) -> f64, lo: f64, hi: f64, resolution: f64) -> (f64, bool) {
    let f_lo = f(lo);
    if f_lo <= 0.0 {
        return (lo, f_lo < -CLAMP_TOL);
    }
    let f_hi = f(hi);
    if f_hi >= 0.0 {
        return (hi, f_hi > CLAMP_TOL);
    }
    let (mut lo, mut hi) = (lo, hi);
    // 64 halvings exceed f64's resolution on any slider range: the loop is bounded even if a
    // caller hands over a zero or NaN resolution.
    for _ in 0..64 {
        let span = hi - lo;
        if span.is_nan() || span <= resolution {
            break;
        }
        let mid = 0.5 * (lo + hi);
        if f(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (0.5 * (lo + hi), false)
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn rounded(v: f64, digits: i32) -> f64 {
    let s = 10f64.powi(digits);
    (v * s).round() / s
}
