//! Ports of `OrientationTests.swift`, `PreprocessRectTests.swift`, the rect parts of
//! `AuditRectAndGeometryTests.swift` and the `downsampled` parts of
//! `ImagePipelineSeamTests.swift`.
//!
//! Left for other owners (same Swift files):
//! - `OrientationTests.sidecarRoundTripsOrientation`, `PreprocessRectTests.sidecarRoundTripsRects`
//!   → `tests/sidecar.rs` (settings codec).
//! - `PreprocessRectTests.analysisRectEqualsPreCroppedWithZeroBuffer` /
//!   `cropRectScopesAnalysisWithBufferInside` / `analysisRectWinsOverCrop` /
//!   `degenerateRectIsIgnored` and `AuditRectAndGeometryTests.prepareWithOffFrameCropRectDoesNotTrap`
//!   compare whole `ExposureKernel.analyze` / `prepare` results → `tests/analysis.rs` (agent B).
//!   Their region-resolution core (`resolve_analysis_region` + `prefilter_log_grid`) is pinned
//!   here instead, so the priority rules are tested where they live.
//! - `AuditRectAndGeometryTests.fitScaleIsZeroWhenBoxCenterOutsideRotatedFrame` and the whole of
//!   `CropGeometryTests` / `DisplayAspectTests` → wave 3 (`crop_geometry`).
//! - `ImagePipelineSeamTests.reusedPreparedMatchesFreshAnalysis` /
//!   `neutralAxisIsPreTrimAndOffsetsStillReachTheRender` → `tests/analysis.rs` (agent B).

use negkit::image::AreaTaps;
use negkit::prefilter;
use negkit::rect::{NormalizedRect, PixelRoi};
use negkit::{RgbImage, fixtures};

// MARK: - helpers

/// `img[y, x, c]` in the Swift subscript order.
fn px(img: &RgbImage, y: usize, x: usize, c: usize) -> f32 {
    img.pixel(x, y).expect("in range")[c]
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

/// The shared bit-exact generator (`negkit::fixtures::synthetic_grid_input`).
fn synthetic_grid_input() -> RgbImage {
    fixtures::synthetic_grid_input().expect("generator")
}

fn synthetic64_input() -> RgbImage {
    fixtures::read_rgb("synthetic64/input.bin", 64, 64).expect("fixture")
}

// MARK: - OrientationTests

/// 2×3 test image with unique per-pixel values.
fn probe() -> RgbImage {
    filled(3, 2, |y, x, c| (y * 100 + x * 10 + c) as f32)
}

#[test]
fn rotate_90_cw() {
    let out = probe().oriented(90, false).into_owned();
    assert!(out.width == 2 && out.height == 3);
    // Top-left of a 90° CW rotation is the source's bottom-left.
    assert_eq!(px(&out, 0, 0, 0), px(&probe(), 1, 0, 0));
    assert_eq!(px(&out, 0, 1, 0), px(&probe(), 0, 0, 0));
    assert_eq!(px(&out, 2, 0, 0), px(&probe(), 1, 2, 0));
}

#[test]
fn rotate_270_is_inverse_of_90() {
    let img = probe();
    let round_trip = img.oriented(90, false).oriented(270, false).into_owned();
    assert_eq!(round_trip.data, img.data);
}

#[test]
fn rotate_180() {
    let img = probe();
    let out = img.oriented(180, false).into_owned();
    assert!(out.width == 3 && out.height == 2);
    assert_eq!(px(&out, 0, 0, 1), px(&img, 1, 2, 1));
}

#[test]
fn flip_horizontal() {
    let img = probe();
    let out = img.oriented(0, true).into_owned();
    assert_eq!(px(&out, 0, 0, 0), px(&img, 0, 2, 0));
    assert_eq!(px(&out, 1, 2, 2), px(&img, 1, 0, 2));
    // Involution.
    assert_eq!(out.flipped_horizontally().data, img.data);
}

#[test]
fn negative_and_wrapped_rotations() {
    let img = probe();
    assert_eq!(img.oriented(-90, false).data, img.oriented(270, false).data);
    assert_eq!(img.oriented(360, false).data, img.data);
}

#[test]
fn dcraw_flip_codes_match_the_user_rotations() {
    // applying_flip is the primitive `oriented` composes: 6 = 90° CW, 3 = 180°, 5 = 90° CCW,
    // anything else untouched (borrowed).
    let img = probe();
    assert_eq!(img.applying_flip(6).data, img.oriented(90, false).data);
    assert_eq!(img.applying_flip(3).data, img.oriented(180, false).data);
    assert_eq!(img.applying_flip(5).data, img.oriented(270, false).data);
    assert!(matches!(
        img.applying_flip(0),
        std::borrow::Cow::Borrowed(_)
    ));
    assert!(matches!(
        img.applying_flip(7),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn fine_rotation_zero_is_identity() {
    let img = synthetic_grid_input();
    let out = img.oriented_with_fine_rotation(0, false, 0.0);
    assert_eq!(out.data, img.data);
    // Below the 0.005° threshold too — no resample for a straighten slider at rest.
    assert!(matches!(
        img.oriented_with_fine_rotation(0, false, 0.004),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn fine_rotation_crops_to_inscribed_rect() {
    let img = synthetic_grid_input(); // 1600×1066
    let degrees = 3.5;
    let out = img.fine_rotated(degrees).into_owned();
    let (ew, eh) = RgbImage::inscribed_rect_size(
        img.width as f64,
        img.height as f64,
        degrees * std::f64::consts::PI / 180.0,
    );
    assert!(
        (out.width as f64 - ew).abs() <= 1.0,
        "width {} vs {ew}",
        out.width
    );
    assert!(
        (out.height as f64 - eh).abs() <= 1.0,
        "height {} vs {eh}",
        out.height
    );
    // Smaller than the source in both dimensions (corners cropped away).
    assert!(out.width < img.width && out.height < img.height);
    // Floored on this tier's grid (never rounded up: no empty corner may leak in).
    assert_eq!(out.width as f64, ew.floor());
    assert_eq!(out.height as f64, eh.floor());
}

#[test]
fn fine_rotation_of_uniform_image_stays_uniform() {
    let img = filled(300, 200, |_, _, _| 0.42);
    let out = img.fine_rotated(-7.3).into_owned();
    // Bilinear resampling of a constant field is exact; no empty corners may leak in (they'd
    // show as 0).
    for v in &out.data {
        assert!((v - 0.42).abs() < 1e-5);
    }
}

#[test]
fn inscribed_rect_size_closed_form_corners() {
    // Zero angle: the whole frame.
    let (w, h) = RgbImage::inscribed_rect_size(1600.0, 1066.0, 0.0);
    assert!((w - 1600.0).abs() < 1e-9 && (h - 1066.0).abs() < 1e-9);
    // 45° on a square: the half-constrained branch, side = s/√2.
    let (w, h) = RgbImage::inscribed_rect_size(100.0, 100.0, std::f64::consts::FRAC_PI_4);
    assert!((w - 100.0 / 2f64.sqrt()).abs() < 1e-9);
    assert!((h - 100.0 / 2f64.sqrt()).abs() < 1e-9);
    // Portrait and landscape are mirror images of each other.
    let (lw, lh) = RgbImage::inscribed_rect_size(1600.0, 1066.0, 0.2);
    let (pw, ph) = RgbImage::inscribed_rect_size(1066.0, 1600.0, 0.2);
    assert!((lw - ph).abs() < 1e-9 && (lh - pw).abs() < 1e-9);
}

// MARK: - PreprocessRectTests (the parts that don't need the analysis kernel)

const RECT: NormalizedRect = NormalizedRect::new(0.25, 0.2, 0.5, 0.55);

#[test]
fn cropped_image_matches_manual_slice() {
    let img = synthetic_grid_input();
    let out = img.cropped(&RECT).into_owned();
    let roi = RECT.pixel_roi(img.width, img.height).expect("roi");
    assert!(out.width == roi.x1 - roi.x0 && out.height == roi.y1 - roi.y0);
    // Spot-check corners map to the source ROI.
    assert_eq!(px(&out, 0, 0, 0), px(&img, roi.y0, roi.x0, 0));
    assert_eq!(
        px(&out, out.height - 1, out.width - 1, 2),
        px(&img, roi.y1 - 1, roi.x1 - 1, 2)
    );
    // Every row, not just the corners.
    for y in 0..out.height {
        assert_eq!(
            out.row(y).expect("row"),
            &img.row(y + roi.y0).expect("row")[roi.x0 * 3..roi.x1 * 3]
        );
    }
}

#[test]
fn analysis_rect_equals_pre_cropped_with_zero_buffer() {
    let img = synthetic_grid_input();
    let (via_rect, buffer) = prefilter::resolve_analysis_region(&img, None, Some(&RECT), 0.10);
    assert_eq!(
        buffer, 0.0,
        "a freehand analysis rect disables the buffer inset"
    );
    let via_crop = img.cropped(&RECT);
    assert_eq!(via_rect.data, via_crop.data);
    let grid_rect = prefilter::prefilter_log_grid(&via_rect, buffer);
    let grid_crop = prefilter::prefilter_log_grid(&via_crop, 0.0);
    assert_eq!(grid_rect, grid_crop);
    // And it actually changes the result vs the full frame.
    let full = prefilter::prefilter_log_grid(&img, 0.10);
    assert_ne!(grid_rect, full);
}

#[test]
fn crop_rect_scopes_analysis_with_buffer_inside() {
    let img = synthetic_grid_input();
    let (via_crop_setting, buffer) =
        prefilter::resolve_analysis_region(&img, Some(&RECT), None, 0.10);
    assert_eq!(buffer, 0.10, "the crop keeps the buffer, applied inside it");
    let via_pre_crop = img.cropped(&RECT);
    assert_eq!(via_crop_setting.data, via_pre_crop.data);
    assert_eq!(
        prefilter::prefilter_log_grid(&via_crop_setting, buffer),
        prefilter::prefilter_log_grid(&via_pre_crop, 0.10)
    );
}

#[test]
fn analysis_rect_wins_over_crop() {
    let img = synthetic_grid_input();
    let other = NormalizedRect::new(0.0, 0.0, 0.4, 0.4);
    let both = prefilter::resolve_analysis_region(&img, Some(&other), Some(&RECT), 0.10);
    let rect_only = prefilter::resolve_analysis_region(&img, None, Some(&RECT), 0.10);
    assert_eq!(both.0.data, rect_only.0.data);
    assert_eq!(both.1, rect_only.1);
}

#[test]
fn degenerate_rect_is_ignored() {
    let img = synthetic64_input();
    let tiny = NormalizedRect::new(0.5, 0.5, 0.001, 0.001);
    let (with, buffer) = prefilter::resolve_analysis_region(&img, None, Some(&tiny), 0.10);
    assert!(matches!(with, std::borrow::Cow::Borrowed(_)));
    assert_eq!(
        buffer, 0.10,
        "a degenerate rect must not disable the buffer either"
    );
    let (with_crop, buffer) = prefilter::resolve_analysis_region(&img, Some(&tiny), None, 0.10);
    assert!(matches!(with_crop, std::borrow::Cow::Borrowed(_)));
    assert_eq!(buffer, 0.10);
}

// MARK: - AuditRectAndGeometryTests (rect parts)

/// 64×64 ramp image, values 0.1…0.9 (never 0/1, so log stays finite).
fn ramp_image(width: usize, height: usize) -> RgbImage {
    let n = width * height;
    filled(width, height, |y, x, c| {
        let i = y * width + x;
        let t = i as f32 / (n - 1).max(1) as f32;
        let v = 0.1 + 0.8 * t;
        match c {
            0 => v,
            1 => v * 0.9,
            _ => v * 0.8,
        }
    })
}

/// The pathological rects the crash fix is about. Expected ROI results derived by hand from the
/// truncating mapping on a 64×64 frame:
///   x0 = max(Int(x·w), 0), x1 = min(Int((x+width)·w), w), need x1−x0 ≥ 2.
fn pathological() -> [(NormalizedRect, &'static str); 4] {
    [
        // x0 = max(Int(−32), 0) = 0; x1 = min(Int(−6.4), 64) = −6 → span < 2 → nil.
        (NormalizedRect::new(-0.5, 0.2, 0.4, 0.6), "left off-frame"),
        // x0 = max(Int(76.8), 0) = 76; x1 = min(Int(102.4), 64) = 64 → span < 0 → nil.
        (NormalizedRect::new(1.2, 0.2, 0.4, 0.6), "right off-frame"),
        // x0 = max(−64, 0) = 0; x1 = min(128, 64) = 64 → full frame, both axes.
        (NormalizedRect::new(-1.0, -1.0, 3.0, 3.0), "surrounds frame"),
        // 0.0005·64 = 0.032 px wide → span < 2 → nil.
        (
            NormalizedRect::new(0.5, 0.2, 0.0005, 0.5),
            "degenerate width",
        ),
    ]
}

#[test]
fn pixel_roi_clamps_and_rejects_off_frame_rects() {
    let (w, h) = (64usize, 64usize);
    let p = pathological();

    // Hand-derived expectations (see the table above).
    assert_eq!(p[0].0.pixel_roi(w, h), None);
    assert_eq!(p[1].0.pixel_roi(w, h), None);
    assert_eq!(
        p[2].0.pixel_roi(w, h),
        Some(PixelRoi {
            x0: 0,
            y0: 0,
            x1: w,
            y1: h
        })
    );
    assert_eq!(p[3].0.pixel_roi(w, h), None);

    // Invariant sweep: every ROI ever returned must be a valid in-frame box with both spans ≥ 2
    // (the contract `cropped` relies on).
    let mut probes: Vec<NormalizedRect> = p.iter().map(|(r, _)| *r).collect();
    let mut x = -1.5;
    while x <= 1.5 + 1e-12 {
        for width in [0.0, 0.001, 0.3, 1.0, 2.0] {
            probes.push(NormalizedRect::new(x, x / 2.0, width, width));
        }
        x += 0.25;
    }
    // Hostile sidecar values: NaN / infinities must be rejected or clamped, never trap.
    probes.push(NormalizedRect::new(f64::NAN, 0.0, 1.0, 1.0));
    probes.push(NormalizedRect::new(0.0, 0.0, f64::INFINITY, f64::INFINITY));
    probes.push(NormalizedRect::new(
        f64::NEG_INFINITY,
        0.0,
        f64::INFINITY,
        1.0,
    ));
    for rect in probes {
        let Some(roi) = rect.pixel_roi(w, h) else {
            continue;
        };
        assert!(roi.x0 <= roi.x1 && roi.x1 <= w);
        assert!(roi.y0 <= roi.y1 && roi.y1 <= h);
        assert!(roi.x1 - roi.x0 >= 2 && roi.y1 - roi.y0 >= 2);
    }
}

#[test]
fn cropped_with_pathological_rects_never_traps() {
    let img = ramp_image(64, 64);
    for (rect, note) in pathological() {
        let out = img.cropped(&rect);
        // Either self (nil ROI) or an image whose dims match the ROI.
        if let Some(roi) = rect.pixel_roi(img.width, img.height) {
            assert_eq!(out.width, roi.x1 - roi.x0, "{note}");
            assert_eq!(out.height, roi.y1 - roi.y0, "{note}");
        } else {
            assert!(out.width == img.width && out.height == img.height, "{note}");
            assert!(matches!(out, std::borrow::Cow::Borrowed(_)), "{note}");
        }
        assert_eq!(out.data.len(), out.width * out.height * 3, "{note}");
    }
    // An explicit pixel ROI past the frame is clamped, not trusted.
    let over = img.cropped_to_pixels(PixelRoi {
        x0: 60,
        y0: 60,
        x1: 1000,
        y1: 1000,
    });
    assert_eq!((over.width, over.height), (4, 4));
    let inverted = img.cropped_to_pixels(PixelRoi {
        x0: 60,
        y0: 0,
        x1: 10,
        y1: 64,
    });
    assert!(matches!(inverted, std::borrow::Cow::Borrowed(_)));
}

/// `prepareWithOffFrameCropRectDoesNotTrap` at the level this crate owns: the region resolution
/// + prefilter must ignore an off-frame/degenerate rect (full frame metered) and produce a
///   finite, non-empty grid. The bounds/anchor finiteness half rides `tests/analysis.rs`.
#[test]
fn prefilter_with_off_frame_crop_rect_does_not_trap() {
    let img = ramp_image(64, 64);
    for (rect, note) in pathological() {
        for (crop, analysis) in [(Some(&rect), None), (None, Some(&rect))] {
            let (metered, buffer) = prefilter::resolve_analysis_region(&img, crop, analysis, 0.10);
            let grid = prefilter::prefilter_log_grid(&metered, buffer);
            assert!(grid.width > 0 && grid.height > 0, "{note}");
            assert!(grid.data.iter().all(|v| v.is_finite()), "{note}");
        }
    }
}

/// `oriented(rotationCW: 90, flipHorizontal: true)` — expected mapping derived by hand from
/// PlanarImage.swift:
///   applyingFlip(6) (90° CW): out is h×w→w×h; out[y, x] = self[H−1−x, y]
///     (display: source (r, c) lands at (c, H−1−r)).
///   flippedHorizontally on the rotated image (width H):
///     (c, H−1−r) → (c, H−1−(H−1−r)) = (c, r).
/// Composition: final[y, x] = original[x, y] — a pure transpose.
#[test]
fn oriented_ninety_cw_plus_flip_is_transpose() {
    let (w, h) = (4usize, 3usize);
    // Unique value per (y, x, c).
    let img = filled(w, h, |y, x, c| ((y * w + x) * 3 + c) as f32);
    let out = img.oriented_with_fine_rotation(90, true, 0.0).into_owned();
    assert!(out.width == h && out.height == w);
    for y in 0..out.height {
        // out rows: 0..4
        for x in 0..out.width {
            // out cols: 0..3
            for c in 0..3 {
                // Explicit index math, not the helpers: transpose.
                assert_eq!(
                    out.data[(y * out.width + x) * 3 + c],
                    ((x * w + y) * 3 + c) as f32
                );
            }
        }
    }
}

#[test]
fn from_corners_standardizes_and_clamps() {
    let r = NormalizedRect::from_corners((0.9, -0.2), (0.3, 0.5));
    assert_eq!(r, NormalizedRect::new(0.3, 0.0, 0.9 - 0.3, 0.5));
    let r = NormalizedRect::from_corners((1.5, 1.5), (0.5, 0.5));
    assert_eq!(r, NormalizedRect::new(0.5, 0.5, 0.5, 0.5));
}

// MARK: - ImagePipelineSeamTests (RGBImage.downsampled)

fn gradient(width: usize, height: usize) -> RgbImage {
    filled(width, height, |y, x, c| match c {
        0 => x as f32 / (width - 1).max(1) as f32,
        1 => y as f32 / (height - 1).max(1) as f32,
        _ => 0.25,
    })
}

#[test]
fn downsample_dims_preserve_aspect() {
    let out = gradient(64, 48).downsampled(32).into_owned();
    assert!(out.width == 32 && out.height == 24);
    let odd = gradient(100, 75).downsampled(64).into_owned();
    assert!(odd.width == 64 && odd.height == 48);
    let portrait = gradient(48, 64).downsampled(32).into_owned();
    assert!(portrait.width == 24 && portrait.height == 32);
}

#[test]
fn downsample_at_or_below_cap_is_identity() {
    let img = gradient(64, 48);
    assert_eq!(img.downsampled(64).data, img.data);
    assert_eq!(img.downsampled(128).data, img.data);
    assert!(matches!(img.downsampled(64), std::borrow::Cow::Borrowed(_)));
}

/// Area averaging over an exact 2× factor: every output pixel is the mean of its 2×2 block, so
/// the global channel means are preserved exactly (this is what makes analysis on the proxy
/// match analysis on full res).
#[test]
fn exact_factor_preserves_channel_means() {
    let img = gradient(64, 48);
    let out = img.downsampled(32).into_owned();
    fn means(i: &RgbImage) -> [f64; 3] {
        let mut m = [0f64; 3];
        for p in i.data.chunks_exact(3) {
            for c in 0..3 {
                m[c] += f64::from(p[c]);
            }
        }
        let n = (i.width * i.height) as f64;
        [m[0] / n, m[1] / n, m[2] / n]
    }
    let (a, b) = (means(&img), means(&out));
    assert!((a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5 && (a[2] - b[2]).abs() < 1e-5);
}

#[test]
fn constant_image_stays_constant() {
    let img = filled(50, 30, |_, _, _| 0.42);
    let out = img.downsampled(16).into_owned();
    assert!(out.data.iter().all(|v| (v - 0.42).abs() < 1e-6));
}

/// `downsampled` is a separable area filter (cv2 INTER_AREA's kernel), parallelized over rows in
/// both passes. Each output sample sums its taps in a fixed order, so the parallel split must be
/// BYTE-identical to a serial run — and must survive output heights that aren't a multiple of
/// the slice count.
fn serial_separable(img: &RgbImage, max_long_edge: usize) -> Vec<f32> {
    let long = img.width.max(img.height);
    if long <= max_long_edge {
        return img.data.clone();
    }
    let scale = max_long_edge as f64 / long as f64;
    let ow = ((img.width as f64 * scale).round() as usize).max(1);
    let oh = ((img.height as f64 * scale).round() as usize).max(1);
    let (w, h) = (img.width, img.height);
    let hx = RgbImage::area_taps(w, ow);
    let vy = RgbImage::area_taps(h, oh);
    let mut mid = vec![0f32; ow * h * 3];
    for y in 0..h {
        for ox in 0..ow {
            let s0 = hx.start[ox];
            let (mut r, mut g, mut b) = (0f32, 0f32, 0f32);
            for k in 0..hx.stride {
                let wt = hx.weight[ox * hx.stride + k];
                if wt == 0.0 {
                    continue;
                }
                let i = (y * w + (s0 + k).min(w - 1)) * 3;
                r += img.data[i] * wt;
                g += img.data[i + 1] * wt;
                b += img.data[i + 2] * wt;
            }
            let o = (y * ow + ox) * 3;
            mid[o] = r;
            mid[o + 1] = g;
            mid[o + 2] = b;
        }
    }
    let mut out = vec![0f32; ow * oh * 3];
    for oy in 0..oh {
        let s0 = vy.start[oy];
        for ox in 0..ow {
            let (mut r, mut g, mut b) = (0f32, 0f32, 0f32);
            for k in 0..vy.stride {
                let wt = vy.weight[oy * vy.stride + k];
                if wt == 0.0 {
                    continue;
                }
                let i = ((s0 + k).min(h - 1) * ow + ox) * 3;
                r += mid[i] * wt;
                g += mid[i + 1] * wt;
                b += mid[i + 2] * wt;
            }
            let o = (oy * ow + ox) * 3;
            out[o] = r;
            out[o + 1] = g;
            out[o + 2] = b;
        }
    }
    out
}

#[test]
fn parallel_downsample_matches_serial_at_every_ragged_height() {
    for (w, h, cap) in [
        (100, 75, 64),
        (64, 48, 32),
        (37, 29, 13),
        (101, 53, 17),
        (200, 13, 10),
        (19, 200, 10),
        (64, 48, 1),
        (9, 9, 2),
        (1536, 1025, 1536),
        // The preview's real 1.96 ratio.
        (301, 201, 154),
    ] {
        let img = gradient(w, h);
        assert_eq!(
            img.downsampled(cap).data,
            serial_separable(&img, cap),
            "{w}x{h} -> cap {cap}"
        );
    }
}

/// THE property the area filter exists for, and the one the old integer-boundary box got wrong:
/// a UNIFORM sample phase.
///
/// Downsampling a linear ramp must give evenly spaced output values — every sample sits at the
/// centre of its span. Integer box boundaries make boxes 1 or 2 source pixels wide at the
/// preview's 1.96 ratio, so their centres drift and the steps come out uneven. That unevenness
/// is a low-frequency geometric warp of the proxy, which showed up as local misalignment against
/// the full-resolution tier.
#[test]
fn area_filter_samples_on_a_uniform_grid() {
    let (w, h, cap) = (3012usize, 4usize, 1536usize);
    let img = filled(w, h, |_, x, _| x as f32 / (w - 1) as f32);
    let out = img.downsampled(cap).into_owned();

    fn step_spread(row: &[f32], w: usize) -> f64 {
        let diffs: Vec<f64> = row.windows(2).map(|p| f64::from(p[1] - p[0])).collect();
        let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
        // Expressed in SOURCE pixels, which is the geometric error.
        diffs.iter().map(|d| (d - mean).abs()).fold(0.0, f64::max) * (w - 1) as f64
    }

    let ours: Vec<f32> = (0..out.width).map(|x| px(&out, 0, x, 0)).collect();
    let ours_spread = step_spread(&ours, w);
    assert!(
        ours_spread < 0.05,
        "sample phase drifted {ours_spread} source px"
    );

    // The OLD integer-boundary box, for the same ramp — so this test can't quietly become
    // vacuous if the filter is ever swapped back.
    let sx = w as f64 / out.width as f64;
    let mut old = Vec::with_capacity(out.width);
    for ox in 0..out.width {
        let x0 = (ox as f64 * sx) as usize;
        let x1 = w.min((x0 + 1).max(((ox + 1) as f64 * sx) as usize));
        let mut acc = 0f32;
        for x in x0..x1 {
            acc += px(&img, 0, x, 0);
        }
        old.push(acc / (x1 - x0) as f32);
    }
    // ~0.46 source px on this ramp: a box widening from 1 to 2 cells moves its centre by half a
    // cell, and consecutive steps disagree by that.
    let old_spread = step_spread(&old, w);
    assert!(
        old_spread > 0.3,
        "the old box should drift; got {old_spread}"
    );
    assert!(
        old_spread > ours_spread * 5.0,
        "the fix must be an order of magnitude better, not marginally"
    );
}

/// Separability check: the two 1-D passes must equal the naive 2-D area average (float ordering
/// aside).
#[test]
fn separable_passes_equal_the_naive_two_dimensional_average() {
    let img = gradient(211, 143);
    let cap = 97;
    let out = img.downsampled(cap).into_owned();
    let (ow, oh) = (out.width, out.height);
    let sx = img.width as f64 / ow as f64;
    let sy = img.height as f64 / oh as f64;
    let mut worst = 0.0f64;
    for oy in 0..oh {
        let (fy0, fy1) = (oy as f64 * sy, (oy + 1) as f64 * sy);
        for ox in 0..ow {
            let (fx0, fx1) = (ox as f64 * sx, (ox + 1) as f64 * sx);
            let (mut acc, mut wsum) = (0.0f64, 0.0f64);
            for y in (fy0 as usize)..img.height.min(fy1.ceil() as usize) {
                let wy = ((y + 1) as f64).min(fy1) - (y as f64).max(fy0);
                if wy <= 0.0 {
                    continue;
                }
                for x in (fx0 as usize)..img.width.min(fx1.ceil() as usize) {
                    let wx = ((x + 1) as f64).min(fx1) - (x as f64).max(fx0);
                    if wx <= 0.0 {
                        continue;
                    }
                    acc += wy * wx * f64::from(px(&img, y, x, 0));
                    wsum += wy * wx;
                }
            }
            worst = worst.max((acc / wsum - f64::from(px(&out, oy, ox, 0))).abs());
        }
    }
    assert!(worst < 1e-5, "separable vs naive 2-D differed by {worst}");
}

#[test]
fn area_tap_weights_sum_to_one_and_cover_the_span() {
    for (src, dst) in [(3012, 1536), (2010, 1025), (100, 7), (17, 16), (9, 2)] {
        let taps: AreaTaps = RgbImage::area_taps(src, dst);
        let s = src as f64 / dst as f64;
        assert_eq!(taps.start.len(), dst);
        assert_eq!(taps.weight.len(), dst * taps.stride);
        for j in 0..dst {
            let sum: f32 = taps.weight[j * taps.stride..(j + 1) * taps.stride]
                .iter()
                .sum();
            assert!(
                (sum - 1.0).abs() < 1e-5,
                "taps for {src}->{dst} row {j} sum to {sum}"
            );
            assert!(taps.start[j] < src);
            // The first tap must be the cell containing the span's start.
            assert_eq!(taps.start[j], (j as f64 * s).floor() as usize);
        }
    }
}
