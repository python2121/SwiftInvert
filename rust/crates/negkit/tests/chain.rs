//! `AnalysisParityTests.fullChain` — stage-boundary parity of the CPU reference chain against
//! NegPy on the synthetic64 fixture (normalized → curve_linear → output, three configs, max
//! abs diff < 1e-4), plus the contracts the Swift suite pins only transitively: the histogram
//! twin, the sRGB flag, the in-place entry points, and a timing print for the 1536×1024 CPU
//! render (`cargo test -p negkit --test chain -- --nocapture`).
//!
//! The analysis comes from each config's `analysis.json` (NegPy's own measurement of the
//! image, identical across configs) rather than `ExposureKernel.analyze` as in Swift, so the
//! gate is independent of the analysis port (agent B); `tests/curve.rs` pins the derive half
//! the same way.

// The Swift tests mutate a default field by field; that style IS the drift-catcher here.
#![allow(clippy::field_reassign_with_default)]

use negkit::constants::WorkingOetf;
use negkit::derive::derive_render_params;
use negkit::kernel::ExposureAnalysis;
use negkit::reference::{self, RenderOptions};
use negkit::{ExposureSettings, LogNegativeBounds, RenderParams, RgbImage, V3, fixtures};
use serde_json::Value;

fn v3(v: &Value) -> V3 {
    let a = v.as_array().unwrap_or_else(|| panic!("not an array: {v}"));
    [
        a[0].as_f64().unwrap(),
        a[1].as_f64().unwrap(),
        a[2].as_f64().unwrap(),
    ]
}

/// The analysis NegPy itself measured on synthetic64, read from the config's `analysis.json`.
fn analysis_from_fixture(name: &str) -> ExposureAnalysis {
    let j =
        fixtures::read_json(&format!("synthetic64/{name}/analysis.json")).expect("analysis.json");
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

/// The Swift `settingsFrom(_:)`: the config's exposure_config over NegPy-neutral
/// SwiftInvert-only defaults (preSaturation 1.0, skinProtection 0 — so the colour-pop stage is
/// identity and `curve_linear.bin` is the bare print curve).
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
    s.contrast_mask = e["contrast_mask"].as_f64().unwrap_or(0.0);
    s.mask_spacer = e["mask_spacer"].as_f64().unwrap_or(4.0);
    s
}

fn synthetic64_input() -> RgbImage {
    fixtures::read_rgb("synthetic64/input.bin", 64, 64).expect("synthetic64/input.bin")
}

fn params_for(name: &str) -> RenderParams {
    let j =
        fixtures::read_json(&format!("synthetic64/{name}/analysis.json")).expect("analysis.json");
    derive_render_params(&settings_from(&j), &analysis_from_fixture(name))
}

/// `expectImageClose`: max abs diff strictly under the tolerance (mean reported).
#[track_caller]
fn expect_image_close(got: &RgbImage, rel: &str, tolerance: f64) {
    let expected = fixtures::read_f32(rel).expect(rel);
    assert_eq!(got.data.len(), expected.len(), "{rel} size");
    let (max, mean) = fixtures::max_mean_abs_diff(&got.data, &expected);
    assert!(
        max < tolerance,
        "{rel}: max diff {max:.3e} (mean {mean:.3e}) ≥ {tolerance:e}"
    );
    println!("{rel}: max {max:.3e} mean {mean:.3e}");
}

// ───────────────────────── AnalysisParityTests.fullChain ─────────────────────────

fn full_chain(name: &str) {
    let input = synthetic64_input();
    let params = params_for(name);
    let normalized = reference::normalize(&input, &params.final_bounds);
    expect_image_close(
        &normalized,
        &format!("synthetic64/{name}/normalized.bin"),
        1e-4,
    );
    let positive = reference::apply_print_curve(&normalized, &params, None);
    expect_image_close(
        &positive,
        &format!("synthetic64/{name}/curve_linear.bin"),
        1e-4,
    );
    let encoded = reference::encode_output(&positive, &[Vec::new(), Vec::new(), Vec::new()]);
    expect_image_close(&encoded, &format!("synthetic64/{name}/output.bin"), 1e-4);

    // The one-shot render is the same three stages (levels empty, no mask, working encode).
    let rendered = reference::render(&input, &params, RenderOptions::default());
    assert_eq!(
        rendered.data, encoded.data,
        "{name}: render() ≠ staged chain"
    );
}

#[test]
fn full_chain_default() {
    full_chain("default");
}

#[test]
fn full_chain_expo_dark() {
    // expo_dark fires Highlight Hold at its 0.5 cap: the burn's kernel term is chain-pinned.
    assert!(params_for("expo_dark").auto_highlight > 0.0);
    full_chain("expo_dark");
}

#[test]
fn full_chain_expo_cmy() {
    full_chain("expo_cmy");
}

/// The Swift `fullChain` uses the LIVE analysis (`Synthetic64.analysis`), not the fixture's
/// JSON: prepare → finalize → derive → render against the three dumped stage boundaries at
/// 1e-4, so a bounds error just inside the analysis gate can't hide behind 1/range in the
/// normalized pixels. (Review 2026-10-15, finding 1.)
fn full_chain_live(name: &str) {
    let input = synthetic64_input();
    let j =
        fixtures::read_json(&format!("synthetic64/{name}/analysis.json")).expect("analysis.json");
    let analysis = negkit::kernel::analyze(&input, None, None, 0.05).expect("live analysis");
    let params = derive_render_params(&settings_from(&j), &analysis);
    let normalized = reference::normalize(&input, &params.final_bounds);
    expect_image_close(
        &normalized,
        &format!("synthetic64/{name}/normalized.bin"),
        1e-4,
    );
    let positive = reference::apply_print_curve(&normalized, &params, None);
    expect_image_close(
        &positive,
        &format!("synthetic64/{name}/curve_linear.bin"),
        1e-4,
    );
    let encoded = reference::encode_output(&positive, &[Vec::new(), Vec::new(), Vec::new()]);
    expect_image_close(&encoded, &format!("synthetic64/{name}/output.bin"), 1e-4);
}

#[test]
fn full_chain_live_default() {
    full_chain_live("default");
}

#[test]
fn full_chain_live_expo_dark() {
    full_chain_live("expo_dark");
}

#[test]
fn full_chain_live_expo_cmy() {
    full_chain_live("expo_cmy");
}

/// The in-place entry points are the allocating ones' exact bytes (the big-buffer path the
/// session renderer takes).
#[test]
fn in_place_matches_allocating() {
    let input = synthetic64_input();
    let params = params_for("default");
    let staged = reference::render(&input, &params, RenderOptions::default());
    let mut buf = input.clone();
    reference::render_in_place(&mut buf, &params, RenderOptions::default());
    assert_eq!(buf.data, staged.data);
}

// ───────────────────────── histogram twin ─────────────────────────

/// Every pixel lands in exactly one bin per channel; a flat frame lands in one bin; the bins
/// are the truncated `clamp(v·255)` of the ENCODED values with luma on the encoded triplet.
#[test]
fn histogram_counts_and_bins() {
    let input = synthetic64_input();
    let params = params_for("default");
    let encoded = reference::render(&input, &params, RenderOptions::default());
    let bins = reference::histogram_256(&encoded);
    let n = encoded.pixel_count() as u32;
    for c in 0..4 {
        let total: u32 = bins[c * 256..(c + 1) * 256].iter().sum();
        assert_eq!(total, n, "channel {c} must count every pixel once");
    }
    // Serial twin of the kernel on the same pixels (independent of the row split).
    let mut twin = [0u32; 1024];
    for px in encoded.data.chunks_exact(3) {
        let luma = px[0] * 0.2126 + px[1] * 0.7152 + px[2] * 0.0722;
        for (c, v) in [px[0], px[1], px[2], luma].into_iter().enumerate() {
            twin[c * 256 + (v * 255.0).clamp(0.0, 255.0) as usize] += 1;
        }
    }
    assert_eq!(bins, twin);

    // A flat encoded mid-grey (0.5 → bin 127) fills exactly one bin everywhere.
    let flat = RgbImage::new_with(7, 5, |b| b.fill(0.5)).unwrap();
    let fb = reference::histogram_256(&flat);
    for c in 0..4 {
        assert_eq!(fb[c * 256 + 127], 35, "channel {c}");
        assert_eq!(fb[c * 256..(c + 1) * 256].iter().sum::<u32>(), 35);
    }
    // Out-of-range values clamp into the end bins rather than vanishing.
    let hot = RgbImage::from_vec(1, 1, vec![1.5, -0.3, 1.0]).unwrap();
    let hb = reference::histogram_256(&hot);
    assert_eq!((hb[255], hb[256], hb[512 + 255]), (1, 1, 1));
}

// ───────────────────────── sRGB flag ─────────────────────────

/// The flags-bit leg on the encode: identical to a straight f64 evaluation of the GLSL
/// (decode Adobe TRC → XYZ → linear sRGB → clamp → piecewise OETF) within f32 noise, applied
/// AFTER the levels remap, and off = the plain working encode byte for byte.
#[test]
fn srgb_flag_matches_cpu_transform() {
    let input = synthetic64_input();
    let params = params_for("default");
    let linear = reference::apply_print_curve(
        &reference::normalize(&input, &params.final_bounds),
        &params,
        None,
    );
    let levels: [Vec<[f64; 2]>; 3] = [vec![[0.3, 0.4]], Vec::new(), vec![[0.6, 0.5]]];

    let working = reference::encode_output_with(&linear, &levels, false);
    assert_eq!(
        working.data,
        reference::encode_output(&linear, &levels).data
    );
    let srgb = reference::encode_output_with(&linear, &levels, true);

    let to_xyz = negkit::lab::TO_XYZ;
    let from_xyz = reference::SRGB_FROM_XYZ;
    let oetf = |v: f64| {
        if v <= 0.0031308 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    };
    let mut max = 0.0f64;
    for (w, s) in working.data.chunks_exact(3).zip(srgb.data.chunks_exact(3)) {
        let lin: Vec<f64> = w
            .iter()
            .map(|&e| WorkingOetf::decode_f64(f64::from(e)))
            .collect();
        let xyz: Vec<f64> = to_xyz
            .iter()
            .map(|r| r[0] * lin[0] + r[1] * lin[1] + r[2] * lin[2])
            .collect();
        for (ch, r) in from_xyz.iter().enumerate() {
            let v = (r[0] * xyz[0] + r[1] * xyz[1] + r[2] * xyz[2]).clamp(0.0, 1.0);
            max = max.max((oetf(v) - f64::from(s[ch])).abs());
        }
    }
    assert!(max < 1e-4, "sRGB leg vs f64 reference: max diff {max:.3e}");

    // Greys stay grey (the D65 matrices compose to the identity on the achromatic axis) and
    // the transform is the kernels' per-pixel map: a mid-grey working value lands on the
    // sRGB encode of the same linear value.
    let grey = RgbImage::from_vec(1, 1, vec![0.2, 0.2, 0.2]).unwrap();
    let g = reference::encode_output_with(&grey, &[Vec::new(), Vec::new(), Vec::new()], true);
    let want = oetf(0.2);
    for ch in 0..3 {
        assert!(
            (f64::from(g.data[ch]) - want).abs() < 2e-4,
            "grey ch{ch}: {} vs {want}",
            g.data[ch]
        );
    }
    // The flag composes with render().
    let via_render = reference::render(
        &input,
        &params,
        RenderOptions {
            srgb_encode: true,
            mask_plane: None,
        },
    );
    let via_stages = reference::encode_output_with(&linear, &params.levels_points, true);
    assert_eq!(via_render.data, via_stages.data);
}

// ───────────────────────── mask hook ─────────────────────────

/// With `mask_val_scale` zero the plane is never sampled; with it set, the sample adds
/// `scale × plane` to the pre-curve value — a constant plane equals the same constant folded
/// into `cmy_offsets`.
#[test]
fn mask_sampler_hook_adds_scaled_plane() {
    let input = synthetic64_input();
    let mut params = params_for("default");
    let normalized = reference::normalize(&input, &params.final_bounds);
    let sampled = std::sync::atomic::AtomicUsize::new(0);
    let plane = |_x: usize, _y: usize, _w: usize, _h: usize| -> f32 {
        sampled.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        0.25
    };
    let off = reference::apply_print_curve(&normalized, &params, Some(&plane));
    assert_eq!(
        sampled.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "mask off: never sampled"
    );
    assert_eq!(
        off.data,
        reference::apply_print_curve(&normalized, &params, None).data
    );

    params.mask_val_scale = [-0.4, -0.3, -0.2];
    let on = reference::apply_print_curve(&normalized, &params, Some(&plane));
    assert_eq!(sampled.load(std::sync::atomic::Ordering::Relaxed), 64 * 64);
    let mut folded = params.clone();
    folded.mask_val_scale = [0.0; 3];
    for ch in 0..3 {
        folded.cmy_offsets[ch] += params.mask_val_scale[ch] * 0.25;
    }
    let via_offsets = reference::apply_print_curve(&normalized, &folded, None);
    let (max, _) = fixtures::max_mean_abs_diff(&on.data, &via_offsets.data);
    assert!(max < 1e-6, "constant plane ≠ folded offset: {max:e}");
    // A nil plane with the mask on renders unmasked (the Swift's `maskPlane == nil`).
    assert_eq!(
        reference::apply_print_curve(&normalized, &params, None).data,
        off.data
    );
}

// ───────────────────────── timing ─────────────────────────

/// Wall time of the full CPU chain on a 1536×1024 frame with the app's default settings
/// (skin protection 0.5 ⇒ the colour-pop stage runs, as it does in practice). Printed, not
/// asserted — the PLAN target is ≤ 300 ms with rayon on the 12-thread box.
#[test]
fn time_render_1536() {
    let (w, h) = (1536usize, 1024usize);
    let image = RgbImage::new_with(w, h, |buf| {
        for (i, lane) in buf.iter_mut().enumerate() {
            let x = (i / 3) % w;
            let y = (i / 3) / w;
            let c = i % 3;
            let base = 0.05 + 0.6 * (x as f32 / w as f32) + 0.3 * (y as f32 / h as f32);
            *lane = (base * [0.9, 0.6, 0.35][c] + 0.03 * ((i * 7919) % 13) as f32 / 13.0)
                .clamp(1e-4, 1.0);
        }
    })
    .unwrap();
    let analysis = analysis_from_fixture("default");
    let app_defaults = derive_render_params(&ExposureSettings::default(), &analysis);
    let mut neutral = ExposureSettings::default();
    neutral.pre_saturation = 1.0;
    neutral.skin_protection = 0.0;
    let parity = derive_render_params(&neutral, &analysis);

    for (label, p) in [
        ("app defaults (colour pop on)", &app_defaults),
        ("parity-neutral", &parity),
    ] {
        // Warm up once (thread pool spin-up, page faults), then time three runs.
        let _ = reference::render(&image, p, RenderOptions::default());
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t0 = std::time::Instant::now();
            let out = reference::render(&image, p, RenderOptions::default());
            let dt = t0.elapsed().as_secs_f64() * 1e3;
            best = best.min(dt);
            assert!(out.data.iter().all(|v| v.is_finite()));
        }
        let t0 = std::time::Instant::now();
        let out = reference::render(&image, p, RenderOptions::default());
        let hist = reference::histogram_256(&out);
        let with_hist = t0.elapsed().as_secs_f64() * 1e3;
        assert_eq!(hist[..256].iter().sum::<u32>(), (w * h) as u32);
        println!(
            "render 1536×1024 [{label}]: best {best:.1} ms; render+histogram {with_hist:.1} ms (rayon {})",
            cfg!(feature = "rayon")
        );
    }
}
