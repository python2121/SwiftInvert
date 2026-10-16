//! GPU vs CPU through the SAME session semantics: a `Session` rendered through `CpuRenderer`
//! and through `GpuRenderer` agrees at the project gates (float: mean < 0.01, max < 0.04;
//! the rgba8 path within 1.5/255 of the float path), including a Contrast Mask case through
//! the REAL `negkit::contrast_mask::build_plane` from the proxy (the handoff the review
//! flagged as untested) and an export-sized render on the export pipeline. Skips (and says
//! so) without a GPU.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

mod common;

use std::sync::Arc;

use negkit::{NormalizedRect, RgbImage};
use negsession::{
    CpuRenderer, Frame, GpuRenderer, OutputKind, RenderRequest, RenderTier, Renderer, Session,
};

const GATE_MEAN: f64 = 0.01;
const GATE_MAX: f64 = 0.04;
const TIGHT_MEAN: f64 = 1e-3;
const TIGHT_MAX: f64 = 0.02;

fn gpu() -> Option<Arc<GpuRenderer>> {
    match GpuRenderer::new_headless() {
        Ok(Some(g)) => {
            eprintln!("GPU parity on {}", g.name());
            Some(Arc::new(g))
        }
        Ok(None) => {
            eprintln!(
                "SKIP (no GPU): {}",
                neggpu::unavailable_reason().unwrap_or_default()
            );
            None
        }
        Err(e) => panic!("GPU present but the renderer failed: {e}"),
    }
}

fn pair(gpu: Arc<GpuRenderer>) -> (Session, Session) {
    let native = common::synthetic_negative();
    let full = common::upscale2x(&native);
    let cpu = Session::synthetic(
        "parity.CR3",
        native.clone(),
        Some(full.clone()),
        Arc::new(CpuRenderer),
    );
    let gpu = Session::synthetic("parity.CR3", native, Some(full), gpu);
    (cpu, gpu)
}

struct Gate {
    mean: f64,
    max: f64,
}

fn gate_float(label: &str, a: &RgbImage, b: &RgbImage) -> Gate {
    assert_eq!((a.width, a.height), (b.width, b.height), "{label}: dims");
    let mut sum = 0.0;
    let mut max: f64 = 0.0;
    for (x, y) in a.data.iter().zip(&b.data) {
        let d = f64::from((x - y).abs());
        sum += d;
        max = max.max(d);
    }
    let mean = sum / a.data.len() as f64;
    eprintln!("{label}: mean {mean:.3e} max {max:.3e}");
    Gate { mean, max }
}

fn gate_rgba8(label: &str, a: &[u8], b: &[u8]) -> Gate {
    assert_eq!(a.len(), b.len(), "{label}: lengths");
    let mut sum = 0.0;
    let mut max: f64 = 0.0;
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if i % 4 == 3 {
            assert_eq!((*x, *y), (255, 255), "{label}: alpha");
            continue;
        }
        let d = f64::from(x.abs_diff(*y)) / 255.0;
        sum += d;
        max = max.max(d);
    }
    let mean = sum / (a.len() / 4 * 3) as f64;
    eprintln!("{label}: mean {mean:.3e} max {max:.3e} (of 1.0)");
    Gate { mean, max }
}

fn expect(label: &str, g: &Gate, mean: f64, max: f64) {
    assert!(g.mean < mean, "{label}: mean {} ≥ {mean}", g.mean);
    assert!(g.max < max, "{label}: max {} ≥ {max}", g.max);
}

fn float(req: RenderRequest) -> RenderRequest {
    RenderRequest {
        output: OutputKind::Float,
        ..req
    }
}

#[test]
fn proxy_display_and_float_agree() {
    let Some(gpu) = gpu() else { return };
    let (mut cpu, mut g) = pair(gpu);
    for s in [&mut cpu, &mut g] {
        s.update_settings(|st| {
            st.exposure_stops = 0.4;
            st.shadows = 0.2;
            st.saturation = 1.3;
            st.temp = 0.3;
            st.levels_green = vec![[0.3, 0.25], [0.7, 0.8]];
        });
    }
    let req = RenderRequest {
        srgb: true,
        ..RenderRequest::default()
    };
    // Float path at the standard gate.
    let fc = cpu.render(float(req)).unwrap();
    let fg = g.render(float(req)).unwrap();
    let gf = gate_float(
        "proxy float",
        fc.frame.image().unwrap(),
        fg.frame.image().unwrap(),
    );
    expect("proxy float", &gf, GATE_MEAN, GATE_MAX);
    // Display path: GPU rgba8 within 1.5/255 of the GPU float path; CPU rgba8 vs GPU rgba8
    // at the float gate plus one quantization step.
    let dc = cpu.render(req).unwrap();
    let dg = g.render(req).unwrap();
    let gpu_bytes = dg.frame.rgba8().unwrap();
    let mut worst: f64 = 0.0;
    for (i, px) in fg.frame.image().unwrap().data.iter().enumerate() {
        let b = f64::from(gpu_bytes[i / 3 * 4 + i % 3]) / 255.0;
        worst = worst.max((b - f64::from(px.clamp(0.0, 1.0))).abs());
    }
    eprintln!("gpu rgba8 vs gpu float: max {:.3}/255", worst * 255.0);
    assert!(worst <= 1.5 / 255.0 + 1e-9);
    let gd = gate_rgba8(
        "proxy rgba8 cpu vs gpu",
        dc.frame.rgba8().unwrap(),
        gpu_bytes,
    );
    expect(
        "proxy rgba8",
        &gd,
        GATE_MEAN + 0.5 / 255.0,
        GATE_MAX + 1.0 / 255.0,
    );
    // Histograms bin the same values: totals equal, shapes close.
    let (hc, hg) = (dc.histogram.unwrap(), dg.histogram.unwrap());
    for ch in 0..4 {
        let tc: u64 = hc[ch * 256..(ch + 1) * 256]
            .iter()
            .map(|&c| u64::from(c))
            .sum();
        let tg: u64 = hg[ch * 256..(ch + 1) * 256]
            .iter()
            .map(|&c| u64::from(c))
            .sum();
        assert_eq!(tc, tg);
    }
    let peak_c = hc[256..512].iter().position_max();
    let peak_g = hg[256..512].iter().position_max();
    assert!(
        peak_c.abs_diff(peak_g) <= 2,
        "green peak bin {peak_c} vs {peak_g}"
    );
    // A second tick re-uploads nothing (the source stays cached by id).
    g.update_settings(|st| st.density = 1.1);
    g.render(req).unwrap();
    assert_eq!(g.stats().source_builds, 1);
}

trait PositionMax {
    fn position_max(self) -> usize;
}

impl<'a, I: Iterator<Item = &'a u32>> PositionMax for I {
    fn position_max(self) -> usize {
        self.enumerate()
            .max_by_key(|(_, v)| **v)
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
}

/// The Contrast Mask handoff: both sessions build the plane with the real builder from the
/// cropped proxy; the GPU samples its uploaded copy, the CPU the plane itself — tight gate.
#[test]
fn contrast_mask_through_the_real_plane() {
    let Some(gpu) = gpu() else { return };
    let (mut cpu, mut g) = pair(gpu);
    for s in [&mut cpu, &mut g] {
        s.update_settings(|st| {
            st.contrast_mask = 0.6;
            st.mask_spacer = 3.0;
            st.crop_rect = Some(NormalizedRect::new(0.08, 0.1, 0.8, 0.75));
        });
    }
    let req = float(RenderRequest::default());
    let fc = cpu.render(req).unwrap();
    let fg = g.render(req).unwrap();
    assert_eq!(cpu.stats().mask_builds, 1);
    assert_eq!(g.stats().mask_builds, 1);
    let gf = gate_float(
        "mask float",
        fc.frame.image().unwrap(),
        fg.frame.image().unwrap(),
    );
    expect("mask float", &gf, TIGHT_MEAN, TIGHT_MAX);
    // The mask actually did something on both sides.
    for s in [&mut cpu, &mut g] {
        s.update_settings(|st| st.contrast_mask = 0.0);
    }
    let pc = cpu.render(req).unwrap();
    let pg = g.render(req).unwrap();
    let moved = gate_float(
        "mask vs no mask (cpu)",
        fc.frame.image().unwrap(),
        pc.frame.image().unwrap(),
    );
    assert!(moved.max > 0.02, "mask had no effect on the CPU side");
    let pf = gate_float(
        "plain float",
        pc.frame.image().unwrap(),
        pg.frame.image().unwrap(),
    );
    expect("plain float", &pf, GATE_MEAN, GATE_MAX);
    // Display path with the mask, HQ tier (the plane samples up under the normalized mapping
    // on a bitmap that isn't the plane's size).
    for s in [&mut cpu, &mut g] {
        s.update_settings(|st| st.contrast_mask = 0.6);
    }
    let hq = RenderRequest {
        tier: RenderTier::Full,
        retain_full: true,
        ..RenderRequest::default()
    };
    let dc = cpu.render(hq).unwrap();
    let dg = g.render(hq).unwrap();
    assert_eq!(dc.tier, RenderTier::Full);
    let gd = gate_rgba8(
        "mask rgba8 full tier",
        dc.frame.rgba8().unwrap(),
        dg.frame.rgba8().unwrap(),
    );
    expect(
        "mask rgba8 full tier",
        &gd,
        TIGHT_MEAN + 0.5 / 255.0,
        TIGHT_MAX + 1.0 / 255.0,
    );
}

/// Export: the full-size float render (above the display pipeline's cache budget → the export
/// pipeline) with the proxy's analysis and plane, sRGB leg in-kernel.
#[test]
fn export_render_agrees() {
    let Some(gpu) = gpu() else { return };
    let (mut cpu, mut g) = pair(gpu);
    for s in [&mut cpu, &mut g] {
        s.update_settings(|st| {
            st.contrast_mask = 0.3;
            st.fine_rotation = 2.0;
            st.highlights = -0.2;
        });
    }
    let ec = cpu.export_render(true).unwrap();
    let eg = g.export_render(true).unwrap();
    assert!(
        ec.pixel_count() as u64 > neggpu::CACHE_PIXEL_BUDGET,
        "the synthetic full frame must exercise the export pipeline"
    );
    let gf = gate_float("export float sRGB", &ec, &eg);
    expect("export float sRGB", &gf, GATE_MEAN, GATE_MAX);
    // Nothing export-sized stayed resident on the display side.
    let stats = g
        .renderer()
        .as_any()
        .and_then(|a| a.downcast_ref::<GpuRenderer>())
        .map(GpuRenderer::stats)
        .unwrap();
    assert!(stats.display.large_display.is_none(), "{stats:?}");
    assert!(stats.export.small_sizes.is_empty() && stats.export.large_display.is_none());
}

/// The medium tier (the free half-size buffer) and the zero-allocation + forget paths.
#[test]
fn medium_tier_and_source_lifecycle() {
    let Some(gpu) = gpu() else { return };
    let (mut cpu, mut g) = pair(Arc::clone(&gpu));
    let req = RenderRequest {
        tier: RenderTier::MediumIfFree,
        srgb: true,
        ..RenderRequest::default()
    };
    let dc = cpu.render(req).unwrap();
    let dg = g.render(req).unwrap();
    assert_eq!(dg.tier, RenderTier::MediumIfFree);
    assert_eq!((dg.width, dg.height), (1600, 1066));
    let gd = gate_rgba8(
        "medium rgba8",
        dc.frame.rgba8().unwrap(),
        dg.frame.rgba8().unwrap(),
    );
    expect(
        "medium rgba8",
        &gd,
        GATE_MEAN + 0.5 / 255.0,
        GATE_MAX + 1.0 / 255.0,
    );
    // Proxy + medium sources are uploaded; a crop replaces the proxy's cropped slot.
    g.render(RenderRequest::default()).unwrap();
    assert_eq!(gpu.stats().sources.len(), 2);
    g.update_settings(|st| st.crop_rect = Some(NormalizedRect::new(0.2, 0.2, 0.6, 0.6)));
    g.render(RenderRequest::default()).unwrap();
    assert_eq!(gpu.stats().sources.len(), 3, "{:?}", gpu.stats());
    g.update_settings(|st| st.crop_rect = Some(NormalizedRect::new(0.1, 0.2, 0.6, 0.6)));
    g.render(RenderRequest::default()).unwrap();
    assert_eq!(
        gpu.stats().sources.len(),
        3,
        "the replaced crop was forgotten"
    );
    // Releasing the enhanced tiers forgets the medium upload; dropping the session the rest.
    g.release_enhanced_tiers();
    assert_eq!(gpu.stats().sources.len(), 2);
    drop(g);
    assert!(gpu.stats().sources.is_empty(), "{:?}", gpu.stats());
    // Detached and export renders never leave an upload behind.
    let (_, mut g) = pair(Arc::clone(&gpu));
    let zero = g.settings().clone();
    g.render_detached(&zero, true).unwrap();
    assert!(gpu.stats().sources.is_empty());
    // The zero-allocation handoff writes the same bytes as the allocating path.
    let out = g.render(req).unwrap();
    let Frame::Rgba8(expected) = out.frame else {
        panic!()
    };
    let mut dest = vec![0u8; expected.len()];
    let analysis = g.analysis().unwrap();
    let params = negkit::derive::derive_render_params(g.settings(), &analysis);
    let src = negsession::renderer::RenderSource {
        id: u64::MAX,
        image: Arc::new(common::synthetic_negative()),
    };
    let opts = negsession::renderer::RenderOptions {
        srgb: true,
        compute_histogram: true,
        mask_plane: None,
    };
    let hist = gpu
        .render_display_into(&src, &params, &opts, &mut dest)
        .unwrap()
        .unwrap();
    assert_eq!(dest, expected);
    assert_eq!(Some(hist), out.histogram);
    let mut short = vec![0u8; 16];
    assert!(
        gpu.render_display_into(&src, &params, &opts, &mut short)
            .unwrap()
            .is_none()
    );
    gpu.forget(u64::MAX);
}
