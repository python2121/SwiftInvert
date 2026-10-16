//! `derive_render_params` — settings + analysis → the GPU uniform payload (µs per tick).
//! Port of `ExposureKernel.deriveRenderParams` / `sanitizeLevels` in `ExposureSettings.swift`
//! (NegPy PhotometricProcessor's parameter derivation; 8532dd92 Highlight Hold; 515c1f5
//! Contrast Mask scale; 2125a34 offset-independent analysis — offsets fold in HERE only).
//!
//! `RenderParams` is the shared contract consumed by `reference`, `neggpu::uniforms`, zone
//! placement and the test strip. Slider changes re-run only this and the render; analysis
//! never.

use crate::constants::K;
use crate::curve_logic::{self as cl, NeutralAxisNorm};
use crate::kernel::ExposureAnalysis;
use crate::settings::ExposureSettings;
use crate::{LogNegativeBounds, V2, V3, V4};

/// The full per-render parameter set. Field semantics mirror the Swift struct one-to-one;
/// see `ShaderTypes.swift` / `neggpu::uniforms` for how they pack.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderParams {
    /// Base bounds with the white/black-point offsets already folded in.
    pub final_bounds: LogNegativeBounds,
    pub slopes: V3,
    pub pivots: V3,
    pub curvatures: V3,
    /// Pre-curve offsets in normalized space: WB filtration (+ temp/tint) + exposure stops.
    pub cmy_offsets: V3,
    pub toe_eff: f64,
    pub shoulder_eff: f64,
    pub toe_width: f64,
    pub shoulder_width: f64,
    /// Paper base density (0.06 with `paperDmin`, else 0).
    pub d_min: f64,
    /// The reference tone's printed density (anchor target), the pivot of the paper-S and contrasts.
    pub v_star: f64,
    pub shadows: f64,
    pub shadow_contrast: f64,
    pub dark_shadows: f64,
    pub highlights: f64,
    pub highlight_contrast: f64,
    /// Highlight Hold's automatic burn (derive-computed, never a setting; 0 = off).
    pub auto_highlight: f64,
    pub vibrance: f64,
    pub saturation: f64,
    pub skin_protection: f64,
    /// Colour-mixer hue shifts, R/Y/G/B lanes.
    pub band_hues: V4,
    /// Colour-mixer saturation scales, R/Y/G/B lanes (1 = identity).
    pub band_saturations: V4,
    pub pre_saturation: f64,
    pub print_saturation: f64,
    pub separation_damping: f64,
    /// Hue Trim in radians.
    pub hue_trim: f64,
    /// Contrast Mask per-channel pre-curve scale (`−gamma·lumRange/range_ch`); all-zero gates the feature off.
    pub mask_val_scale: V3,
    pub true_black: bool,
    pub shadow_cmy: V3,
    pub mid_cmy: V3,
    pub highlight_cmy: V3,
    /// Levels-remap anchors per channel (R, G, B), pre-sanitized: sorted, 0.02 off the endpoints,
    /// monotone outputs, at most 8.
    pub levels_points: [Vec<V2>; 3],
}

/// Levels endpoint guard band (both axes, both ends).
pub const LEVELS_CLAMP: f64 = 0.02;
/// Max anchors per channel (the GPU buffer's fixed capacity).
pub const LEVELS_MAX_POINTS: usize = 8;
/// Contrast Mask gamma slider limit (`ContrastMask.gammaLimit`, ±0.5).
pub const CONTRAST_MASK_GAMMA_LIMIT: f64 = 0.5;
/// Temp's coupled magenta per unit yellow: NegPy's mired slopes `km/ky` (0.0029/0.0057).
pub const PLANCKIAN_RATIO: f64 = 0.0029 / 0.0057;

/// `ContrastMask.valScale` (NegPy 515c1f5): their `contrast_mask_scale` (stops per plane
/// unit × lumRange) folded with the dodge/burn EV scale (`log10(2)/range_ch`, the same
/// domain `exposureStops` rides), leaving `val_ch += −gamma · lumRange / range_ch · plane`.
/// All-zero at gamma 0 gates the whole feature.
pub fn mask_val_scale(gamma: f64, lum_range: f64, final_bounds: &LogNegativeBounds) -> V3 {
    if gamma == 0.0 {
        return [0.0; 3];
    }
    let mut out = [0.0; 3];
    for (o, (c, f)) in out
        .iter_mut()
        .zip(final_bounds.ceils.iter().zip(&final_bounds.floors))
    {
        let range = (c - f).abs().max(1e-6);
        *o = -gamma * lum_range / range;
    }
    out
}

/// Anchor hygiene the kernel relies on: sorted by input, clamped off the endpoints, inputs
/// strictly increasing (dupes dropped), outputs non-decreasing (the map stays monotone
/// whatever the UI hands over), capped at [`LEVELS_MAX_POINTS`]. All-identity sets (within
/// noise) collapse to empty so the kernels' empty fast path stays authoritative for "no remap".
pub fn sanitize_levels(points: &[V2]) -> Vec<V2> {
    if points.is_empty() {
        return Vec::new();
    }
    let (lo, hi) = (LEVELS_CLAMP, 1.0 - LEVELS_CLAMP);
    let mut sorted = points.to_vec();
    // Stable sort by input (Swift's `sorted(by:)` keeps the first of equal inputs too).
    sorted.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<V2> = Vec::with_capacity(sorted.len().min(LEVELS_MAX_POINTS));
    let (mut last_x, mut last_y) = (-1.0, 0.0);
    for p in sorted {
        let x = p[0].clamp(lo, hi);
        // A NaN input must be DROPPED (Swift's `guard x > lastX + 1e-4 else { continue }`).
        if x.is_nan() || x <= last_x + 1e-4 {
            continue;
        }
        let y = p[1].clamp(lo.max(last_y), hi);
        out.push([x, y]);
        last_x = x;
        last_y = y;
        if out.len() == LEVELS_MAX_POINTS {
            break;
        }
    }
    if out.iter().all(|p| (p[0] - p[1]).abs() < 1e-9) {
        Vec::new()
    } else {
        out
    }
}

/// PhotometricProcessor's parameter derivation (the cheap per-slider path). Order matters —
/// see CLAUDE.md §"Parameter derivation".
pub fn derive_render_params(
    settings: &ExposureSettings,
    analysis: &ExposureAnalysis,
) -> RenderParams {
    let final_bounds = analysis
        .base_bounds
        .applying_offsets(settings.white_point_offset, settings.black_point_offset);
    let d_min = if settings.paper_dmin { K::D_MIN } else { 0.0 };
    let anchor = if settings.auto_exposure {
        Some(analysis.anchor)
    } else {
        None
    };
    let strength =
        cl::effective_cast_strength(settings.cast_removal_strength, analysis.neutral_confidence);

    let neutral_axis_norm = match (analysis.neutral_mid, analysis.neutral_shadow) {
        (Some(mid), Some(shadow)) => Some(NeutralAxisNorm {
            mid: cl::normalize_refs(mid, &final_bounds),
            shadow: cl::normalize_refs(shadow, &final_bounds),
            highlight: analysis
                .neutral_highlight
                .map(|h| cl::normalize_refs(h, &final_bounds)),
        }),
        _ => None,
    };

    // NegPy computes norm_density_range before the wp/bp offsets are applied.
    let lum_range = analysis.base_bounds.luminance_density_range();
    let cl::CurveParams {
        mut slopes,
        mut pivots,
        mut curvatures,
    } = cl::per_channel_curve_params(
        settings.grade,
        settings.density,
        settings.auto_normalize_contrast,
        strength,
        Some(lum_range),
        Some(cl::normalize_refs(analysis.shadow_refs, &final_bounds)),
        Some(analysis.textural_range),
        d_min,
        anchor,
        neutral_axis_norm,
        analysis.shadow_point,
    );

    // Overall contrast: v → v + k·(v − v*) folded exactly into the core: slopes and
    // curvatures scale by (1+k); the pivot shifts by k·v*/s' so the reference tone's print
    // density is unchanged (and the midtone paper S stays centered, anchored at v* too).
    if settings.overall_contrast != 0.0 {
        let k = settings.overall_contrast * K::OVERALL_CONTRAST_MAX;
        let v_star = cl::reference_linear_value(d_min, None);
        for ch in 0..3 {
            let scaled = slopes[ch] * (1.0 + k);
            curvatures[ch] *= 1.0 + k;
            pivots[ch] += k * v_star / scaled;
            slopes[ch] = scaled;
        }
    }

    // Highlight Hold (8532dd92): the automatic highlight-zone burn, from the green reference
    // line — solved AFTER the overall-contrast fold (a SwiftInvert-only control upstream
    // lacks) so the hold's promise is kept against the line actually rendered; identical to
    // upstream at overallContrast 0, which is every parity config.
    let mut auto_highlight = 0.0;
    if settings.auto_normalize_contrast
        && let Some(hp) = analysis.highlight_point
    {
        auto_highlight = cl::highlight_hold_offset(slopes[1], pivots[1], hp, d_min);
    }

    // Temp rides the Planckian direction (yellow + coupled magenta, NegPy's mired slopes
    // km/ky), Tint the green↔magenta axis; both compose with the CMY trim sliders.
    let mut cmy_offsets = cl::filtration_offsets(
        [
            settings.wb_cyan,
            settings.wb_magenta + settings.tint + settings.temp * PLANCKIAN_RATIO,
            settings.wb_yellow + settings.temp,
        ],
        Some(&final_bounds),
    );
    // Global exposure: a uniform pre-curve print-exposure offset, one stop = −log10(2) over
    // each channel's stretch range (the local_ev_scale domain, like dodging the whole print).
    // Negative scale ⇒ positive stops brighten.
    if settings.exposure_stops != 0.0 {
        for (o, (c, f)) in cmy_offsets
            .iter_mut()
            .zip(final_bounds.ceils.iter().zip(&final_bounds.floors))
        {
            let range = (c - f).abs().max(1e-6);
            *o += settings.exposure_stops * (-K::LOG10_TWO / range);
        }
    }
    let knees = cl::grade_coupled_shape(slopes[1], settings.toe, settings.shoulder);

    // Negative shadow-contrast side remapped so slider −3 reaches exactly the monotone floor
    // (positive side passes through, scaled in the kernel).
    let shadow_contrast = if settings.shadow_contrast >= 0.0 {
        settings.shadow_contrast
    } else {
        settings.shadow_contrast
            * (K::SHADOW_CONTRAST_NEG_FLOOR.abs() / (3.0 * K::SHADOW_CONTRAST_MAX))
    };

    RenderParams {
        final_bounds,
        slopes,
        pivots,
        curvatures,
        cmy_offsets,
        toe_eff: knees.toe,
        shoulder_eff: knees.shoulder,
        toe_width: settings.toe_width,
        shoulder_width: settings.shoulder_width,
        d_min,
        v_star: cl::reference_linear_value(d_min, None),
        shadows: settings.shadows,
        shadow_contrast,
        dark_shadows: settings.dark_shadows,
        highlights: settings.highlights,
        highlight_contrast: settings.highlight_contrast,
        auto_highlight,
        vibrance: settings.vibrance,
        saturation: settings.saturation,
        skin_protection: settings.skin_protection.clamp(0.0, 1.0),
        band_hues: [
            settings.red_hue,
            settings.yellow_hue,
            settings.green_hue,
            settings.blue_hue,
        ],
        band_saturations: [
            settings.red_saturation,
            settings.yellow_saturation,
            settings.green_saturation,
            settings.blue_saturation,
        ],
        pre_saturation: settings.pre_saturation,
        print_saturation: settings
            .print_saturation
            .clamp(0.0, K::PRINT_SATURATION_MAX),
        separation_damping: settings.separation_damping.clamp(0.0, 1.0),
        hue_trim: settings.hue_trim.clamp(-30.0, 30.0) * std::f64::consts::PI / 180.0,
        // Contrast Mask: stops-per-plane-unit × the exposureStops EV domain, folded to one
        // per-channel multiplier. lumRange is the pre-offset base range (the same NegPy quirk
        // as grade's).
        mask_val_scale: mask_val_scale(
            settings
                .contrast_mask
                .clamp(-CONTRAST_MASK_GAMMA_LIMIT, CONTRAST_MASK_GAMMA_LIMIT),
            lum_range,
            &final_bounds,
        ),
        true_black: settings.true_black,
        // Band sliders ±1 → ±cmy_max_density print-density offsets (NegPy's shadow/highlight
        // CMY scale, plus a mids band).
        shadow_cmy: scale3(settings.color_shadows, K::CMY_MAX_DENSITY),
        mid_cmy: scale3(settings.color_mids, K::CMY_MAX_DENSITY),
        highlight_cmy: scale3(settings.color_highs, K::CMY_MAX_DENSITY),
        levels_points: [
            sanitize_levels(&settings.levels_red),
            sanitize_levels(&settings.levels_green),
            sanitize_levels(&settings.levels_blue),
        ],
    }
}

#[inline]
fn scale3(v: V3, k: f64) -> V3 {
    [v[0] * k, v[1] * k, v[2] * k]
}
