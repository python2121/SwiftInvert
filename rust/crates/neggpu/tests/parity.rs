//! `GPUParityTests` / `VulkanParityTests`: the wgpu chain against the CPU reference
//! (`negkit::reference`, itself pinned to the NegPy fixtures) at NegPy's own gates —
//! mean < 0.01, max < 0.04 — across every control group, plus the tighter gates the Swift
//! suites apply where a half-pixel or a dead branch could hide under the standard one
//! (contrast mask, hue trim, gamut-aware saturation: mean < 1e-3, max < 0.02), the rgba8
//! display path within 1.5/255 of the float path, the histogram contract, the zero-allocation
//! and texture handoffs, and a `--nocapture` timing print.
//!
//! Every GPU test calls `gpu_or_skip!()` and reports the skip reason when no adapter is
//! available (`NEG_GPU=off`, headless CI).

#![allow(clippy::field_reassign_with_default)]

#[macro_use]
mod common;

use std::time::Instant;

use common::*;
use neggpu::RenderOptions;
use negkit::constants::K;
use negkit::derive::derive_render_params;
use negkit::reference::{
    self, MaskSampler, RenderOptions as CpuOptions, histogram_256, srgb_from_encoded,
};
use negkit::{ExposureSettings, LogNegativeBounds, RgbImage, fixtures};

const GATE_MEAN: f64 = 0.01;
const GATE_MAX: f64 = 0.04;
const TIGHT_MEAN: f64 = 1e-3;
const TIGHT_MAX: f64 = 0.02;

fn image() -> RgbImage {
    synthetic_negative(192, 128)
}

/// Render `settings` on both sides over the synthetic negative and assert the standard gate.
fn assert_parity(label: &str, settings: &ExposureSettings) {
    let gpu = gpu_or_skip!();
    let img = image();
    let analysis = analysis_for(&img);
    let params = derive_render_params(settings, &analysis);
    let cpu = reference::render(&img, &params, CpuOptions::default());
    let source = gpu.pipeline.upload(&img).unwrap();
    let out = gpu
        .pipeline
        .render(&source, &params, RenderOptions::default())
        .unwrap();
    expect_gate(label, &out.image, &cpu, GATE_MEAN, GATE_MAX);
}

// ───────────────────────── fixtureParity (synthetic64 × 3 configs) ─────────────────────────

fn fixture_parity(name: &str) {
    let gpu = gpu_or_skip!();
    let input = synthetic64_input();
    let params = fixture_params(name);
    let source = gpu.pipeline.upload(&input).unwrap();
    let out = gpu
        .pipeline
        .render(&source, &params, RenderOptions::default())
        .unwrap();
    // GPU vs the CPU reference chain …
    let cpu = reference::render(&input, &params, CpuOptions::default());
    expect_gate(
        &format!("{name} GPU vs CPU"),
        &out.image,
        &cpu,
        GATE_MEAN,
        GATE_MAX,
    );
    // … and vs NegPy's own output.bin, closing the loop to the fixture.
    let expected = fixtures::read_rgb(&format!("synthetic64/{name}/output.bin"), 64, 64).unwrap();
    expect_gate(
        &format!("{name} GPU vs NegPy"),
        &out.image,
        &expected,
        GATE_MEAN,
        GATE_MAX,
    );
}

#[test]
fn fixture_parity_default() {
    fixture_parity("default");
}

#[test]
fn fixture_parity_expo_dark() {
    fixture_parity("expo_dark");
}

#[test]
fn fixture_parity_expo_cmy() {
    fixture_parity("expo_cmy");
}

// ───────────────────────── control groups (VulkanParityTests) ─────────────────────────

#[test]
fn default_settings_match_cpu() {
    assert_parity(
        "defaults (preSaturation 1.15, skinProtection 0.5)",
        &ExposureSettings::default(),
    );
}

#[test]
fn negpy_neutral_matches_cpu() {
    let mut s = ExposureSettings::default();
    s.pre_saturation = 1.0;
    s.skin_protection = 0.0;
    assert_parity("NegPy-neutral (colour-pop pass fully off)", &s);
}

#[test]
fn exposure_controls_match_cpu() {
    let mut s = ExposureSettings::default();
    s.density = 1.4;
    s.grade = 140.0;
    s.exposure_stops = 0.7;
    s.wb_cyan = 0.3;
    s.wb_magenta = -0.2;
    s.wb_yellow = 0.5;
    s.white_point_offset = 0.05;
    s.black_point_offset = -0.04;
    s.temp = 0.4;
    s.tint = -0.3;
    assert_parity("density/grade/EV/filtration/temp-tint/wp-bp", &s);
}

#[test]
fn tone_and_band_controls_match_cpu() {
    let mut s = ExposureSettings::default();
    s.shadows = 0.8;
    s.shadow_contrast = -1.2;
    s.highlights = 0.5;
    s.highlight_contrast = 1.5;
    s.color_shadows = [0.2, -0.1, 0.15];
    s.color_mids = [-0.15, 0.1, 0.05];
    s.color_highs = [0.1, 0.05, -0.2];
    s.overall_contrast = 0.6;
    assert_parity("tone controls + 3-band CMY + overall contrast", &s);
}

#[test]
fn dark_shadows_and_contrasts_match_cpu() {
    // Shadows + highlight contrast TOGETHER: the parallel form (both masks on the incoming
    // v) is the oracle; a sequential port drifts here.
    let mut s = ExposureSettings::default();
    s.shadows = 0.8;
    s.shadow_contrast = -0.5;
    s.dark_shadows = 1.2;
    s.highlights = -0.9;
    s.highlight_contrast = 0.6;
    assert_parity("dark shadows + both contrasts", &s);
}

#[test]
fn color_pop_controls_match_cpu() {
    let mut s = ExposureSettings::default();
    s.hue_trim = 12.0;
    s.vibrance = 1.5;
    s.saturation = 1.6; // exercises the gamut-aware boost + bisection
    s.skin_protection = 0.8;
    s.red_hue = 0.4;
    s.blue_saturation = 1.3;
    s.green_hue = -0.5;
    assert_parity(
        "hueTrim + mixer bands + vibrance + gamut-aware saturation",
        &s,
    );
}

#[test]
fn mixer_bands_match_cpu() {
    let mut s = ExposureSettings::default();
    s.pre_saturation = 1.3;
    s.red_hue = 0.6;
    s.red_saturation = 0.7;
    s.yellow_hue = -0.4;
    s.yellow_saturation = 1.2;
    s.green_saturation = 1.4;
    s.blue_hue = 0.5;
    s.blue_saturation = 0.8;
    assert_parity("all four mixer bands", &s);
}

#[test]
fn print_controls_match_cpu() {
    let mut s = ExposureSettings::default();
    s.toe = 0.6;
    s.shoulder = -0.4;
    s.print_saturation = 1.3;
    s.separation_damping = 0.6;
    s.true_black = true;
    s.paper_dmin = true;
    assert_parity(
        "toe/shoulder knees + printSaturation/damping + trueBlack + paperDmin",
        &s,
    );
}

#[test]
fn negative_toe_and_true_black_off_match_cpu() {
    let mut s = ExposureSettings::default();
    s.toe = -0.7;
    s.toe_width = 1.8;
    s.shoulder = 0.5;
    s.shoulder_width = 3.2;
    s.true_black = false;
    s.print_saturation = 0.7;
    assert_parity("negative toe + trueBlack off + print desaturation", &s);
}

#[test]
fn metal_combined_controls_match_cpu() {
    // GPUParityTests.toneControlsParityWithCPU: everything at once.
    let mut s = ExposureSettings::default();
    s.exposure_stops = 0.7;
    s.shadows = 0.8;
    s.shadow_contrast = -0.5;
    s.dark_shadows = 1.2;
    s.highlights = -0.9;
    s.highlight_contrast = 0.6;
    s.vibrance = 1.5;
    s.saturation = 1.3;
    s.red_hue = 0.6;
    s.red_saturation = 0.7;
    s.yellow_hue = -0.4;
    s.green_saturation = 1.4;
    s.blue_hue = 0.5;
    s.blue_saturation = 0.8;
    s.pre_saturation = 1.3;
    s.true_black = true;
    s.temp = 0.4;
    s.tint = -0.3;
    s.color_shadows = [0.5, -0.3, 0.2];
    s.color_mids = [-0.2, 0.4, -0.5];
    s.color_highs = [0.3, 0.1, 0.6];
    s.print_saturation = 1.4;
    s.separation_damping = 0.7;
    s.hue_trim = 17.0;
    assert_parity("every control group at once", &s);
}

#[test]
fn levels_remap_matches_cpu() {
    let mut s = ExposureSettings::default();
    s.levels_red = vec![[0.3, 0.22], [0.7, 0.8]];
    s.levels_green = vec![[0.5, 0.42]];
    s.levels_blue = vec![[0.2, 0.3], [0.6, 0.55], [0.85, 0.9]];
    assert_parity("per-channel levels anchors", &s);
}

#[test]
fn autos_off_match_cpu() {
    let mut s = ExposureSettings::default();
    s.auto_exposure = false;
    s.auto_normalize_contrast = false;
    s.cast_removal_strength = 1.4;
    s.grade = 90.0;
    assert_parity("autos off + overcorrecting cast removal", &s);
}

// ───────────────────────── tight-gate cases (GPUParityTests) ─────────────────────────

#[test]
fn hue_trim_tight_parity() {
    let gpu = gpu_or_skip!();
    let input = synthetic64_input();
    let analysis = fixture_analysis("default");
    let mut s = ExposureSettings::default();
    // Deliberately chromatic: a rotation barely moves low-chroma pixels.
    s.pre_saturation = 1.6;
    s.saturation = 1.5;
    let mut off = s.clone();
    off.hue_trim = 0.0;
    s.hue_trim = 24.0;
    let on_params = derive_render_params(&s, &analysis);
    let off_params = derive_render_params(&off, &analysis);
    let source = gpu.pipeline.upload(&input).unwrap();
    let gpu_on = gpu
        .pipeline
        .render(&source, &on_params, RenderOptions::default())
        .unwrap()
        .image;
    let gpu_off = gpu
        .pipeline
        .render(&source, &off_params, RenderOptions::default())
        .unwrap()
        .image;
    let (moved, _) = compare(&gpu_on, &gpu_off);
    assert!(moved > 2e-3, "hue trim barely moved the frame ({moved})");
    let cpu = reference::render(&input, &on_params, CpuOptions::default());
    expect_gate("hue trim GPU/CPU", &gpu_on, &cpu, TIGHT_MEAN, TIGHT_MAX);
}

#[test]
fn saturation_boost_tight_parity() {
    let gpu = gpu_or_skip!();
    let input = synthetic64_input();
    let analysis = fixture_analysis("default");
    let mut s = ExposureSettings::default();
    s.pre_saturation = 1.0;
    s.saturation = 1.6; // strong boost → the bisection path runs
    let params = derive_render_params(&s, &analysis);
    let cpu = reference::render(&input, &params, CpuOptions::default());
    let source = gpu.pipeline.upload(&input).unwrap();
    let out = gpu
        .pipeline
        .render(&source, &params, RenderOptions::default())
        .unwrap();
    expect_gate(
        "gamut-aware saturation GPU/CPU",
        &out.image,
        &cpu,
        TIGHT_MEAN,
        TIGHT_MAX,
    );
}

#[test]
fn skin_rein_moves_output_and_matches_gpu() {
    let gpu = gpu_or_skip!();
    let input = synthetic64_input();
    let analysis = fixture_analysis("default");
    // Warm the print into the skin band so the rein has real work.
    let mut s = ExposureSettings::default();
    s.pre_saturation = 1.0;
    s.temp = 1.5;
    s.vibrance = 1.4;
    s.skin_protection = 1.0;
    let mut off = s.clone();
    off.skin_protection = 0.0;
    let on_params = derive_render_params(&s, &analysis);
    let off_params = derive_render_params(&off, &analysis);
    let cpu_on = reference::render(&input, &on_params, CpuOptions::default());
    let cpu_off = reference::render(&input, &off_params, CpuOptions::default());
    let (bite, _) = compare(&cpu_on, &cpu_off);
    assert!(
        bite > 2e-3,
        "precondition: the rein must move this frame (mean {bite}) — a dead mirror cannot hide behind an unmoved frame"
    );
    let source = gpu.pipeline.upload(&input).unwrap();
    let out = gpu
        .pipeline
        .render(&source, &on_params, RenderOptions::default())
        .unwrap();
    expect_gate(
        "skin rein GPU/CPU",
        &out.image,
        &cpu_on,
        GATE_MEAN,
        GATE_MAX,
    );
}

/// Cast-removal curvature at its clamp over input far outside the frame (u from −4 to 2):
/// the kernel's vertex hold (NegPy d4dc3e49) must match the CPU reference.
#[test]
fn quadratic_vertex_hold_matches_cpu() {
    let gpu = gpu_or_skip!();
    let img = image();
    let analysis = analysis_for(&img);
    let mut params = derive_render_params(&ExposureSettings::default(), &analysis);
    // floors −1, ceils 0: a pixel of 10^(u−1) normalizes to exactly u.
    params.final_bounds = LogNegativeBounds {
        floors: [-1.0; 3],
        ceils: [0.0; 3],
    };
    let k = params.slopes[1];
    params.curvatures = [
        K::NEUTRAL_AXIS_CURV_MAX_RATIO * k,
        0.0,
        -K::NEUTRAL_AXIS_CURV_MAX_RATIO * k,
    ];
    let (w, h) = (256usize, 4usize);
    let input = RgbImage::new_with(w, h, |buf| {
        for y in 0..h {
            for x in 0..w {
                let u = -4.0 + 6.0 * x as f64 / (w - 1) as f64;
                for ch in 0..3 {
                    buf[(y * w + x) * 3 + ch] = 10f64.powf(u - 1.0) as f32;
                }
            }
        }
    })
    .unwrap();
    let cpu = reference::render(&input, &params, CpuOptions::default());
    let source = gpu.pipeline.upload(&input).unwrap();
    let out = gpu
        .pipeline
        .render(&source, &params, RenderOptions::default())
        .unwrap();
    expect_gate("vertex hold", &out.image, &cpu, GATE_MEAN, GATE_MAX);
}

// ───────────────────────── contrast mask (tight gate, 3× render) ─────────────────────────

#[test]
fn contrast_mask_matches_cpu() {
    let gpu = gpu_or_skip!();
    let img = image();
    let analysis = analysis_for(&img);
    let mut s = ExposureSettings::default();
    s.pre_saturation = 1.0;
    s.skin_protection = 0.0;
    s.contrast_mask = 0.35;
    s.mask_spacer = 4.0;
    let params = derive_render_params(&s, &analysis);
    assert_ne!(params.mask_val_scale, [0.0; 3], "mask gate should be on");

    // A plane on a coarser grid than the render (the analysis-grid situation).
    let plane = TestPlane::new(48, 32);
    let cpu_plane = negkit::contrast_mask::Plane::new(48, 32, plane.data.clone()).unwrap();
    let mask = gpu
        .pipeline
        .upload_mask(&plane.data, plane.width, plane.height)
        .unwrap();

    let cpu = reference::render(
        &img,
        &params,
        CpuOptions {
            srgb_encode: false,
            mask_plane: Some(&cpu_plane),
        },
    );
    let unmasked = reference::render(&img, &params, CpuOptions::default());
    let (moved, _) = compare(&cpu, &unmasked);
    assert!(
        moved > 2e-3,
        "precondition: the mask must move the frame ({moved})"
    );

    let source = gpu.pipeline.upload(&img).unwrap();
    let out = gpu
        .pipeline
        .render(
            &source,
            &params,
            RenderOptions {
                srgb_encode: false,
                mask: Some(&mask),
            },
        )
        .unwrap();
    expect_gate("contrast mask", &out.image, &cpu, TIGHT_MEAN, TIGHT_MAX);

    // RENDER-SIZE ≠ PLANE-SIZE: upsample the source 3× so the bilinear mapping is
    // exercised off the identity path (one proxy-built plane sampled by a bigger render).
    let (bw, bh) = (img.width * 3, img.height * 3);
    let big = RgbImage::new_with(bw, bh, |dst| {
        for y in 0..bh {
            for x in 0..bw {
                let s_idx = ((y / 3) * img.width + (x / 3)) * 3;
                let d_idx = (y * bw + x) * 3;
                dst[d_idx..d_idx + 3].copy_from_slice(&img.data[s_idx..s_idx + 3]);
            }
        }
    })
    .unwrap();
    let cpu_big = reference::render(
        &big,
        &params,
        CpuOptions {
            srgb_encode: false,
            mask_plane: Some(&plane),
        },
    );
    let source_big = gpu.pipeline.upload(&big).unwrap();
    let out_big = gpu
        .pipeline
        .render(
            &source_big,
            &params,
            RenderOptions {
                srgb_encode: false,
                mask: Some(&mask),
            },
        )
        .unwrap();
    expect_gate(
        "contrast mask 3×",
        &out_big.image,
        &cpu_big,
        TIGHT_MEAN,
        TIGHT_MAX,
    );

    // The mask travels with the plane: scale set but no plane bound renders unmasked.
    let out_no_plane = gpu
        .pipeline
        .render(&source, &params, RenderOptions::default())
        .unwrap();
    expect_gate(
        "mask scale without a plane",
        &out_no_plane.image,
        &unmasked,
        GATE_MEAN,
        GATE_MAX,
    );
}

/// A masked render LARGER than 2^24 pixels: the kernel splits its flat index into (x, y)
/// with integer math (the GLSL's f32 split loses integer exactness past 2^24 and sampled
/// the opposite plane edge on ~1200 right-edge pixels of a 24 MP export). 4200 × 4200 =
/// 17.64 MP with the coarse plane; the rows past 2^24 get their own max check so the
/// whole-frame mean can't dilute them.
#[test]
fn contrast_mask_above_2_24_pixels_matches_cpu() {
    let gpu = gpu_or_skip!();
    let base = image();
    let analysis = analysis_for(&base);
    let mut s = ExposureSettings::default();
    s.pre_saturation = 1.0;
    s.skin_protection = 0.0;
    s.contrast_mask = 0.35;
    let params = derive_render_params(&s, &analysis);
    let plane = TestPlane::new(48, 32);
    let cpu_plane = negkit::contrast_mask::Plane::new(48, 32, plane.data.clone()).unwrap();
    // The test sampler and negkit's Plane are the same arithmetic.
    for &(x, y) in &[
        (0usize, 0usize),
        (4199, 4199),
        (2100, 17),
        (4199, 3995),
        (0, 4199),
    ] {
        assert_eq!(
            plane.sample(x, y, 4200, 4200),
            cpu_plane.sample_at(x, y, 4200, 4200)
        );
    }
    let (w, h) = (4200usize, 4200usize);
    assert!(w * h > 1 << 24);
    let big = RgbImage::new_with(w, h, |dst| {
        for y in 0..h {
            let sy = y * base.height / h;
            for x in 0..w {
                let sx = x * base.width / w;
                let s_idx = (sy * base.width + sx) * 3;
                dst[(y * w + x) * 3..(y * w + x) * 3 + 3]
                    .copy_from_slice(&base.data[s_idx..s_idx + 3]);
            }
        }
    })
    .unwrap();
    let source = match gpu.pipeline.upload(&big) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: 17.6 MP buffers exceed this device ({e})");
            return;
        }
    };
    let mask = gpu
        .pipeline
        .upload_mask(&plane.data, plane.width, plane.height)
        .unwrap();
    let t = Instant::now();
    let cpu = reference::render(
        &big,
        &params,
        CpuOptions {
            srgb_encode: false,
            mask_plane: Some(&cpu_plane),
        },
    );
    println!(
        "CPU reference at {w}×{h} with mask: {:.2} s",
        t.elapsed().as_secs_f64()
    );
    let out = gpu
        .pipeline
        .render(
            &source,
            &params,
            RenderOptions {
                srgb_encode: false,
                mask: Some(&mask),
            },
        )
        .unwrap();
    expect_gate(
        "contrast mask > 2^24 px",
        &out.image,
        &cpu,
        TIGHT_MEAN,
        TIGHT_MAX,
    );
    // The rows whose flat index exceeds 2^24, right edge: where the f32 split went wrong.
    let first_bad_row = (1usize << 24) / w;
    let mut worst = 0.0f64;
    for y in first_bad_row..h {
        for x in (w - 4)..w {
            let i = (y * w + x) * 3;
            for ch in 0..3 {
                worst = worst
                    .max((f64::from(out.image.data[i + ch]) - f64::from(cpu.data[i + ch])).abs());
            }
        }
    }
    println!("right-edge rows ≥ {first_bad_row}: worst {worst:.3e}");
    assert!(
        worst < TIGHT_MAX,
        "right-edge rows past 2^24 deviate {worst}"
    );
}

// ───────────────────────── display / sRGB / handoffs ─────────────────────────

fn worst_u8_vs_float(float: &RgbImage, rgba8: &[u8]) -> f64 {
    let n = float.width * float.height;
    let mut worst = 0.0f64;
    for i in 0..n {
        for ch in 0..3 {
            let f = f64::from(float.data[i * 3 + ch]) * 255.0;
            let d = f64::from(rgba8[i * 4 + ch]);
            worst = worst.max((f - d).abs());
        }
        assert_eq!(rgba8[i * 4 + 3], 255, "alpha at pixel {i}");
    }
    worst
}

#[test]
fn display_path_matches_float_path() {
    let gpu = gpu_or_skip!();
    let img = image();
    let params = derive_render_params(&ExposureSettings::default(), &analysis_for(&img));
    let source = gpu.pipeline.upload(&img).unwrap();
    let float = gpu
        .pipeline
        .render(&source, &params, RenderOptions::default())
        .unwrap();
    let display = gpu
        .pipeline
        .render_display(&source, &params, RenderOptions::default())
        .unwrap();
    assert_eq!((display.width, display.height), (192, 128));
    let worst = worst_u8_vs_float(&float.image, &display.rgba8);
    println!("display vs float: worst {worst:.3}/255");
    assert!(
        worst <= 1.5,
        "display path deviates {worst}/255 from float path"
    );
    assert_eq!(
        display.histogram, float.histogram,
        "both paths bin the same content"
    );
}

#[test]
fn srgb_display_flag_matches_cpu_transform() {
    let gpu = gpu_or_skip!();
    let img = image();
    let params = derive_render_params(&ExposureSettings::default(), &analysis_for(&img));
    let source = gpu.pipeline.upload(&img).unwrap();
    let float = gpu
        .pipeline
        .render(&source, &params, RenderOptions::default())
        .unwrap();
    let display = gpu
        .pipeline
        .render_display(
            &source,
            &params,
            RenderOptions {
                srgb_encode: true,
                mask: None,
            },
        )
        .unwrap();
    // The canvas transform computed scalar-CPU from the float path's own output.
    let mut expected = float.image.clone();
    for px in expected.data.chunks_exact_mut(3) {
        let e = srgb_from_encoded([px[0], px[1], px[2]]);
        px.copy_from_slice(&e);
    }
    let worst = worst_u8_vs_float(&expected, &display.rgba8);
    println!("sRGB display vs CPU transform: worst {worst:.3}/255");
    assert!(
        worst <= 1.5,
        "sRGB display path deviates {worst}/255 from CPU transform"
    );

    // And the float path honours the same flag (the sRGB export leg) at the standard gate
    // against the CPU chain's own sRGB leg.
    let float_srgb = gpu
        .pipeline
        .render(
            &source,
            &params,
            RenderOptions {
                srgb_encode: true,
                mask: None,
            },
        )
        .unwrap();
    let cpu_srgb = reference::render(
        &img,
        &params,
        CpuOptions {
            srgb_encode: true,
            mask_plane: None,
        },
    );
    expect_gate(
        "sRGB float path",
        &float_srgb.image,
        &cpu_srgb,
        GATE_MEAN,
        GATE_MAX,
    );
    assert_eq!(
        float_srgb.histogram, float.histogram,
        "the histogram bins the working encode regardless of the sRGB flag"
    );
}

#[test]
fn render_display_into_matches_allocating_path() {
    let gpu = gpu_or_skip!();
    let img = image();
    let params = derive_render_params(&ExposureSettings::default(), &analysis_for(&img));
    let source = gpu.pipeline.upload(&img).unwrap();
    let opts = RenderOptions {
        srgb_encode: true,
        mask: None,
    };
    let allocating = gpu.pipeline.render_display(&source, &params, opts).unwrap();
    let mut dest = vec![0u8; source.rgba8_len() + 16];
    let bins = gpu
        .pipeline
        .render_display_into(&source, &params, opts, &mut dest)
        .unwrap()
        .expect("buffer is large enough");
    assert_eq!(&dest[..source.rgba8_len()], &allocating.rgba8[..]);
    assert!(
        dest[source.rgba8_len()..].iter().all(|b| *b == 0),
        "no overrun"
    );
    assert_eq!(bins, allocating.histogram);
    // Too small: nothing written, `None` back.
    let mut small = vec![7u8; source.rgba8_len() - 1];
    let none = gpu
        .pipeline
        .render_display_into(&source, &params, opts, &mut small)
        .unwrap();
    assert!(none.is_none());
    assert!(small.iter().all(|b| *b == 7));
}

#[test]
fn render_display_to_texture_matches_readback() {
    let gpu = gpu_or_skip!();
    // 61 px wide → 244-byte rows, so the texture copy needs the padded 256-byte pitch.
    let img = synthetic_negative(61, 47);
    let params = derive_render_params(&ExposureSettings::default(), &analysis_for(&img));
    let source = gpu.pipeline.upload(&img).unwrap();
    let opts = RenderOptions {
        srgb_encode: true,
        mask: None,
    };
    let device = gpu.ctx.device();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("display"),
        size: wgpu::Extent3d {
            width: 61,
            height: 47,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let bins = gpu
        .pipeline
        .render_display_to_texture(&source, &params, opts, &texture)
        .unwrap();
    let expected = gpu.pipeline.render_display(&source, &params, opts).unwrap();
    assert_eq!(bins, expected.histogram);

    // Read the texture back through a padded staging buffer and compare row by row.
    let pitch = 256u32;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("tex readback"),
        size: u64::from(pitch) * 47,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(pitch),
                rows_per_image: Some(47),
            },
        },
        wgpu::Extent3d {
            width: 61,
            height: 47,
            depth_or_array_layers: 1,
        },
    );
    gpu.ctx.queue().submit(Some(encoder.finish()));
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    let view = staging.slice(..).get_mapped_range().unwrap();
    for y in 0..47usize {
        let row = &view[y * 256..y * 256 + 61 * 4];
        assert_eq!(row, &expected.rgba8[y * 244..(y + 1) * 244], "row {y}");
    }
}

// ───────────────────────── histogram contract ─────────────────────────

#[test]
fn histogram_matches_cpu_binning_of_gpu_output() {
    let gpu = gpu_or_skip!();
    let img = image();
    let mut s = ExposureSettings::default();
    s.levels_green = vec![[0.4, 0.3]]; // the bins include the levels remap
    let params = derive_render_params(&s, &analysis_for(&img));
    let source = gpu.pipeline.upload(&img).unwrap();
    let out = gpu
        .pipeline
        .render(&source, &params, RenderOptions::default())
        .unwrap();
    // The encode_f output IS the encoded content the histogram kernel bins — same
    // arithmetic lane for lane — so negkit's CPU binning of it must match EXACTLY.
    let cpu_bins = histogram_256(&out.image);
    assert_eq!(
        out.histogram, cpu_bins,
        "GPU bins vs CPU binning of the GPU encode"
    );
    let n = (img.width * img.height) as u32;
    for ch in 0..4 {
        let sum: u32 = out.histogram[ch * 256..(ch + 1) * 256].iter().sum();
        assert_eq!(sum, n, "channel {ch} bins sum to the pixel count");
    }
    // Against the CPU reference's own encode: edge-of-bin disagreements only (the Swift
    // L1 ≤ n/25 rule).
    let cpu = reference::render(&img, &params, CpuOptions::default());
    let ref_bins = histogram_256(&cpu);
    let l1: u64 = out
        .histogram
        .iter()
        .zip(ref_bins.iter())
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    println!("histogram L1 vs CPU reference: {l1} of {} samples", n * 4);
    assert!(l1 <= u64::from(n) / 25, "histogram L1 diff {l1}");
}

#[test]
fn odd_pixel_count_dispatches_cleanly() {
    // 61×47 is not a multiple of the 256-wide workgroup — pins the bounds check.
    let gpu = gpu_or_skip!();
    let img = synthetic_negative(61, 47);
    let analysis = analysis_for(&img);
    let params = derive_render_params(&ExposureSettings::default(), &analysis);
    let cpu = reference::render(&img, &params, CpuOptions::default());
    let source = gpu.pipeline.upload(&img).unwrap();
    let out = gpu
        .pipeline
        .render(&source, &params, RenderOptions::default())
        .unwrap();
    expect_gate("odd-size", &out.image, &cpu, GATE_MEAN, GATE_MAX);
}

#[test]
fn upload_rejects_shape_mismatch() {
    let gpu = gpu_or_skip!();
    let bad = RgbImage {
        width: 4,
        height: 4,
        data: vec![0.5; 4 * 4 * 3 - 3],
    };
    assert!(gpu.pipeline.upload(&bad).is_err());
    assert!(gpu.pipeline.upload_mask(&[0.0; 5], 2, 2).is_err());
}

// ───────────────────────── timing (`--nocapture`) ─────────────────────────

#[test]
fn timing_proxy_display_and_export_render() {
    let gpu = gpu_or_skip!();
    let proxy = synthetic_negative(1536, 1024);
    let analysis = analysis_for(&proxy);
    let params = derive_render_params(&ExposureSettings::default(), &analysis);
    let source = gpu.pipeline.upload(&proxy).unwrap();
    let opts = RenderOptions {
        srgb_encode: true,
        mask: None,
    };
    let mut dest = vec![0u8; source.rgba8_len()];
    for _ in 0..3 {
        gpu.pipeline
            .render_display_into(&source, &params, opts, &mut dest)
            .unwrap();
    }
    let mut best = f64::MAX;
    for _ in 0..30 {
        let t = Instant::now();
        gpu.pipeline
            .render_display_into(&source, &params, opts, &mut dest)
            .unwrap();
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    println!(
        "[{}] 1536×1024 render_display_into incl. histogram + sRGB + readback: best {best:.2} ms (target ≤ 2.5 ms)",
        gpu.ctx.backend_name()
    );

    // 24 MP float render (export).
    let (w, h) = (6000usize, 4000usize);
    let big = synthetic_negative(w, h);
    match gpu.pipeline.upload(&big) {
        Ok(big_source) => {
            let mut best = f64::MAX;
            for _ in 0..3 {
                let t = Instant::now();
                let out = gpu
                    .pipeline
                    .render(&big_source, &params, RenderOptions::default())
                    .unwrap();
                best = best.min(t.elapsed().as_secs_f64());
                assert_eq!(out.image.width, w);
            }
            println!(
                "[{}] {w}×{h} float render incl. readback: best {best:.3} s (target ≤ 0.45 s)",
                gpu.ctx.backend_name()
            );
        }
        Err(e) => println!("24 MP render skipped: {e}"),
    }
}
