//! Ports of the analysis-stage tests: `AnalysisParityTests` (bounds/meters + the curve-param
//! chain from a live `prepare`), `GridParityTests.boundsMetersAndParams`,
//! `AnalysisEquivalenceTests`, `AuditAnalysisEdgeTests`, the textured-gate cases of
//! `AutoHelpersTests`, the whole-`analyze` cases of `PreprocessRectTests`,
//! `AuditRectAndGeometryTests.prepareWithOffFrameCropRectDoesNotTrap` and the prepare/finalize
//! half of `ImagePipelineSeamTests`.
//!
//! Left to other owners: `AutoHelpersTests`' Shadow Reach / Highlight Hold / derive cases
//! (`tests/curve.rs`), its `burnDarkensHighlightsOnly` and all of `DensityChromaTests`'
//! print-curve cases (the CPU reference chain, `tests/chain.rs` / `tests/curve.rs`), and the
//! sidecar round-trips (`tests/sidecar.rs`).
//!
//! Tolerances are the Swift ones: 1e-4 on bounds/meters/neutral refs, 2.5e-4 on the
//! synthetic_grid slopes/pivots/curvatures (Accelerate-vs-libm ulp amplification through the
//! percentile → grade-range → slope chain).

use std::borrow::Cow;
use std::time::Instant;

use negkit::bounds::{self, LogNegativeBounds, SortedChannels};
use negkit::constants::K;
use negkit::derive::derive_render_params;
use negkit::kernel::{self, DEFAULT_ANALYSIS_BUFFER, ExposureAnalysis};
use negkit::rect::NormalizedRect;
use negkit::{ExposureSettings, RgbImage, V3, fixtures, meters, prefilter, stats};
use serde_json::Value;

// MARK: - helpers

fn expect_close(got: f64, expected: f64, accuracy: f64, label: &dyn std::fmt::Display) {
    assert!(
        (got - expected).abs() <= accuracy,
        "{label}: got {got}, expected {expected} ± {accuracy}"
    );
}

fn v3(v: &Value) -> V3 {
    let a = v.as_array().expect("triple");
    [
        a[0].as_f64().expect("f64"),
        a[1].as_f64().expect("f64"),
        a[2].as_f64().expect("f64"),
    ]
}

fn synthetic64_input() -> RgbImage {
    fixtures::read_rgb("synthetic64/input.bin", 64, 64).expect("fixture")
}

fn synthetic_grid_input() -> RgbImage {
    fixtures::synthetic_grid_input().expect("generator")
}

/// `ExposureKernel.analyze(linearImage:analysisBuffer: 0.05)` on synthetic64 (`Synthetic64.analysis`).
fn synthetic64_analysis() -> ExposureAnalysis {
    kernel::analyze(&synthetic64_input(), None, None, 0.05).expect("analysis")
}

fn filled(width: usize, height: usize, f: impl Fn(usize, usize, usize) -> f32) -> RgbImage {
    RgbImage::new_with(width, height, |buf| {
        for y in 0..height {
            for x in 0..width {
                for c in 0..3 {
                    buf[(y * width + x) * 3 + c] = f(y, x, c);
                }
            }
        }
    })
    .expect("non-empty")
}

/// `img[y, x, c]` in the Swift subscript order.
fn px(img: &RgbImage, y: usize, x: usize, c: usize) -> f32 {
    img.pixel(x, y).expect("in range")[c]
}

/// The Swift `settingsFrom(_:)`: the config's exposure_config over NegPy-neutral defaults.
fn settings_from(config: &Value) -> ExposureSettings {
    let e = &config["exposure_config"];
    let f = |k: &str| e[k].as_f64().unwrap_or_else(|| panic!("{k}"));
    let b = |k: &str| e[k].as_bool().unwrap_or_else(|| panic!("{k}"));
    ExposureSettings {
        // NegPy fixtures predate pre-saturation (a SwiftInvert-only default of 1.15); parity
        // requires the neutral value.
        pre_saturation: 1.0,
        skin_protection: 0.0,
        true_black: b("true_black"),
        density: f("density"),
        grade: f("grade"),
        wb_cyan: f("wb_cyan"),
        wb_magenta: f("wb_magenta"),
        wb_yellow: f("wb_yellow"),
        auto_exposure: b("auto_exposure"),
        auto_normalize_contrast: b("auto_normalize_contrast"),
        cast_removal_strength: f("cast_removal_strength"),
        toe: f("toe"),
        toe_width: f("toe_width"),
        shoulder: f("shoulder"),
        shoulder_width: f("shoulder_width"),
        paper_dmin: b("paper_dmin"),
        // Contrast Mask keys exist only in the contrast_mask fixture's config (NegPy >= 0.52).
        contrast_mask: e["contrast_mask"].as_f64().unwrap_or(0.0),
        mask_spacer: e["mask_spacer"].as_f64().unwrap_or(4.0),
        ..ExposureSettings::default()
    }
}

/// `AnalysisParityTests.boundsAndMeters`' key-by-key comparison of a live analysis against a
/// fixture's `analysis.json` at 1e-4 (bounds, anchor, textural range, shadow/highlight points,
/// shadow refs, neutral refs + confidence, nil-ness included).
fn expect_analysis_matches_fixture(analysis: &ExposureAnalysis, j: &Value, name: &str) {
    let bounds = &j["bounds"];
    let meters = &j["meters"];
    let base_floors = v3(&bounds["base_floors"]);
    let base_ceils = v3(&bounds["base_ceils"]);
    for ch in 0..3 {
        expect_close(
            analysis.base_bounds.floors[ch],
            base_floors[ch],
            1e-4,
            &format!("{name} floor {ch}"),
        );
        expect_close(
            analysis.base_bounds.ceils[ch],
            base_ceils[ch],
            1e-4,
            &format!("{name} ceil {ch}"),
        );
    }
    expect_close(
        analysis.anchor,
        meters["metered_anchor"].as_f64().unwrap(),
        1e-4,
        &format!("{name} anchor"),
    );
    expect_close(
        analysis.textural_range,
        meters["textural_range"].as_f64().unwrap(),
        1e-4,
        &format!("{name} textural"),
    );
    expect_close(
        analysis.shadow_point.expect("shadow_point"),
        meters["shadow_point"].as_f64().unwrap(),
        1e-4,
        &format!("{name} shadow_point"),
    );
    expect_close(
        analysis.highlight_point.expect("highlight_point"),
        meters["highlight_point"].as_f64().unwrap(),
        1e-4,
        &format!("{name} highlight_point"),
    );
    let shadow_refs = v3(&meters["shadow_log_refs"]);
    for (ch, want) in shadow_refs.iter().enumerate() {
        expect_close(
            analysis.shadow_refs[ch],
            *want,
            1e-4,
            &format!("{name} shadow ref {ch}"),
        );
    }
    let na = meters["neutral_axis_refs"]
        .as_array()
        .expect("neutral_axis_refs");
    if na[0].is_null() {
        assert!(
            analysis.neutral_mid.is_none(),
            "{name}: expected no neutral axis"
        );
        return;
    }
    let mid = v3(&na[0]);
    let shadow = v3(&na[1]);
    let confidence = na[3].as_f64().expect("confidence");
    let got_mid = analysis.neutral_mid.expect("neutral mid");
    let got_shadow = analysis.neutral_shadow.expect("neutral shadow");
    for ch in 0..3 {
        expect_close(
            got_mid[ch],
            mid[ch],
            1e-4,
            &format!("{name} neutral mid {ch}"),
        );
        expect_close(
            got_shadow[ch],
            shadow[ch],
            1e-4,
            &format!("{name} neutral shadow {ch}"),
        );
    }
    if na[2].is_null() {
        assert!(
            analysis.neutral_highlight.is_none(),
            "{name}: expected no highlight ref, got {:?}",
            analysis.neutral_highlight
        );
    } else {
        let hl = v3(&na[2]);
        let got_hl = analysis.neutral_highlight.expect("neutral highlight");
        for ch in 0..3 {
            expect_close(
                got_hl[ch],
                hl[ch],
                1e-4,
                &format!("{name} neutral highlight {ch}"),
            );
        }
    }
    expect_close(
        analysis.neutral_confidence.expect("confidence"),
        confidence,
        1e-4,
        &format!("{name} confidence"),
    );
}

// MARK: - AnalysisParityTests (synthetic64)

/// `boundsAndMeters`: the analysis is settings-independent, so every config's `analysis.json`
/// carries the same bounds/meters — check all three against one live analysis.
#[test]
fn bounds_and_meters() {
    let analysis = synthetic64_analysis();
    for name in ["default", "expo_dark", "expo_cmy"] {
        let j = fixtures::read_json(&format!("synthetic64/{name}/analysis.json")).expect("json");
        expect_analysis_matches_fixture(&analysis, &j, name);
    }
}

/// `curveParams`, end to end from THIS crate's `prepare`/`finalize` (agent C pins derive from
/// the fixture's analysis; this proves the live chain lands on the same curve).
#[test]
fn curve_params_from_live_analysis() {
    let analysis = synthetic64_analysis();
    for name in ["default", "expo_dark", "expo_cmy"] {
        let j = fixtures::read_json(&format!("synthetic64/{name}/analysis.json")).expect("json");
        let cp = &j["curve_params"];
        let params = derive_render_params(&settings_from(&j), &analysis);
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
        // Highlight Hold's burn (8532dd92) — expo_dark drives it to the cap.
        expect_close(
            params.auto_highlight,
            cp["auto_highlight"].as_f64().unwrap(),
            1e-4,
            &format!("{name} auto_highlight"),
        );
    }
}

// MARK: - GridParityTests.boundsMetersAndParams (synthetic_grid)

/// Bounds, every meter and the neutral axis at 1e-4 (the Swift test gates floors/ceils/
/// anchor/textural; the remaining keys pass at the same tolerance and are gated too), then
/// the curve params at 2.5e-4 with stock settings. The full quadratic cast-removal path must
/// engage (nonzero curvatures).
#[test]
fn synthetic_grid_bounds_meters_and_params() {
    let j = fixtures::read_json("synthetic_grid/default/analysis.json").expect("json");
    let analysis = kernel::analyze(&synthetic_grid_input(), None, None, 0.05).expect("analysis");
    expect_analysis_matches_fixture(&analysis, &j, "synthetic_grid");

    let cp = &j["curve_params"];
    let curvatures = v3(&cp["curvatures"]);
    assert!(
        curvatures[0] != 0.0 || curvatures[2] != 0.0,
        "fixture should exercise the quadratic path"
    );
    let params = derive_render_params(&ExposureSettings::default(), &analysis);
    let slopes = v3(&cp["slopes"]);
    let pivots = v3(&cp["pivots"]);
    for ch in 0..3 {
        expect_close(
            params.slopes[ch],
            slopes[ch],
            2.5e-4,
            &format!("slope {ch}"),
        );
        expect_close(
            params.pivots[ch],
            pivots[ch],
            2.5e-4,
            &format!("pivot {ch}"),
        );
        expect_close(
            params.curvatures[ch],
            curvatures[ch],
            2.5e-4,
            &format!("curv {ch}"),
        );
    }
}

// MARK: - AnalysisEquivalenceTests

/// Deterministic pseudo-random log-density grid (the shape the meters see: negative, finite,
/// heavily duplicated after the block median).
fn lcg_grid(width: usize, height: usize, seed: u64) -> RgbImage {
    let mut s = seed;
    RgbImage::new_with(width, height, |dst| {
        for v in dst.iter_mut() {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            *v = -((s >> 40) as f32) / 16_777_216.0 * 3.0;
        }
    })
    .expect("non-empty")
}

/// Linear-domain frame in (0, 1] — what `prefilter_log_grid` actually receives.
fn lcg_linear(width: usize, height: usize, seed: u64) -> RgbImage {
    let mut s = seed;
    RgbImage::new_with(width, height, |dst| {
        for v in dst.iter_mut() {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            // Spans below the 1e-6 clip and up to 1.0 so both clip branches fire.
            *v = (((s >> 40) as f32) / 16_777_216.0).max(1e-9);
        }
    })
    .expect("non-empty")
}

fn serial_sorted_channels(g: &RgbImage) -> SortedChannels {
    let lane = |c: usize| {
        let mut v: Vec<f32> = g.data.chunks_exact(3).map(|p| p[c]).collect();
        v.sort_by(f32::total_cmp);
        v
    };
    [lane(0), lane(1), lane(2)]
}

#[test]
fn parallel_channel_sort_matches_serial() {
    // Sizes on both sides of the radix sort's 512-element fallback.
    for (w, h) in [(1, 1), (8, 8), (23, 17), (615, 410)] {
        let g = lcg_grid(w, h, (w * 31 + h) as u64);
        assert_eq!(
            bounds::sorted_channels(&g),
            serial_sorted_channels(&g),
            "{w}x{h}"
        );
    }
}

/// A data race between the three slots would most likely show up as an occasional wrong/empty
/// channel rather than a consistent one, so repeat.
#[test]
fn parallel_channel_sort_is_deterministic() {
    let g = lcg_grid(615, 410, 4242);
    let reference = bounds::sorted_channels(&g);
    assert_eq!(reference.len(), 3);
    assert!(reference.iter().all(|c| c.len() == g.pixel_count()));
    for run in 0..25 {
        assert_eq!(bounds::sorted_channels(&g), reference, "run {run}");
    }
}

/// The percentile consumers must agree too — this is what actually reaches the bounds and the
/// shadow refs.
#[test]
fn channel_percentiles_survive_the_parallel_sort() {
    let g = lcg_grid(615, 410, 4242);
    let parallel = bounds::sorted_channels(&g);
    let serial = serial_sorted_channels(&g);
    for c in 0..3 {
        for q in [0.01, 1.0, 50.0, 90.0, 98.0, 100.0] {
            assert_eq!(
                stats::percentile_of_sorted(&parallel[c], q),
                stats::percentile_of_sorted(&serial[c], q),
                "channel {c} at q={q}"
            );
        }
    }
}

/// NegPy's order is log10 → crop → block median; ours is crop → log10 → block median. log10
/// is elementwise, so the pixels that survive the crop hold identical values either way —
/// this pins that the divergence is purely about how much work is skipped, never the result.
#[test]
fn crop_before_log_matches_upstream_ordering() {
    for buffer in [0.05, 0.10, 0.3] {
        for (w, h) in [(1536, 1025), (200, 130), (37, 29)] {
            let img = lcg_linear(w, h, (w + h) as u64);
            let ours = prefilter::prefilter_log_grid(&img, buffer);
            let upstream_order = prefilter::block_median_grid(
                prefilter::analysis_crop(&prefilter::log_image(&img), buffer).into_owned(),
            );
            assert_eq!(
                (ours.width, ours.height),
                (upstream_order.width, upstream_order.height)
            );
            assert!(ours.data == upstream_order.data, "{w}x{h} buffer {buffer}");
        }
    }
}

/// buffer = 0 skips the crop entirely — the two orderings collapse to the same call sequence,
/// but the guard should still be exercised.
#[test]
fn zero_buffer_skips_the_crop_in_both_orderings() {
    let img = lcg_linear(200, 130, 99);
    let ours = prefilter::prefilter_log_grid(&img, 0.0);
    let upstream_order = prefilter::block_median_grid(prefilter::log_image(&img));
    assert!(ours.data == upstream_order.data);
}

/// `RGBImage(width:height:initializingWith:)`'s contract, on `RgbImage::new_with`: the body
/// fills every lane and the result is indistinguishable from a zero-filled buffer the caller
/// overwrote.
#[test]
fn uninitialized_init_matches_fill_then_overwrite() {
    let (w, h) = (37usize, 29usize);
    let via_init = RgbImage::new_with(w, h, |dst| {
        for (i, v) in dst.iter_mut().enumerate() {
            *v = i as f32 * 0.5 - 3.0;
        }
    })
    .expect("non-empty");
    let mut via_fill = RgbImage::zeros(w, h).expect("non-empty");
    for (i, v) in via_fill.data.iter_mut().enumerate() {
        *v = i as f32 * 0.5 - 3.0;
    }
    assert_eq!((via_init.width, via_init.height), (w, h));
    assert_eq!(via_init.data.len(), w * h * 3);
    assert!(via_init.data == via_fill.data);
}

// MARK: - AuditAnalysisEdgeTests

/// (a) A 6×6 grid holds 36 pixels total — no luma band can ever reach
/// `K::NEUTRAL_AXIS_MIN_PIXELS` (64), so the mid-band guard fails → `None`.
#[test]
fn tiny_grid_yields_nil_neutral_axis() {
    let grid = RgbImage::new_with(6, 6, |buf| {
        for i in 0..36 {
            let t = i as f32 / 35.0;
            buf[i * 3] = -2.0 + 1.5 * t;
            buf[i * 3 + 1] = -2.1 + 1.5 * t;
            buf[i * 3 + 2] = -1.9 + 1.5 * t;
        }
    })
    .expect("non-empty");
    let bounds = LogNegativeBounds {
        floors: [-2.1, -2.2, -2.0],
        ceils: [-0.4, -0.5, -0.3],
    };
    assert!(meters::neutral_axis(&grid, &bounds).is_none());
}

/// (b) Two-tone fully saturated 64×64 (half pure red 0.8/0.05/0.05, half pure blue) → `None`.
///
/// Pinned empirically: `None` here comes from an EMPTY mid band, not the chroma caps. Under
/// the recombined bounds the two tones normalize to lumas ≈ 0.32 (red half) and ≈ 0.11 (blue
/// half) — the mid band (0.40–0.60) captures no pixels at all, so the min-pixel guard fires
/// before any chroma cap is consulted. Either way the contract holds: a frame with no neutral
/// content must not produce a neutral axis.
#[test]
fn two_tone_saturated_grid_yields_nil_neutral_axis() {
    let red = [0.8f32.log10(), 0.05f32.log10(), 0.05f32.log10()];
    let blue = [0.05f32.log10(), 0.05f32.log10(), 0.8f32.log10()];
    let grid = filled(64, 64, |y, _, c| if y < 32 { red[c] } else { blue[c] });
    let bounds = bounds::analyze(&grid).expect("bounds");
    assert!(meters::neutral_axis(&grid, &bounds).is_none());
}

/// A negative percentile_clip switches to margin mode: percentiles are taken at the BASE clip
/// and the (positive) margin is added outward. Expected values recomputed here from `stats`
/// percentiles directly.
#[test]
fn negative_clip_is_base_percentiles_plus_margin() {
    // Three distinct, known sorted channels (floors < ceils).
    let channels: SortedChannels = [
        (0..=100).map(|i| -2.0f32 + i as f32 * 0.01).collect(), // −2.0 … −1.0
        (0..=100).map(|i| -1.8f32 + i as f32 * 0.012).collect(), // −1.8 … −0.6
        (0..=100).map(|i| -2.4f32 + i as f32 * 0.02).collect(), // −2.4 … −0.4
    ];
    let base = K::BASE_LUMA_CLIP; // 0.01
    let margin = 0.5;
    let got = bounds::sample_log_bounds(&channels, -margin, base).expect("bounds");
    for (ch, sorted) in channels.iter().enumerate() {
        let floor = stats::percentile_of_sorted(sorted, base).unwrap();
        let ceil = stats::percentile_of_sorted(sorted, 100.0 - base).unwrap();
        // floors < ceils here, so margin pushes floors down, ceils up.
        assert_eq!(got.floors[ch], floor - margin, "ch {ch}");
        assert_eq!(got.ceils[ch], ceil + margin, "ch {ch}");
    }
}

/// A two-patch saturated grid (90 % one colour, 10 % another) whose luma-extreme band holds no
/// trustworthy neutrals: the same-pixel refs return `None` (verified directly), and `analyze`
/// must then equal the percentile-colour-floor recombination, recomputed here from
/// `sample_log_bounds` + the recombination formula.
#[test]
fn analyze_falls_back_to_percentile_colour_floors() {
    let a = [0.7f32.log10(), 0.2f32.log10(), 0.1f32.log10()];
    let b = [0.05f32.log10(), 0.3f32.log10(), 0.6f32.log10()];
    let n = 64 * 64;
    let grid = filled(
        64,
        64,
        |y, x, c| if y * 64 + x < n / 10 { b[c] } else { a[c] },
    );

    let channels = bounds::sorted_channels(&grid);
    let luma = bounds::sample_log_bounds(&channels, 0.0, K::BASE_LUMA_CLIP).unwrap();
    let colour = bounds::sample_log_bounds(&channels, K::BASE_COLOR_CLIP, 0.0).unwrap();

    // The chroma-gated band refs reject this frame (saturated two-tone content, no neutral
    // subset) — the fallback precondition.
    let sp = bounds::same_pixel_color_floor_refs(
        &grid,
        luma.floors,
        luma.ceils,
        colour.ceils,
        K::BASE_COLOR_CLIP,
    );
    assert!(sp.is_none());

    // Recombination: luma axis sets the mean centre, colour axis the per-channel offset from
    // the median channel.
    let mean_lf = (luma.floors[0] + luma.floors[1] + luma.floors[2]) / 3.0;
    let mean_lc = (luma.ceils[0] + luma.ceils[1] + luma.ceils[2]) / 3.0;
    let med = |v: V3| {
        let mut s = v;
        s.sort_by(f64::total_cmp);
        s[1]
    };
    let med_cf = med(colour.floors);
    let med_cc = med(colour.ceils);
    let expected = LogNegativeBounds {
        floors: [
            mean_lf + (colour.floors[0] - med_cf),
            mean_lf + (colour.floors[1] - med_cf),
            mean_lf + (colour.floors[2] - med_cf),
        ],
        ceils: [
            mean_lc + (colour.ceils[0] - med_cc),
            mean_lc + (colour.ceils[1] - med_cc),
            mean_lc + (colour.ceils[2] - med_cc),
        ],
    };
    assert_eq!(bounds::analyze(&grid), Some(expected));
}

/// 2500×40 column gradient → b = ceil(2500/1024) = 3; grid is (2499/3)×(39/3) = 833×13. Each
/// 3×3 block spans 3 columns × 3 equal rows, so its per-channel median is exactly the middle
/// column's log10 value — recomputed here by sorting the block explicitly.
#[test]
fn block_median_three_wide_blocks() {
    let (w, h) = (2500usize, 40usize);
    let value = |col: usize| 0.1 + 0.8 * col as f32 / (w - 1) as f32;
    let ch_scale = [1.0f32, 0.9, 0.8];
    let img = filled(w, h, |_, x, c| value(x) * ch_scale[c]);
    let grid = prefilter::prefilter_log_grid(&img, 0.0);
    assert_eq!((grid.width, grid.height), (833, 13));

    for (gy, gx) in [(0, 0), (0, 1), (12, 832), (6, 400), (3, 511)] {
        for (c, scale) in ch_scale.iter().enumerate() {
            // Independent expectation: gather the 3×3 block, log10 with the prefilter's clip,
            // sort, take the middle of 9.
            let mut block: Vec<f32> = Vec::new();
            for by in 0..3 {
                for bx in 0..3 {
                    let v = px(&img, gy * 3 + by, gx * 3 + bx, c);
                    block.push(v.clamp(1e-6, 1.0).log10());
                }
            }
            block.sort_by(f32::total_cmp);
            let expected = block[4];
            assert!(
                (px(&grid, gy, gx, c) - expected).abs() < 1e-5,
                "cell ({gy},{gx}) ch {c}"
            );
            // And that middle element IS the block's centre column.
            let centre = (value(gx * 3 + 1) * scale).clamp(1e-6, 1.0).log10();
            assert!((expected - centre).abs() < 1e-6);
        }
    }
}

/// The analysis buffer is clamped at 0.3 per side: a 0.45 request must produce byte-identical
/// output to 0.3 (both through the public chain and the internal crop).
#[test]
fn analysis_buffer_clamps_at_point_three() {
    let img = RgbImage::new_with(200, 120, |buf| {
        for (i, v) in buf.iter_mut().enumerate() {
            *v = 0.05 + 0.9 * (((i as u64 * 2_654_435_761) % 1000) as f32) / 999.0;
        }
    })
    .expect("non-empty");
    let a = prefilter::prefilter_log_grid(&img, 0.45);
    let b = prefilter::prefilter_log_grid(&img, 0.3);
    assert_eq!((a.width, a.height), (b.width, b.height));
    assert!(a.data == b.data);

    let ca = prefilter::analysis_crop(&img, 0.45);
    let cb = prefilter::analysis_crop(&img, 0.3);
    assert_eq!((ca.width, ca.height), (cb.width, cb.height));
    assert!(ca.data == cb.data);
    // 0.3 of each side actually cut: 200 − 2·60 = 80, 120 − 2·36 = 48.
    assert_eq!((cb.width, cb.height), (80, 48));
}

// MARK: - AutoHelpersTests (textured-cell gate)

/// A grid smaller than one sector (2×2 blocks of ACTIVITY_BLOCK cells) can't vote — every
/// cell passes through.
#[test]
fn gate_falls_back_on_tiny_grids() {
    let lum = vec![0.5f32; 8 * 8];
    assert_eq!(meters::textured_cells(&lum, 8, 8).len(), lum.len());
}

/// A flat frame has no active sectors — the min-fraction fallback keeps every cell voting
/// instead of metering an empty set.
#[test]
fn gate_falls_back_on_flat_frames() {
    let lum = vec![0.7f32; 32 * 32];
    let kept = meters::textured_cells(&lum, 32, 32);
    assert_eq!(kept.len(), lum.len());
    assert!(
        matches!(kept, Cow::Borrowed(_)),
        "the fallback must not copy"
    );
}

/// Half the frame carries texture, half is a flat wall: only the textured sectors' cells
/// survive, and they keep their values.
#[test]
fn gate_drops_flat_sectors() {
    let (w, h) = (32usize, 32usize);
    let mut lum = vec![0.2f32; w * h];
    // Left two sectors (x < 16): a checkerboard whose 8×8 block means differ well past the
    // 0.05 gate.
    for y in 0..h {
        for x in 0..16 {
            lum[y * w + x] = if ((x / 8) + (y / 8)) % 2 == 0 {
                0.1
            } else {
                0.9
            };
        }
    }
    let kept = meters::textured_cells(&lum, w, h);
    assert_eq!(kept.len(), 16 * 32);
    assert!(kept.iter().all(|&v| v != 0.2));
}

// MARK: - PreprocessRectTests (whole-analysis equalities)

const RECT: NormalizedRect = NormalizedRect::new(0.25, 0.2, 0.5, 0.55);

#[test]
fn analysis_rect_equals_pre_cropped_with_zero_buffer() {
    let img = synthetic_grid_input();
    let via_rect = kernel::analyze(&img, None, Some(&RECT), DEFAULT_ANALYSIS_BUFFER).unwrap();
    let via_crop = kernel::analyze(&img.cropped(&RECT), None, None, 0.0).unwrap();
    assert_eq!(via_rect, via_crop);
    // And it actually changes the result vs the full frame.
    let full = kernel::analyze(&img, None, None, DEFAULT_ANALYSIS_BUFFER).unwrap();
    assert_ne!(via_rect.base_bounds, full.base_bounds);
}

#[test]
fn crop_rect_scopes_analysis_with_buffer_inside() {
    let img = synthetic_grid_input();
    let via_crop_setting =
        kernel::analyze(&img, Some(&RECT), None, DEFAULT_ANALYSIS_BUFFER).unwrap();
    let via_pre_crop =
        kernel::analyze(&img.cropped(&RECT), None, None, DEFAULT_ANALYSIS_BUFFER).unwrap();
    assert_eq!(via_crop_setting, via_pre_crop);
}

#[test]
fn analysis_rect_wins_over_crop() {
    let img = synthetic_grid_input();
    let other = NormalizedRect::new(0.0, 0.0, 0.4, 0.4);
    let both = kernel::analyze(&img, Some(&other), Some(&RECT), DEFAULT_ANALYSIS_BUFFER).unwrap();
    let rect_only = kernel::analyze(&img, None, Some(&RECT), DEFAULT_ANALYSIS_BUFFER).unwrap();
    assert_eq!(both, rect_only);
}

#[test]
fn degenerate_rect_is_ignored() {
    let img = synthetic64_input();
    let tiny = NormalizedRect::new(0.5, 0.5, 0.001, 0.001);
    let with = kernel::analyze(&img, None, Some(&tiny), DEFAULT_ANALYSIS_BUFFER).unwrap();
    let without = kernel::analyze(&img, None, None, DEFAULT_ANALYSIS_BUFFER).unwrap();
    assert_eq!(with, without);
}

// MARK: - AuditRectAndGeometryTests.prepareWithOffFrameCropRectDoesNotTrap

/// 64×64 ramp image, values 0.1…0.9 (never 0/1, so log stays finite).
fn ramp_image() -> RgbImage {
    let n = 64 * 64;
    filled(64, 64, |y, x, c| {
        let t = (y * 64 + x) as f32 / (n - 1) as f32;
        let v = 0.1 + 0.8 * t;
        v * [1.0, 0.9, 0.8][c]
    })
}

/// The pathological rects the pixel-ROI clamp fix is about.
fn pathological() -> [(NormalizedRect, &'static str); 4] {
    [
        (NormalizedRect::new(-0.5, 0.2, 0.4, 0.6), "left off-frame"),
        (NormalizedRect::new(1.2, 0.2, 0.4, 0.6), "right off-frame"),
        (NormalizedRect::new(-1.0, -1.0, 3.0, 3.0), "surrounds frame"),
        (
            NormalizedRect::new(0.5, 0.2, 0.0005, 0.5),
            "degenerate width",
        ),
    ]
}

#[test]
fn prepare_with_off_frame_crop_rect_does_not_trap() {
    let img = ramp_image();
    for (rect, note) in pathological() {
        // An off-frame/degenerate crop rect must be ignored (no ROI), not crash prepare; the
        // full frame is metered instead.
        let prepared =
            kernel::prepare(&img, Some(&rect), None, DEFAULT_ANALYSIS_BUFFER).expect(note);
        assert!(
            prepared.grid.width > 0 && prepared.grid.height > 0,
            "{note}"
        );
        for ch in 0..3 {
            assert!(prepared.base_bounds.floors[ch].is_finite(), "{note}");
            assert!(prepared.base_bounds.ceils[ch].is_finite(), "{note}");
        }
        assert!(prepared.anchor.is_finite(), "{note}");
    }
    // And the same rects as analysis regions.
    for (rect, note) in pathological() {
        let prepared =
            kernel::prepare(&img, None, Some(&rect), DEFAULT_ANALYSIS_BUFFER).expect(note);
        assert!(
            prepared.grid.width > 0 && prepared.grid.height > 0,
            "{note}"
        );
    }
}

// MARK: - ImagePipelineSeamTests (prepare/finalize — ImageSession's two-tier cache contract)

/// A cached `Prepared` reused later must match a fresh end-to-end analysis: prepare must be
/// deterministic and finalize must not mutate its input.
#[test]
fn reused_prepared_matches_fresh_analysis() {
    let image = synthetic64_input();
    let prepared = kernel::prepare(&image, None, None, 0.05).unwrap();
    let cached = kernel::finalize(&prepared);
    let fresh = kernel::analyze(&image, None, None, 0.05).unwrap();
    assert_eq!(cached, fresh);
    // And finalize is pure: a second call agrees with the first.
    assert_eq!(kernel::finalize(&prepared), cached);
}

/// The 2125a34 semantics: the neutral axis is measured against the PRE-trim base bounds (the
/// film's cast is a source property), so the whole analysis is offset-independent — while the
/// white/black-point handles still reach the RENDER through derive-time final bounds.
#[test]
fn neutral_axis_is_pre_trim_and_offsets_still_reach_the_render() {
    let prepared = kernel::prepare(&synthetic64_input(), None, None, 0.05).unwrap();
    let analysis = kernel::finalize(&prepared);

    // The axis is exactly the base-bounds measurement.
    let direct = meters::neutral_axis(&prepared.grid, &prepared.base_bounds);
    assert_eq!(analysis.neutral_mid, direct.map(|d| d.mid));
    assert_eq!(analysis.neutral_shadow, direct.map(|d| d.shadow));
    assert_eq!(analysis.neutral_confidence, direct.map(|d| d.confidence));

    // Offsets are a render-side fold: final bounds move, analysis doesn't.
    let settings = ExposureSettings {
        white_point_offset: 0.10,
        black_point_offset: -0.08,
        ..ExposureSettings::default()
    };
    let params = derive_render_params(&settings, &analysis);
    assert_eq!(
        params.final_bounds,
        analysis.base_bounds.applying_offsets(0.10, -0.08)
    );
}

// MARK: - Timing (PLAN.md §2 budget: prepare ≤ 20 ms, finalize ≤ 8 ms on a 1536 px proxy)

/// Prints prepare/finalize wall time on two 1536 px proxies (`--nocapture`): the synthetic_grid
/// negative downsampled to the proxy size, and a worst-case full-noise frame (every sector
/// textured, no duplicate values for the sorts). Informational — asserts only that the
/// results are finite, so a slow CI box can't fail the suite.
#[test]
fn prepare_and_finalize_timing() {
    let proxies = [
        (
            "synthetic_grid@1536",
            synthetic_grid_input().downsampled(1536).into_owned(),
        ),
        ("noise 1536x1025", lcg_linear(1536, 1025, 7)),
    ];
    for (name, img) in &proxies {
        // Warm-up (page faults, rayon pool spin-up).
        let _ = kernel::analyze(img, None, None, DEFAULT_ANALYSIS_BUFFER).unwrap();
        let mut prep_ms = Vec::new();
        let mut fin_ms = Vec::new();
        for _ in 0..7 {
            let t0 = Instant::now();
            let prepared = kernel::prepare(img, None, None, DEFAULT_ANALYSIS_BUFFER).unwrap();
            prep_ms.push(t0.elapsed().as_secs_f64() * 1e3);
            let t1 = Instant::now();
            let analysis = kernel::finalize(&prepared);
            fin_ms.push(t1.elapsed().as_secs_f64() * 1e3);
            assert!(analysis.anchor.is_finite());
        }
        prep_ms.sort_by(f64::total_cmp);
        fin_ms.sort_by(f64::total_cmp);
        eprintln!(
            "{name} ({}x{}): prepare min {:.2} ms / median {:.2} ms; finalize min {:.2} ms / median {:.2} ms",
            img.width, img.height, prep_ms[0], prep_ms[3], fin_ms[0], fin_ms[3]
        );
        // Stage breakdown (best of 5 each), to see where a regression lands.
        let best = |f: &dyn Fn()| {
            (0..5)
                .map(|_| {
                    let t = Instant::now();
                    f();
                    t.elapsed().as_secs_f64() * 1e3
                })
                .fold(f64::INFINITY, f64::min)
        };
        let grid = prefilter::prefilter_log_grid(img, DEFAULT_ANALYSIS_BUFFER);
        let channels = bounds::sorted_channels(&grid);
        let base = bounds::analyze_sorted(&grid, &channels, 0.0, K::BASE_COLOR_CLIP).unwrap();
        eprintln!(
            "  prefilter {:.2} | sorted_channels {:.2} | analyze_sorted {:.2} | textured_luma_meters {:.2} | textural_range {:.2} | neutral_axis {:.2} ms  (grid {}x{})",
            best(&|| {
                let _ = prefilter::prefilter_log_grid(img, DEFAULT_ANALYSIS_BUFFER);
            }),
            best(&|| {
                let _ = bounds::sorted_channels(&grid);
            }),
            best(&|| {
                let _ = bounds::analyze_sorted(&grid, &channels, 0.0, K::BASE_COLOR_CLIP);
            }),
            best(&|| {
                let _ = meters::textured_luma_meters(&grid, &base);
            }),
            best(&|| {
                let _ = meters::textural_range(&grid);
            }),
            best(&|| {
                let _ = meters::neutral_axis(&grid, &base);
            }),
            grid.width,
            grid.height
        );
    }
}

/// ColorGradingTests.castStrengthBeyondOneStaysBounded: strength 2 overcorrects harder than
/// 0.5 but the kernel clamps keep the per-channel curve sane (slope range, finite pivots and
/// curvatures). Confidence scaling is always on (NegPy 0.36); both settings share the frame's
/// confidence, so the comparison is deterministic.
#[test]
fn cast_strength_beyond_one_stays_bounded() {
    let analysis = kernel::analyze(&synthetic_grid_input(), None, None, 0.05).expect("analysis");
    let strong = ExposureSettings {
        cast_removal_strength: 2.0,
        ..ExposureSettings::default()
    };
    let mild = ExposureSettings {
        cast_removal_strength: 0.5,
        ..ExposureSettings::default()
    };
    let p_strong = derive_render_params(&strong, &analysis);
    let p_mild = derive_render_params(&mild, &analysis);
    let mut stronger_somewhere = false;
    for ch in [0usize, 2] {
        assert!(
            p_strong.slopes[ch] >= negkit::constants::K::SLOPE_MIN
                && p_strong.slopes[ch] <= negkit::constants::K::SLOPE_MAX,
            "slope {ch} out of range: {}",
            p_strong.slopes[ch]
        );
        assert!(p_strong.pivots[ch].is_finite() && p_strong.curvatures[ch].is_finite());
        if (p_strong.slopes[ch] - p_strong.slopes[1]).abs()
            > (p_mild.slopes[ch] - p_mild.slopes[1]).abs() + 1e-9
        {
            stronger_somewhere = true;
        }
    }
    assert!(
        stronger_somewhere,
        "strength 2 should tilt R/B harder than 0.5"
    );
}
