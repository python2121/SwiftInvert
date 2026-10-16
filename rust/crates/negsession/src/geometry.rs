//! Display-layout geometry — the `displayAspect` / `contentWindow` half of
//! `Sources/NegativeKit/CropGeometry.swift` the session reports on every render.
//!
//! Local port: `negkit::crop_geometry` (the full rotated-box / fitScale / remapCrop module) is
//! being written concurrently by wave 3 / agent M. These three functions are the ones
//! `ImageSession` calls; once the negkit module lands they can become thin re-exports (the
//! formulas are identical by construction — same Swift lines).
//!
//! Why they exist (CLAUDE.md §1 "Tier-swap stability"): the proxy and the full-resolution tier
//! quantize differently — `fine_rotated` FLOORS the inscribed rect on each tier's own grid and
//! the crop `pixel_roi` truncates onto that bitmap — so two renders of one setting cover
//! slightly different windows and have aspects that differ by up to ~0.2 %. The canvas lays out
//! from the CONTINUOUS aspect and places each bitmap at the window it really covers, so both
//! tiers put the same content on the same screen point.

use negkit::rect::{NormalizedRect, PixelRoi};
use negkit::RgbImage;

/// Continuous inscribed-rect size of a `frame` rotated by `radians` (the straighten auto-crop);
/// the frame itself below 1e-9 rad (`CropGeometry.inscribedSize`).
pub fn inscribed_size(frame: [f64; 2], radians: f64) -> [f64; 2] {
    if radians.abs() <= 1e-9 {
        return frame;
    }
    let (w, h) = RgbImage::inscribed_rect_size(frame[0], frame[1], radians);
    [w, h]
}

/// Width ÷ height the canvas should LAY OUT at, in continuous space: the orientation-only
/// frame, through the inscribed rect, through the crop — never a pixel count
/// (`CropGeometry.displayAspect`). Degenerate inputs fall back to the frame's own ratio.
pub fn display_aspect(frame: [f64; 2], radians: f64, crop: Option<&NormalizedRect>) -> f64 {
    let mut size = inscribed_size(frame, radians);
    if let Some(crop) = crop {
        size[0] *= crop.width;
        size[1] *= crop.height;
    }
    if size[0] > 0.0 && size[1] > 0.0 && frame[1] > 0.0 {
        size[0] / size[1]
    } else {
        (frame[0] / frame[1].max(1.0)).max(1e-6)
    }
}

/// Where a produced bitmap actually sits inside the IDEAL display rectangle, in units of that
/// rectangle — nominally `(0, 0, 1, 1)`, off it by well under a pixel
/// (`CropGeometry.contentWindow`).
///
/// * `frame` — THIS tier's orientation-only frame size (a tier must be measured against its own
///   frame: the full-res bitmap against the proxy's frame yields a window ~3.9× the ideal).
/// * `oriented_pixels` — the tier's oriented (fine-rotated, inscribed) bitmap dims.
/// * `roi` — the pixel window the crop actually used on that bitmap (`None` = whole bitmap).
pub fn content_window(
    frame: [f64; 2],
    radians: f64,
    crop: Option<&NormalizedRect>,
    oriented_pixels: [f64; 2],
    roi: Option<PixelRoi>,
) -> NormalizedRect {
    let inscribed = inscribed_size(frame, radians);
    if !(inscribed[0] > 0.0
        && inscribed[1] > 0.0
        && oriented_pixels[0] > 0.0
        && oriented_pixels[1] > 0.0)
    {
        return NormalizedRect::FULL;
    }
    // Stage 1, in units of the ideal inscribed rect.
    let mut origin = [
        (1.0 - oriented_pixels[0] / inscribed[0]) / 2.0,
        (1.0 - oriented_pixels[1] / inscribed[1]) / 2.0,
    ];
    let mut size = [
        oriented_pixels[0] / inscribed[0],
        oriented_pixels[1] / inscribed[1],
    ];
    // Stage 2: fold in the crop ROI, then re-express in units of the ideal CROPPED rect —
    // which is what the canvas fits.
    if let (Some(crop), Some(roi)) = (crop, roi)
        && crop.width > 0.0
        && crop.height > 0.0
    {
        // Pixel counts → f64 (exact below 2^53).
        let (x0, y0, x1, y1) = (
            roi.x0 as f64,
            roi.y0 as f64,
            roi.x1 as f64,
            roi.y1 as f64,
        );
        origin[0] += size[0] * (x0 / oriented_pixels[0]);
        origin[1] += size[1] * (y0 / oriented_pixels[1]);
        size[0] *= (x1 - x0) / oriented_pixels[0];
        size[1] *= (y1 - y0) / oriented_pixels[1];
        origin[0] = (origin[0] - crop.x) / crop.width;
        origin[1] = (origin[1] - crop.y) / crop.height;
        size[0] /= crop.width;
        size[1] /= crop.height;
    }
    NormalizedRect::new(origin[0], origin[1], size[0], size[1])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn aspect_follows_frame_rotation_and_crop() {
        assert!((display_aspect([1536.0, 1024.0], 0.0, None) - 1.5).abs() < 1e-12);
        let crop = NormalizedRect::new(0.0, 0.0, 0.5, 1.0);
        assert!((display_aspect([1536.0, 1024.0], 0.0, Some(&crop)) - 0.75).abs() < 1e-12);
        let tilted = display_aspect([1536.0, 1024.0], 10f64.to_radians(), None);
        assert!(tilted > 0.0 && (tilted - 1.5).abs() > 0.01);
        // Degenerate frame → frame ratio, never NaN.
        assert!(display_aspect([100.0, 0.0], 0.0, None).is_finite());
    }

    #[test]
    fn content_window_is_identity_for_exact_bitmaps() {
        let w = content_window([1536.0, 1024.0], 0.0, None, [1536.0, 1024.0], None);
        assert_eq!(w, NormalizedRect::FULL);
        // A floored inscribed bitmap sits centred, slightly inside the ideal rect.
        let r = 3f64.to_radians();
        let ideal = inscribed_size([1536.0, 1024.0], r);
        let px = [ideal[0].floor(), ideal[1].floor()];
        let w = content_window([1536.0, 1024.0], r, None, px, None);
        assert!(w.x > 0.0 && w.x < 0.001 && w.width < 1.0 && w.width > 0.998);
        assert!(((w.x * 2.0 + w.width) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn content_window_folds_the_crop_roi() {
        let crop = NormalizedRect::new(0.25, 0.25, 0.5, 0.5);
        let roi = crop.pixel_roi(1536, 1024).unwrap();
        let w = content_window(
            [1536.0, 1024.0],
            0.0,
            Some(&crop),
            [1536.0, 1024.0],
            Some(roi),
        );
        // 0.25·1536 = 384 exactly, 0.25·1024 = 256 exactly → identity window.
        assert!((w.x).abs() < 1e-12 && (w.y).abs() < 1e-12);
        assert!((w.width - 1.0).abs() < 1e-12 && (w.height - 1.0).abs() < 1e-12);
    }
}
