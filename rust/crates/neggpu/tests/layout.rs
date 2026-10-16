//! `LayoutTests` / `VulkanLayoutTests`: the uniform structs are byte-identical to
//! `ShaderTypes.swift` (strides 48 / 304, every CurveUniforms offset, the 51-float levels
//! buffer) and `Push` is the 16-byte block the WGSL declares.

use std::mem::{offset_of, size_of};

use neggpu::uniforms::{CurveUniforms, LEVELS_BUFFER_FLOATS, LEVELS_FLOATS, NormUniforms, Push};

#[test]
fn uniform_strides_match_std140() {
    assert_eq!(size_of::<NormUniforms>(), 48);
    assert_eq!(size_of::<CurveUniforms>(), 304);
    assert_eq!(size_of::<Push>(), 16);
    assert_eq!(LEVELS_FLOATS, 51);
    assert_eq!(LEVELS_BUFFER_FLOATS, 52);
    assert_eq!(
        LEVELS_BUFFER_FLOATS * 4 % 16,
        0,
        "levels upload is a 16-byte multiple"
    );
}

#[test]
fn norm_uniform_offsets() {
    assert_eq!(offset_of!(NormUniforms, floors), 0);
    assert_eq!(offset_of!(NormUniforms, ceils), 16);
    assert_eq!(offset_of!(NormUniforms, wp_offset), 32);
    assert_eq!(offset_of!(NormUniforms, bp_offset), 36);
    assert_eq!(offset_of!(NormUniforms, _pad), 40);
}

#[test]
fn curve_uniform_offsets() {
    // 8 × vec4
    assert_eq!(offset_of!(CurveUniforms, pivots), 0);
    assert_eq!(offset_of!(CurveUniforms, slopes), 16);
    assert_eq!(offset_of!(CurveUniforms, curvatures), 32);
    assert_eq!(offset_of!(CurveUniforms, cmy_offsets), 48);
    assert_eq!(offset_of!(CurveUniforms, shadow_cmy), 64);
    assert_eq!(offset_of!(CurveUniforms, mid_cmy), 80);
    assert_eq!(offset_of!(CurveUniforms, highlight_cmy), 96);
    assert_eq!(offset_of!(CurveUniforms, d_min_rgb), 112);
    // 24 scalars in the Swift order
    assert_eq!(offset_of!(CurveUniforms, toe), 128);
    assert_eq!(offset_of!(CurveUniforms, shoulder), 132);
    assert_eq!(offset_of!(CurveUniforms, toe_width), 136);
    assert_eq!(offset_of!(CurveUniforms, shoulder_width), 140);
    assert_eq!(offset_of!(CurveUniforms, d_max), 144);
    assert_eq!(offset_of!(CurveUniforms, a_toe_base), 148);
    assert_eq!(offset_of!(CurveUniforms, a_sh_base), 152);
    assert_eq!(offset_of!(CurveUniforms, width_ref), 156);
    assert_eq!(offset_of!(CurveUniforms, toe_height), 160);
    assert_eq!(offset_of!(CurveUniforms, sh_height), 164);
    assert_eq!(offset_of!(CurveUniforms, zone_center), 168);
    assert_eq!(offset_of!(CurveUniforms, true_black), 172);
    assert_eq!(offset_of!(CurveUniforms, dark_shadows_lift), 176);
    assert_eq!(offset_of!(CurveUniforms, v_star), 180);
    assert_eq!(offset_of!(CurveUniforms, midtone_gamma), 184);
    assert_eq!(offset_of!(CurveUniforms, gamma_width), 188);
    assert_eq!(offset_of!(CurveUniforms, shadows_lift), 192);
    assert_eq!(offset_of!(CurveUniforms, shadow_contrast), 196);
    assert_eq!(offset_of!(CurveUniforms, highlights_shift), 200);
    assert_eq!(offset_of!(CurveUniforms, highlight_contrast), 204);
    assert_eq!(offset_of!(CurveUniforms, vibrance), 208);
    assert_eq!(offset_of!(CurveUniforms, saturation), 212);
    assert_eq!(offset_of!(CurveUniforms, pre_saturation), 216);
    assert_eq!(offset_of!(CurveUniforms, print_saturation), 220);
    // bands + tail (the Swift LayoutTests' pinned offsets)
    assert_eq!(offset_of!(CurveUniforms, band_hues), 224);
    assert_eq!(offset_of!(CurveUniforms, band_saturations), 240);
    assert_eq!(offset_of!(CurveUniforms, separation_damping), 256);
    assert_eq!(offset_of!(CurveUniforms, skin_protection), 260);
    assert_eq!(offset_of!(CurveUniforms, hue_trim), 264);
    assert_eq!(offset_of!(CurveUniforms, auto_highlight), 268);
    assert_eq!(offset_of!(CurveUniforms, mask_scale), 272);
    assert_eq!(offset_of!(CurveUniforms, mask_dims), 288);
}

#[test]
fn push_offsets() {
    assert_eq!(offset_of!(Push, n), 0);
    assert_eq!(offset_of!(Push, flags), 4);
    assert_eq!(offset_of!(Push, width), 8);
    assert_eq!(offset_of!(Push, row_pitch), 12);
}

#[test]
fn wgsl_declares_the_same_fields_in_order() {
    // Field order in the WGSL struct IS the layout (naga lays it out by declaration): pin
    // that the shader source lists CurveUniforms' members in the Rust order.
    let src = neggpu::pipeline::WGSL_SOURCE;
    let start = src
        .find("struct CurveUniforms")
        .expect("CurveUniforms in WGSL");
    let body = &src[start..src[start..].find('}').expect("struct end") + start];
    let fields: Vec<&str> = body
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            if t.starts_with("//") || !t.contains(':') || t.starts_with("struct") {
                None
            } else {
                t.split(':').next().map(str::trim)
            }
        })
        .collect();
    let expected = [
        "pivots",
        "slopes",
        "curvatures",
        "cmy_offsets",
        "shadow_cmy",
        "mid_cmy",
        "highlight_cmy",
        "d_min_rgb",
        "toe",
        "shoulder",
        "toe_width",
        "shoulder_width",
        "d_max",
        "a_toe_base",
        "a_sh_base",
        "width_ref",
        "toe_height",
        "sh_height",
        "zone_center",
        "true_black",
        "dark_shadows_lift",
        "v_star",
        "midtone_gamma",
        "gamma_width",
        "shadows_lift",
        "shadow_contrast",
        "highlights_shift",
        "highlight_contrast",
        "vibrance",
        "saturation",
        "pre_saturation",
        "print_saturation",
        "band_hues",
        "band_saturations",
        "separation_damping",
        "skin_protection",
        "hue_trim",
        "auto_highlight",
        "mask_scale",
        "mask_dims",
    ];
    assert_eq!(fields, expected);
}
