//! Derive-level curve tests — ports of `AuditCurveFallbackTests`, the curve-logic/derive half of
//! `AutoHelpersTests`, the sanitizer/derive half of `LevelsRemapTests`, and the `curveParams`
//! half of `AnalysisParityTests` (3 synthetic64 configs, fed through `derive_render_params`
//! from the fixtures' own `analysis.json` so the gate is independent of the analysis port).
//!
//! Left for wave 2 (they drive `ReferenceCurve` / `Meters`): `AutoHelpersTests`' textured-gate
//! tests and `burnDarkensHighlightsOnly`, `LevelsRemapTests`' remap-kernel tests,
//! `AuditStatsAndLevelsTests.levelsInverseRoundTrip`, `DensityChromaTests` (bar its sidecar
//! test, in `sidecar.rs`), and `AnalysisParityTests.fullChain`.

// The Swift tests mutate a default field by field; that style IS the drift-catcher here.
#![allow(clippy::field_reassign_with_default)]

use negkit::constants::K;
use negkit::curve_logic as cl;
use negkit::derive::{
    self, LEVELS_CLAMP, LEVELS_MAX_POINTS, derive_render_params, sanitize_levels,
};
use negkit::kernel::ExposureAnalysis;
use negkit::{ExposureSettings, LogNegativeBounds, V3, fixtures};
use serde_json::Value;

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

/// A fixture-shaped analysis with no neutral axis and unmeasured tails (the Swift tests'
/// hand-built `ExposureAnalysis`).
fn flat_analysis() -> ExposureAnalysis {
    ExposureAnalysis {
        base_bounds: LogNegativeBounds {
            floors: [-2.0; 3],
            ceils: [-0.5; 3],
        },
        anchor: 0.46,
        textural_range: 1.0,
        shadow_point: None,
        highlight_point: None,
        shadow_refs: [-0.8; 3],
        neutral_mid: None,
        neutral_shadow: None,
        neutral_highlight: None,
        neutral_confidence: None,
    }
}

// ───────────────────────── AuditCurveFallbackTests ─────────────────────────
//
// perChannelCurveParams' shadow-ref fallback branch (no neutral axis): the one-point slope tie
// at the P98 shadow refs, its ±castRemovalMaxOffset clamp, and the branch's zero curvatures.
// Expectations are hand-derived from the implementation formula:
//   cast     = clamp(strength · (rG − ref[ch]), ±K.castRemovalMaxOffset)
//   slope_ch = clamp(baseSlope · (anchor − rG) / (anchor − (rG − cast)), 2…10)
//   pivot_ch = anchor − v*/slope_ch + (1 − density) · K.densityMultiplier

mod fallback {
    use super::*;

    const GRADE: f64 = 115.0;
    const LUM_RANGE: f64 = 1.2;
    const ANCHOR: f64 = 0.5;
    const DENSITY: f64 = 1.0;

    /// baseSlope by hand: autoNormalizeContrast = false → range = lumRange; er = 115/100,
    /// rng = 1.2 → k = 2.9 · 1.2 / 1.15 = 3.02608695…, inside [2, 10] so no clamp.
    fn base_slope() -> f64 {
        K::GRADE_CONTRAST_SCALE * LUM_RANGE / (GRADE / 100.0)
    }

    /// v* from the documented closed form (K.dMax 2.3, target 0.75, sharpness 6/3), written
    /// out with libm inverse-softplus — NOT by calling reference_linear_value.
    fn v_star() -> f64 {
        let v1 = K::D_MAX
            - (K::TOE_SHARPNESS_BASE * (K::D_MAX - K::ANCHOR_TARGET_DENSITY))
                .exp_m1()
                .ln()
                / K::TOE_SHARPNESS_BASE;
        (K::SHOULDER_SHARPNESS_BASE * v1).exp_m1().ln() / K::SHOULDER_SHARPNESS_BASE
    }

    fn params(refs: V3, strength: f64) -> cl::CurveParams {
        cl::per_channel_curve_params(
            GRADE,
            DENSITY,
            false,
            strength,
            Some(LUM_RANGE),
            Some(refs),
            None,
            0.0,
            Some(ANCHOR),
            None,
            None,
        )
    }

    #[test]
    fn shadow_ref_tie_matches_closed_form() {
        // refs = (0.95, 0.90, 0.85), strength 0.5 — casts stay inside ±0.1:
        //   cast_r = 0.5·(0.90 − 0.95) = −0.025, cast_b = +0.025
        let refs = [0.95, 0.90, 0.85];
        let p = params(refs, 0.5);
        let base = base_slope();
        let numer = ANCHOR - refs[1]; // 0.5 − 0.9 = −0.4

        // Green is the reference: exactly the base slope, no tie.
        assert_eq!(p.slopes[1], base);

        // Red: denom = 0.5 − (0.90 − (−0.025)) = −0.425 → slope_r = base · 0.941176…
        let slope_r = base * numer / (ANCHOR - (refs[1] + 0.025));
        assert!((p.slopes[0] - slope_r).abs() < 1e-12);
        // Blue: denom = 0.5 − (0.90 − 0.025) = −0.375 → slope_b = base · 1.0666…
        let slope_b = base * numer / (ANCHOR - (refs[1] - 0.025));
        assert!((p.slopes[2] - slope_b).abs() < 1e-12);
        // Both inside [2, 10], so the clamp changed nothing (≈2.85, ≈3.23).
        assert!((K::SLOPE_MIN..=K::SLOPE_MAX).contains(&p.slopes[0]));
        assert!((K::SLOPE_MIN..=K::SLOPE_MAX).contains(&p.slopes[2]));

        // Curvatures are identically zero in this branch (straight lines).
        assert_eq!(p.curvatures, [0.0; 3]);

        // Pivots from the closed form (density 1.0 drops the last term).
        for ch in 0..3 {
            let expected =
                ANCHOR - v_star() / p.slopes[ch] + (1.0 - DENSITY) * K::DENSITY_MULTIPLIER;
            assert!((p.pivots[ch] - expected).abs() < 1e-12, "ch {ch}");
        }
    }

    /// Extreme refs engage the cast clamp: the clamp applies AFTER the strength scaling, so
    /// the effective offset is exactly ±K.castRemovalMaxOffset (not strength · offset).
    /// Back-solved from the returned slope: cast = base·numer/slope − anchor + rG.
    #[test]
    fn extreme_refs_clamp_at_cast_removal_max_offset() {
        // Red ref 0.5, strength 1.0: raw cast = +0.4 → +0.1. Blue ref 1.4: −0.5 → −0.1.
        let refs = [0.5, 0.9, 1.4];
        let p = params(refs, 1.0);
        let base = base_slope();
        let numer = ANCHOR - refs[1];

        // Forward check with the clamped literal (both inside [2, 10], no slope clamp).
        assert!(
            (p.slopes[0] - base * numer / (ANCHOR - (refs[1] - K::CAST_REMOVAL_MAX_OFFSET))).abs()
                < 1e-12
        );
        assert!(
            (p.slopes[2] - base * numer / (ANCHOR - (refs[1] + K::CAST_REMOVAL_MAX_OFFSET))).abs()
                < 1e-12
        );

        // Back-solve the effective cast from each returned slope; it must be EXACTLY the
        // clamp constant.
        let cast_r = base * numer / p.slopes[0] - ANCHOR + refs[1];
        let cast_b = base * numer / p.slopes[2] - ANCHOR + refs[1];
        assert!((cast_r - K::CAST_REMOVAL_MAX_OFFSET).abs() < 1e-12);
        assert!((cast_b + K::CAST_REMOVAL_MAX_OFFSET).abs() < 1e-12);

        assert_eq!(p.slopes[1], base);
        assert_eq!(p.curvatures, [0.0; 3]);
    }

    /// The tie scales with strength below the clamp: half strength moves the red slope
    /// exactly to the half-cast tie.
    #[test]
    fn strength_scales_the_cast_before_the_clamp() {
        let refs = [0.98, 0.90, 0.90];
        let base = base_slope();
        let numer = ANCHOR - refs[1];
        for strength in [0.25, 0.5, 1.0] {
            let p = params(refs, strength);
            // cast = strength · (0.90 − 0.98) = −0.08·strength — never clamped.
            let cast = strength * (refs[1] - refs[0]);
            assert!(cast.abs() < K::CAST_REMOVAL_MAX_OFFSET);
            let expected = base * numer / (ANCHOR - (refs[1] - cast));
            assert!(
                (p.slopes[0] - expected).abs() < 1e-12,
                "strength {strength}"
            );
            // Untied channels stay at base (blue's cast is exactly 0, so its tie degenerates
            // to base·numer/numer — fp round-trip, not bitwise).
            assert_eq!(p.slopes[1], base);
            assert!((p.slopes[2] - base).abs() < 1e-12);
        }
    }
}

// ───────────────────────── AutoHelpersTests (curve + derive half) ─────────────────────────

mod auto_helpers {
    use super::*;

    /// The reach only ever raises: a slope already past the required line is returned
    /// untouched, a flat one is lifted onto it, and the lift stays under the global clamp.
    #[test]
    fn reach_never_lowers_and_clamps() {
        let raised = cl::shadow_reach_slope(2.0, 0.46, 0.95, 0.0);
        assert!(raised > 2.0);
        assert_eq!(
            cl::shadow_reach_slope(raised + 1.0, 0.46, 0.95, 0.0),
            raised + 1.0
        );
        // A hair past the anchor needs an absurd slope — the clamp holds it.
        assert_eq!(cl::shadow_reach_slope(2.0, 0.46, 0.4601, 0.0), K::SLOPE_MAX);
        // Degenerate / inverted spans change nothing.
        assert_eq!(cl::shadow_reach_slope(2.0, 0.46, 0.46, 0.0), 2.0);
        assert_eq!(cl::shadow_reach_slope(2.0, 0.6, 0.5, 0.0), 2.0);
    }

    /// The raised slope really does print the dark tail at the reach density: the line through
    /// the anchor's v* evaluated at the tail lands on reference_linear_value(target: reach).
    #[test]
    fn reach_lands_the_tail() {
        let (anchor, tail) = (0.46, 0.95);
        let slope = cl::shadow_reach_slope(2.0, anchor, tail, 0.0);
        let v_at_tail = cl::reference_linear_value(0.0, None) + slope * (tail - anchor);
        expect_close(
            v_at_tail,
            cl::reference_linear_value(0.0, Some(K::SHADOW_REACH_DENSITY)),
            1e-9,
            &"tail straight-line value",
        );
    }

    /// The hold never lifts (0 when the tail already prints deep enough), burns when the line
    /// would print it brighter, and caps.
    #[test]
    fn hold_never_lifts_and_caps() {
        assert_eq!(cl::highlight_hold_offset(3.0, 0.22, 0.30, 0.0), 0.0);
        let burn = cl::highlight_hold_offset(3.0, 0.22, 0.05, 0.0);
        assert!(burn > 0.0);
        assert!(burn <= K::HIGHLIGHT_HOLD_MAX);
        assert_eq!(
            cl::highlight_hold_offset(3.0, 0.55, 0.10, 0.0),
            K::HIGHLIGHT_HOLD_MAX
        );
    }

    /// derive_render_params gates the burn on Auto Grade and the measured bright tail, and
    /// routes it through RenderParams.auto_highlight — never a settings field.
    #[test]
    fn derive_gates_the_burn() {
        let mut analysis = flat_analysis();
        analysis.shadow_point = Some(0.9);
        analysis.highlight_point = Some(0.02);
        let mut s = ExposureSettings::default();
        let on = derive_render_params(&s, &analysis);
        assert!(on.auto_highlight > 0.0);
        expect_close(
            on.auto_highlight,
            cl::highlight_hold_offset(on.slopes[1], on.pivots[1], 0.02, on.d_min),
            1e-12,
            &"burn from the rendered green line",
        );

        s.auto_normalize_contrast = false;
        assert_eq!(derive_render_params(&s, &analysis).auto_highlight, 0.0);

        s.auto_normalize_contrast = true;
        analysis.highlight_point = None; // fixture-built analyses: unmeasured → inert
        assert_eq!(derive_render_params(&s, &analysis).auto_highlight, 0.0);
    }
}

// ───────────────────────── LevelsRemapTests (sanitizer + derive half) ─────────────────────────

mod levels {
    use super::*;

    /// The sanitizer: sorts by input, clamps off the endpoints, drops duplicate inputs, forces
    /// outputs monotone, caps at 8, and collapses all-identity sets to empty.
    #[test]
    fn sanitizer_contract() {
        let dirty = [
            [0.9, 0.95],
            [0.001, 0.3], // clamps to (clamp, 0.3)
            [0.5, 0.1],   // output below its left neighbour → raised
            [0.5, 0.6],   // duplicate input → dropped
        ];
        let clean = sanitize_levels(&dirty);
        assert_eq!(clean.len(), 3);
        let xs: Vec<f64> = clean.iter().map(|p| p[0]).collect();
        let mut sorted = xs.clone();
        sorted.sort_by(f64::total_cmp);
        assert_eq!(xs, sorted);
        assert_eq!(clean[0], [LEVELS_CLAMP, 0.3]);
        assert!(clean[1][0] == 0.5 && clean[1][1] == 0.3); // raised to monotone
        assert_eq!(clean[2], [0.9, 0.95]);

        // Cap.
        let many: Vec<[f64; 2]> = (0..12)
            .map(|i| [0.05 + 0.08 * f64::from(i), 0.05 + 0.081 * f64::from(i)])
            .collect();
        assert_eq!(sanitize_levels(&many).len(), LEVELS_MAX_POINTS);

        // Identity-only sets vanish.
        assert!(sanitize_levels(&[[0.3, 0.3], [0.7, 0.7]]).is_empty());
        assert!(sanitize_levels(&[]).is_empty());
    }

    #[test]
    fn derive_sanitizes_per_channel() {
        let analysis = flat_analysis();
        let mut s = ExposureSettings::default();
        s.levels_red = vec![[0.7, 0.6], [0.2, 0.3]]; // unsorted
        s.levels_blue = vec![[0.4, 0.4]]; // identity → empty
        let p = derive_render_params(&s, &analysis);
        assert_eq!(p.levels_points[0], vec![[0.2, 0.3], [0.7, 0.6]]);
        assert!(p.levels_points[1].is_empty());
        assert!(p.levels_points[2].is_empty());
    }
}

// ───────────────────────── AnalysisParityTests.curveParams ─────────────────────────

mod parity {
    use super::*;

    /// The analysis NegPy itself measured on synthetic64, read from the config's
    /// `analysis.json` (identical across configs: same image, same buffer 0.05).
    fn analysis_from_fixture(name: &str) -> ExposureAnalysis {
        let j = fixtures::read_json(&format!("synthetic64/{name}/analysis.json"))
            .expect("analysis.json");
        let bounds = &j["bounds"];
        let meters = &j["meters"];
        let na = meters["neutral_axis_refs"]
            .as_array()
            .expect("neutral_axis_refs");
        let (neutral_mid, neutral_shadow, neutral_highlight, neutral_confidence) =
            if na[0].is_null() {
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

    /// The Swift `settingsFrom(_:)`: the config's exposure_config over NegPy-neutral
    /// SwiftInvert-only defaults (preSaturation 1.0, skinProtection 0).
    fn settings_from(config: &Value) -> ExposureSettings {
        let e = &config["exposure_config"];
        let f = |k: &str| e[k].as_f64().unwrap_or_else(|| panic!("{k}"));
        let b = |k: &str| e[k].as_bool().unwrap_or_else(|| panic!("{k}"));
        let mut s = ExposureSettings::default();
        s.pre_saturation = 1.0;
        s.skin_protection = 0.0;
        s.true_black = b("true_black");
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
        s.paper_dmin = b("paper_dmin");
        // Contrast Mask keys exist only in the contrast_mask fixture's config (NegPy >= 0.52).
        s.contrast_mask = e["contrast_mask"].as_f64().unwrap_or(0.0);
        s.mask_spacer = e["mask_spacer"].as_f64().unwrap_or(4.0);
        s
    }

    fn check(name: &str) {
        let j = fixtures::read_json(&format!("synthetic64/{name}/analysis.json"))
            .expect("analysis.json");
        let cp = &j["curve_params"];
        let params = derive_render_params(&settings_from(&j), &analysis_from_fixture(name));
        let slopes = v3(&cp["slopes"]);
        let pivots = v3(&cp["pivots"]);
        let curvatures = v3(&cp["curvatures"]);
        let cmy = v3(&cp["cmy_offsets"]);
        for ch in 0..3 {
            expect_close(
                params.slopes[ch],
                slopes[ch],
                1e-4,
                &format!("{name} slope {ch}"),
            );
            expect_close(
                params.pivots[ch],
                pivots[ch],
                1e-4,
                &format!("{name} pivot {ch}"),
            );
            expect_close(
                params.curvatures[ch],
                curvatures[ch],
                1e-4,
                &format!("{name} curv {ch}"),
            );
            expect_close(
                params.cmy_offsets[ch],
                cmy[ch],
                1e-4,
                &format!("{name} cmy {ch}"),
            );
        }
        expect_close(
            params.toe_eff,
            cp["toe_eff"].as_f64().unwrap(),
            1e-4,
            &format!("{name} toe_eff"),
        );
        expect_close(
            params.shoulder_eff,
            cp["shoulder_eff"].as_f64().unwrap(),
            1e-4,
            &format!("{name} shoulder_eff"),
        );
        expect_close(
            params.v_star,
            cp["v_star"].as_f64().unwrap(),
            1e-6,
            &format!("{name} v_star"),
        );
        expect_close(
            params.d_min,
            cp["d_min"].as_f64().unwrap(),
            1e-12,
            &format!("{name} d_min"),
        );
        // Highlight Hold's burn (8532dd92) — expo_dark drives it to the cap.
        expect_close(
            params.auto_highlight,
            cp["auto_highlight"].as_f64().unwrap(),
            1e-4,
            &format!("{name} auto_highlight"),
        );
        // The dump also records the effective cast strength (confidence × slider).
        expect_close(
            cl::effective_cast_strength(
                settings_from(&j).cast_removal_strength,
                analysis_from_fixture(name).neutral_confidence,
            ),
            cp["cast_strength"].as_f64().unwrap(),
            1e-6,
            &format!("{name} cast_strength"),
        );
        // Final bounds = base bounds at zero offsets (every parity config).
        let fb = &j["bounds"];
        assert_eq!(params.final_bounds.floors, v3(&fb["final_floors"]));
        assert_eq!(params.final_bounds.ceils, v3(&fb["final_ceils"]));
        // The manifest's `paper_dmin`/`true_black` reach the params unchanged.
        assert_eq!(
            params.true_black,
            j["exposure_config"]["true_black"].as_bool().unwrap()
        );
    }

    #[test]
    fn curve_params_default() {
        check("default");
    }

    #[test]
    fn curve_params_expo_dark() {
        check("expo_dark");
    }

    #[test]
    fn curve_params_expo_cmy() {
        check("expo_cmy");
    }

    /// `meters.norm_density_range` is NegPy's `luminanceDensityRange` of the base bounds.
    #[test]
    fn luminance_density_range_matches_fixture() {
        let j = fixtures::read_json("synthetic64/default/analysis.json").unwrap();
        let a = analysis_from_fixture("default");
        expect_close(
            a.base_bounds.luminance_density_range(),
            j["meters"]["norm_density_range"].as_f64().unwrap(),
            1e-9,
            &"norm_density_range",
        );
    }
}

// ───────────────────────── derive-level properties (SwiftInvert-only controls) ─────────────────────────

mod derive_props {
    use super::*;

    fn green_density(p: &derive::RenderParams, x: f64) -> f64 {
        p.slopes[1] * (x - p.pivots[1]) + p.curvatures[1] * x * x
    }

    /// Overall contrast folds exactly: slopes and curvatures scale by (1+k) and the reference
    /// tone's straight-line density is invariant (the pivot shift compensates).
    #[test]
    fn overall_contrast_fold_keeps_the_anchor() {
        let analysis = flat_analysis();
        let base = derive_render_params(&ExposureSettings::default(), &analysis);
        // The anchor prints at v* on the base line.
        expect_close(
            green_density(&base, analysis.anchor),
            base.v_star,
            1e-12,
            &"anchor at v*",
        );
        for slider in [-1.0, -0.4, 0.5, 2.0] {
            let mut s = ExposureSettings::default();
            s.overall_contrast = slider;
            let p = derive_render_params(&s, &analysis);
            let k = slider * K::OVERALL_CONTRAST_MAX;
            for ch in 0..3 {
                expect_close(
                    p.slopes[ch],
                    base.slopes[ch] * (1.0 + k),
                    1e-12,
                    &format!("slope {ch}"),
                );
                expect_close(
                    p.curvatures[ch],
                    base.curvatures[ch] * (1.0 + k),
                    1e-12,
                    &format!("curv {ch}"),
                );
                // v(x) = v* + (1+k)(v_base(x) − v*): invariant at the anchor.
                let x = analysis.anchor;
                let v = p.slopes[ch] * (x - p.pivots[ch]) + p.curvatures[ch] * x * x;
                expect_close(v, base.v_star, 1e-12, &format!("anchor invariant ch {ch}"));
            }
        }
    }

    /// Exposure stops ride the EV domain: one stop = −log10(2)/range per channel, + = brighter
    /// (a negative pre-curve offset lowers print density), composing with filtration.
    #[test]
    fn exposure_stops_and_filtration_compose() {
        let analysis = flat_analysis();
        let range = 1.5; // ceils − floors of flat_analysis()
        let mut s = ExposureSettings::default();
        s.exposure_stops = 1.0;
        let p = derive_render_params(&s, &analysis);
        for ch in 0..3 {
            expect_close(
                p.cmy_offsets[ch],
                -K::LOG10_TWO / range,
                1e-12,
                &format!("stop {ch}"),
            );
        }
        s.wb_yellow = 0.5;
        s.temp = 0.2;
        s.tint = -0.1;
        let p = derive_render_params(&s, &analysis);
        let yellow = (0.5 + 0.2) * K::CMY_MAX_DENSITY / range;
        let magenta = (-0.1 + 0.2 * derive::PLANCKIAN_RATIO) * K::CMY_MAX_DENSITY / range;
        expect_close(p.cmy_offsets[0], -K::LOG10_TWO / range, 1e-12, &"cyan");
        expect_close(
            p.cmy_offsets[1],
            magenta - K::LOG10_TWO / range,
            1e-12,
            &"magenta",
        );
        expect_close(
            p.cmy_offsets[2],
            yellow - K::LOG10_TWO / range,
            1e-12,
            &"yellow",
        );
    }

    /// Passthroughs and clamps: shadowContrast's negative side lands −3 on the monotone floor,
    /// hueTrim is clamped ±30° and converted to radians, skin/print/damping clamp, band CMY ×0.2,
    /// the mask scale gates at gamma 0, auto-exposure off falls back to the assumed anchor.
    #[test]
    fn passthroughs_and_clamps() {
        let analysis = flat_analysis();
        let mut s = ExposureSettings::default();
        s.shadow_contrast = -3.0;
        s.hue_trim = 45.0;
        s.skin_protection = 1.7;
        s.print_saturation = 9.0;
        s.separation_damping = -1.0;
        s.color_mids = [0.5, -1.0, 0.25];
        s.contrast_mask = 0.9;
        s.paper_dmin = true;
        s.auto_exposure = false;
        let p = derive_render_params(&s, &analysis);
        // Params carry −3 · |floor|/(3·max) = −1.6; the kernel's ×SHADOW_CONTRAST_MAX then
        // lands the effective gain exactly on the −0.8 monotone floor.
        expect_close(
            p.shadow_contrast,
            -3.0 * K::SHADOW_CONTRAST_NEG_FLOOR.abs() / (3.0 * K::SHADOW_CONTRAST_MAX),
            1e-12,
            &"shadow contrast remap",
        );
        expect_close(
            p.shadow_contrast * K::SHADOW_CONTRAST_MAX,
            K::SHADOW_CONTRAST_NEG_FLOOR,
            1e-12,
            &"shadow contrast floor",
        );
        expect_close(p.hue_trim, 30.0_f64.to_radians(), 1e-15, &"hue trim");
        // HueTrimTests.deriveConvertsDegreesToRadiansAndClamps: in-range values pass through
        // (both signs), out-of-range clamp to ±30°.
        let trim = |deg: f64| {
            let mut s2 = s.clone();
            s2.hue_trim = deg;
            derive_render_params(&s2, &analysis).hue_trim
        };
        expect_close(trim(30.0), 30.0_f64.to_radians(), 1e-15, &"trim 30");
        expect_close(trim(-12.5), (-12.5_f64).to_radians(), 1e-15, &"trim -12.5");
        expect_close(
            trim(400.0),
            30.0_f64.to_radians(),
            1e-15,
            &"trim 400 clamps",
        );
        expect_close(
            trim(-400.0),
            (-30.0_f64).to_radians(),
            1e-15,
            &"trim -400 clamps",
        );
        assert_eq!(p.skin_protection, 1.0);
        assert_eq!(p.print_saturation, K::PRINT_SATURATION_MAX);
        assert_eq!(p.separation_damping, 0.0);
        for ch in 0..3 {
            expect_close(
                p.mid_cmy[ch],
                s.color_mids[ch] * K::CMY_MAX_DENSITY,
                1e-15,
                &"mid cmy",
            );
        }
        assert_eq!(p.d_min, K::D_MIN);
        expect_close(
            p.v_star,
            cl::reference_linear_value(K::D_MIN, None),
            1e-15,
            &"v* with dMin",
        );
        // Mask scale: −gamma(clamped 0.5)·lumRange/range per channel, lumRange from the BASE bounds.
        let lum = analysis.base_bounds.luminance_density_range();
        for ch in 0..3 {
            expect_close(
                p.mask_val_scale[ch],
                -0.5 * lum / 1.5,
                1e-12,
                &format!("mask {ch}"),
            );
        }
        // Auto-exposure off: the pivot solves against K::ASSUMED_ANCHOR.
        expect_close(
            p.pivots[1],
            cl::compute_pivot(p.slopes[1], 1.0, K::D_MIN, None),
            1e-12,
            &"assumed anchor pivot",
        );

        let p0 = derive_render_params(&ExposureSettings::default(), &analysis);
        assert_eq!(p0.mask_val_scale, [0.0; 3]);
        assert_eq!(p0.shadow_contrast, 0.0);
        assert_eq!(p0.hue_trim, 0.0);
        assert_eq!(p0.band_saturations, [1.0; 4]);
        assert_eq!(p0.band_hues, [0.0; 4]);
        assert!(p0.true_black);
        let mut pos = ExposureSettings::default();
        pos.shadow_contrast = 2.0;
        assert_eq!(derive_render_params(&pos, &analysis).shadow_contrast, 2.0);
    }

    /// White/black-point offsets fold into final_bounds only: the base bounds (and the
    /// grade's lumRange) are untouched, and the normalized refs move with the final bounds.
    #[test]
    fn offsets_fold_into_final_bounds_only() {
        let analysis = flat_analysis();
        let mut s = ExposureSettings::default();
        s.white_point_offset = 0.1;
        s.black_point_offset = -0.05;
        let p = derive_render_params(&s, &analysis);
        assert_eq!(p.final_bounds.floors, [-1.9; 3]);
        assert_eq!(p.final_bounds.ceils, [-0.55; 3]);
        let p0 = derive_render_params(&ExposureSettings::default(), &analysis);
        // Same lumRange → same base slope (no neutral axis, flat refs: the shadow-ref tie
        // degenerates to base·numer/numer — an fp round trip), so the slopes don't move
        // with the trims.
        for ch in 0..3 {
            expect_close(p.slopes[ch], p0.slopes[ch], 1e-12, &format!("slope {ch}"));
        }
    }
}
