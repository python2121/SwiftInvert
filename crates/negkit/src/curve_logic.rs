//! Print-curve parameter derivation — port of `CurveLogic.swift` (NegPy
//! `negpy/features/exposure/logic.py`: grade math, pivot solve, cast removal, WB filtration;
//! the 8532dd92 Auto Grade / Shadow Reach / Highlight Hold helpers; d86a5aa separation
//! damping). Single source of truth for the GPU uniforms and the CPU reference curve.
//!
//! All math in `f64` (Swift `Double`). Every function is total: no input panics.

use crate::{LogNegativeBounds, V3, constants::K};

/// The per-channel print-curve core `(slopes, pivots, curvatures)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveParams {
    /// Straight-line slope per channel (clamped `K::SLOPE_MIN..=K::SLOPE_MAX`).
    pub slopes: V3,
    /// Normalized-log input that prints at the reference density, per channel.
    pub pivots: V3,
    /// Quadratic term per channel (0 outside the neutral-axis mode).
    pub curvatures: V3,
}

/// The measured neutral axis, already normalized into the floor→ceil stretch
/// (`normalize_refs`): per-band R/G/B positions of the film's greys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NeutralAxisNorm {
    /// Mid band refs.
    pub mid: V3,
    /// Shadow band refs.
    pub shadow: V3,
    /// Highlight band refs when the band had trustworthy neutrals (enables the quadratic).
    pub highlight: Option<V3>,
}

/// Grade-coupled knee pair returned by [`grade_coupled_shape`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KneeShape {
    /// Effective toe slider (user toe + grade coupling).
    pub toe: f64,
    /// Effective shoulder slider (user shoulder + grade coupling).
    pub shoulder: f64,
}

/// Swift's `min(max(x, lo), hi)`: a NaN stays NaN (Rust's `f64::max` would swallow it).
#[inline]
fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Separation Damping's per-pixel effective k (NegPy `separation_damping_gain`): the
/// frame-wide printSaturation k tapered by the pixel's own RMS dye-density spread.
/// `h = (ref − c)/(ref + c)` runs from 1 at grey to −1 at extreme separation, so at damping 1
/// muted colour takes the full k, the reference spread stays at exactly 1.0 and vivid colour
/// gets 1/k. Clamped at 3 (bounds the k → 0 corner). Mirrored in the kernels — keep in sync.
pub fn separation_damping_gain(k: f64, damping: f64, chroma: f64) -> f64 {
    if k <= 0.0 {
        return 0.0;
    }
    let h =
        (K::SEPARATION_DAMPING_REF_SPREAD - chroma) / (K::SEPARATION_DAMPING_REF_SPREAD + chroma);
    k.powf((1.0 - damping) + damping * h).min(3.0)
}

/// Numerically stable softplus `log(1 + exp(x))`.
#[inline]
pub fn softplus(x: f64) -> f64 {
    if x > 0.0 {
        x + (-x).exp().ln_1p()
    } else {
        x.exp().ln_1p()
    }
}

/// Inverse softplus `log(exp(y) − 1)`, stable for y > 20.
#[inline]
pub fn inv_softplus(y: f64) -> f64 {
    if y > 20.0 {
        y
    } else {
        y.max(1e-12).exp_m1().ln()
    }
}

/// Logistic sigmoid.
#[inline]
pub fn sigmoid(x: f64) -> f64 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let z = x.exp();
        z / (1.0 + z)
    }
}

/// `cmy_to_density`: slider units → density, scaled by the channel's log range.
pub fn cmy_to_density(val: f64, log_range: f64) -> f64 {
    val * K::CMY_MAX_DENSITY / log_range.max(1e-6)
}

/// `density_to_cmy`: the inverse of [`cmy_to_density`].
pub fn density_to_cmy(density: f64, log_range: f64) -> f64 {
    density * log_range / K::CMY_MAX_DENSITY
}

/// Fallback density range when none is measured: a normal negative's, scaled by K
/// (`default_grade_range`, 8532dd92 form).
pub const fn default_grade_range() -> f64 {
    K::AUTO_GRADE_TARGET * K::AUTO_GRADE_NOMINAL_RATIO * K::AUTO_GRADE_NOMINAL_RANGE
}

/// `grade_to_slope`: `k = grade_contrast_scale * range / (ISO R / 100)`, clamped.
pub fn grade_to_slope(grade: f64, density_range: Option<f64>) -> f64 {
    let rng_in = density_range.unwrap_or(default_grade_range());
    let er = clamp(grade, K::ISO_R_MIN, K::ISO_R_MAX) / 100.0;
    let rng = clamp(rng_in.abs(), 0.3, 3.5);
    let k = K::GRADE_CONTRAST_SCALE * rng / er;
    clamp(k, K::SLOPE_MIN, K::SLOPE_MAX)
}

/// `slope_to_grade`: the inverse of [`grade_to_slope`] (non-positive slopes → softest grade).
pub fn slope_to_grade(slope: f64, density_range: Option<f64>) -> f64 {
    let rng_in = density_range.unwrap_or(default_grade_range());
    let rng = clamp(rng_in.abs(), 0.3, 3.5);
    if slope <= 0.0 {
        return K::ISO_R_MAX;
    }
    let er = K::GRADE_CONTRAST_SCALE * rng / slope;
    clamp(er * 100.0, K::ISO_R_MIN, K::ISO_R_MAX)
}

/// `grade_coupled_shape`: hard grades get snappier knees.
pub fn grade_coupled_shape(slope_g: f64, toe: f64, shoulder: f64) -> KneeShape {
    let slope_norm = clamp(
        (slope_g - K::SLOPE_MIN) / (K::SLOPE_MAX - K::SLOPE_MIN),
        0.0,
        1.0,
    );
    KneeShape {
        toe: toe + K::TOE_GRADE_STRENGTH * slope_norm,
        shoulder: shoulder + K::SHOULDER_GRADE_STRENGTH * slope_norm,
    }
}

/// `effective_grade_range` (8532dd92, Alkofer US 4,731,671). Auto Grade off: the measured
/// floor-to-ceil range, a fixed paper. On: the paper gamma follows the negative's textural
/// density scale, shrunk toward a normal negative's — `effective = K · floor_ceil ·
/// ((1−s) + s·n/t)` — so the printed textural range is (1−s) of the frame's own plus s of
/// the norm, capped at maxOverfill of the norm's print span (a grade whose paper range is far
/// shorter than the negative's clips both ends).
pub fn effective_grade_range(
    auto_normalize_contrast: bool,
    floor_ceil_range: Option<f64>,
    textural_range: Option<f64>,
) -> Option<f64> {
    if !auto_normalize_contrast {
        return floor_ceil_range;
    }
    let (Some(textural), Some(floor_ceil)) = (textural_range, floor_ceil_range) else {
        return Some(default_grade_range());
    };
    let measured = textural.abs();
    if measured < 1e-6 {
        return Some(3.5);
    }
    let q = K::AUTO_GRADE_NOMINAL_RANGE / measured;
    let s = K::AUTO_GRADE_STRENGTH;
    let factor = ((1.0 - s) + s * q).min(q * K::AUTO_GRADE_MAX_OVERFILL);
    Some(K::AUTO_GRADE_TARGET * floor_ceil.abs() * factor)
}

/// `_reference_linear_value`: straight-line density v* that the base toe/shoulder bounds map
/// onto `target` (`None` = `K::ANCHOR_TARGET_DENSITY`; Shadow Reach and Highlight Hold pass
/// their own — closed form via inverse softplus).
pub fn reference_linear_value(d_min: f64, target: Option<f64>) -> f64 {
    let t = target.unwrap_or(K::ANCHOR_TARGET_DENSITY);
    let a_hl = K::SHOULDER_SHARPNESS_BASE;
    let a_sh = K::TOE_SHARPNESS_BASE;
    let v1 = K::D_MAX - inv_softplus(a_sh * (K::D_MAX - t)) / a_sh;
    d_min + inv_softplus(a_hl * (v1 - d_min)) / a_hl
}

/// `shadow_reach_slope` (8532dd92; Gindele US 7,113,649, Ajewole US 5,046,118): Auto
/// Grade's floor on the slope — the textured dark tail at `shadow_point` must print at least
/// `K::SHADOW_REACH_DENSITY` while the anchor stays at its target, so the slope is raised to
/// the straight line through both when the grade alone falls short. Never lowered.
pub fn shadow_reach_slope(slope: f64, anchor: f64, shadow_point: f64, d_min: f64) -> f64 {
    let span = shadow_point - anchor;
    if span <= 1e-6 {
        return slope;
    }
    let v_black = reference_linear_value(d_min, Some(K::SHADOW_REACH_DENSITY));
    let needed = (v_black - reference_linear_value(d_min, None)) / span;
    slope.max(needed).min(K::SLOPE_MAX)
}

/// `highlight_hold_offset` (8532dd92): Auto Grade's soft exposure — the highlight-zone burn
/// that lands the textured bright tail at `highlight_point` on `K::HIGHLIGHT_HOLD_DENSITY`
/// when the straight line would print it brighter. Solved against the zone term's own weight
/// at that tone, so the burn lands exactly and stays under the shoulder. Never lifts; 0 when
/// the tone already holds (or the target is 0 = off).
pub fn highlight_hold_offset(slope: f64, pivot: f64, highlight_point: f64, d_min: f64) -> f64 {
    let target = K::HIGHLIGHT_HOLD_DENSITY;
    if target <= 0.0 {
        return 0.0;
    }
    let v = slope * (highlight_point - pivot);
    let v_hold = reference_linear_value(d_min, Some(target));
    if v >= v_hold {
        return 0.0;
    }
    let z_hi = K::ANCHOR_TARGET_DENSITY + K::ZONE_DENSITY_HIGHLIGHT_OFFSET;
    let w = 1.0 - sigmoid(K::ZONE_DENSITY_SHARPNESS * (v - z_hi));
    ((v_hold - v) / w.max(1e-6)).min(K::HIGHLIGHT_HOLD_MAX)
}

/// `compute_pivot`: solve so the reference tone (`anchor`, `None` = `K::ASSUMED_ANCHOR`)
/// prints at `K::ANCHOR_TARGET_DENSITY`, then shift by the density slider.
pub fn compute_pivot(slope: f64, density: f64, d_min: f64, anchor: Option<f64>) -> f64 {
    let r = anchor.unwrap_or(K::ASSUMED_ANCHOR);
    let v_star = reference_linear_value(d_min, None);
    let base = r - v_star / slope;
    base + (1.0 - density) * K::DENSITY_MULTIPLIER
}

/// `normalize_refs`: raw log refs → normalized position in the floor→ceil stretch.
pub fn normalize_refs(refs: V3, bounds: &LogNegativeBounds) -> V3 {
    let eps = 1e-6;
    let mut out = [0.0; 3];
    for ch in 0..3 {
        let mut denom = bounds.ceils[ch] - bounds.floors[ch];
        if denom.abs() < eps {
            denom = if denom >= 0.0 { eps } else { -eps };
        }
        out[ch] = (refs[ch] - bounds.floors[ch]) / denom;
    }
    out
}

/// `effective_cast_strength`: the neutral-set confidence always biases the slider (NegPy
/// 0.36 dropped the auto toggle — clean greys get full strength, ambiguous frames get
/// gentler correction).
pub fn effective_cast_strength(strength: f64, confidence: Option<f64>) -> f64 {
    match confidence {
        Some(c) => c * strength,
        None => strength,
    }
}

/// `filtration_offsets`: WB sliders → normalized-space per-channel offsets (density per
/// slider unit divided by each channel's stretch range when bounds are known).
pub fn filtration_offsets(wb_cmy: V3, bounds: Option<&LogNegativeBounds>) -> V3 {
    let mut out = [0.0; 3];
    for ch in 0..3 {
        let mut d = wb_cmy[ch] * K::CMY_MAX_DENSITY;
        if let Some(b) = bounds {
            d /= (b.ceils[ch] - b.floors[ch]).abs().max(1e-6);
        }
        out[ch] = d;
    }
    out
}

/// `per_channel_curve_params`: `(slopes, pivots, curvatures)` — the heart of C-41 gray
/// balance. Green is the reference; R/B fit its neutral axis (quadratic through 3
/// green-matched points, else 2-point line, else the one-point shadow-ref tie, else a shared
/// linear curve).
///
/// `lum_range` is the PRE-offset base luminance density range (NegPy quirk); `anchor` `None`
/// means auto-exposure off (`K::ASSUMED_ANCHOR`); `shadow_point` feeds Shadow Reach (Auto
/// Grade only).
pub fn per_channel_curve_params(
    grade: f64,
    density: f64,
    auto_normalize_contrast: bool,
    strength: f64,
    lum_range: Option<f64>,
    shadow_refs_norm: Option<V3>,
    textural_range: Option<f64>,
    d_min: f64,
    anchor: Option<f64>,
    neutral_axis_norm: Option<NeutralAxisNorm>,
    shadow_point: Option<f64>,
) -> CurveParams {
    let r_eff = effective_grade_range(auto_normalize_contrast, lum_range, textural_range);
    let mut base_slope = grade_to_slope(grade, r_eff);
    // Shadow Reach rides Auto Grade only: the dark tail's floor raises the shared base slope
    // before any per-channel cast solve.
    if auto_normalize_contrast && let Some(sp) = shadow_point {
        base_slope = shadow_reach_slope(base_slope, anchor.unwrap_or(K::ASSUMED_ANCHOR), sp, d_min);
    }
    let eps = 1e-6;

    if strength > 0.0
        && let Some(na) = neutral_axis_norm
    {
        let limit = K::MIDTONE_CAST_MAX_OFFSET;
        let curv_lim = K::NEUTRAL_AXIS_CURV_MAX_RATIO;
        let m_g = na.mid[1];
        let s_g = na.shadow[1];
        let slope_g = clamp(base_slope, K::SLOPE_MIN, K::SLOPE_MAX);
        let pivot_g = compute_pivot(slope_g, density, d_min, anchor);
        let target = |g: f64| slope_g * (g - pivot_g);
        let t_m = target(m_g);
        let t_s = target(s_g);
        let h_g: Option<f64> = na.highlight.map(|h| h[1]);
        let clamp_dev = |g: f64, v: f64| g + clamp(strength * (v - g), -limit, limit);

        let mut slopes = [0.0; 3];
        let mut pivots = [0.0; 3];
        let mut curvs = [0.0; 3];
        for ch in 0..3 {
            if ch == 1 {
                slopes[ch] = slope_g;
                pivots[ch] = pivot_g;
                curvs[ch] = 0.0;
                continue;
            }
            let u_m = clamp_dev(m_g, na.mid[ch]);
            let u_s = clamp_dev(s_g, na.shadow[ch]);

            let mut curv = 0.0;
            if let (Some(h_g), Some(hl)) = (h_g, na.highlight) {
                let u_h = clamp_dev(h_g, hl[ch]);
                // Leading coefficient of the quadratic through the three green-matched points
                // (divided differences form of the 3×3 Vandermonde solve in the Python
                // original; a singular system there yields curv = 0, mirrored by the guard).
                if (u_h - u_m).abs() > eps && (u_m - u_s).abs() > eps && (u_h - u_s).abs() > eps {
                    let d1 = (target(h_g) - t_m) / (u_h - u_m);
                    let d2 = (t_m - t_s) / (u_m - u_s);
                    curv = (d1 - d2) / (u_h - u_s);
                }
                curv = clamp(curv, -curv_lim * slope_g, curv_lim * slope_g);
            }

            let du = u_m - u_s;
            let mut slope_ch = if du.abs() < eps {
                slope_g
            } else {
                ((t_m - t_s) - curv * (u_m * u_m - u_s * u_s)) / du
            };
            slope_ch = clamp(slope_ch, K::SLOPE_MIN, K::SLOPE_MAX);
            let curv_ch = curv;
            let pivot_ch = if slope_ch.abs() > eps {
                u_m - (t_m - curv_ch * u_m * u_m) / slope_ch
            } else {
                pivot_g
            };
            slopes[ch] = slope_ch;
            pivots[ch] = pivot_ch;
            curvs[ch] = curv_ch;
        }
        return CurveParams {
            slopes,
            pivots,
            curvatures: curvs,
        };
    }

    if strength > 0.0
        && let Some(refs) = shadow_refs_norm
    {
        let anchor_val = anchor.unwrap_or(K::ASSUMED_ANCHOR);
        let limit = K::CAST_REMOVAL_MAX_OFFSET;
        let r_green = refs[1];
        let numer = anchor_val - r_green;

        let mut slopes = [0.0; 3];
        let mut pivots = [0.0; 3];
        for ch in 0..3 {
            let cast = clamp(strength * (r_green - refs[ch]), -limit, limit);
            let denom = anchor_val - (r_green - cast);
            let mut slope_ch = if ch == 1 || denom.abs() < eps {
                base_slope
            } else {
                clamp(base_slope * numer / denom, K::SLOPE_MIN, K::SLOPE_MAX)
            };
            slope_ch = clamp(slope_ch, K::SLOPE_MIN, K::SLOPE_MAX);
            slopes[ch] = slope_ch;
            pivots[ch] = compute_pivot(slope_ch, density, d_min, anchor);
        }
        return CurveParams {
            slopes,
            pivots,
            curvatures: [0.0; 3],
        };
    }

    let s = clamp(base_slope, K::SLOPE_MIN, K::SLOPE_MAX);
    let p = compute_pivot(s, density, d_min, anchor);
    CurveParams {
        slopes: [s; 3],
        pivots: [p; 3],
        curvatures: [0.0; 3],
    }
}
