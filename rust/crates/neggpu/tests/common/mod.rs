//! Shared test support: the one GPU pipeline per test binary (`gpu_or_skip!`), the Swift
//! suites' synthetic negative, a hand-rolled analysis for it (the analysis port lives in
//! another wave; parity only needs the SAME params on both sides), the synthetic64 fixture
//! plumbing from `negkit/tests/chain.rs`, and a CPU mirror of the kernel's plane sampler.

#![allow(dead_code, clippy::field_reassign_with_default)]

use std::sync::OnceLock;

use negkit::derive::derive_render_params;
use negkit::kernel::ExposureAnalysis;
use negkit::reference::MaskSampler;
use negkit::{ExposureSettings, LogNegativeBounds, RenderParams, RgbImage, V3, fixtures};
use neggpu::{GpuContext, RenderPipeline};
use serde_json::Value;

pub struct Gpu {
    pub ctx: GpuContext,
    pub pipeline: RenderPipeline,
}

static GPU: OnceLock<Option<Gpu>> = OnceLock::new();

/// The shared headless context + pipeline, or `None` when no adapter is usable.
pub fn gpu() -> Option<&'static Gpu> {
    GPU.get_or_init(|| {
        let ctx = neggpu::shared_headless()?;
        let pipeline = RenderPipeline::new(&ctx).expect("RenderPipeline::new");
        eprintln!(
            "neggpu tests on {} ({})",
            ctx.adapter_info().name,
            ctx.backend_name()
        );
        Some(Gpu { ctx, pipeline })
    })
    .as_ref()
}

/// Bind the shared GPU or print the skip reason and return from the test.
macro_rules! gpu_or_skip {
    () => {
        match $crate::common::gpu() {
            Some(g) => g,
            None => {
                eprintln!(
                    "SKIP (no GPU): {}",
                    neggpu::unavailable_reason().unwrap_or_default()
                );
                return;
            }
        }
    };
}
pub(crate) use gpu_or_skip;

/// Deterministic synthetic scan (the Swift suites' `syntheticNegative`): smooth ramps +
/// integer-hash noise inside a plausible negative range, channels offset like an
/// orange-masked C-41 frame.
pub fn synthetic_negative(width: usize, height: usize) -> RgbImage {
    RgbImage::new_with(width, height, |buf| {
        for y in 0..height {
            for x in 0..width {
                let i = (y * width + x) * 3;
                let u = x as f64 / (width - 1).max(1) as f64;
                let v = y as f64 / (height - 1).max(1) as f64;
                let mut h = (x as u32)
                    .wrapping_mul(374_761_393)
                    .wrapping_add((y as u32).wrapping_mul(668_265_263));
                h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
                let noise = f64::from(h ^ (h >> 16)) / f64::from(u32::MAX);
                let ramp = 0.15 + 0.55 * u + 0.2 * v * (1.0 - u);
                let n = 0.12 * (noise - 0.5);
                buf[i] = (0.30 * (ramp + n) + 0.18).clamp(0.001, 1.0) as f32;
                buf[i + 1] = (0.22 * (ramp + n) + 0.09).clamp(0.001, 1.0) as f32;
                buf[i + 2] = (0.16 * (ramp + n) + 0.045).clamp(0.001, 1.0) as f32;
            }
        }
    })
    .expect("synthetic negative")
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// A plausible `ExposureAnalysis` for a synthetic negative, measured with plain percentiles
/// (not the NegPy meters — those are another wave's port; the GPU/CPU gate only needs
/// identical params on both sides, and these keep the curve in a realistic regime, with the
/// neutral-axis quadratic cast-removal path engaged).
pub fn analysis_for(img: &RgbImage) -> ExposureAnalysis {
    let n = img.pixel_count();
    let mut logs: [Vec<f64>; 3] = [
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    ];
    for px in img.data.chunks_exact(3) {
        for c in 0..3 {
            logs[c].push(f64::from(px[c].max(1e-6)).log10());
        }
    }
    let mut sorted = logs.clone();
    for s in &mut sorted {
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }
    let floors: V3 = std::array::from_fn(|c| percentile(&sorted[c], 0.5));
    let ceils: V3 = std::array::from_fn(|c| percentile(&sorted[c], 99.5));
    let mut norm_luma: Vec<f64> = (0..n)
        .map(|i| {
            (0..3)
                .map(|c| (logs[c][i] - floors[c]) / (ceils[c] - floors[c]))
                .sum::<f64>()
                / 3.0
        })
        .collect();
    let mut raw_luma: Vec<f64> = (0..n)
        .map(|i| (logs[0][i] + logs[1][i] + logs[2][i]) / 3.0)
        .collect();
    norm_luma.sort_by(|a, b| a.partial_cmp(b).unwrap());
    raw_luma.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let lo = percentile(&norm_luma, 5.0);
    let hi = percentile(&norm_luma, 95.0);
    let window: Vec<f64> = norm_luma
        .iter()
        .copied()
        .filter(|v| *v >= lo && *v <= hi)
        .collect();
    let mean = window.iter().sum::<f64>() / window.len().max(1) as f64;
    let anchor = (0.5 * (mean + 0.5 * (lo + hi))) * 0.8 + 0.46 * 0.2;
    ExposureAnalysis {
        base_bounds: LogNegativeBounds { floors, ceils },
        anchor,
        textural_range: (percentile(&raw_luma, 90.0) - percentile(&raw_luma, 10.0)).abs(),
        shadow_point: Some(percentile(&norm_luma, 99.0)),
        highlight_point: Some(percentile(&norm_luma, 2.0)),
        shadow_refs: std::array::from_fn(|c| percentile(&sorted[c], 98.0)),
        neutral_mid: Some(std::array::from_fn(|c| percentile(&sorted[c], 50.0))),
        neutral_shadow: Some(std::array::from_fn(|c| percentile(&sorted[c], 85.0))),
        neutral_highlight: Some(std::array::from_fn(|c| percentile(&sorted[c], 20.0))),
        neutral_confidence: Some(0.8),
    }
}

// ── synthetic64 fixture plumbing (negkit/tests/chain.rs) ─────────────────────────────────

fn v3(v: &Value) -> V3 {
    let a = v.as_array().unwrap_or_else(|| panic!("not an array: {v}"));
    [
        a[0].as_f64().unwrap(),
        a[1].as_f64().unwrap(),
        a[2].as_f64().unwrap(),
    ]
}

/// The analysis NegPy itself measured on synthetic64, read from the config's `analysis.json`.
pub fn fixture_analysis(name: &str) -> ExposureAnalysis {
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

/// The config's exposure_config over NegPy-neutral SwiftInvert-only defaults (preSaturation
/// 1.0, skinProtection 0 — the colour-pop stage identity).
pub fn fixture_settings(name: &str) -> ExposureSettings {
    let j =
        fixtures::read_json(&format!("synthetic64/{name}/analysis.json")).expect("analysis.json");
    let e = &j["exposure_config"];
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

pub fn synthetic64_input() -> RgbImage {
    fixtures::read_rgb("synthetic64/input.bin", 64, 64).expect("synthetic64/input.bin")
}

pub fn fixture_params(name: &str) -> RenderParams {
    derive_render_params(&fixture_settings(name), &fixture_analysis(name))
}

/// (mean, max) absolute difference.
pub fn compare(a: &RgbImage, b: &RgbImage) -> (f64, f64) {
    assert_eq!(a.data.len(), b.data.len(), "size mismatch");
    let (max, mean) = fixtures::max_mean_abs_diff(&a.data, &b.data);
    (mean, max)
}

/// Assert the standard GPU-vs-CPU gate (mean < 0.01, max < 0.04) or a custom one.
#[track_caller]
pub fn expect_gate(label: &str, a: &RgbImage, b: &RgbImage, gate_mean: f64, gate_max: f64) {
    let (mean, max) = compare(a, b);
    println!("{label}: mean {mean:.3e} max {max:.3e} (gate {gate_mean} / {gate_max})");
    assert!(
        mean < gate_mean && max < gate_max,
        "{label}: mean {mean:.4e} max {max:.4e} exceeds gate {gate_mean} / {gate_max}"
    );
}

/// A hand-made Contrast Mask plane + the CPU twin of the kernel's half-pixel bilinear
/// sample (f32, same arithmetic order), so a mapping error cannot hide on either side.
pub struct TestPlane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl TestPlane {
    /// Zero-mean separable sinusoid, amplitude 0.3 — big enough that a half-pixel shift
    /// shows up at the tight mask gate.
    pub fn new(width: usize, height: usize) -> Self {
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let fx = (2.0 * std::f64::consts::PI * (x as f64 + 0.5) / width as f64).sin();
                let fy = (2.0 * std::f64::consts::PI * (y as f64 + 0.5) / height as f64).cos();
                data.push((0.3 * fx * fy) as f32);
            }
        }
        Self {
            width,
            height,
            data,
        }
    }
}

impl MaskSampler for TestPlane {
    fn sample(&self, x: usize, y: usize, out_width: usize, out_height: usize) -> f32 {
        let dims = [self.width as f32, self.height as f32];
        let pf = [
            (x as f32 + 0.5) * dims[0] / out_width as f32 - 0.5,
            (y as f32 + 0.5) * dims[1] / out_height as f32 - 0.5,
        ];
        let lo = [
            pf[0].floor().clamp(0.0, dims[0] - 1.0),
            pf[1].floor().clamp(0.0, dims[1] - 1.0),
        ];
        let hi = [(lo[0] + 1.0).min(dims[0] - 1.0), (lo[1] + 1.0).min(dims[1] - 1.0)];
        let f = [
            (pf[0] - lo[0]).clamp(0.0, 1.0),
            (pf[1] - lo[1]).clamp(0.0, 1.0),
        ];
        let at = |px: f32, py: f32| self.data[py as usize * self.width + px as usize];
        let v00 = at(lo[0], lo[1]);
        let v10 = at(hi[0], lo[1]);
        let v01 = at(lo[0], hi[1]);
        let v11 = at(hi[0], hi[1]);
        let top = v00 + (v10 - v00) * f[0];
        let bot = v01 + (v11 - v01) * f[0];
        top + (bot - top) * f[1]
    }
}
