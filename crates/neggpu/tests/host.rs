//! Host-side contracts the parity suite can't see: refused-not-aborted oversizes, the tier
//! caches (small LRU cap, the one large display slot, unretained large float scratch),
//! release of dropped source/mask buffers, the shared-device constructor, the texture
//! format rule, the `NEG_GPU` aliases and the rgba8 quantization twin.

#![allow(clippy::field_reassign_with_default)]

#[macro_use]
mod common;

use common::*;
use neggpu::context::backend_choice_from;
use neggpu::{
    CACHE_PIXEL_BUDGET, GpuContext, GpuError, RenderOptions, RenderPipeline, SMALL_TIER_CAP,
    pack_unorm8,
};
use negkit::ExposureSettings;
use negkit::derive::derive_render_params;

fn default_params(img: &negkit::RgbImage) -> negkit::RenderParams {
    derive_render_params(&ExposureSettings::default(), &analysis_for(img))
}

#[test]
fn oversized_render_is_refused_not_aborted() {
    let gpu = gpu_or_skip!();
    // 65536 × 65536 × 12 B = 48 GB: past any device limit, refused before any GPU work.
    let err = gpu.pipeline.fits_render(1 << 16, 1 << 16).unwrap_err();
    match &err {
        GpuError::TooLarge {
            bytes,
            limit,
            limit_name,
        } => {
            assert_eq!(*bytes, 12u64 << 32);
            assert!(*bytes > *limit);
            assert!(limit_name.starts_with("max_"), "{limit_name}");
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
    let text = err.to_string();
    assert!(
        text.contains("max_") && text.contains("required_limits"),
        "{text}"
    );
    // Same refusal through the context directly, and nothing was recorded as uncaptured.
    assert!(matches!(
        gpu.ctx.check_buffer_size(u64::MAX),
        Err(GpuError::TooLarge { .. })
    ));
    assert_eq!(gpu.ctx.take_last_error(), None);
    // A normal render still works afterwards (no poisoned state).
    let img = synthetic_negative(64, 48);
    let source = gpu.pipeline.upload(&img).unwrap();
    gpu.pipeline
        .render(&source, &default_params(&img), RenderOptions::default())
        .unwrap();
}

#[test]
fn from_existing_shares_device_and_limits() {
    let gpu = gpu_or_skip!();
    let shared = GpuContext::from_existing(
        gpu.ctx.device().clone(),
        gpu.ctx.queue().clone(),
        gpu.ctx.adapter_info().clone(),
    );
    assert_eq!(
        shared.limits().max_buffer_size,
        gpu.ctx.limits().max_buffer_size,
        "the shared context records the device's actual limits"
    );
    let pipeline = RenderPipeline::new(&shared).unwrap();
    let img = synthetic_negative(96, 64);
    let params = default_params(&img);
    let a = gpu
        .pipeline
        .render(
            &gpu.pipeline.upload(&img).unwrap(),
            &params,
            RenderOptions::default(),
        )
        .unwrap();
    let b = pipeline
        .render(
            &pipeline.upload(&img).unwrap(),
            &params,
            RenderOptions::default(),
        )
        .unwrap();
    assert_eq!(
        a.image, b.image,
        "two pipelines on one device agree bit for bit"
    );
    assert_eq!(a.histogram, b.histogram);
    assert!(matches!(
        pipeline.fits_render(1 << 16, 1 << 16),
        Err(GpuError::TooLarge { .. })
    ));
    // `required_limits` raises the three buffer/dispatch limits and nothing else.
    let have = gpu.ctx.limits().clone();
    let want = neggpu::required_limits(&have);
    assert!(want.max_buffer_size >= wgpu::Limits::default().max_buffer_size);
    assert!(
        want.max_storage_buffer_binding_size
            >= wgpu::Limits::default().max_storage_buffer_binding_size
    );
    assert_eq!(
        want.max_texture_dimension_2d,
        wgpu::Limits::default().max_texture_dimension_2d
    );
}

#[test]
fn small_tier_cache_evicts_lru_beyond_cap() {
    let gpu = gpu_or_skip!();
    let pipeline = RenderPipeline::new(&gpu.ctx).unwrap();
    let base = synthetic_negative(64, 48);
    let params = default_params(&base);
    // Six distinct small sizes (a crop drag), display and float alternating.
    let sizes: Vec<(usize, usize)> = (0..6).map(|i| (40 + 7 * i, 30 + 5 * i)).collect();
    let mut sources = Vec::new();
    for (i, &(w, h)) in sizes.iter().enumerate() {
        let img = synthetic_negative(w, h);
        let source = pipeline.upload(&img).unwrap();
        if i % 2 == 0 {
            pipeline
                .render(&source, &params, RenderOptions::default())
                .unwrap();
        } else {
            pipeline
                .render_display(&source, &params, RenderOptions::default())
                .unwrap();
        }
        sources.push(source);
    }
    let stats = pipeline.cache_stats();
    assert_eq!(stats.small_sizes.len(), SMALL_TIER_CAP);
    let expected: Vec<(u32, u32)> = sizes[2..]
        .iter()
        .map(|&(w, h)| (w as u32, h as u32))
        .collect();
    assert_eq!(
        stats.small_sizes, expected,
        "least recently rendered evicted first"
    );
    // Re-rendering the oldest retained size touches it to most-recent …
    pipeline
        .render(&sources[2], &params, RenderOptions::default())
        .unwrap();
    let stats = pipeline.cache_stats();
    assert_eq!(stats.small_sizes.last(), Some(&expected[0]));
    // … so a seventh size now evicts sizes[3], not sizes[2].
    let img = synthetic_negative(99, 77);
    let s7 = pipeline.upload(&img).unwrap();
    pipeline
        .render_display(&s7, &params, RenderOptions::default())
        .unwrap();
    let stats = pipeline.cache_stats();
    assert!(stats.small_sizes.contains(&expected[0]));
    assert!(!stats.small_sizes.contains(&expected[1]));
    assert_eq!(stats.small_sizes.len(), SMALL_TIER_CAP);
    assert_eq!(stats.large_display, None);
    pipeline.release_cached();
    assert_eq!(pipeline.cache_stats(), Default::default());
}

#[test]
fn large_display_tier_is_retained_one_slot() {
    let gpu = gpu_or_skip!();
    let pipeline = RenderPipeline::new(&gpu.ctx).unwrap();
    let base = synthetic_negative(64, 48);
    let params = default_params(&base);
    // Just over the budget: 2048 × 2000 = 4.096 MP.
    let (w1, h1) = (2048usize, 2000usize);
    assert!((w1 * h1) as u64 > CACHE_PIXEL_BUDGET);
    let big1 = pipeline.upload(&synthetic_negative(w1, h1)).unwrap();
    if let Err(e) = pipeline.fits_render(w1 as u32, h1 as u32) {
        eprintln!("SKIP: {e}");
        return;
    }
    let mut dest = vec![0u8; big1.rgba8_len()];
    pipeline
        .render_display_into(&big1, &params, RenderOptions::default(), &mut dest)
        .unwrap();
    assert_eq!(
        pipeline.cache_stats().large_display,
        Some((w1 as u32, h1 as u32))
    );
    assert!(pipeline.cache_stats().small_sizes.is_empty());

    // A large FLOAT render is scratch: the display slot is untouched afterwards.
    let (w2, h2) = (2100usize, 2000usize);
    let big2 = pipeline.upload(&synthetic_negative(w2, h2)).unwrap();
    pipeline
        .render(&big2, &params, RenderOptions::default())
        .unwrap();
    assert_eq!(
        pipeline.cache_stats().large_display,
        Some((w1 as u32, h1 as u32))
    );

    // A different large DISPLAY size replaces the slot (never two large tiers).
    let mut dest2 = vec![0u8; big2.rgba8_len()];
    pipeline
        .render_display_into(&big2, &params, RenderOptions::default(), &mut dest2)
        .unwrap();
    assert_eq!(
        pipeline.cache_stats().large_display,
        Some((w2 as u32, h2 as u32))
    );

    // Small renders never touch the slot; release_cached clears it.
    let small = pipeline.upload(&base).unwrap();
    pipeline
        .render_display(&small, &params, RenderOptions::default())
        .unwrap();
    assert_eq!(
        pipeline.cache_stats().large_display,
        Some((w2 as u32, h2 as u32))
    );
    pipeline.release_cached();
    assert_eq!(pipeline.cache_stats().large_display, None);
}

#[test]
fn dropped_source_and_mask_are_released_on_next_upload() {
    let gpu = gpu_or_skip!();
    let pipeline = RenderPipeline::new(&gpu.ctx).unwrap();
    let img = synthetic_negative(80, 60);
    let mut s = ExposureSettings::default();
    s.contrast_mask = 0.3;
    let params = derive_render_params(&s, &analysis_for(&img));
    let plane = TestPlane::new(20, 15);
    let source = pipeline.upload(&img).unwrap();
    let mask = pipeline.upload_mask(&plane.data, 20, 15).unwrap();
    let (sid, mid) = (source.id(), mask.id());
    pipeline
        .render(
            &source,
            &params,
            RenderOptions {
                srgb_encode: false,
                mask: Some(&mask),
            },
        )
        .unwrap();
    let stats = pipeline.cache_stats();
    assert_eq!(stats.retained_sources, vec![sid]);
    assert_eq!(stats.retained_masks, vec![mid]);

    // Dropping the caller's handles alone changes nothing yet (the bind groups hold them) …
    drop(source);
    drop(mask);
    assert_eq!(pipeline.cache_stats().retained_sources, vec![sid]);
    // … the next upload forgets them.
    let source2 = pipeline.upload(&img).unwrap();
    let stats = pipeline.cache_stats();
    assert!(stats.retained_sources.is_empty(), "{stats:?}");
    assert!(stats.retained_masks.is_empty(), "{stats:?}");
    // And a render with the new source rebuilds its bind group and renders correctly.
    let out = pipeline
        .render(&source2, &params, RenderOptions::default())
        .unwrap();
    assert_eq!(pipeline.cache_stats().retained_sources, vec![source2.id()]);
    assert_eq!(out.image.width, 80);
    // A dropped source is also forgotten at the next render (no upload needed).
    let source3 = pipeline.upload(&img).unwrap();
    drop(source2);
    pipeline
        .render(&source3, &params, RenderOptions::default())
        .unwrap();
    assert_eq!(pipeline.cache_stats().retained_sources, vec![source3.id()]);
}

#[test]
fn texture_path_accepts_rgba8unorm_only() {
    let gpu = gpu_or_skip!();
    let img = synthetic_negative(64, 48);
    let params = default_params(&img);
    let source = gpu.pipeline.upload(&img).unwrap();
    let make = |format| {
        gpu.ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 64,
                height: 48,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    };
    let srgb = make(wgpu::TextureFormat::Rgba8UnormSrgb);
    let err = gpu
        .pipeline
        .render_display_to_texture(&source, &params, RenderOptions::default(), &srgb)
        .unwrap_err();
    assert!(
        matches!(
            err,
            GpuError::TextureFormat(wgpu::TextureFormat::Rgba8UnormSrgb)
        ),
        "{err:?}"
    );
    let bgra = make(wgpu::TextureFormat::Bgra8Unorm);
    assert!(matches!(
        gpu.pipeline
            .render_display_to_texture(&source, &params, RenderOptions::default(), &bgra),
        Err(GpuError::TextureFormat(_))
    ));
    let ok = make(wgpu::TextureFormat::Rgba8Unorm);
    gpu.pipeline
        .render_display_to_texture(&source, &params, RenderOptions::default(), &ok)
        .unwrap();
    assert_eq!(gpu.ctx.take_last_error(), None);
}

#[test]
fn neg_gpu_off_aliases() {
    for off in ["off", "OFF", "0", "false", "no", "cpu", " Cpu "] {
        assert_eq!(
            backend_choice_from(Some(off), None).unwrap(),
            None,
            "{off:?}"
        );
        assert!(neggpu::context::gpu_disabled_by(off));
    }
    for on in ["1", "on", "true", "vulkan", ""] {
        assert!(
            backend_choice_from(Some(on), None).unwrap().is_some(),
            "{on:?}"
        );
    }
    assert_eq!(
        backend_choice_from(None, Some("vulkan")).unwrap(),
        Some(wgpu::Backends::VULKAN)
    );
    assert_eq!(
        backend_choice_from(None, Some("Metal")).unwrap(),
        Some(wgpu::Backends::METAL)
    );
    assert_eq!(
        backend_choice_from(None, Some("dx12")).unwrap(),
        Some(wgpu::Backends::DX12)
    );
    assert_eq!(backend_choice_from(None, Some("off")).unwrap(), None);
    assert!(matches!(
        backend_choice_from(None, Some("opengl")),
        Err(GpuError::BadBackend(_))
    ));
    // NEG_GPU=off wins over a backend override.
    assert_eq!(
        backend_choice_from(Some("0"), Some("vulkan")).unwrap(),
        None
    );
}

#[test]
fn pack_unorm8_matches_gpu_quantization() {
    assert_eq!(pack_unorm8(0.0), 0);
    assert_eq!(pack_unorm8(1.0), 255);
    assert_eq!(pack_unorm8(-0.3), 0);
    assert_eq!(pack_unorm8(2.0), 255);
    assert_eq!(pack_unorm8(0.5), 128); // 127.5 rounds up — truncation would give 127
    assert_eq!(pack_unorm8(0.001), 0);
    assert_eq!(pack_unorm8(0.002), 1);
    let gpu = gpu_or_skip!();
    let img = synthetic_negative(192, 128);
    let params = default_params(&img);
    let source = gpu.pipeline.upload(&img).unwrap();
    for srgb in [false, true] {
        let opts = RenderOptions {
            srgb_encode: srgb,
            mask: None,
        };
        let float = gpu.pipeline.render(&source, &params, opts).unwrap();
        let display = gpu.pipeline.render_display(&source, &params, opts).unwrap();
        let mut mismatches = 0usize;
        for (px, out) in float
            .image
            .data
            .chunks_exact(3)
            .zip(display.rgba8.chunks_exact(4))
        {
            for (f, d) in px.iter().zip(out) {
                if pack_unorm8(*f) != *d {
                    mismatches += 1;
                }
            }
        }
        println!(
            "pack_unorm8 vs GPU bytes (srgb {srgb}): {mismatches} mismatches of {}",
            img.pixel_count() * 3
        );
        // Exact ties (x·255 + 0.5 integral) may round differently in hardware; anything
        // beyond a handful of lanes means the CPU twin is wrong.
        assert!(
            mismatches <= img.pixel_count() * 3 / 1000,
            "{mismatches} mismatches"
        );
    }
}
