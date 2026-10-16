//! Ports of `CropGeometryTests.swift`, `DisplayAspectTests.swift` and
//! `AuditRectAndGeometryTests.fitScaleIsZeroWhenBoxCenterOutsideRotatedFrame` — the unified
//! Crop & Straighten geometry and the tier-swap stability layers (`display_aspect` /
//! `content_window`), including the cases that pin that the OLD bitmap-derived behaviours
//! really did drift, so the equality tests can't go vacuous.

use negkit::crop_geometry::{self as cg, PixelWindow};
use negkit::rect::PixelRoi;
use negkit::{NormalizedRect, RgbImage, V2};
use std::f64::consts::PI;

const FRAME: V2 = [3000.0, 2000.0];

fn deg(d: f64) -> f64 {
    d * PI / 180.0
}

fn length(a: V2, b: V2) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NormalizedRect {
    NormalizedRect::new(x, y, w, h)
}

// MARK: - CropGeometryTests

#[test]
fn zero_angle_is_identity() {
    // At θ=0 the inscribed rect is the frame; a normalized rect round-trips exactly.
    let r = rect(0.1, 0.2, 0.5, 0.6);
    let b = cg::box_from_rect(&r, FRAME, 0.0);
    let back = cg::normalized_rect(b.center, b.half_extents, FRAME, 0.0);
    assert!((back.x - r.x).abs() < 1e-9 && (back.width - r.width).abs() < 1e-9);
    assert!((back.y - r.y).abs() < 1e-9 && (back.height - r.height).abs() < 1e-9);
}

#[test]
fn full_inscribed_box_always_fits() {
    for d in [0.0, 3.0, -7.5, 20.0, -45.0] {
        let radians = deg(d);
        let ins = cg::inscribed_size(FRAME, radians);
        assert!(
            cg::box_fits(
                [0.0, 0.0],
                [ins[0] / 2.0, ins[1] / 2.0],
                radians,
                FRAME,
                1e-3
            ),
            "inscribed rect must fit at {d}°"
        );
    }
}

#[test]
fn fit_scale_shrinks_only_when_needed() {
    let radians = deg(5.0);
    // A tiny centered box is unaffected.
    let small = cg::fit_scale([0.0, 0.0], [100.0, 80.0], radians, FRAME, 1.0);
    assert_eq!(small, 1.0);
    // The full frame at 5° must shrink.
    let half = [FRAME[0] / 2.0, FRAME[1] / 2.0];
    let big = cg::fit_scale([0.0, 0.0], half, radians, FRAME, 1.0);
    assert!(big < 1.0 && big > 0.5);
    // And the shrunk result actually fits.
    assert!(cg::box_fits(
        [0.0, 0.0],
        [half[0] * big, half[1] * big],
        radians,
        FRAME,
        1e-6
    ));
}

#[test]
fn constrained_box_always_fits() {
    // Off-center boxes at assorted angles: after constrain, always inside.
    for d in [2.0, -10.0, 25.0, -40.0] {
        let radians = deg(d);
        let b = cg::constrain([1200.0, -700.0], [600.0, 500.0], radians, FRAME, 1.0);
        assert!(cg::box_fits(
            b.center,
            b.half_extents,
            radians,
            FRAME,
            1e-6
        ));
        assert!(b.half_extents[0] > 0.0 && b.half_extents[1] > 0.0);
    }
}

#[test]
fn remap_is_identity_at_same_angle() {
    let r = rect(0.2, 0.1, 0.5, 0.7);
    let radians = deg(8.0);
    let out = cg::remap_crop(&r, radians, radians, FRAME);
    assert!((out.x - r.x).abs() < 1e-6 && (out.y - r.y).abs() < 1e-6);
    assert!((out.width - r.width).abs() < 1e-6 && (out.height - r.height).abs() < 1e-6);
}

#[test]
fn remap_preserves_content_center() {
    // The frame point under the crop center must stay under it.
    let r = rect(0.3, 0.25, 0.3, 0.4);
    let a = deg(4.0);
    let b = deg(-6.0);
    let before = cg::box_from_rect(&r, FRAME, a);
    let content_before = cg::rotate(before.center, -a);

    let remapped = cg::remap_crop(&r, a, b, FRAME);
    let after = cg::box_from_rect(&remapped, FRAME, b);
    let content_after = cg::rotate(after.center, -b);
    assert!(length(content_before, content_after) < 1.0); // within a pixel

    // And the remapped crop is representable (fits its rotation).
    assert!(cg::box_fits(
        after.center,
        after.half_extents,
        b,
        FRAME,
        1e-3
    ));
}

#[test]
fn remap_round_trip_keeps_size() {
    // a → b → a returns to the original size when nothing forced a shrink.
    let r = rect(0.35, 0.3, 0.3, 0.35);
    let a = deg(3.0);
    let b = deg(-3.0);
    let there = cg::remap_crop(&r, a, b, FRAME);
    let back = cg::remap_crop(&there, b, a, FRAME);
    let origin = cg::box_from_rect(&r, FRAME, a);
    let fin = cg::box_from_rect(&back, FRAME, a);
    assert!(length(origin.half_extents, fin.half_extents) < 1.0);
    assert!(length(origin.center, fin.center) < 1.0);
}

// MARK: - AuditRectAndGeometryTests (crop part)

#[test]
fn fit_scale_is_zero_when_box_center_outside_rotated_frame() {
    let frame = [100.0, 80.0];
    // θ = 0: the rotated frame is ±(50, 40); center (60, 0) is outside on x. For the +x
    // corners, t.x = +hx > 0 and limit = (50 − 60)/hx < 0, so k = min(k, max(limit, 0)) = 0.
    assert_eq!(cg::fit_scale([60.0, 0.0], [10.0, 10.0], 0.0, frame, 1.0), 0.0);
    // Rotated case: center far outside every possible bound.
    assert_eq!(
        cg::fit_scale([200.0, 200.0], [5.0, 5.0], 0.3, frame, 1.0),
        0.0
    );
    // A zero-extent box through normalized_rect produces a degenerate NormalizedRect that
    // pixel_roi must reject (None), not crash.
    let degenerate = cg::normalized_rect([0.0, 0.0], [0.0, 0.0], frame, 0.0);
    assert!(degenerate.pixel_roi(100, 80).is_none());
}

// MARK: - DisplayAspectTests

/// Proxy and full-resolution dimensions for a 24MP frame: the preview is decoded half-size
/// (3012×2010) then capped at 1536 on the long edge.
const PROXY: V2 = [1536.0, 1025.0];
/// The retained half-size buffer the preview decode already produced.
const MEDIUM: V2 = [3012.0, 2010.0];
const FULL: V2 = [6024.0, 4020.0];

/// What the OLD code laid out from — the cropped/rotated bitmap's real pixel ratio, rounded
/// exactly as the pipeline rounds it.
fn bitmap_aspect(frame: V2, radians: f64, crop: Option<&NormalizedRect>) -> f64 {
    let (mut w, mut h) = (frame[0] as usize, frame[1] as usize);
    if radians.abs() > 1e-9 {
        let (iw, ih) = RgbImage::inscribed_rect_size(w as f64, h as f64, radians);
        w = (iw.floor() as usize).max(1);
        h = (ih.floor() as usize).max(1);
    }
    if let Some(roi) = crop.and_then(|c| c.pixel_roi(w, h)) {
        w = roi.width();
        h = roi.height();
    }
    w as f64 / h as f64
}

fn crops() -> Vec<Option<NormalizedRect>> {
    vec![
        None,
        Some(rect(0.0, 0.0, 1.0, 1.0)),
        Some(rect(0.1, 0.08, 0.8, 0.84)),
        Some(rect(0.05, 0.05, 0.9, 0.9)),
        Some(rect(0.33, 0.25, 0.34, 0.5)),
        Some(rect(0.2013, 0.1077, 0.6431, 0.7752)),
    ]
}

const ANGLES: [f64; 6] = [0.0, 0.4, -1.3, 2.7, -5.0, 12.0];

/// THE invariant: both tiers must lay out identically. The proxy and the full-resolution
/// render describe the same continuous rectangle, so the aspect must not depend on which one
/// produced the pixels.
#[test]
fn aspect_is_identical_for_proxy_and_full_resolution() {
    for crop in crops() {
        for d in ANGLES {
            let radians = deg(d);
            let p = cg::display_aspect(PROXY, radians, crop.as_ref());
            let f = cg::display_aspect(FULL, radians, crop.as_ref());
            // The two bases differ by the proxy's own 1.9e-5 rounding (1536/1025 vs
            // 6024/4020); everything downstream is continuous, so nothing may amplify it.
            assert!(
                (p - f).abs() / f < 5e-5,
                "crop {crop:?} at {d}°: proxy {p} vs full {f}"
            );
        }
    }
}

/// The bug, pinned: the old bitmap-derived layout really did disagree between tiers, by far
/// more than the fix's tolerance. If this ever stops holding, the test above has become
/// vacuous.
#[test]
fn bitmap_derived_aspect_disagreed_between_tiers() {
    let crop = rect(0.33, 0.25, 0.34, 0.5);
    let p = bitmap_aspect(PROXY, 0.0, Some(&crop));
    let f = bitmap_aspect(FULL, 0.0, Some(&crop));
    // ~2e-3 — at 1200pt fitted and 4x zoom that is several points of jump.
    assert!((p - f).abs() / f > 1e-3);
    // And the fix removes it for that same crop.
    let pf = cg::display_aspect(PROXY, 0.0, Some(&crop));
    let ff = cg::display_aspect(FULL, 0.0, Some(&crop));
    assert!((pf - ff).abs() / ff < 5e-5);
}

/// The layout aspect must still describe the picture the render produced — close enough to
/// each tier's true bitmap ratio that filling the rect is a sub-pixel stretch rather than a
/// visible distortion.
#[test]
fn aspect_stays_close_to_each_tiers_real_bitmap() {
    for crop in crops() {
        for d in ANGLES {
            let radians = deg(d);
            for frame in [PROXY, FULL] {
                let ideal = cg::display_aspect(frame, radians, crop.as_ref());
                let real = bitmap_aspect(frame, radians, crop.as_ref());
                assert!(
                    (ideal - real).abs() / real < 3e-3,
                    "crop {crop:?} at {d}° frame {frame:?}"
                );
            }
        }
    }
}

// MARK: Plain correctness

#[test]
fn uncropped_unrotated_is_the_frame_aspect() {
    assert_eq!(cg::display_aspect(PROXY, 0.0, None), 1536.0 / 1025.0);
    let square = [1000.0, 1000.0];
    assert_eq!(cg::display_aspect(square, 0.0, None), 1.0);
}

#[test]
fn crop_scales_the_aspect_by_its_own_ratio() {
    let frame = [1000.0, 1000.0];
    // A half-width, full-height crop of a square is 2:1 tall — aspect 0.5.
    let tall = rect(0.25, 0.0, 0.5, 1.0);
    assert!((cg::display_aspect(frame, 0.0, Some(&tall)) - 0.5).abs() < 1e-12);
    let wide = rect(0.0, 0.25, 1.0, 0.5);
    assert!((cg::display_aspect(frame, 0.0, Some(&wide)) - 2.0).abs() < 1e-12);
}

/// A crop is normalized over the INSCRIBED rect, so rotation and crop compose: the crop's
/// ratio multiplies whatever straightening left.
#[test]
fn rotation_and_crop_compose() {
    let radians = deg(5.0);
    let inscribed = cg::inscribed_size(PROXY, radians);
    let crop = rect(0.1, 0.1, 0.6, 0.7);
    let expected = (inscribed[0] * 0.6) / (inscribed[1] * 0.7);
    assert!((cg::display_aspect(PROXY, radians, Some(&crop)) - expected).abs() < 1e-12);
}

/// Straightening a landscape frame narrows it toward square — the inscribed rect loses more
/// width than height.
#[test]
fn rotation_moves_aspect_toward_square() {
    let flat = cg::display_aspect(PROXY, 0.0, None);
    let mut previous = flat;
    for d in [1.0, 3.0, 6.0, 10.0] {
        let a = cg::display_aspect(PROXY, deg(d), None);
        assert!(a > previous, "{d}° should be narrower than the last");
        previous = a;
    }
    // Sign of the angle can't matter: ±θ inscribe the same rectangle.
    for d in [0.4, 2.7, 9.0] {
        let plus = cg::display_aspect(PROXY, deg(d), None);
        let minus = cg::display_aspect(PROXY, deg(-d), None);
        assert!((plus - minus).abs() < 1e-12, "{d}°");
    }
}

/// `cropped_to_pixels` is the primitive the anchoring rides on: it must select exactly the
/// requested window and refuse degenerate ones.
#[test]
fn cropped_to_pixels_selects_exactly_the_requested_window() {
    let img = RgbImage::new_with(20, 10, |buf| {
        for y in 0..10 {
            for x in 0..20 {
                buf[(y * 20 + x) * 3] = (y * 20 + x) as f32;
            }
        }
    })
    .unwrap();
    let out = img.cropped_to_pixels(PixelRoi {
        x0: 3,
        y0: 2,
        x1: 11,
        y1: 8,
    });
    assert_eq!((out.width, out.height), (8, 6));
    assert_eq!(out.pixel(0, 0).unwrap()[0], (2 * 20 + 3) as f32);
    assert_eq!(out.pixel(7, 5).unwrap()[0], (7 * 20 + 10) as f32);

    // Out-of-range clamps rather than trapping.
    let clamped = img.cropped_to_pixels(PixelRoi {
        x0: 0,
        y0: 0,
        x1: 999,
        y1: 999,
    });
    assert_eq!((clamped.width, clamped.height), (20, 10));
    // Degenerate returns self untouched.
    let degenerate = img.cropped_to_pixels(PixelRoi {
        x0: 5,
        y0: 5,
        x1: 6,
        y1: 6,
    });
    assert_eq!((degenerate.width, degenerate.height), (20, 10));
}

// MARK: The end-to-end invariant: same content, same screen position

/// Simulate a tier all the way to the screen: floor the inscribed rect and the crop ROI
/// exactly as the pipeline does, then ask where a given point of the FRAME lands in the
/// fitted rect.
///
/// `f` is in ideal-inscribed-rect coordinates (0…1 across the straightened frame). Returns
/// the screen x/y in points.
fn screen_position(f: V2, frame: V2, radians: f64, crop: Option<&NormalizedRect>) -> V2 {
    let fitted_width = 1200.0;
    let inscribed = cg::inscribed_size(frame, radians);
    // The oriented bitmap: inscribed rect floored to whole pixels.
    let ow = (inscribed[0].floor() as usize).max(1) as f64;
    let oh = (inscribed[1].floor() as usize).max(1) as f64;

    // Each tier crops independently on its own grid — no anchoring. The placement must be
    // exact anyway.
    let mut roi: Option<PixelWindow> = None;
    if let Some(crop) = crop {
        let Some(p) = crop.pixel_roi(ow as usize, oh as usize) else {
            return [0.0, 0.0];
        };
        roi = Some([p.x0 as f64, p.y0 as f64, p.x1 as f64, p.y1 as f64]);
    }

    // Each tier measures its own bitmap against its OWN frame...
    let cw = cg::content_window(frame, radians, crop, [ow, oh], roi);
    // ...but the canvas is laid out from ONE aspect for both, which the app takes from the
    // proxy (`orientedFrameSize` always reads basePreview).
    let aspect = cg::display_aspect(PROXY, radians, crop);
    let fitted = [fitted_width, fitted_width / aspect];

    // The bitmap's screen rect, placed by its real window.
    let origin_pt = [fitted[0] * cw.x, fitted[1] * cw.y];
    let size_pt = [fitted[0] * cw.width, fitted[1] * cw.height];

    // Where `f` sits inside the bitmap, as a fraction of it.
    let ideal = crop.copied().unwrap_or(NormalizedRect::FULL);
    // f in ideal-cropped units, then into bitmap units via the window.
    let in_cropped = [
        (f[0] - ideal.x) / ideal.width,
        (f[1] - ideal.y) / ideal.height,
    ];
    let p = [
        (in_cropped[0] - cw.x) / cw.width,
        (in_cropped[1] - cw.y) / cw.height,
    ];
    [
        origin_pt[0] + size_pt[0] * p[0],
        origin_pt[1] + size_pt[1] * p[1],
    ]
}

/// THE invariant the whole fix exists for: a given point of the picture must land on the
/// same pixel of the screen whether the proxy or the full-resolution tier produced it.
/// Before the fix this drifted by several points at zoom; placing each bitmap by its true
/// window makes the two tiers resolve to the SAME affine frame→screen map, so it is exact.
#[test]
fn same_content_lands_at_the_same_screen_position_in_both_tiers() {
    for crop in crops() {
        for d in ANGLES {
            let radians = deg(d);
            // Sample the frame, including the corners of the crop.
            for f in [[0.2, 0.2], [0.5, 0.5], [0.8, 0.75], [0.35, 0.62]] {
                let p = screen_position(f, PROXY, radians, crop.as_ref());
                let m = screen_position(f, MEDIUM, radians, crop.as_ref());
                let h = screen_position(f, FULL, radians, crop.as_ref());
                // All THREE tiers must agree — Auto swaps proxy↔medium and On swaps
                // proxy↔full, so any pair can meet on screen.
                let drift = [
                    (p[0] - h[0]).abs(),
                    (p[1] - h[1]).abs(),
                    (p[0] - m[0]).abs(),
                    (p[1] - m[1]).abs(),
                    (m[0] - h[0]).abs(),
                    (m[1] - h[1]).abs(),
                ]
                .into_iter()
                .fold(0.0f64, f64::max);
                // Float noise, not geometry: ~1e-4 pt on a 1200 pt canvas is a billionth
                // of the frame. The user-visible figure this replaces was several POINTS.
                assert!(
                    drift < 1e-3,
                    "{crop:?} at {d}°, point {f:?}: drifted {drift} pt"
                );
            }
        }
    }
}

/// The fix must beat the previous behaviour by orders of magnitude, not marginally —
/// otherwise the tolerance above is doing the work. Compares placing each bitmap by its
/// window against stretching both to fill.
#[test]
fn placing_beats_filling_by_orders_of_magnitude() {
    let crop = rect(0.33, 0.25, 0.34, 0.5);
    let f = [0.5, 0.5];

    // Filling: f maps through each bitmap's own window onto the same rect.
    let filled = |frame: V2| -> f64 {
        let (ow, oh) = (frame[0], frame[1]);
        let Some(roi) = crop.pixel_roi(ow as usize, oh as usize) else {
            return 0.0;
        };
        let ax = roi.x0 as f64 / ow;
        let aw = roi.width() as f64 / ow;
        1200.0 * ((f[0] - ax) / aw)
    };
    let fill_drift = (filled(PROXY) - filled(FULL)).abs();

    let place_drift = (screen_position(f, PROXY, 0.0, Some(&crop))[0]
        - screen_position(f, FULL, 0.0, Some(&crop))[0])
        .abs();

    assert!(
        fill_drift > 0.01,
        "filling should still drift; got {fill_drift} pt"
    );
    assert!(
        place_drift < fill_drift / 100.0,
        "placing ({place_drift} pt) should be <1% of filling ({fill_drift} pt)"
    );
}

#[test]
fn content_window_is_the_unit_rect_when_nothing_quantizes() {
    // No rotation and no crop: the bitmap IS the ideal rect.
    let cw = cg::content_window(PROXY, 0.0, None, PROXY, None);
    assert!(cw.x.abs() < 1e-12 && cw.y.abs() < 1e-12);
    assert!((cw.width - 1.0).abs() < 1e-12 && (cw.height - 1.0).abs() < 1e-12);
}

#[test]
fn content_window_stays_near_the_unit_rect() {
    // Whatever the settings, the bitmap is at most a pixel off the ideal — a window far from
    // (0,0,1,1) would mean the algebra is wrong.
    for crop in crops() {
        for d in ANGLES {
            let radians = deg(d);
            let inscribed = cg::inscribed_size(PROXY, radians);
            let ow = (inscribed[0].floor() as usize).max(1) as f64;
            let oh = (inscribed[1].floor() as usize).max(1) as f64;
            let roi: Option<PixelWindow> = crop
                .as_ref()
                .and_then(|c| c.pixel_roi(ow as usize, oh as usize))
                .map(|p| [p.x0 as f64, p.y0 as f64, p.x1 as f64, p.y1 as f64]);
            let cw = cg::content_window(PROXY, radians, crop.as_ref(), [ow, oh], roi);
            assert!(cw.x.abs() < 0.01 && cw.y.abs() < 0.01, "origin {cw:?} for {d}°");
            assert!(
                (cw.width - 1.0).abs() < 0.01 && (cw.height - 1.0).abs() < 0.01,
                "size {cw:?} for {d}°"
            );
        }
    }
}

#[test]
fn degenerate_inputs_fall_back_rather_than_producing_garbage() {
    let zero_crop = rect(0.5, 0.5, 0.0, 0.0);
    let a = cg::display_aspect(PROXY, 0.0, Some(&zero_crop));
    assert!(a.is_finite() && a > 0.0);
    let degenerate_frame = [0.0, 0.0];
    let b = cg::display_aspect(degenerate_frame, 0.0, None);
    assert!(b.is_finite() && b > 0.0);
    // And content_window on a degenerate frame is the unit rect, not NaN.
    let cw = cg::content_window(degenerate_frame, 0.0, None, [0.0, 0.0], None);
    assert_eq!(cw, NormalizedRect::FULL);
}
