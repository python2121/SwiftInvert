//! `derive_render_params` — settings + analysis → the GPU uniform payload (µs per tick).
//! Port of `ExposureKernel.deriveRenderParams` / `sanitizeLevels` in `ExposureSettings.swift`.
//!
//! Owner: wave 1 / agent C. `RenderParams` is the shared contract consumed by `reference`,
//! `neggpu::uniforms`, zone placement and the test strip.

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
