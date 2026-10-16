//! Kernel-level ports of the Swift print-curve suites, at the Swift tolerances:
//! `RampCurveTests` (ramp257 fixture, 1e-5), `QuadraticCoreTests`, `TrueBlackTests`,
//! `PreSaturationTests`, `ToneControlsTests`, `ColorGradingTests` (render halves), the
//! remap-kernel half of `LevelsRemapTests`, `AuditStatsAndLevelsTests.levelsInverseRoundTrip`,
//! `AutoHelpersTests.burnDarkensHighlightsOnly`, `DensityChromaTests` +
//! `SeparationDampingTests` (chain parts), and `HueTrimTests`' two derive-level tests.
//!
//! Not here (other owners): the sidecar round-trips of these suites (`tests/sidecar.rs`),
//! `AutoHelpersTests`' textured-gate tests (`Meters.texturedCells` → `tests/analysis.rs`),
//! `ColorGradingTests.castStrengthBeyondOneStaysBounded` (needs the synthetic_grid analysis →
//! `tests/analysis.rs`), and the derive-only halves agent C already ported in `tests/curve.rs`
//! (`tempTintFoldIntoOffsets`, the sanitizer, `deriveGatesTheBurn`).
//!
//! Where Swift uses `Synthetic64.analysis` (= `ExposureKernel.analyze` on the fixture image),
//! this file reads NegPy's own measurement from `synthetic64/default/analysis.json` — the same
//! numbers within 1e-4 and independent of the analysis port.

// The Swift tests mutate a default field by field; that style IS the drift-catcher here.
#![allow(clippy::field_reassign_with_default)]

use negkit::constants::{K, WorkingOetf};
use negkit::curve_logic as cl;
use negkit::derive::{derive_render_params, sanitize_levels};
use negkit::kernel::ExposureAnalysis;
use negkit::reference::{self, RenderOptions};
use negkit::{ExposureSettings, LogNegativeBounds, RenderParams, RgbImage, V2, V3, fixtures};
use serde_json::Value;
use std::f64::consts::PI;

// ───────────────────────── helpers ─────────────────────────

#[track_caller]
fn expect_close(got: f64, expected: f64, accuracy: f64, label: &dyn std::fmt::Display) {
    assert!(
        (got - expected).abs() <= accuracy,
        "{label} got {got}, expected {expected} ± {accuracy}"
    );
}

fn v3(v: &Value) -> V3 {
    let a = v.as_array().unwrap_or_else(|| panic!("not an array: {v}"));
    [
        a[0].as_f64().unwrap(),
        a[1].as_f64().unwrap(),
        a[2].as_f64().unwrap(),
    ]
}

/// The Swift `RenderParams` memberwise init with its defaults (everything past `vStar`
/// identity / off).
fn render_params(
    final_bounds: LogNegativeBounds,
    slopes: V3,
    pivots: V3,
    curvatures: V3,
    toe_eff: f64,
    shoulder_eff: f64,
    d_min: f64,
) -> RenderParams {
    RenderParams {
        final_bounds,
        slopes,
        pivots,
        curvatures,
        cmy_offsets: [0.0; 3],
        toe_eff,
        shoulder_eff,
        toe_width: 2.5,
        shoulder_width: 2.5,
        d_min,
        v_star: cl::reference_linear_value(d_min, None),
        shadows: 0.0,
        shadow_contrast: 0.0,
        dark_shadows: 0.0,
        highlights: 0.0,
        highlight_contrast: 0.0,
        auto_highlight: 0.0,
        vibrance: 1.0,
        saturation: 1.0,
        skin_protection: 0.0,
        band_hues: [0.0; 4],
        band_saturations: [1.0; 4],
        pre_saturation: 1.0,
        print_saturation: 1.0,
        separation_damping: 0.0,
        hue_trim: 0.0,
        mask_val_scale: [0.0; 3],
        true_black: false,
        shadow_cmy: [0.0; 3],
        mid_cmy: [0.0; 3],
        highlight_cmy: [0.0; 3],
        levels_points: [Vec::new(), Vec::new(), Vec::new()],
    }
}

fn unit_bounds() -> LogNegativeBounds {
    LogNegativeBounds {
        floors: [0.0; 3],
        ceils: [1.0; 3],
    }
}

/// The Swift `ExposureAnalysis(baseBounds:anchor:texturalRange:shadowRefs:)` convenience:
/// floors −2 / ceils −0.5, no neutral axis, unmeasured tails.
fn flat_analysis(shadow_ref: f64) -> ExposureAnalysis {
    ExposureAnalysis {
        base_bounds: LogNegativeBounds {
            floors: [-2.0; 3],
            ceils: [-0.5; 3],
        },
        anchor: 0.46,
        textural_range: 1.0,
        shadow_point: None,
        highlight_point: None,
        shadow_refs: [shadow_ref; 3],
        neutral_mid: None,
        neutral_shadow: None,
        neutral_highlight: None,
        neutral_confidence: None,
    }
}

/// NegPy's own analysis of synthetic64 (`Synthetic64.analysis` in the Swift suite).
fn synthetic64_analysis() -> ExposureAnalysis {
    let j = fixtures::read_json("synthetic64/default/analysis.json").expect("analysis.json");
    let bounds = &j["bounds"];
    let meters = &j["meters"];
    let na = meters["neutral_axis_refs"]
        .as_array()
        .expect("neutral_axis_refs");
    let (neutral_mid, neutral_shadow, neutral_highlight, neutral_confidence) = if na[0].is_null() {
        (None, None, None, None)
    } else {
        (
            Some(v3(&na[0])),
            Some(v3(&na[1])),
            if na[2].is_null() {
                None
            } else {
                Some(v3(&na[2]))
            },
            na[3].as_f64(),
        )
    };
    ExposureAnalysis {
        base_bounds: LogNegativeBounds {
            floors: v3(&bounds["base_floors"]),
            ceils: v3(&bounds["base_ceils"]),
        },
        anchor: meters["metered_anchor"].as_f64().unwrap(),
        textural_range: meters["textural_range"].as_f64().unwrap(),
        shadow_point: meters["shadow_point"].as_f64(),
        highlight_point: meters["highlight_point"].as_f64(),
        shadow_refs: v3(&meters["shadow_log_refs"]),
        neutral_mid,
        neutral_shadow,
        neutral_highlight,
        neutral_confidence,
    }
}

fn synthetic64_input() -> RgbImage {
    fixtures::read_rgb("synthetic64/input.bin", 64, 64).expect("synthetic64/input.bin")
}

/// A `n × 1` grey ramp `x = i / (n − 1)`.
fn ramp_image(n: usize) -> RgbImage {
    RgbImage::new_with(n, 1, |buf| {
        for i in 0..n {
            let x = i as f32 / (n - 1) as f32;
            for c in 0..3 {
                buf[i * 3 + c] = x;
            }
        }
    })
    .unwrap()
}

fn no_levels() -> [Vec<V2>; 3] {
    [Vec::new(), Vec::new(), Vec::new()]
}

fn render_settings(s: &ExposureSettings) -> RgbImage {
    let params = derive_render_params(s, &synthetic64_analysis());
    reference::render(&synthetic64_input(), &params, RenderOptions::default())
}

// ───────────────────────── RampCurveTests ─────────────────────────

/// ramp257 fixture: the raw print curve (no analysis) over a 257-sample ramp, mirroring
/// NegPy's tests/test_characteristic_curve.py::_curve.
#[test]
fn ramp_cases() {
    let j = fixtures::read_json("ramp257.json").expect("ramp257.json");
    let xs: Vec<f32> = j["x"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect();
    let d_min = j["d_min"].as_f64().unwrap();
    let cases = j["cases"].as_object().unwrap();
    assert_eq!(cases.len(), 5);

    for (name, c) in cases {
        let slope = c["slope"].as_f64().unwrap();
        let pivot = c["pivot"].as_f64().unwrap();
        // The fixture passes raw toe/shoulder into apply_characteristic_curve (no grade
        // coupling), so RenderParams carries them unmodified.
        let params = render_params(
            unit_bounds(),
            [slope; 3],
            [pivot; 3],
            [0.0; 3],
            c["toe"].as_f64().unwrap(),
            c["shoulder"].as_f64().unwrap(),
            d_min,
        );
        let ramp = RgbImage::new_with(xs.len(), 1, |buf| {
            for (i, x) in xs.iter().enumerate() {
                for ch in 0..3 {
                    buf[i * 3 + ch] = *x;
                }
            }
        })
        .unwrap();

        let linear = reference::apply_print_curve(&ramp, &params, None);
        let encoded = reference::encode_output(&linear, &no_levels());
        let exp_linear = c["output_linear"].as_array().unwrap();
        let exp_encoded = c["output_encoded"].as_array().unwrap();
        for i in 0..xs.len() {
            expect_close(
                f64::from(linear.data[i * 3]),
                f64::from(exp_linear[i].as_f64().unwrap() as f32),
                1e-5,
                &format!("{name} linear[{i}]"),
            );
            expect_close(
                f64::from(encoded.data[i * 3]),
                f64::from(exp_encoded[i].as_f64().unwrap() as f32),
                1e-5,
                &format!("{name} encoded[{i}]"),
            );
        }
    }
}

// ───────────────────────── QuadraticCoreTests ─────────────────────────

mod quadratic_core {
    use super::*;

    fn slope() -> f64 {
        cl::grade_to_slope(115.0, Some(1.3))
    }
    fn pivot() -> f64 {
        cl::compute_pivot(slope(), 1.0, K::D_MIN, None)
    }
    /// R at the positive curvature clamp, G straight, B at the negative one.
    fn curvatures() -> V3 {
        [
            K::NEUTRAL_AXIS_CURV_MAX_RATIO * slope(),
            0.0,
            -K::NEUTRAL_AXIS_CURV_MAX_RATIO * slope(),
        ]
    }

    #[test]
    fn identity_inside_the_vertex_and_held_beyond() {
        let (k, p) = (slope(), pivot());
        for curv in [curvatures()[0], curvatures()[2]] {
            let vertex = -k / (2.0 * curv);
            let at_vertex = k * (vertex - p) + curv * vertex * vertex;
            // Toward the data from the vertex: the plain quadratic.
            let inside = vertex + if curv > 0.0 { 0.5 } else { -0.5 };
            assert_eq!(
                reference::quadratic_core(k, p, curv, inside),
                k * (inside - p) + curv * inside * inside
            );
            // Past it: held, however far out.
            for far in [0.01, 1.0, 5.0] {
                let u = vertex - if curv > 0.0 { far } else { -far };
                assert_eq!(reference::quadratic_core(k, p, curv, u), at_vertex);
            }
        }
        assert_eq!(reference::quadratic_core(k, p, 0.0, -9.0), k * (-9.0 - p));
    }

    /// Far-out-of-range input (holder edges and dust below 0, bare light above 1) must keep
    /// printing monotonically in every channel.
    #[test]
    fn print_curve_stays_monotone_far_outside_the_frame() {
        let p = render_params(
            unit_bounds(),
            [slope(); 3],
            [pivot(); 3],
            curvatures(),
            0.0,
            0.0,
            K::D_MIN,
        );
        let n = 601;
        let u_at = |i: usize| -4.0 + 6.0 * i as f64 / (n - 1) as f64;
        let ramp = RgbImage::new_with(n, 1, |buf| {
            for i in 0..n {
                for ch in 0..3 {
                    buf[i * 3 + ch] = u_at(i) as f32;
                }
            }
        })
        .unwrap();
        let out = reference::apply_print_curve(&ramp, &p, None);
        for ch in 0..3 {
            for i in 1..n {
                assert!(
                    out.data[i * 3 + ch] <= out.data[(i - 1) * 3 + ch] + 1e-6,
                    "channel {ch} reverses at u = {}",
                    u_at(i)
                );
            }
            // The dense end prints the straight channel's paper white, not a folded-back tone.
            assert!((out.data[ch] - out.data[1]).abs() < 0.02);
        }
    }
}

// ───────────────────────── ToneControlsTests ─────────────────────────

mod tone_controls {
    use super::*;

    /// Default print curve params over a ramp; tone fields injected per test.
    pub fn params(
        shadows: f64,
        shadow_contrast: f64,
        dark_shadows: f64,
        highlights: f64,
        highlight_contrast: f64,
    ) -> RenderParams {
        let slope = cl::grade_to_slope(115.0, Some(1.3));
        let pivot = cl::compute_pivot(slope, 1.0, K::D_MIN, None);
        let mut p = render_params(
            unit_bounds(),
            [slope; 3],
            [pivot; 3],
            [0.0; 3],
            0.0,
            0.0,
            K::D_MIN,
        );
        p.shadows = shadows;
        p.shadow_contrast = shadow_contrast;
        p.dark_shadows = dark_shadows;
        p.highlights = highlights;
        p.highlight_contrast = highlight_contrast;
        p
    }

    pub fn base() -> RenderParams {
        params(0.0, 0.0, 0.0, 0.0, 0.0)
    }

    pub fn ramp_encoded(p: &RenderParams) -> Vec<f32> {
        let n = 257;
        let out = reference::encode_output(
            &reference::apply_print_curve(&ramp_image(n), p, None),
            &no_levels(),
        );
        (0..n).map(|i| out.data[i * 3]).collect()
    }

    // Ramp regions: x near 0 prints bright (highlights), x near 1 prints dark.
    pub const HI_IDX: std::ops::Range<usize> = 0..40; // x < ~0.16
    pub const SH_IDX: std::ops::Range<usize> = 215..257; // x > ~0.84

    /// Ramp index whose straight-line print density is `v` (before knees).
    pub fn idx(at_density: f64) -> usize {
        let n = 257usize;
        let slope = cl::grade_to_slope(115.0, Some(1.3));
        let pivot = cl::compute_pivot(slope, 1.0, K::D_MIN, None);
        let x = pivot + at_density / slope;
        ((x * (n - 1) as f64).round() as i64).clamp(0, n as i64 - 1) as usize
    }

    fn max_abs_delta(a: &[f32], b: &[f32], range: std::ops::Range<usize>) -> f32 {
        range.map(|i| (a[i] - b[i]).abs()).fold(0.0f32, f32::max)
    }

    fn assert_monotone(out: &[f32], label: &dyn std::fmt::Display) {
        for i in 1..out.len() {
            assert!(
                out[i] <= out[i - 1] + 1e-4,
                "non-monotone at {i} for {label}"
            );
        }
    }

    #[test]
    fn shadows_lift_acts_only_on_shadows() {
        let base = ramp_encoded(&base());
        let lifted = ramp_encoded(&params(1.0, 0.0, 0.0, 0.0, 0.0));
        let shadow_delta = SH_IDX.map(|i| lifted[i] - base[i]).fold(f32::MIN, f32::max);
        assert!(
            shadow_delta > 0.05,
            "shadows +1 should lighten the dark end (got {shadow_delta})"
        );
        let leak = max_abs_delta(&lifted, &base, HI_IDX);
        assert!(leak < 0.015, "shadows leaked into highlights: {leak}");
    }

    #[test]
    fn highlights_recovery_acts_only_on_highlights() {
        let base = ramp_encoded(&base());
        let recovered = ramp_encoded(&params(0.0, 0.0, 0.0, -1.0, 0.0));
        let highlight_delta = HI_IDX
            .map(|i| base[i] - recovered[i])
            .fold(f32::MIN, f32::max);
        assert!(
            highlight_delta > 0.04,
            "highlights −1 should darken the bright end (got {highlight_delta})"
        );
        let leak = max_abs_delta(&recovered, &base, SH_IDX);
        assert!(leak < 0.015, "highlights leaked into shadows: {leak}");
    }

    #[test]
    fn shadow_contrast_expands_dark_separation() {
        let base = ramp_encoded(&base());
        let contrasty = ramp_encoded(&params(0.0, 1.0, 0.0, 0.0, 0.0));
        // Separation grows around the shadow anchor (measured between a medium-dark and a
        // deep-shadow tone that isn't yet toe-crushed).
        let a = idx(K::SHADOW_TONE_ANCHOR - 0.25);
        let b = idx(K::SHADOW_TONE_ANCHOR + 0.4);
        let base_spread = base[a] - base[b];
        let new_spread = contrasty[a] - contrasty[b];
        assert!(
            new_spread > base_spread + 0.01,
            "shadow contrast spread {base_spread} → {new_spread}"
        );
        let leak = max_abs_delta(&contrasty, &base, HI_IDX);
        assert!(
            leak < 0.015,
            "shadow contrast leaked into highlights: {leak}"
        );
    }

    #[test]
    fn highlight_contrast_expands_bright_separation() {
        let base = ramp_encoded(&base());
        let contrasty = ramp_encoded(&params(0.0, 0.0, 0.0, 0.0, 1.0));
        let a = idx(K::HIGHLIGHT_TONE_ANCHOR - 0.2);
        let b = idx(K::HIGHLIGHT_TONE_ANCHOR + 0.3);
        let base_spread = base[a] - base[b];
        let new_spread = contrasty[a] - contrasty[b];
        assert!(
            new_spread > base_spread + 0.01,
            "highlight contrast spread {base_spread} → {new_spread}"
        );
        let leak = max_abs_delta(&contrasty, &base, SH_IDX);
        assert!(
            leak < 0.015,
            "highlight contrast leaked into shadows: {leak}"
        );
    }

    #[test]
    fn monotone_at_full_deflection() {
        let configs: [[f64; 4]; 12] = [
            [1.0, 0.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0, 0.0],
            [-2.0, 0.0, 0.0, 0.0], // extended shadows range
            [0.0, 1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0, 0.0],
            [0.0, 6.0, 0.0, 0.0],
            [0.0, -3.0, 0.0, 0.0], // extended shadow-contrast range (kernel floor guards −3)
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, -1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0, -1.0],
        ];
        for c in configs {
            let out = ramp_encoded(&params(c[0], c[1], 0.0, c[2], c[3]));
            assert_monotone(&out, &format!("{c:?}"));
        }
    }

    #[test]
    fn exposure_stops_brighten_globally() {
        let analysis = synthetic64_analysis();
        let mut brighter = ExposureSettings::default();
        brighter.exposure_stops = 1.0;
        let base = derive_render_params(&ExposureSettings::default(), &analysis);
        let plus_one = derive_render_params(&brighter, &analysis);
        // One stop = −log10(2)/range on every channel's pre-curve offset.
        for ch in 0..3 {
            let range = (base.final_bounds.ceils[ch] - base.final_bounds.floors[ch]).abs();
            expect_close(
                plus_one.cmy_offsets[ch] - base.cmy_offsets[ch],
                -K::LOG10_TWO / range,
                1e-9,
                &format!("offset ch{ch}"),
            );
        }
        // And the rendered frame gets brighter everywhere it isn't clipped.
        let mean = |img: &RgbImage| {
            img.data.iter().map(|&v| f64::from(v)).sum::<f64>() / img.data.len() as f64
        };
        let mean_base = mean(&render_settings(&ExposureSettings::default()));
        let mean_bright = mean(&render_settings(&brighter));
        assert!(
            mean_bright > mean_base + 0.05,
            "exposure +1: mean {mean_base} → {mean_bright}"
        );
    }

    #[test]
    fn overall_contrast_rotates_around_anchor() {
        let analysis = synthetic64_analysis();
        let mut contrasty = ExposureSettings::default();
        contrasty.overall_contrast = 1.5;
        let mut flat = ExposureSettings::default();
        flat.overall_contrast = -1.0;
        let base = derive_render_params(&ExposureSettings::default(), &analysis);
        let up = derive_render_params(&contrasty, &analysis);
        let down = derive_render_params(&flat, &analysis);

        // Slopes scale by (1+k); k = slider × overallContrastMax.
        for ch in 0..3 {
            expect_close(
                up.slopes[ch],
                base.slopes[ch] * 1.75,
                1e-9,
                &format!("slope up ch{ch}"),
            );
            expect_close(
                down.slopes[ch],
                base.slopes[ch] * 0.5,
                1e-9,
                &format!("slope down ch{ch}"),
            );
        }

        // The input that hits v* on the base curve still hits v* (green, curv 0): the
        // reference tone's print density is invariant under contrast.
        let v_star = base.v_star;
        let u_star = base.pivots[1] + v_star / base.slopes[1];
        let v_up = up.slopes[1] * (u_star - up.pivots[1]) + up.curvatures[1] * u_star * u_star;
        expect_close(v_up, v_star, 1e-9, &"anchor invariance");

        // End-to-end: rendered spread grows, anchor output stays put, monotone.
        let base_out = ramp_encoded(&self::base());
        let mut p = self::base();
        let k = 1.5 * K::OVERALL_CONTRAST_MAX;
        for ch in 0..3 {
            let scaled = p.slopes[ch] * (1.0 + k);
            p.curvatures[ch] *= 1.0 + k;
            p.pivots[ch] += k * p.v_star / scaled;
            p.slopes[ch] = scaled;
        }
        let contrast_out = ramp_encoded(&p);
        let anchor_idx = (0.46f64 * 256.0).round() as usize; // assumed_anchor input
        expect_close(
            f64::from(contrast_out[anchor_idx]),
            f64::from(base_out[anchor_idx]),
            0.01,
            &"anchor output",
        );
        let (a, b) = (idx(0.3), idx(1.3));
        assert!(
            contrast_out[a] - contrast_out[b] > base_out[a] - base_out[b] + 0.02,
            "spread grows"
        );
        assert_monotone(&contrast_out, &"overall contrast");
    }

    #[test]
    fn dark_shadows_target_deeper_than_shadows() {
        let base = ramp_encoded(&base());
        let dark = ramp_encoded(&params(0.0, 0.0, 1.0, 0.0, 0.0));
        // Strong effect near the dark anchor, minimal at the broad-shadows anchor and in the
        // mids/highlights.
        let deep_idx = idx(K::DARK_SHADOW_TONE_ANCHOR + 0.15);
        let broad_idx = idx(K::SHADOW_TONE_ANCHOR - 0.35);
        let mid_idx = idx(K::ANCHOR_TARGET_DENSITY);
        assert!(
            dark[deep_idx] - base[deep_idx] > 0.015,
            "deep lift (got {})",
            dark[deep_idx] - base[deep_idx]
        );
        assert!(
            (dark[broad_idx] - base[broad_idx]).abs() < 0.012,
            "broad-shadow leak"
        );
        assert!(
            (dark[mid_idx] - base[mid_idx]).abs() < 0.006,
            "midtone leak"
        );
        // Monotone at both extremes.
        for extreme in [2.0, -2.0] {
            let out = ramp_encoded(&params(0.0, 0.0, extreme, 0.0, 0.0));
            assert_monotone(&out, &format!("darkShadows {extreme}"));
        }
        // (The sidecar round-trip + legacy default live in tests/sidecar.rs.)
    }

    /// Belt-and-braces on top of the fixture tests: explicit zero tone fields produce
    /// byte-identical output to a params struct without them.
    #[test]
    fn zero_settings_are_identity() {
        let base = ramp_encoded(&base());
        let explicit_zero = ramp_encoded(&params(0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(base, explicit_zero);
    }
}

// ───────────────────────── TrueBlackTests ─────────────────────────

/// True Black (black point compensation, NegPy 0.36): paper Dmax maps to display black
/// instead of floating at 10^−2.3 ≈ 0.5% gray.
#[test]
fn true_black_maps_paper_black_to_zero() {
    let mut p = tone_controls::base();
    let base = tone_controls::ramp_encoded(&p);
    p.true_black = true;
    let bpc = tone_controls::ramp_encoded(&p);

    // Without BPC paper black floats well above display black; BPC pulls it down hard. Exact
    // 0 is unreachable at neutral toe — the softplus bound approaches d_max only
    // asymptotically. Asserted in the LINEAR (decoded) domain so the contract is
    // OETF-independent.
    let base_lin = f64::from(WorkingOetf::decode(*base.last().unwrap()));
    let bpc_lin = f64::from(WorkingOetf::decode(*bpc.last().unwrap()));
    assert!(
        base_lin > 0.003,
        "without BPC paper black floats (linear {base_lin})"
    );
    assert!(
        bpc_lin < 0.0015 && bpc_lin < base_lin * 0.4,
        "BPC should pull toward display black (linear {base_lin} → {bpc_lin})"
    );
    // Highlights are essentially untouched (normalization by 1−black ≈ 1.005).
    expect_close(
        f64::from(bpc[0]),
        f64::from(base[0]),
        0.01,
        &"highlight end",
    );
    // Still monotone.
    for i in 1..bpc.len() {
        assert!(bpc[i] <= bpc[i - 1] + 1e-4, "non-monotone at {i}");
    }
}

// ───────────────────────── PreSaturationTests ─────────────────────────

mod pre_saturation {
    use super::*;

    #[test]
    fn identity_at_one() {
        let mut s = ExposureSettings::default();
        s.pre_saturation = 1.0;
        let p = derive_render_params(&s, &synthetic64_analysis());
        assert_eq!(p.pre_saturation, 1.0);
        // Fixture parity tests already pin the full default output.
    }

    /// Equal channels have zero deviation — pre-saturation must not touch them.
    #[test]
    fn neutral_pixels_unchanged() {
        let gray = RgbImage::new_with(4, 1, |buf| {
            for i in 0..4 {
                for c in 0..3 {
                    buf[i * 3 + c] = 0.2 + 0.2 * i as f32;
                }
            }
        })
        .unwrap();
        let mut p = tone_controls::base();
        let base = reference::apply_print_curve(&gray, &p, None);
        p.pre_saturation = 1.5;
        let boosted = reference::apply_print_curve(&gray, &p, None);
        for i in 0..base.data.len() {
            expect_close(
                f64::from(boosted.data[i]),
                f64::from(base.data[i]),
                1e-6,
                &format!("gray {i}"),
            );
        }
    }

    /// On the synthetic64 image (colored corners), pre-saturation > 1 must raise the mean
    /// CIELAB chroma of the rendered positive.
    #[test]
    fn increases_output_chroma() {
        let mut base = ExposureSettings::default();
        base.pre_saturation = 1.0;
        let mut boosted = ExposureSettings::default();
        boosted.pre_saturation = 1.3;
        let out_base = render_settings(&base);
        let out_boost = render_settings(&boosted);

        let mean_chroma = |img: &RgbImage| {
            let mut total = 0.0;
            for px in img.data.chunks_exact(3) {
                // Decode the working TRC back to linear for the Lab measure.
                let rgb = [
                    f64::from(WorkingOetf::decode(px[0])),
                    f64::from(WorkingOetf::decode(px[1])),
                    f64::from(WorkingOetf::decode(px[2])),
                ];
                let lab = negkit::lab::rgb_to_lab(rgb);
                total += (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
            }
            total / img.pixel_count() as f64
        };
        let c_base = mean_chroma(&out_base);
        let c_boost = mean_chroma(&out_boost);
        // Margin 1.02 (was 1.05 in ROMM/D50 Lab): the smaller-gamut Lab measures the same
        // numeric boost as a smaller chroma delta; direction is the contract.
        assert!(c_boost > c_base * 1.02, "chroma {c_base} → {c_boost}");
    }
}

// ───────────────────────── ColorGradingTests ─────────────────────────

mod color_grading {
    use super::*;

    /// Ramp through the print curve with band CMY injected.
    fn ramp_encoded(shadow_cmy: V3, mid_cmy: V3, highlight_cmy: V3) -> Vec<[f32; 3]> {
        let n = 257;
        let mut p = tone_controls::base();
        p.shadow_cmy = shadow_cmy.map(|v| v * K::CMY_MAX_DENSITY);
        p.mid_cmy = mid_cmy.map(|v| v * K::CMY_MAX_DENSITY);
        p.highlight_cmy = highlight_cmy.map(|v| v * K::CMY_MAX_DENSITY);
        let out = reference::encode_output(
            &reference::apply_print_curve(&ramp_image(n), &p, None),
            &no_levels(),
        );
        (0..n)
            .map(|i| [out.data[i * 3], out.data[i * 3 + 1], out.data[i * 3 + 2]])
            .collect()
    }

    fn base() -> Vec<[f32; 3]> {
        ramp_encoded([0.0; 3], [0.0; 3], [0.0; 3])
    }

    #[test]
    fn shadow_band_affects_only_shadows() {
        let base = base();
        // Shadows R→C full: adds red-channel density in the dark band → less red.
        let graded = ramp_encoded([1.0, 0.0, 0.0], [0.0; 3], [0.0; 3]);
        let s_idx = tone_controls::idx(K::SHADOW_TONE_ANCHOR);
        let h_idx = tone_controls::idx(K::HIGHLIGHT_TONE_ANCHOR);
        // ~0.018 encoded delta for a 0.2 D shift: dark tones are compressed by the paper toe
        // and the OETF, so the gate is calibrated, not round.
        assert!(
            base[s_idx][0] - graded[s_idx][0] > 0.015,
            "red should drop in shadows"
        );
        expect_close(
            f64::from(graded[s_idx][1]),
            f64::from(base[s_idx][1]),
            1e-5,
            &"green untouched",
        );
        assert!(
            (graded[h_idx][0] - base[h_idx][0]).abs() < 0.012,
            "highlights leak"
        );
    }

    #[test]
    fn high_band_affects_only_highs() {
        let base = base();
        // Highs B→Y full: adds blue-channel density in the bright band → yellow.
        let graded = ramp_encoded([0.0; 3], [0.0; 3], [0.0, 0.0, 1.0]);
        let s_idx = tone_controls::idx(K::SHADOW_TONE_ANCHOR + 0.3);
        let h_idx = tone_controls::idx(K::HIGHLIGHT_TONE_ANCHOR);
        assert!(
            base[h_idx][2] - graded[h_idx][2] > 0.02,
            "blue should drop in highs"
        );
        assert!(
            (graded[s_idx][2] - base[s_idx][2]).abs() < 0.012,
            "shadows leak"
        );
    }

    #[test]
    fn mid_band_centered() {
        let base = base();
        let graded = ramp_encoded([0.0; 3], [0.0, 1.0, 0.0], [0.0; 3]);
        let m_idx = tone_controls::idx(0.85); // between the anchors
        let s_idx = tone_controls::idx(K::SHADOW_TONE_ANCHOR + 0.45);
        let mid_drop = base[m_idx][1] - graded[m_idx][1];
        assert!(mid_drop > 0.02, "green should drop in mids");
        assert!(
            (graded[s_idx][1] - base[s_idx][1]).abs() < mid_drop.abs() / 2.0,
            "mids dominate"
        );
    }
}

// ───────────────────────── LevelsRemapTests (kernel half) ─────────────────────────

mod levels_remap {
    use super::*;

    /// One anchor: the classic two-segment stretch/compress.
    #[test]
    fn single_anchor_matches_closed_form() {
        let encoded: [f32; 6] = [0.0, 0.1, 0.2, 0.5, 0.8, 1.0];
        let source = RgbImage::new_with(encoded.len(), 1, |buf| {
            for (i, e) in encoded.iter().enumerate() {
                let lin = WorkingOetf::decode(*e);
                buf[i * 3..i * 3 + 3].fill(lin);
            }
        })
        .unwrap();
        let plain = reference::encode_output(&source, &no_levels());
        let img = reference::encode_output(&source, &[vec![[0.2, 0.4]], Vec::new(), Vec::new()]);

        let expect_red = |i: usize, want: f32| {
            assert!(
                (img.data[i * 3] - want).abs() < 1e-5,
                "red[{i}] = {}, want {want}",
                img.data[i * 3]
            );
        };
        expect_red(0, 0.0); // endpoint pinned
        expect_red(1, 0.2); // 0.1 × (0.4/0.2) — left segment stretches
        expect_red(2, 0.4); // the anchor lands at its output
        expect_red(3, 0.625); // 0.4 + 0.3 × (0.6/0.8) — right segment compresses
        expect_red(4, 0.85);
        expect_red(5, 1.0); // endpoint pinned

        // Untouched channels: bit-equal to the plain encode.
        for i in 0..encoded.len() {
            assert_eq!(img.data[i * 3 + 1], plain.data[i * 3 + 1]);
            assert_eq!(img.data[i * 3 + 2], plain.data[i * 3 + 2]);
        }
    }

    /// Two anchors: three independent segments; each anchor pins its output and tones between
    /// interpolate linearly.
    #[test]
    fn two_anchors_piecewise() {
        let pts: [[f32; 2]; 2] = [[0.25, 0.4], [0.75, 0.8]];
        let remap = |e: f32| reference::levels_remap(e, &pts);
        assert!((remap(0.25) - 0.4).abs() < 1e-6);
        assert!((remap(0.75) - 0.8).abs() < 1e-6);
        // Midpoint of the middle segment: (0.25..0.75)→(0.4..0.8) is linear.
        assert!((remap(0.5) - 0.6).abs() < 1e-6);
        // Left segment: 0.125 → 0.2; right segment: 0.875 → 0.9.
        assert!((remap(0.125) - 0.2).abs() < 1e-6);
        assert!((remap(0.875) - 0.9).abs() < 1e-6);
        // Endpoints pinned.
        assert_eq!(remap(0.0), 0.0);
        assert!((remap(1.0) - 1.0).abs() < 1e-6);
        // An anchor moved later only bends ITS segments: tones left of the untouched first
        // anchor are identical under both maps.
        let moved: [[f32; 2]; 2] = [[0.25, 0.4], [0.75, 0.6]];
        for e in [0.05f32, 0.15, 0.25] {
            assert_eq!(
                reference::levels_remap(e, &pts),
                reference::levels_remap(e, &moved)
            );
        }
    }

    #[test]
    fn empty_is_identity() {
        let img = RgbImage::new_with(4, 4, |buf| {
            for (i, v) in buf.iter_mut().enumerate() {
                *v = i as f32 / 47.0;
            }
        })
        .unwrap();
        let plain = reference::encode_output(&img, &no_levels());
        let dflt = reference::encode_output(&img, &[Vec::new(), Vec::new(), Vec::new()]);
        assert_eq!(plain.data, dflt.data);
        // And the plain encode is the OETF lane for lane.
        for (p, v) in plain.data.iter().zip(&img.data) {
            assert_eq!(*p, WorkingOetf::encode(*v));
        }
    }

    #[test]
    fn remap_is_monotone() {
        let ramp: Vec<f32> = (0..=256).map(|i| i as f32 / 256.0).collect();
        let anchor_sets: Vec<Vec<[f32; 2]>> = vec![
            vec![[0.1, 0.6]],
            vec![[0.8, 0.3]],
            vec![[0.2, 0.3], [0.5, 0.55], [0.9, 0.95]],
            vec![[0.3, 0.3], [0.31, 0.9]], // near-vertical segment
        ];
        for pts in &anchor_sets {
            let mut last = -1.0f32;
            for &e in &ramp {
                let y = reference::levels_remap(e, pts);
                assert!(y >= last - 1e-6, "non-monotone at {e} for {pts:?}");
                last = y;
            }
        }
    }
}

// ───────────────────────── AuditStatsAndLevelsTests.levelsInverseRoundTrip ─────────────────────────

/// Forward map runs in f32 (the shader mirror); the inverse in f64 (the interactive
/// histogram's grab math). Round trip must agree within 1e-4 for sanitized anchor sets,
/// including a near-vertical segment.
#[test]
fn levels_inverse_round_trip() {
    let anchor_sets: Vec<Vec<V2>> = vec![
        vec![[0.3, 0.6]],                           // 1 anchor
        vec![[0.2, 0.3], [0.5, 0.55], [0.9, 0.95]], // 3 anchors
        vec![[0.3, 0.3], [0.31, 0.9]],              // steep near-vertical segment
    ];
    for raw in &anchor_sets {
        let pts = sanitize_levels(raw);
        assert!(!pts.is_empty()); // none of these are identity sets
        let float_pts: Vec<[f32; 2]> = pts.iter().map(|p| [p[0] as f32, p[1] as f32]).collect();
        for step in 0..=20 {
            let x = step as f64 * 0.05;
            let y = reference::levels_remap(x as f32, &float_pts);
            let back = reference::levels_inverse_remap(f64::from(y), &pts);
            assert!(
                (back - x).abs() < 1e-4,
                "x={x} → y={y} → back={back} for {pts:?}"
            );
        }
    }
}

// ───────────────────────── AutoHelpersTests.burnDarkensHighlightsOnly ─────────────────────────

/// The kernel term only ever adds density (the print gets darker, reflectance drops) and its
/// weight lives in the highlight zone: a deep shadow tone moves by orders of magnitude less
/// than a highlight tone.
#[test]
fn burn_darkens_highlights_only() {
    let mut params = derive_render_params(&ExposureSettings::default(), &flat_analysis(-0.8));
    // A luma ramp from print highlights to shadows (normalized-log in).
    let n = 64;
    let ramp = RgbImage::new_with(n, 1, |buf| {
        for i in 0..n {
            let t = i as f32 / (n - 1) as f32 * 1.2 - 0.1;
            buf[i * 3..i * 3 + 3].fill(t);
        }
    })
    .unwrap();
    let base = reference::apply_print_curve(&ramp, &params, None);
    params.auto_highlight = 0.3;
    let burned = reference::apply_print_curve(&ramp, &params, None);
    let mut max_highlight_drop = 0.0f32;
    let mut max_shadow_drop = 0.0f32;
    for i in 0..n {
        let drop = base.data[i * 3 + 1] - burned.data[i * 3 + 1];
        assert!(drop >= -1e-6, "burn must never brighten (i={i})");
        if base.data[i * 3 + 1] > 0.5 {
            max_highlight_drop = max_highlight_drop.max(drop);
        }
        if base.data[i * 3 + 1] < 0.02 {
            max_shadow_drop = max_shadow_drop.max(drop);
        }
    }
    assert!(
        max_highlight_drop > 0.05,
        "the burn must land in the highlights"
    );
    assert!(
        max_shadow_drop < max_highlight_drop / 20.0,
        "deep shadows must be spared"
    );
}

// ───────────────────────── DensityChromaTests / SeparationDampingTests ─────────────────────────

mod density_chroma {
    use super::*;

    /// A tiny params set with fixed bounds so pixel values are predictable.
    pub fn params(mutate: impl FnOnce(&mut RenderParams)) -> RenderParams {
        let mut p = render_params(
            LogNegativeBounds {
                floors: [-2.0; 3],
                ceils: [-0.5; 3],
            },
            [3.0; 3],
            [0.4; 3],
            [0.0; 3],
            0.0,
            0.0,
            0.0,
        );
        mutate(&mut p);
        p
    }

    /// Curve output for one normalized-log triplet.
    pub fn print1(rgb: V3, p: &RenderParams) -> V3 {
        let img =
            RgbImage::from_vec(1, 1, vec![rgb[0] as f32, rgb[1] as f32, rgb[2] as f32]).unwrap();
        let out = reference::apply_print_curve(&img, p, None);
        [
            f64::from(out.data[0]),
            f64::from(out.data[1]),
            f64::from(out.data[2]),
        ]
    }

    /// Print densities (relative, BPC off) for spread measurement.
    pub fn densities(rgb: V3, p: &RenderParams) -> V3 {
        print1(rgb, p).map(|t| -t.max(1e-9).log10())
    }

    pub fn spread(d: V3) -> f64 {
        d[0].max(d[1]).max(d[2]) - d[0].min(d[1]).min(d[2])
    }

    #[test]
    fn default_is_identity() {
        let base = params(|_| {});
        let active = params(|p| p.print_saturation = 1.0);
        let px = [0.3, 0.45, 0.6];
        assert_eq!(print1(px, &base), print1(px, &active));
    }

    /// Neutral pixels (equal channels) never move at any strength.
    #[test]
    fn neutrals_are_invariant() {
        let neutral = [0.5; 3];
        let base = print1(neutral, &params(|_| {}));
        for k in [1.8, 0.3] {
            let moved = print1(neutral, &params(|p| p.print_saturation = k));
            for ch in 0..3 {
                assert!((moved[ch] - base[ch]).abs() < 1e-9, "k {k} ch{ch}");
            }
        }
    }

    #[test]
    fn print_saturation_scales_density_separation() {
        let px = [0.35, 0.45, 0.55];
        let s0 = spread(densities(px, &params(|_| {})));
        let s_up = spread(densities(px, &params(|p| p.print_saturation = 1.6)));
        let s_down = spread(densities(px, &params(|p| p.print_saturation = 0.4)));
        assert!(s0 > 0.01);
        assert!(
            s_up > s0 * 1.3,
            "k > 1 must widen dye separation ({s0} → {s_up})"
        );
        assert!(s_down < s0 * 0.7, "k < 1 must narrow it ({s0} → {s_down})");
        // k = 0 collapses to the achromatic mean.
        let s_zero = spread(densities(px, &params(|p| p.print_saturation = 0.0)));
        assert!(s_zero < 1e-6);
    }

    // ── SeparationDampingTests ──

    #[test]
    fn gain_closed_form() {
        let r = K::SEPARATION_DAMPING_REF_SPREAD;
        // Grey pixel (chroma 0): h = 1 → full k at any damping.
        assert!((cl::separation_damping_gain(1.4, 1.0, 0.0) - 1.4).abs() < 1e-12);
        // At the reference spread: h = 0 → k^(1−τ); τ=1 → exactly 1.
        assert!((cl::separation_damping_gain(1.4, 1.0, r) - 1.0).abs() < 1e-12);
        assert!((cl::separation_damping_gain(1.4, 0.5, r) - 1.4f64.powf(0.5)).abs() < 1e-12);
        // Far above the reference: h → −1 → 1/k at τ=1 (the reversal).
        let far = cl::separation_damping_gain(1.4, 1.0, 100.0);
        assert!((far - 1.0 / 1.4).abs() < 0.01);
        // τ=0 is exactly the flat k, whatever the chroma.
        assert_eq!(cl::separation_damping_gain(1.4, 0.0, 0.7), 1.4);
        // Clamp and the k≤0 guard.
        assert_eq!(cl::separation_damping_gain(2.9, 1.0, 0.0), 2.9);
        assert_eq!(cl::separation_damping_gain(0.0, 0.5, 0.1), 0.0);
    }

    /// Inert at printSaturation 1 (any damping), and damping 0 is bit-equal to the flat path.
    #[test]
    fn inert_and_flat_equivalence() {
        let px = [0.3, 0.45, 0.65];
        let base = densities(px, &params(|_| {}));
        let damped1 = densities(px, &params(|p| p.separation_damping = 1.0));
        assert_eq!(base, damped1); // printSaturation 1 → nothing to redistribute
        let flat = densities(px, &params(|p| p.print_saturation = 1.5));
        let tau0 = densities(
            px,
            &params(|p| {
                p.print_saturation = 1.5;
                p.separation_damping = 0.0;
            }),
        );
        assert_eq!(flat, tau0);
    }

    /// At full damping with k > 1: a muted pixel's dye spread still widens, a vivid pixel's
    /// spread NARROWS (the reversal a flat k cannot do).
    #[test]
    fn push_redistributes_between_populations() {
        let muted = [0.46, 0.48, 0.50];
        let vivid = [0.2, 0.45, 0.75];
        let p_flat = params(|p| p.print_saturation = 1.5);
        let p_damped = params(|p| {
            p.print_saturation = 1.5;
            p.separation_damping = 1.0;
        });

        let muted_base = spread(densities(muted, &params(|_| {})));
        let muted_damped = spread(densities(muted, &p_damped));
        assert!(
            muted_damped > muted_base * 1.2,
            "muted must still widen ({muted_base} → {muted_damped})"
        );

        let vivid_base = spread(densities(vivid, &params(|_| {})));
        let vivid_flat = spread(densities(vivid, &p_flat));
        let vivid_damped = spread(densities(vivid, &p_damped));
        assert!(vivid_flat > vivid_base, "flat k widens vivid too");
        assert!(
            vivid_damped < vivid_base,
            "damped push must REVERSE on vivid ({vivid_base} → {vivid_damped})"
        );
    }
}

// ───────────────────────── HueTrimTests (derive-level) ─────────────────────────

mod hue_trim {
    use super::*;

    /// The default must be identity or every NegPy fixture would move (the derive half; the
    /// settings default is pinned in tests/lab.rs).
    #[test]
    fn default_setting_is_off() {
        assert_eq!(ExposureSettings::default().hue_trim, 0.0);
        let p = derive_render_params(&ExposureSettings::default(), &flat_analysis(-0.6));
        assert_eq!(p.hue_trim, 0.0);
        // And a zero trim leaves the rendered print bit-identical to one without the stage:
        // ColorPop::is_active is false for the parity-neutral settings.
        let mut neutral = ExposureSettings::default();
        neutral.pre_saturation = 1.0;
        neutral.skin_protection = 0.0;
        let pn = derive_render_params(&neutral, &flat_analysis(-0.6));
        assert!(!negkit::lab::ColorPop::from_render_params(&pn).is_active());
    }

    #[test]
    fn derive_converts_degrees_to_radians_and_clamps() {
        let analysis = flat_analysis(-0.6);
        let trim = |deg: f64| {
            let mut s = ExposureSettings::default();
            s.hue_trim = deg;
            derive_render_params(&s, &analysis).hue_trim
        };
        assert!((trim(30.0) - 30.0 * PI / 180.0).abs() < 1e-12);
        assert!((trim(-12.5) - (-12.5 * PI / 180.0)).abs() < 1e-12);
        // Out-of-range input clamps to the slider's ±30 rather than rotating wildly.
        assert!((trim(400.0) - 30.0 * PI / 180.0).abs() < 1e-12);
        assert!((trim(-400.0) - (-30.0 * PI / 180.0)).abs() < 1e-12);
    }

    /// The reference chain routes a non-zero trim through the colour-pop stage: the rendered
    /// print equals the bare curve followed by `ColorPop::apply_pixel`, and neutrals hold.
    #[test]
    fn chain_applies_the_trim_after_the_curve() {
        let mut s = ExposureSettings::default();
        s.pre_saturation = 1.0;
        s.skin_protection = 0.0;
        s.hue_trim = 8.0;
        let p = derive_render_params(&s, &synthetic64_analysis());
        let normalized = reference::normalize(&synthetic64_input(), &p.final_bounds);
        let with_pop = reference::apply_print_curve(&normalized, &p, None);
        let mut bare = p.clone();
        bare.hue_trim = 0.0;
        let bare_out = reference::apply_print_curve(&normalized, &bare, None);
        let pop = negkit::lab::ColorPop::from_render_params(&p);
        assert!(pop.is_active());
        for (a, b) in with_pop
            .data
            .chunks_exact(3)
            .zip(bare_out.data.chunks_exact(3))
        {
            let want = pop.apply_pixel([f64::from(b[0]), f64::from(b[1]), f64::from(b[2])]);
            for ch in 0..3 {
                assert!((f64::from(a[ch]) - want[ch]).abs() < 1e-6);
            }
        }
    }
}
