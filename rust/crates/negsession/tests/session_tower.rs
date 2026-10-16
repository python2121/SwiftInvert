//! Session cache-key invariants on a SYNTHETIC frame (no decoder): what each kind of settings
//! change rebuilds, and what it must never rebuild. Plus `MeterPreviewAliasTests` and
//! `HQModeTests` (Tests/SwiftInvertTests/), whose decisions live here now.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

mod common;

use std::sync::Arc;

use negkit::{ExposureSettings, NormalizedRect, RgbImage};
use negsession::hq::HQ_AUTO_ZOOM_THRESHOLD;
use negsession::session::{aliases_meter_preview, baseline_settings};
use negsession::{
    CpuRenderer, Frame, HqMode, OutputKind, RenderRequest, RenderTier, Session, ToolMode,
};

fn session() -> Session {
    let native = common::synthetic_negative();
    let full = common::upscale2x(&native);
    Session::synthetic("synthetic.CR3", native, Some(full), Arc::new(CpuRenderer))
}

fn proxy() -> RenderRequest {
    RenderRequest::default()
}

#[test]
fn open_builds_proxy_and_keeps_the_free_medium_tier() {
    let s = session();
    assert_eq!(
        s.proxy_size(),
        (1536, 1023),
        "1600×1066 → long edge 1536, rounded"
    );
    assert!(
        s.has_medium_tier(),
        "native 1600×1066 ≤ 20 MP and wider than the proxy"
    );
    assert!(!s.has_full_tier(), "full decodes lazily");
    assert!(
        s.needs_preparation(RenderTier::Proxy),
        "nothing prepared yet"
    );
    assert!(s.bytes_resident() >= 1536 * 1023 * 12);

    // A preview already at/below the proxy size is NOT a medium tier.
    let small = RgbImage::new_with(1024, 683, |d| d.fill(0.3)).unwrap();
    let s2 = Session::synthetic("small.CR3", small, None, Arc::new(CpuRenderer));
    assert_eq!(s2.proxy_size(), (1024, 683));
    assert!(!s2.has_medium_tier());
}

/// A slider change re-runs derive + render only: no decode, no orient, no prepare, no finalize.
#[test]
fn slider_changes_rebuild_nothing_heavy() {
    let mut s = session();
    let out = s.render(proxy()).unwrap();
    assert_eq!((out.width, out.height), (1536, 1023));
    assert!(matches!(out.frame, Frame::Rgba8(_)));
    assert_eq!(out.frame.rgba8().unwrap().len(), 1536 * 1023 * 4);
    assert!(out.histogram.is_some());
    assert!(out.density_range > 0.0);
    assert!((out.display_aspect - 1536.0 / 1023.0).abs() < 1e-9);
    assert_eq!(out.content_window, NormalizedRect::FULL);
    assert_eq!(out.tier, RenderTier::Proxy);
    let before = s.stats();
    assert_eq!(before.prepares, 1);
    assert_eq!(before.finalizes, 1);
    assert_eq!(before.source_builds, 1);
    assert!(!s.needs_preparation(RenderTier::Proxy));

    for (i, f) in [0.8, 1.3, 0.95].into_iter().enumerate() {
        s.update_settings(|st| {
            st.density = f;
            st.white_point_offset = 0.01 * (i as f64 + 1.0);
            st.black_point_offset = -0.02;
            st.contrast_mask = 0.0;
        });
        s.render(proxy()).unwrap();
    }
    let after = s.stats();
    assert_eq!(after.prepares, 1, "WP/BP drags never re-run prepare");
    assert_eq!(
        after.finalizes, 1,
        "…nor finalize (offset-independent since 2125a34)"
    );
    assert_eq!(after.orients, before.orients);
    assert_eq!(after.source_builds, 1, "the source is reused");
    assert_eq!(after.preview_decodes, 0);
    assert_eq!(after.renders, 4);
}

/// Orientation changes re-orient and rebuild the render source — but never the decode.
#[test]
fn orientation_change_invalidates_render_source_not_decode() {
    let mut s = session();
    s.render(proxy()).unwrap();
    let before = s.stats();
    s.update_settings(|st| st.rotation = 90);
    let out = s.render(proxy()).unwrap();
    assert_eq!((out.width, out.height), (1023, 1536));
    assert_eq!(out.frame_size, (1023, 1536));
    let after = s.stats();
    assert!(after.orients > before.orients);
    assert_eq!(after.source_builds, before.source_builds + 1);
    assert_eq!(
        after.prepares,
        before.prepares + 1,
        "90° steps re-meter (new meter key)"
    );
    assert_eq!(after.preview_decodes, 0);
    assert_eq!(after.full_decodes, 0);
    assert_eq!(s.bytes_resident() % 4, 0);
}

/// THE invariant: straightening never re-meters. The conversion must not drift as the user
/// rotates, and `needs_preparation` must not flash "Analyzing…" for it.
#[test]
fn fine_rotation_never_re_meters() {
    let mut s = session();
    let flat = s.render(proxy()).unwrap();
    let analysis_flat = s.analysis().unwrap();
    let before = s.stats();
    s.update_settings(|st| st.fine_rotation = 7.5);
    assert!(
        !s.needs_preparation(RenderTier::Proxy),
        "a re-orient is cheap"
    );
    let tilted = s.render(proxy()).unwrap();
    let after = s.stats();
    assert_eq!(after.prepares, before.prepares);
    assert_eq!(after.finalizes, before.finalizes);
    assert_eq!(s.analysis().unwrap(), analysis_flat, "same analysis object");
    assert_eq!(
        after.orients,
        before.orients + 1,
        "one re-orient of the render preview"
    );
    // The render IS rotated: inscribed, smaller, different aspect; density range unchanged.
    assert!(tilted.width < flat.width && tilted.height < flat.height);
    assert_eq!(tilted.density_range, flat.density_range);
    assert!((tilted.display_aspect - flat.display_aspect).abs() > 0.01);
    // The bitmap lands centred, a hair inside the ideal rect (floored inscribed size).
    let w = tilted.content_window;
    assert!(
        w.x >= 0.0 && w.x < 0.002 && w.width <= 1.0 && w.width > 0.996,
        "{w:?}"
    );
    // Back to 0 aliases the meter image again (no new copy beyond the one re-orient).
    s.update_settings(|st| st.fine_rotation = 0.0);
    s.render(proxy()).unwrap();
    assert_eq!(s.stats().orients, after.orients);
}

/// `uncropped` widens what is RENDERED, never what is METERED (the Qt lesson).
#[test]
fn uncropped_does_not_change_the_analysis() {
    let mut s = session();
    s.update_settings(|st| st.crop_rect = Some(NormalizedRect::new(0.25, 0.25, 0.5, 0.5)));
    let cropped = s.render(proxy()).unwrap();
    let a_cropped = s.analysis().unwrap();
    // 0.25…0.75 of 1023 rows: ⌊255.75⌋ = 255 … ⌊767.25⌋ = 767 → 512 (pixel_roi truncates).
    assert_eq!((cropped.width, cropped.height), (768, 512));
    assert!((cropped.display_aspect - (1536.0 * 0.5) / (1023.0 * 0.5)).abs() < 1e-9);
    assert_eq!(s.stats().prepares, 1);

    let wide = s
        .render(RenderRequest {
            uncropped: true,
            ..proxy()
        })
        .unwrap();
    assert_eq!(
        (wide.width, wide.height),
        (1536, 1023),
        "the whole frame is rendered"
    );
    assert_eq!(
        s.analysis().unwrap(),
        a_cropped,
        "…but the meter stayed scoped by the crop"
    );
    assert_eq!(s.stats().prepares, 1);
    assert_eq!(
        s.stats().source_builds,
        2,
        "a second (uncropped) source, the crop kept"
    );
    assert!((wide.display_aspect - 1536.0 / 1023.0).abs() < 1e-9);

    // The tool previews: analysis tool = uncropped at the live angle; crop tool = unrotated.
    s.update_settings(|st| st.fine_rotation = 5.0);
    let analysis_tool = s
        .render_tool_preview(ToolMode::AnalysisRegion, proxy())
        .unwrap();
    assert!(analysis_tool.width < 1536, "fine rotation kept (inscribed)");
    let crop_tool = s.render_tool_preview(ToolMode::Crop, proxy()).unwrap();
    assert_eq!(
        (crop_tool.width, crop_tool.height),
        (1536, 1023),
        "crop tool shows the unrotated frame"
    );
    assert_eq!(s.stats().prepares, 1, "no tool re-metered anything");

    // Changing the crop re-meters (crop scopes the meter) — once.
    s.update_settings(|st| st.crop_rect = Some(NormalizedRect::new(0.1, 0.1, 0.8, 0.8)));
    s.render(proxy()).unwrap();
    assert_eq!(s.stats().prepares, 2);
    assert_ne!(s.analysis().unwrap(), a_cropped);
}

/// A manual analysis region pins the meter to the angle it was drawn at (the Mac keying).
#[test]
fn analysis_region_pins_its_drawn_angle_and_breaks_the_alias() {
    let mut s = session();
    s.update_settings(|st| st.fine_rotation = 4.0);
    s.render(proxy()).unwrap();
    let before = s.stats();
    // Drawn on the 4°-straightened display: the angle rides along.
    s.commit_analysis_region(Some(NormalizedRect::new(0.2, 0.2, 0.6, 0.6)));
    assert_eq!(s.settings().analysis_rect_fine_rotation, 4.0);
    assert!(s.needs_preparation(RenderTier::Proxy));
    s.render(proxy()).unwrap();
    let after = s.stats();
    assert_eq!(after.prepares, before.prepares + 1);
    assert!(
        after.orients > before.orients,
        "the meter image was rebuilt at 4°"
    );
    // Returning the straighten slider to 0 keeps the region's angle pinned: the render preview
    // must NOT alias the (tilted) meter image — it is rendered flat and full-size.
    s.update_settings(|st| st.fine_rotation = 0.0);
    let out = s.render(proxy()).unwrap();
    assert_eq!((out.width, out.height), (1536, 1023));
    assert_eq!(
        s.stats().prepares,
        after.prepares,
        "no re-meter: the region's angle is unchanged"
    );
    // Clearing the region zeroes the angle.
    s.commit_analysis_region(None);
    assert_eq!(s.settings().analysis_rect_fine_rotation, 0.0);
}

#[test]
fn alias_rule_matches_the_swift_decision() {
    assert!(aliases_meter_preview(0.0, 0.0));
    assert!(aliases_meter_preview(0.004, 0.004));
    assert!(aliases_meter_preview(0.005, 0.005));
    assert!(!aliases_meter_preview(0.006, 0.0));
    assert!(!aliases_meter_preview(0.0, 0.006));
    for pinned in [0.5, 3.0, 10.0, -7.5, 45.0] {
        assert!(!aliases_meter_preview(0.0, pinned), "pinned {pinned}");
    }
    for angle in [0.5, 3.0, -10.0, 45.0] {
        assert!(!aliases_meter_preview(angle, 0.0));
        assert!(!aliases_meter_preview(angle, angle));
    }
    // Why it matters: a pinned angle changes the render input's SHAPE.
    let base = RgbImage::new_with(1536, 1024, |d| d.fill(0.5)).unwrap();
    let pinned = base.oriented_with_fine_rotation(0, false, 10.0);
    assert!(pinned.width < 1536 && pinned.height < 1024);
    let flat_aspect = 1536.0 / 1024.0;
    let pinned_aspect = pinned.width as f64 / pinned.height as f64;
    assert!((flat_aspect - pinned_aspect).abs() > 0.1);
}

/// Analysis ALWAYS runs on the proxy: every tier renders the same conversion, and the medium
/// tier never costs a decode or a prepare.
#[test]
fn proxy_analysis_is_identical_across_tiers() {
    let mut s = session();
    let p = s.render(proxy()).unwrap();
    let a = s.analysis().unwrap();
    let m = s
        .render(RenderRequest {
            tier: RenderTier::MediumIfFree,
            ..proxy()
        })
        .unwrap();
    assert_eq!(m.tier, RenderTier::MediumIfFree);
    assert_eq!((m.width, m.height), (1600, 1066));
    assert_eq!(s.stats().full_decodes, 0, "medium never decodes");
    let f = s
        .render(RenderRequest {
            tier: RenderTier::Full,
            ..proxy()
        })
        .unwrap();
    assert_eq!(f.tier, RenderTier::Full);
    assert_eq!((f.width, f.height), (3200, 2132));
    assert_eq!(s.stats().full_decodes, 1);
    assert!(s.has_full_tier());
    assert_eq!(s.stats().prepares, 1, "tier switches never re-analyze");
    assert_eq!(s.analysis().unwrap(), a);
    // Same density range, same continuous aspect on every tier; frame_size is the PROXY's.
    for out in [&p, &m, &f] {
        assert_eq!(out.density_range, p.density_range);
        assert!((out.display_aspect - p.display_aspect).abs() < 1e-12);
        assert_eq!(out.frame_size, (1536, 1023));
        assert_eq!(out.content_window, NormalizedRect::FULL);
    }
    // The same pixel reads the same tone on the proxy and the full tier (a loose gate: the
    // tiers are different resamples of one synthetic frame).
    let (px, py) = (700usize, 500usize);
    let pb = p.frame.rgba8().unwrap();
    let fb = f.frame.rgba8().unwrap();
    let pi = (py * 1536 + px) * 4;
    let fi = ((py * 3200 / 1536) * 3200 + px * 3200 / 1536) * 4;
    for c in 0..3 {
        let d = (pb[pi + c] as i32 - fb[fi + c] as i32).abs();
        assert!(
            d <= 40,
            "channel {c}: proxy {} vs full {}",
            pb[pi + c],
            fb[fi + c]
        );
    }
}

#[test]
fn full_tier_retention_follows_the_hq_rules() {
    let mut s = session();
    s.render(RenderRequest {
        tier: RenderTier::Full,
        ..proxy()
    })
    .unwrap();
    assert!(s.has_full_tier());
    assert!(!s.needs_preparation(RenderTier::Full));
    // A proxy render with retain_full keeps it (auto/on zooming out)…
    s.render(RenderRequest {
        retain_full: true,
        ..proxy()
    })
    .unwrap();
    assert!(s.has_full_tier());
    // …a plain proxy render frees it (HQ off).
    s.render(proxy()).unwrap();
    assert!(!s.has_full_tier());
    assert!(
        s.needs_preparation(RenderTier::Full),
        "the full tier would have to decode"
    );
    assert!(
        !s.needs_preparation(RenderTier::MediumIfFree),
        "medium never flashes Analyzing…"
    );
    // Re-request: decodes again (count 2), then a 90° step marks it stale until re-oriented.
    s.render(RenderRequest {
        tier: RenderTier::Full,
        ..proxy()
    })
    .unwrap();
    assert_eq!(s.stats().full_decodes, 2);
    s.update_settings(|st| st.rotation = 180);
    assert!(s.needs_preparation(RenderTier::Full));
    s.render(RenderRequest {
        tier: RenderTier::Full,
        ..proxy()
    })
    .unwrap();
    assert_eq!(
        s.stats().full_decodes,
        2,
        "orientation change re-orients, never re-decodes"
    );
    // release_enhanced_tiers drops full AND medium; the proxy tower stays warm.
    let prepares = s.stats().prepares;
    let resident = s.bytes_resident();
    s.release_enhanced_tiers();
    assert!(!s.has_full_tier() && !s.has_medium_tier());
    assert!(s.bytes_resident() < resident);
    s.render(proxy()).unwrap();
    assert_eq!(s.stats().prepares, prepares);
    // With no medium tier, MediumIfFree resolves to Full.
    let out = s
        .render(RenderRequest {
            tier: RenderTier::MediumIfFree,
            ..proxy()
        })
        .unwrap();
    assert_eq!(out.tier, RenderTier::Full);
}

/// Export shares the PROXY analysis (the what-you-see invariant) and the full geometry chain.
#[test]
fn export_shares_the_proxy_analysis() {
    let mut s = session();
    s.update_settings(|st| {
        st.rotation = 90;
        st.fine_rotation = 3.0;
        st.crop_rect = Some(NormalizedRect::new(0.1, 0.1, 0.8, 0.8));
    });
    let display = s
        .render(RenderRequest {
            output: OutputKind::Float,
            ..proxy()
        })
        .unwrap();
    let a = s.analysis().unwrap();
    let encoded = s.export_render(false).unwrap();
    assert_eq!(s.stats().prepares, 1, "export ran no analysis of its own");
    assert_eq!(s.analysis().unwrap(), a);
    assert!(!s.has_full_tier(), "a fresh export decode is not retained");
    // Geometry: full (3200×2132) → rotated 90° (2132×3200) → inscribed at 3° → 80 % crop.
    let (iw, ih) = RgbImage::inscribed_rect_size(2132.0, 3200.0, 3f64.to_radians());
    let roi = NormalizedRect::new(0.1, 0.1, 0.8, 0.8)
        .pixel_roi(iw.floor() as usize, ih.floor() as usize)
        .unwrap();
    assert_eq!((encoded.width, encoded.height), (roi.width(), roi.height()));
    let ratio = encoded.width as f64 / encoded.height as f64;
    assert!(
        (ratio - display.display_aspect).abs() < 0.01,
        "{ratio} vs {}",
        display.display_aspect
    );
    // Values are encoded (in [0, 1]) and the sRGB leg changes them.
    assert!(encoded.data.iter().all(|v| (0.0..=1.0).contains(v)));
    let srgb = s.export_render(true).unwrap();
    assert_eq!((srgb.width, srgb.height), (encoded.width, encoded.height));
    assert_ne!(srgb.data, encoded.data);
    // A retained HQ tier is reused by export instead of a fresh decode.
    s.render(RenderRequest {
        tier: RenderTier::Full,
        ..proxy()
    })
    .unwrap();
    let decodes = s.stats().full_decodes;
    s.export_render(false).unwrap();
    assert_eq!(s.stats().full_decodes, decodes);
}

/// Display rgba8 is the float path quantized (within the GPU-path gate of 1.5/255 → here exact
/// nearest rounding).
#[test]
fn display_path_is_the_float_path_quantized() {
    let mut s = session();
    let f = s
        .render(RenderRequest {
            output: OutputKind::Float,
            srgb: true,
            ..proxy()
        })
        .unwrap();
    let d = s
        .render(RenderRequest {
            srgb: true,
            ..proxy()
        })
        .unwrap();
    let img = f.frame.image().unwrap();
    let bytes = d.frame.rgba8().unwrap();
    assert_eq!(f.histogram, d.histogram);
    for (i, px) in img.data.chunks_exact(3).enumerate().step_by(997) {
        for c in 0..3 {
            let want = (px[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            assert_eq!(bytes[i * 4 + c], want);
        }
        assert_eq!(bytes[i * 4 + 3], 255);
    }
    // Histogram: 4 × 256 raw counts summing to the pixel count per channel.
    let h = d.histogram.unwrap();
    for ch in 0..4 {
        let total: u64 = h[ch * 256..(ch + 1) * 256]
            .iter()
            .map(|&c| u64::from(c))
            .sum();
        assert_eq!(total, (1536 * 1023) as u64);
    }
}

#[test]
fn session_info_zone_pin_and_detached_render() {
    let mut s = session();
    let info = s.session_info().unwrap();
    assert_eq!((info.width, info.height), (1536, 1023));
    assert!(info.density_range > 0.0);
    assert!(info.character.is_some());
    assert!((info.default_grade_range - negkit::curve_logic::default_grade_range()).abs() < 1e-12);
    assert!(info.anchor > 0.0 && info.anchor < 1.0);

    let pin = s.sample_zone_pin(0.5, 0.5, 2).unwrap();
    assert!(pin.val_luma.is_finite());
    assert_eq!(pin.mask_val, 0.0, "mask off");
    let expected = 0.2126 * pin.val_rgb[0] + 0.7152 * pin.val_rgb[1] + 0.0722 * pin.val_rgb[2];
    assert!((pin.val_luma - expected).abs() < 1e-12);
    // Edge pins clamp rather than fail.
    assert!(s.sample_zone_pin(1.0, 1.0, 2).is_ok());
    assert!(s.sample_zone_pin(-0.5, 2.0, 2).is_ok());

    // Detached render: the straighten 0° base touches no cache state.
    s.update_settings(|st| st.fine_rotation = 6.0);
    s.render(proxy()).unwrap();
    let stats = s.stats();
    let mut zero = s.settings().clone();
    zero.fine_rotation = 0.0;
    let out = s.render_detached(&zero, false).unwrap();
    assert_eq!((out.width, out.height), (1536, 1023));
    let after = s.stats();
    assert_eq!(
        after.prepares, stats.prepares,
        "reused the cached analysis read-only"
    );
    assert_eq!(
        after.source_builds, stats.source_builds,
        "no cache slot touched"
    );
    assert_eq!(after.orients, stats.orients);
    // The committed orientation is still warm: the next render re-orients nothing.
    s.render(proxy()).unwrap();
    assert_eq!(s.stats().orients, stats.orients);
}

/// With the mask on, the key plumbing runs (and renders) even though the plane builder is the
/// TODO(mask) hook: no plane → unmasked render, no crash, no spurious analysis.
#[test]
fn contrast_mask_on_renders_through_the_hook() {
    let mut s = session();
    s.render(proxy()).unwrap();
    s.update_settings(|st| st.contrast_mask = 0.3);
    let out = s.render(proxy()).unwrap();
    assert_eq!(out.width, 1536);
    assert_eq!(s.stats().prepares, 1);
    let pin = s.sample_zone_pin(0.5, 0.5, 2).unwrap();
    assert_eq!(pin.mask_val, 0.0);
}

#[test]
fn baseline_settings_keep_only_the_frame_geometry() {
    let mut s = ExposureSettings::default();
    s.grade = 90.0;
    s.rotation = 90;
    s.flip_horizontal = true;
    s.fine_rotation = 3.0;
    s.crop_rect = Some(NormalizedRect::new(0.1, 0.1, 0.5, 0.5));
    s.analysis_rect = Some(NormalizedRect::new(0.2, 0.2, 0.5, 0.5));
    s.analysis_rect_fine_rotation = 3.0;
    let b = baseline_settings(&s);
    assert_eq!(b.grade, 115.0);
    assert_eq!(b.rotation, 90);
    assert!(b.flip_horizontal);
    assert_eq!(
        b.fine_rotation, 0.0,
        "the Mac's baseline drops the straighten"
    );
    assert_eq!(b.crop_rect, s.crop_rect);
    assert_eq!(b.analysis_rect, s.analysis_rect);
}

// ───────────────────────────── HQModeTests ─────────────────────────────

fn resolve(
    mode: HqMode,
    zoom: f64,
    has_selection: bool,
    proxy_tool: bool,
    crop_tool: bool,
) -> bool {
    mode.resolve(
        zoom,
        HQ_AUTO_ZOOM_THRESHOLD,
        has_selection,
        proxy_tool,
        crop_tool,
    )
}

#[test]
fn hq_mode_resolution() {
    for zoom in [0.5, 1.0, 1.99, 2.0, 4.0, 8.0] {
        assert!(!resolve(HqMode::Off, zoom, true, false, false));
        assert!(resolve(HqMode::On, zoom, true, false, false));
    }
    assert!(!resolve(HqMode::Auto, 1.0, true, false, false));
    assert!(!resolve(HqMode::Auto, 1.99, true, false, false));
    assert!(
        resolve(HqMode::Auto, 2.0, true, false, false),
        "the threshold itself engages"
    );
    assert!(resolve(HqMode::Auto, 8.0, true, false, false));
    assert_eq!(HqMode::ALL, [HqMode::Off, HqMode::Auto, HqMode::On]);
    assert_eq!(HqMode::default(), HqMode::Auto);
    assert_eq!(HqMode::Off.next(), HqMode::Auto);
    assert_eq!(HqMode::Auto.next(), HqMode::On);
    assert_eq!(HqMode::On.next(), HqMode::Off);
    for mode in HqMode::ALL {
        assert_eq!(mode.next().next().next(), mode);
        assert!(
            !resolve(mode, 8.0, true, true, false),
            "proxy-owned canvas suppresses {mode:?}"
        );
        assert!(
            !resolve(mode, 8.0, false, false, false),
            "no selection suppresses {mode:?}"
        );
    }
    assert!(!resolve(HqMode::Auto, 8.0, true, false, true));
    assert!(resolve(HqMode::On, 8.0, true, false, true));
    assert!(!resolve(HqMode::Off, 8.0, true, false, true));
}

#[test]
fn hq_mode_chooses_the_right_source() {
    assert_eq!(HqMode::Off.render_tier(false), RenderTier::Proxy);
    assert_eq!(HqMode::Auto.render_tier(false), RenderTier::Proxy);
    assert_eq!(HqMode::On.render_tier(false), RenderTier::Proxy);
    assert_eq!(HqMode::Auto.render_tier(true), RenderTier::MediumIfFree);
    assert_eq!(HqMode::On.render_tier(true), RenderTier::Full);
    assert_eq!(HqMode::Off.render_tier(true), RenderTier::MediumIfFree);
    for active in [true, false] {
        assert_ne!(HqMode::Auto.render_tier(active), RenderTier::Full);
    }
    // Must sit above fit (or Auto is just On) and within the gesture's range.
    const { assert!(HQ_AUTO_ZOOM_THRESHOLD > 1.0 && HQ_AUTO_ZOOM_THRESHOLD <= 8.0) };
}
