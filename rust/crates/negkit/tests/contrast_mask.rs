//! Port of `ContrastMaskTests.swift` — Contrast Mask (NegPy 515c1f5 port) parity + properties.
//! The fixtures in `Tests/Fixtures/contrast_mask/` are dumped from NegPy's own
//! `contrast_mask_plane` / `contrast_mask_ev` / masked PhotometricProcessor chain
//! (`dump_fixtures.py contrast_mask`, additive).
//!
//! The Swift tests measure the analysis with `ExposureKernel.analyze(…, analysisBuffer: 0.05)`;
//! here the analysis comes from `synthetic64/default/analysis.json` — the contrast_mask input
//! IS the synthetic64 input (byte-identical), and that file is NegPy's own measurement of it
//! (the same source `tests/chain.rs` uses), so the gates are independent of the analysis port.

// The Swift tests mutate a default field by field; that style IS the drift-catcher here.
#![allow(clippy::field_reassign_with_default)]

use negkit::constants::K;
use negkit::contrast_mask::{self, Plane};
use negkit::derive::derive_render_params;
use negkit::kernel::ExposureAnalysis;
use negkit::reference::{self, MaskSampler, RenderOptions};
use negkit::zone_placement;
use negkit::{ExposureSettings, LogNegativeBounds, RgbImage, V3, fixtures};
use serde_json::Value;

fn manifest() -> Value {
    fixtures::read_json("contrast_mask/manifest.json").expect("manifest")
}

fn input() -> RgbImage {
    fixtures::read_rgb("contrast_mask/input.bin", 64, 64).expect("input")
}

fn v3(v: &Value) -> V3 {
    let a = v.as_array().unwrap_or_else(|| panic!("not an array: {v}"));
    [
        a[0].as_f64().unwrap(),
        a[1].as_f64().unwrap(),
        a[2].as_f64().unwrap(),
    ]
}

fn bounds() -> LogNegativeBounds {
    let m = manifest();
    LogNegativeBounds {
        floors: v3(&m["bounds"]["floors"]),
        ceils: v3(&m["bounds"]["ceils"]),
    }
}

/// NegPy's own analysis of the (shared) synthetic64 input, from `default/analysis.json`.
fn analysis() -> ExposureAnalysis {
    let j = fixtures::read_json("synthetic64/default/analysis.json").expect("analysis.json");
    let b = &j["bounds"];
    let m = &j["meters"];
    let na = m["neutral_axis_refs"].as_array().expect("neutral_axis_refs");
    let (neutral_mid, neutral_shadow, neutral_highlight, neutral_confidence) = if na[0].is_null() {
        (None, None, None, None)
    } else {
        (
            Some(v3(&na[0])),
            Some(v3(&na[1])),
            (!na[2].is_null()).then(|| v3(&na[2])),
            na[3].as_f64(),
        )
    };
    ExposureAnalysis {
        base_bounds: LogNegativeBounds {
            floors: v3(&b["base_floors"]),
            ceils: v3(&b["base_ceils"]),
        },
        anchor: m["metered_anchor"].as_f64().unwrap(),
        textural_range: m["textural_range"].as_f64().unwrap(),
        shadow_point: m["shadow_point"].as_f64(),
        highlight_point: m["highlight_point"].as_f64(),
        shadow_refs: v3(&m["shadow_log_refs"]),
        neutral_mid,
        neutral_shadow,
        neutral_highlight,
        neutral_confidence,
    }
}

/// The Swift `settingsFrom(_:)`: the masked_chain exposure_config over NegPy-neutral
/// SwiftInvert-only defaults.
fn masked_chain_settings() -> ExposureSettings {
    let m = manifest();
    let e = &m["masked_chain"]["exposure_config"];
    let f = |k: &str| e[k].as_f64().unwrap_or_else(|| panic!("{k}"));
    let b = |k: &str| e[k].as_bool().unwrap_or_else(|| panic!("{k}"));
    let mut s = ExposureSettings::default();
    s.pre_saturation = 1.0;
    s.skin_protection = 0.0;
    s.true_black = b("true_black");
    s.paper_dmin = b("paper_dmin");
    s.density = f("density");
    s.grade = f("grade");
    s.wb_cyan = f("wb_cyan");
    s.wb_magenta = f("wb_magenta");
    s.wb_yellow = f("wb_yellow");
    s.auto_exposure = b("auto_exposure");
    s.auto_normalize_contrast = b("auto_normalize_contrast");
    s.cast_removal_strength = f("cast_removal_strength");
    s.toe = f("toe");
    s.toe_width = f("toe_width");
    s.shoulder = f("shoulder");
    s.shoulder_width = f("shoulder_width");
    s.contrast_mask = f("contrast_mask");
    s.mask_spacer = f("mask_spacer");
    s
}

fn plane_values(spacer: f64) -> Vec<f32> {
    contrast_mask::build_plane(&input(), &bounds(), spacer)
        .expect("plane")
        .data
}

// MARK: - Parity with NegPy

fn plane_matches_fixture(name: &str) {
    let m = manifest();
    let spacer = m["planes"][name]["spacer"].as_f64().unwrap();
    let expected = fixtures::read_f32(&format!("contrast_mask/plane_{name}.bin")).unwrap();
    let plane = contrast_mask::build_plane(&input(), &bounds(), spacer).expect("plane");
    assert_eq!((plane.width, plane.height), (64, 64));
    assert_eq!(plane.data.len(), expected.len());
    let (max, _) = fixtures::max_mean_abs_diff(&plane.data, &expected);
    assert!(max < 1e-5, "plane {name}: max diff {max}");
}

#[test]
fn plane_matches_fixture_spacer4() {
    plane_matches_fixture("spacer4");
}

#[test]
fn plane_matches_fixture_spacer2_5() {
    plane_matches_fixture("spacer2_5");
}

/// The bilinear half-pixel expansion × contrast_mask_scale, against upstream's
/// cv2.INTER_LINEAR expansion — this is the exact arithmetic all three kernels hand-roll.
#[test]
fn ev_expansion_matches_fixture() {
    let m = manifest();
    let gamma = m["ev"]["gamma"].as_f64().unwrap();
    let lum_range = m["ev"]["density_range"].as_f64().unwrap();
    let expected = fixtures::read_f32("contrast_mask/ev_96x128.bin").unwrap();
    let values = fixtures::read_f32("contrast_mask/plane_spacer4.bin").unwrap();
    let plane = Plane::new(64, 64, values).unwrap();

    // Their EV map is stops (plane × contrast_mask_scale); the per-channel ev_scale fold
    // happens later on both sides.
    let scale = (-gamma * lum_range / K::LOG10_TWO) as f32;
    let (out_w, out_h) = (128usize, 96usize);
    assert_eq!(expected.len(), out_w * out_h);
    let mut max_diff = 0.0f32;
    for y in 0..out_h {
        for x in 0..out_w {
            let got = plane.sample(x, y, out_w, out_h) * scale;
            max_diff = max_diff.max((got - expected[y * out_w + x]).abs());
        }
    }
    assert!(max_diff < 1e-5, "ev expansion: max diff {max_diff}");
}

/// End-to-end: the masked chain through our CPU reference equals NegPy's masked
/// PhotometricProcessor output at the standing full-chain gate.
#[test]
fn masked_full_chain_matches_fixture() {
    let settings = masked_chain_settings();
    assert_ne!(settings.contrast_mask, 0.0);
    let analysis = analysis();
    let params = derive_render_params(&settings, &analysis);
    assert_ne!(params.mask_val_scale, [0.0; 3]);
    // The Swift `ReferenceCurve.render(linearImage:settings:analysis:)`: the plane from the
    // render input against the BASE bounds (the recorded divergence), spacer from settings.
    let plane = contrast_mask::build_plane(&input(), &analysis.base_bounds, settings.mask_spacer)
        .expect("plane");
    let encoded = reference::render(
        &input(),
        &params,
        RenderOptions {
            srgb_encode: false,
            mask_plane: Some(&plane),
        },
    );
    let expected = fixtures::read_f32("contrast_mask/masked_output.bin").unwrap();
    assert_eq!(encoded.data.len(), expected.len());
    let (max, mean) = fixtures::max_mean_abs_diff(&encoded.data, &expected);
    assert!(max < 1e-4, "masked chain: max diff {max} (mean {mean})");
}

// MARK: - Properties

/// Zero-mean: the mask must never move overall print density.
#[test]
fn plane_is_zero_mean() {
    let plane = contrast_mask::build_plane(&input(), &bounds(), 4.0).expect("plane");
    assert!(plane.mean().abs() < 1e-6);
}

/// The spacer clamps at both ends: a saved value outside 2–6 renders exactly at the limit
/// (upstream 1739522 — "cannot render wider than the slider can show").
#[test]
fn spacer_clamps_both_ends() {
    assert_eq!(plane_values(0.5), plane_values(2.0));
    assert_eq!(plane_values(9.0), plane_values(6.0));
    assert_ne!(plane_values(2.0), plane_values(6.0));
}

/// Gamma 0 = identity: no scale, and the derive gates the whole feature.
#[test]
fn zero_gamma_is_identity() {
    assert_eq!(contrast_mask::val_scale(0.0, 1.0, &bounds()), [0.0; 3]);
    let mut settings = ExposureSettings::default();
    settings.pre_saturation = 1.0;
    settings.skin_protection = 0.0;
    let params = derive_render_params(&settings, &analysis());
    assert_eq!(params.mask_val_scale, [0.0; 3]);
    assert_eq!(contrast_mask::GAMMA_LIMIT, 0.5);
}

/// Direction: positive gamma (blurred positive) must COMPRESS the normalized-val spread —
/// the whole point of the control.
#[test]
fn positive_gamma_compresses_val_range() {
    let analysis = analysis();
    let mut settings = ExposureSettings::default();
    settings.pre_saturation = 1.0;
    settings.skin_protection = 0.0;
    let mut luma_spread = |gamma: f64| -> f64 {
        settings.contrast_mask = gamma;
        let params = derive_render_params(&settings, &analysis);
        let plane = (gamma != 0.0)
            .then(|| contrast_mask::build_plane(&input(), &analysis.base_bounds, 4.0))
            .flatten();
        let norm = reference::normalize(&input(), &params.final_bounds);
        // Val-domain green spread after the mask add (pre-curve), which is what the paper
        // then re-normalizes.
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let w = norm.width;
        for (pix, p) in norm.data.chunks_exact(3).enumerate() {
            let mut v = p[1];
            if let Some(plane) = &plane {
                v += params.mask_val_scale[1] as f32 * plane.sample(pix % w, pix / w, w, norm.height);
            }
            lo = lo.min(v);
            hi = hi.max(v);
        }
        f64::from(hi - lo)
    };
    let base = luma_spread(0.0);
    assert!(luma_spread(0.5) < base);
    assert!(luma_spread(-0.5) > base);
}

/// The zone model's constant-addend shortcut is exact: predicted_zone with a pin's plane
/// sample equals the full spatial kernel at that pixel.
#[test]
fn zone_model_sees_the_mask() {
    let analysis = analysis();
    let mut settings = ExposureSettings::default();
    settings.pre_saturation = 1.0;
    settings.skin_protection = 0.0;
    settings.contrast_mask = 0.4;
    let plane = contrast_mask::build_plane(&input(), &analysis.base_bounds, 4.0).expect("plane");
    let mask_val = f64::from(plane.sample(20, 31, 64, 64));
    let with = zone_placement::predicted_zone(&settings, &analysis, 0.55, mask_val);
    let without = zone_placement::predicted_zone(&settings, &analysis, 0.55, 0.0);
    assert_ne!(with, without, "a nonzero plane sample must move the model");
    // And gamma 0 makes the sample inert even when carried.
    settings.contrast_mask = 0.0;
    let off = zone_placement::predicted_zone(&settings, &analysis, 0.55, mask_val);
    let off_plain = zone_placement::predicted_zone(&settings, &analysis, 0.55, 0.0);
    assert_eq!(off, off_plain);
}

// MARK: - Seams the Swift pins only through the kernels

/// The plane's own sampler and the normalized-coordinate form agree at pixel centres
/// (`u = (x+0.5)/W` collapses the half-pixel mapping to `u·w − 0.5`).
#[test]
fn normalized_sample_matches_pixel_sample_at_centres() {
    let plane = contrast_mask::build_plane(&input(), &bounds(), 4.0).expect("plane");
    for (x, y) in [(0usize, 0usize), (20, 31), (63, 63), (7, 50)] {
        let by_pixel = plane.sample(x, y, 64, 64);
        let by_uv = plane.sample_normalized((x as f64 + 0.5) / 64.0, (y as f64 + 0.5) / 64.0);
        assert!(
            (by_pixel - by_uv).abs() < 1e-6,
            "({x},{y}): {by_pixel} vs {by_uv}"
        );
    }
}

/// A 3× render samples the same plane with edge replication: every sample lies within the
/// plane's own range, and the plane's centre pixel sits at the render's centre.
#[test]
fn sampler_at_other_render_sizes_stays_within_the_plane() {
    let plane = contrast_mask::build_plane(&input(), &bounds(), 4.0).expect("plane");
    let (lo, hi) = plane
        .data
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    for y in (0..192).step_by(7) {
        for x in (0..192).step_by(5) {
            let s = plane.sample(x, y, 192, 192);
            assert!(s >= lo - 1e-6 && s <= hi + 1e-6, "({x},{y}) = {s} outside [{lo},{hi}]");
        }
    }
    // Pixel (1,1) of a 3× render maps to plane coordinate (0.0, 0.0) → the corner value.
    assert!((plane.sample(1, 1, 192, 192) - plane.data[0]).abs() < 1e-6);
}

/// Degenerate inputs yield no plane rather than garbage.
#[test]
fn degenerate_inputs_yield_no_plane() {
    let tiny = RgbImage::from_vec(1, 1, vec![0.5; 3]).unwrap();
    assert!(contrast_mask::build_plane(&tiny, &bounds(), 4.0).is_none());
    let flat = LogNegativeBounds {
        floors: [-1.0; 3],
        ceils: [-1.0; 3],
    };
    assert!(contrast_mask::build_plane(&input(), &flat, 4.0).is_none());
    assert!(Plane::new(2, 2, vec![0.0; 3]).is_err());
    assert!(Plane::new(0, 2, Vec::new()).is_err());
}

/// cv2's kernel-size rule: `ksize = round(8σ+1) | odd`, taps sum to 1 and are symmetric.
#[test]
fn gaussian_taps_follow_cv2_rule() {
    let taps = contrast_mask::gaussian_taps(2.56); // σ for spacer 4 on a 64-px grid
    assert_eq!(taps.len(), 21); // round(21.48) = 21
    let sum: f64 = taps.iter().map(|&t| f64::from(t)).sum();
    assert!((sum - 1.0).abs() < 1e-6);
    for i in 0..taps.len() / 2 {
        assert_eq!(taps[i], taps[taps.len() - 1 - i]);
    }
    // round(8·1.6+1) = 14 → bumped to 15.
    assert_eq!(contrast_mask::gaussian_taps(1.6).len(), 15);
    assert!(contrast_mask::gaussian_taps(0.0).is_empty());
}
