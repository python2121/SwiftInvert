//! Uniform packing — mirrors `ShaderTypes.swift` (`NormUniforms`, `CurveUniforms`,
//! `UniformsBuilder`) byte for byte, so the WGSL structs in `negpipeline.wgsl`, the Metal/GLSL
//! mirrors and this crate share ONE layout. `tests/layout.rs` pins every offset and stride
//! (48 / 304 / 16 and the levels buffer's 51 floats), as `LayoutTests` / `VulkanLayoutTests` do
//! in Swift.
//!
//! The constants folded here (`K::TOE_SHOULDER_STRENGTH`, `K::D_MAX`, the knee sharpness
//! bases, the tone-control amplitudes, the paper-S gamma …) are exactly those
//! `UniformsBuilder.curveUniforms` folds, so `negkit::reference::PrintCurve::new` and the
//! kernels compute the same per-render scalars from the same inputs.

use bytemuck::{Pod, Zeroable};
use negkit::RenderParams;
use negkit::constants::K;

/// `flags` bit 0: re-express the encoded output as sRGB (display on an unmanaged canvas, the
/// sRGB export leg). Off = Adobe-RGB-encoded working space (every parity fixture).
pub const FLAG_SRGB: u32 = 1;

/// Floats the levels remap consumes: per channel c, `[c*16 + 0..8]` anchor inputs,
/// `[c*16 + 8..16]` anchor outputs, `[48 + c]` anchor count (`UniformsBuilder.levelsBufferFloats`).
pub const LEVELS_FLOATS: usize = 51;
/// The levels storage buffer's length as uploaded: 51 floats padded to a 16-byte multiple.
pub const LEVELS_BUFFER_FLOATS: usize = 52;

/// `normalize_log`'s block: the final (offset-folded) bounds; the wp/bp fields are legacy
/// zeros (`derive_render_params` folds the offsets into `final_bounds`). 48 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct NormUniforms {
    pub floors: [f32; 4],
    pub ceils: [f32; 4],
    pub wp_offset: f32,
    pub bp_offset: f32,
    pub _pad: [f32; 2],
}

/// `print_curve` / `color_pop`'s block. 304 bytes; field order is the layout contract
/// (`auto_highlight` at 268, `mask_scale` at 272, `mask_dims` at 288).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct CurveUniforms {
    pub pivots: [f32; 4],
    pub slopes: [f32; 4],
    pub curvatures: [f32; 4],
    pub cmy_offsets: [f32; 4],
    pub shadow_cmy: [f32; 4],
    pub mid_cmy: [f32; 4],
    pub highlight_cmy: [f32; 4],
    pub d_min_rgb: [f32; 4],
    pub toe: f32,
    pub shoulder: f32,
    pub toe_width: f32,
    pub shoulder_width: f32,
    pub d_max: f32,
    pub a_toe_base: f32,
    pub a_sh_base: f32,
    pub width_ref: f32,
    pub toe_height: f32,
    pub sh_height: f32,
    pub zone_center: f32,
    pub true_black: f32,
    pub dark_shadows_lift: f32,
    pub v_star: f32,
    pub midtone_gamma: f32,
    pub gamma_width: f32,
    pub shadows_lift: f32,
    pub shadow_contrast: f32,
    pub highlights_shift: f32,
    pub highlight_contrast: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub pre_saturation: f32,
    /// Post-curve density-space chroma (rides the ex-pad slot: layout stable).
    pub print_saturation: f32,
    /// Colour-mixer bands, R/Y/G/B lanes.
    pub band_hues: [f32; 4],
    pub band_saturations: [f32; 4],
    /// Separation Damping (0 = off).
    pub separation_damping: f32,
    /// Skin Protection rein strength (0 = off).
    pub skin_protection: f32,
    /// Hue Trim in radians.
    pub hue_trim: f32,
    /// Highlight Hold's automatic burn (offset 268).
    pub auto_highlight: f32,
    /// Contrast Mask: xyz = per-channel pre-curve scale for the plane sample
    /// (`−gamma·lumRange/range_ch`), w = 1 when the mask is active (the kernels' uniform gate).
    pub mask_scale: [f32; 4],
    /// Contrast Mask plane dims (x, y) and RENDER dims (z, w) — the kernel maps its flat
    /// pixel index to (x, y) through zw and the plane sample through xy.
    pub mask_dims: [f32; 4],
}

/// The per-dispatch scalars (a 16-byte uniform — push constants are native-only in wgpu).
/// `width` / `row_pitch` (pixels) serve `encode_u8`'s padded-row texture path; `row_pitch`
/// 0 = tightly packed.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct Push {
    pub n: u32,
    pub flags: u32,
    pub width: u32,
    pub row_pitch: u32,
}

/// `SIMD4<Float>(Float(v.x), Float(v.y), Float(v.z), w)` — boundary narrowing of the derive
/// `f64`s to the kernels' `f32`.
#[inline]
fn f4(v: [f64; 3], w: f32) -> [f32; 4] {
    [v[0] as f32, v[1] as f32, v[2] as f32, w]
}

/// `UniformsBuilder.normUniforms`.
pub fn pack_norm(params: &RenderParams) -> NormUniforms {
    NormUniforms {
        floors: f4(params.final_bounds.floors, 0.0),
        ceils: f4(params.final_bounds.ceils, 0.0),
        wp_offset: 0.0,
        bp_offset: 0.0,
        _pad: [0.0; 2],
    }
}

/// `UniformsBuilder.curveUniforms` plus the Vulkan host's `maskDims.zw` fill. `mask_dims` is
/// the Contrast Mask plane's (width, height) when a plane is bound; `None` leaves the mask
/// gated off even if the params carry a scale (no plane, no reads — the two travel together).
/// `render_dims` is the (width, height) of the frame being rendered.
pub fn pack_curve(
    params: &RenderParams,
    mask_dims: Option<(u32, u32)>,
    render_dims: (u32, u32),
) -> CurveUniforms {
    let d_min = params.d_min.max(0.0) as f32;
    let mut u = CurveUniforms {
        pivots: f4(params.pivots, 0.0),
        slopes: f4(params.slopes, 0.0),
        curvatures: f4(params.curvatures, 0.0),
        cmy_offsets: f4(params.cmy_offsets, 0.0),
        shadow_cmy: f4(params.shadow_cmy, 0.0),
        mid_cmy: f4(params.mid_cmy, 0.0),
        highlight_cmy: f4(params.highlight_cmy, 0.0),
        d_min_rgb: [d_min, d_min, d_min, 0.0],
        toe: (params.toe_eff * K::TOE_SHOULDER_STRENGTH) as f32,
        shoulder: (params.shoulder_eff * K::TOE_SHOULDER_STRENGTH) as f32,
        toe_width: params.toe_width as f32,
        shoulder_width: params.shoulder_width as f32,
        d_max: K::D_MAX as f32,
        a_toe_base: K::TOE_SHARPNESS_BASE as f32,
        a_sh_base: K::SHOULDER_SHARPNESS_BASE as f32,
        width_ref: K::TOE_SHOULDER_WIDTH_REF as f32,
        toe_height: K::TOE_HEIGHT as f32,
        sh_height: K::SHOULDER_HEIGHT as f32,
        zone_center: K::ANCHOR_TARGET_DENSITY as f32,
        true_black: if params.true_black { 1.0 } else { 0.0 },
        dark_shadows_lift: (params.dark_shadows * K::SHADOWS_MAX_LIFT) as f32,
        v_star: params.v_star as f32,
        midtone_gamma: K::PAPER_MIDTONE_GAMMA as f32,
        gamma_width: K::PAPER_GAMMA_WIDTH as f32,
        shadows_lift: (params.shadows * K::SHADOWS_MAX_LIFT) as f32,
        shadow_contrast: (params.shadow_contrast * K::SHADOW_CONTRAST_MAX)
            .max(K::SHADOW_CONTRAST_NEG_FLOOR) as f32,
        highlights_shift: (params.highlights * K::HIGHLIGHTS_MAX_SHIFT) as f32,
        highlight_contrast: (params.highlight_contrast * K::HIGHLIGHT_CONTRAST_MAX) as f32,
        vibrance: params.vibrance as f32,
        saturation: params.saturation as f32,
        pre_saturation: params.pre_saturation as f32,
        print_saturation: params.print_saturation as f32,
        band_hues: params.band_hues.map(|v| v as f32),
        band_saturations: params.band_saturations.map(|v| v as f32),
        separation_damping: params.separation_damping as f32,
        skin_protection: params.skin_protection as f32,
        hue_trim: params.hue_trim as f32,
        auto_highlight: params.auto_highlight as f32,
        mask_scale: [0.0; 4],
        mask_dims: [0.0; 4],
    };
    if let Some((w, h)) = mask_dims
        && params.mask_val_scale != [0.0; 3]
    {
        u.mask_scale = f4(params.mask_val_scale, 1.0);
        u.mask_dims = [
            w as f32,
            h as f32,
            render_dims.0 as f32,
            render_dims.1 as f32,
        ];
    }
    u
}

/// `UniformsBuilder.levelsBuffer`: the flat 51-float layout `levels_remap` reads, padded to
/// [`LEVELS_BUFFER_FLOATS`]. Anchors come pre-sanitized from `derive_render_params` (sorted,
/// monotone, ≤ 8).
pub fn pack_levels(params: &RenderParams) -> [f32; LEVELS_BUFFER_FLOATS] {
    let mut buf = [0.0f32; LEVELS_BUFFER_FLOATS];
    for (ch, points) in params.levels_points.iter().enumerate() {
        let n = points.len().min(8);
        for (i, p) in points.iter().take(n).enumerate() {
            if let Some(x) = buf.get_mut(ch * 16 + i) {
                *x = p[0] as f32;
            }
            if let Some(y) = buf.get_mut(ch * 16 + 8 + i) {
                *y = p[1] as f32;
            }
        }
        if let Some(c) = buf.get_mut(48 + ch) {
            *c = n as f32;
        }
    }
    buf
}
