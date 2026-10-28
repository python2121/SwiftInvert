//! Port of `ClosedFormTests.swift`: scalar oracles dumped from NegPy
//! (`fixtures/closed_form.json`), at the Swift tolerances.

use negkit::LogNegativeBounds;
use negkit::constants::{K, WorkingOetf};
use negkit::curve_logic as cl;
use negkit::fixtures;
use serde_json::Value;

fn fixture() -> Value {
    fixtures::read_json("closed_form.json").expect("closed_form.json")
}

fn cases(key: &str) -> Vec<Value> {
    fixture()[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} missing"))
        .clone()
}

fn num(c: &Value, key: &str) -> f64 {
    c[key]
        .as_f64()
        .unwrap_or_else(|| panic!("{key} not a number in {c}"))
}

fn opt(c: &Value, key: &str) -> Option<f64> {
    c[key].as_f64()
}

#[track_caller]
fn expect_close(got: f64, expected: f64, accuracy: f64, label: &dyn std::fmt::Display) {
    assert!(
        (got - expected).abs() <= accuracy,
        "{label} got {got}, expected {expected} ± {accuracy}"
    );
}

/// `percentile`: the oracle vectors exercise `Stats.percentile` (numpy linear semantics),
/// which agent A ports as `negkit::stats`. Wired here so the vectors are not forgotten;
/// swap the body for `negkit::stats::percentile(&data, q)` once the module lands.
#[test]
fn percentile() {
    let cs = cases("percentile");
    assert!(!cs.is_empty());
    for c in &cs {
        let data: Vec<f32> = c["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        let q = num(c, "q");
        let out = num(c, "out");
        assert!(!data.is_empty());
        expect_close(
            negkit::stats::percentile(&data, q).expect("non-empty"),
            out,
            1e-6,
            c,
        );
    }
}

#[test]
fn cmy_density_conversions() {
    for c in cases("cmy_to_density") {
        expect_close(
            cl::cmy_to_density(num(&c, "val"), num(&c, "log_range")),
            num(&c, "out"),
            1e-9,
            &c,
        );
    }
    for c in cases("density_to_cmy") {
        expect_close(
            cl::density_to_cmy(num(&c, "density"), num(&c, "log_range")),
            num(&c, "out"),
            1e-9,
            &c,
        );
    }
}

#[test]
fn grade_slope() {
    for c in cases("grade_to_slope") {
        expect_close(
            cl::grade_to_slope(num(&c, "grade"), opt(&c, "range")),
            num(&c, "out"),
            1e-9,
            &c,
        );
    }
    for c in cases("slope_to_grade") {
        expect_close(
            cl::slope_to_grade(num(&c, "slope"), opt(&c, "range")),
            num(&c, "out"),
            1e-9,
            &c,
        );
    }
}

#[test]
fn pivot_and_reference() {
    for c in cases("compute_pivot") {
        let got = cl::compute_pivot(
            num(&c, "slope"),
            num(&c, "density"),
            num(&c, "d_min"),
            opt(&c, "anchor"),
        );
        expect_close(got, num(&c, "out"), 1e-9, &c);
    }
    for c in cases("reference_linear_value") {
        expect_close(
            cl::reference_linear_value(num(&c, "d_min"), None),
            num(&c, "out"),
            1e-9,
            &c,
        );
    }
    for c in cases("reference_linear_value_target") {
        expect_close(
            cl::reference_linear_value(num(&c, "d_min"), opt(&c, "target")),
            num(&c, "out"),
            1e-9,
            &c,
        );
    }
}

/// Shadow Reach / Highlight Hold oracles (8532dd92): the synthetic64 images never fire the
/// reach, so these vectors are its real pin — dumped from upstream's own
/// shadow_reach_slope/highlight_hold_offset.
#[test]
fn shadow_reach_and_highlight_hold() {
    for c in cases("shadow_reach_slope") {
        let got = cl::shadow_reach_slope(
            num(&c, "slope"),
            num(&c, "anchor"),
            num(&c, "shadow_point"),
            num(&c, "d_min"),
        );
        expect_close(got, num(&c, "out"), 1e-9, &c);
    }
    for c in cases("highlight_hold_offset") {
        let got = cl::highlight_hold_offset(
            num(&c, "slope"),
            num(&c, "pivot"),
            num(&c, "highlight_point"),
            num(&c, "d_min"),
        );
        expect_close(got, num(&c, "out"), 1e-9, &c);
    }
}

#[test]
fn softplus_oracles() {
    for c in cases("softplus") {
        expect_close(cl::softplus(num(&c, "x")), num(&c, "out"), 1e-12, &c);
    }
    for c in cases("inv_softplus") {
        expect_close(cl::inv_softplus(num(&c, "y")), num(&c, "out"), 1e-9, &c);
    }
}

#[test]
fn grade_range_and_shape() {
    for c in cases("effective_grade_range") {
        let got = cl::effective_grade_range(
            c["auto"].as_bool().expect("auto"),
            opt(&c, "floor_ceil"),
            opt(&c, "textural"),
        );
        expect_close(got.expect("some range"), num(&c, "out"), 1e-9, &c);
    }
    for c in cases("grade_coupled_shape") {
        let out = c["out"].as_array().expect("out");
        let got = cl::grade_coupled_shape(num(&c, "slope_g"), num(&c, "toe"), num(&c, "shoulder"));
        expect_close(got.toe, out[0].as_f64().unwrap(), 1e-9, &c);
        expect_close(got.shoulder, out[1].as_f64().unwrap(), 1e-9, &c);
    }
    for c in cases("effective_cast_strength") {
        let got = cl::effective_cast_strength(num(&c, "strength"), opt(&c, "confidence"));
        expect_close(got, num(&c, "out"), 1e-9, &c);
    }
}

#[test]
fn working_oetf() {
    for c in cases("working_oetf") {
        let x = num(&c, "x") as f32;
        expect_close(f64::from(WorkingOetf::encode(x)), num(&c, "enc"), 1e-6, &c);
        expect_close(
            f64::from(WorkingOetf::decode(WorkingOetf::encode(x))),
            num(&c, "dec_of_enc"),
            1e-6,
            &c,
        );
        // The f64 twins agree with the pixel path to float precision.
        expect_close(
            WorkingOetf::encode_f64(f64::from(x)),
            num(&c, "enc"),
            1e-6,
            &c,
        );
        expect_close(
            WorkingOetf::decode_f64(WorkingOetf::encode_f64(f64::from(x))),
            num(&c, "dec_of_enc"),
            1e-6,
            &c,
        );
    }
}

#[test]
fn filtration_offsets_sample() {
    let c = fixture()["filtration_offsets_sample"].clone();
    let arr = |k: &str| -> [f64; 3] {
        let a = c[k].as_array().expect(k);
        [
            a[0].as_f64().unwrap(),
            a[1].as_f64().unwrap(),
            a[2].as_f64().unwrap(),
        ]
    };
    let bounds = LogNegativeBounds {
        floors: arr("floors"),
        ceils: arr("ceils"),
    };
    let got = cl::filtration_offsets(arr("wb_cmy"), Some(&bounds));
    let out = arr("out");
    for ch in 0..3 {
        expect_close(got[ch], out[ch], 1e-9, &format!("ch {ch}"));
    }
}

/// The dumped `constants` block is NegPy's own EXPOSURE_CONSTANTS: every `K` value that has
/// a counterpart there must match it exactly (the Swift suite relied on review for this;
/// pinning it costs nothing).
#[test]
fn constants_match_negpy() {
    let k = fixture()["constants"].clone();
    let n = |key: &str| num(&k, key);
    let pairs: &[(&str, f64)] = &[
        ("cmy_max_density", K::CMY_MAX_DENSITY),
        ("density_multiplier", K::DENSITY_MULTIPLIER),
        ("anchor_target_density", K::ANCHOR_TARGET_DENSITY),
        ("zone_density_sharpness", K::ZONE_DENSITY_SHARPNESS),
        (
            "zone_density_highlight_offset",
            K::ZONE_DENSITY_HIGHLIGHT_OFFSET,
        ),
        ("assumed_anchor", K::ASSUMED_ANCHOR),
        ("iso_r_min", K::ISO_R_MIN),
        ("iso_r_max", K::ISO_R_MAX),
        ("slope_min", K::SLOPE_MIN),
        ("slope_max", K::SLOPE_MAX),
        ("d_max", K::D_MAX),
        ("d_min", K::D_MIN),
        ("toe_shoulder_strength", K::TOE_SHOULDER_STRENGTH),
        ("toe_sharpness_base", K::TOE_SHARPNESS_BASE),
        ("shoulder_sharpness_base", K::SHOULDER_SHARPNESS_BASE),
        ("toeshoulder_width_ref", K::TOE_SHOULDER_WIDTH_REF),
        ("toe_height", K::TOE_HEIGHT),
        ("shoulder_height", K::SHOULDER_HEIGHT),
        ("grade_contrast_scale", K::GRADE_CONTRAST_SCALE),
        ("analysis_grid", K::ANALYSIS_GRID as f64),
        ("base_luma_clip", K::BASE_LUMA_CLIP),
        ("base_color_clip", K::BASE_COLOR_CLIP),
        ("shadow_neutral_percentile", K::SHADOW_NEUTRAL_PERCENTILE),
        ("cast_removal_max_offset", K::CAST_REMOVAL_MAX_OFFSET),
        (
            "neutral_axis_chroma_quantile",
            K::NEUTRAL_AXIS_CHROMA_QUANTILE,
        ),
        ("neutral_axis_chroma_cap", K::NEUTRAL_AXIS_CHROMA_CAP),
        (
            "neutral_axis_first_pass_cap",
            K::NEUTRAL_AXIS_FIRST_PASS_CAP,
        ),
        ("neutral_axis_min_pixels", K::NEUTRAL_AXIS_MIN_PIXELS as f64),
        ("neutral_axis_confidence_n0", K::NEUTRAL_AXIS_CONFIDENCE_N0),
        (
            "neutral_axis_agreement_deadzone",
            K::NEUTRAL_AXIS_AGREEMENT_DEADZONE,
        ),
        (
            "neutral_axis_agreement_scale",
            K::NEUTRAL_AXIS_AGREEMENT_SCALE,
        ),
        ("color_bounds_band_width", K::COLOR_BOUNDS_BAND_WIDTH),
        ("midtone_cast_max_offset", K::MIDTONE_CAST_MAX_OFFSET),
        (
            "neutral_axis_curv_max_ratio",
            K::NEUTRAL_AXIS_CURV_MAX_RATIO,
        ),
        ("activity_block", K::ACTIVITY_BLOCK as f64),
        ("activity_gate_density", K::ACTIVITY_GATE_DENSITY),
        ("activity_min_fraction", K::ACTIVITY_MIN_FRACTION),
        ("anchor_trim_clip", K::ANCHOR_TRIM_CLIP),
        ("anchor_meter_band", K::ANCHOR_METER_BAND),
        ("anchor_meter_strength", K::ANCHOR_METER_STRENGTH),
        ("toe_grade_strength", K::TOE_GRADE_STRENGTH),
        ("shoulder_grade_strength", K::SHOULDER_GRADE_STRENGTH),
        ("auto_grade_target", K::AUTO_GRADE_TARGET),
        ("auto_grade_strength", K::AUTO_GRADE_STRENGTH),
        ("auto_grade_max_overfill", K::AUTO_GRADE_MAX_OVERFILL),
        ("auto_grade_nominal_range", K::AUTO_GRADE_NOMINAL_RANGE),
        ("auto_grade_nominal_ratio", K::AUTO_GRADE_NOMINAL_RATIO),
        ("textural_range_clip", K::TEXTURAL_RANGE_CLIP),
        ("shadow_reach_percentile", K::SHADOW_REACH_PERCENTILE),
        ("shadow_reach_density", K::SHADOW_REACH_DENSITY),
        ("highlight_hold_percentile", K::HIGHLIGHT_HOLD_PERCENTILE),
        ("highlight_hold_density", K::HIGHLIGHT_HOLD_DENSITY),
        ("highlight_hold_max", K::HIGHLIGHT_HOLD_MAX),
        ("paper_midtone_gamma", K::PAPER_MIDTONE_GAMMA),
        ("paper_gamma_width", K::PAPER_GAMMA_WIDTH),
        (
            "separation_damping_ref_spread",
            K::SEPARATION_DAMPING_REF_SPREAD,
        ),
    ];
    for (key, ours) in pairs {
        expect_close(*ours, n(key), 1e-12, key);
    }
    let band = |key: &str| -> (f64, f64) {
        let a = k[key].as_array().expect(key);
        (a[0].as_f64().unwrap(), a[1].as_f64().unwrap())
    };
    assert_eq!(
        band("neutral_axis_highlight_band"),
        K::NEUTRAL_AXIS_HIGHLIGHT_BAND
    );
    assert_eq!(band("neutral_axis_mid_band"), K::NEUTRAL_AXIS_MID_BAND);
    assert_eq!(
        band("neutral_axis_shadow_band"),
        K::NEUTRAL_AXIS_SHADOW_BAND
    );
}

/// Pure-function corners the Swift suite covers only transitively.
#[test]
fn helper_corners() {
    // default_grade_range = 0.85 · 1.5 · 0.9.
    expect_close(
        cl::default_grade_range(),
        0.85 * 1.5 * 0.9,
        1e-15,
        &"default_grade_range",
    );
    // grade_to_slope with None uses the default range.
    assert_eq!(
        cl::grade_to_slope(115.0, None),
        cl::grade_to_slope(115.0, Some(cl::default_grade_range()))
    );
    // slope_to_grade inverts grade_to_slope inside the clamps.
    for grade in [60.0, 90.0, 115.0, 150.0] {
        let s = cl::grade_to_slope(grade, Some(1.2));
        expect_close(cl::slope_to_grade(s, Some(1.2)), grade, 1e-9, &grade);
    }
    assert_eq!(cl::slope_to_grade(0.0, Some(1.0)), K::ISO_R_MAX);
    // Sigmoid symmetry and softplus/inverse round trip on both stable branches.
    expect_close(cl::sigmoid(0.0), 0.5, 1e-15, &"sigmoid(0)");
    expect_close(
        cl::sigmoid(3.0) + cl::sigmoid(-3.0),
        1.0,
        1e-15,
        &"sigmoid symmetry",
    );
    for y in [0.05, 1.0, 5.0, 25.0] {
        expect_close(cl::softplus(cl::inv_softplus(y)), y, 1e-9, &y);
    }
    // normalize_refs guards a collapsed channel with ±eps instead of dividing by zero.
    let flat = LogNegativeBounds {
        floors: [-1.0, -1.0, -1.0],
        ceils: [-1.0, -1.0, 0.0],
    };
    let n = cl::normalize_refs([-0.5, -0.5, -0.5], &flat);
    assert!(n[0].is_finite() && n[1].is_finite());
    expect_close(n[2], 0.5, 1e-12, &"normalize_refs");
    // filtration_offsets without bounds is the raw density per slider unit.
    assert_eq!(
        cl::filtration_offsets([1.0, -0.5, 0.25], None),
        [
            K::CMY_MAX_DENSITY,
            -0.5 * K::CMY_MAX_DENSITY,
            0.25 * K::CMY_MAX_DENSITY
        ]
    );
    // separation_damping_gain: k ≤ 0 → 0; damping 0 → k; the reference spread → 1 at full
    // damping; clamp at 3.
    assert_eq!(cl::separation_damping_gain(0.0, 1.0, 0.1), 0.0);
    expect_close(
        cl::separation_damping_gain(1.6, 0.0, 0.9),
        1.6,
        1e-15,
        &"damping 0",
    );
    expect_close(
        cl::separation_damping_gain(1.6, 1.0, K::SEPARATION_DAMPING_REF_SPREAD),
        1.0,
        1e-15,
        &"ref spread",
    );
    assert_eq!(cl::separation_damping_gain(3.0, 1.0, 0.0), 3.0);
    assert!(cl::separation_damping_gain(2.5, 1.0, 1.5) < 1.0);
}
