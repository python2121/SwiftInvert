//! Display-layout geometry the session reports on every render — re-exports of
//! [`negkit::crop_geometry`] (`Sources/NegativeKit/CropGeometry.swift`) plus the one adapter
//! the session needs: `content_window` taking the `PixelRoi` the crop actually used.
//!
//! Why the session reports these (CLAUDE.md §1 "Tier-swap stability"): the proxy and the
//! full-resolution tier quantize differently — `fine_rotated` FLOORS the inscribed rect on
//! each tier's own grid and the crop `pixel_roi` truncates onto that bitmap — so two renders
//! of one setting cover slightly different windows and have aspects that differ by up to
//! ~0.2 %. The canvas lays out from the CONTINUOUS aspect and places each bitmap at the window
//! it really covers, so both tiers put the same content on the same screen point.

pub use negkit::crop_geometry::{
    PixelWindow, RotatedBox, box_fits, box_from_rect, constrain, content_window, display_aspect,
    fit_scale, inscribed_size, normalized_rect, remap_crop, rotate,
};
use negkit::rect::{NormalizedRect, PixelRoi};

/// A crop's pixel window as the continuous `[x0, y0, x1, y1]` `content_window` takes.
pub fn pixel_window(roi: PixelRoi) -> PixelWindow {
    // Pixel counts → f64 (exact below 2^53).
    [roi.x0 as f64, roi.y0 as f64, roi.x1 as f64, roi.y1 as f64]
}

/// [`content_window`] with the crop window as the `PixelRoi` the session's crop used
/// (`None` = the whole bitmap). `frame` is THIS tier's orientation-only frame size and
/// `oriented_pixels` its fine-rotated bitmap dims — a tier must be measured against its own
/// frame.
pub fn content_window_for_roi(
    frame: [f64; 2],
    radians: f64,
    crop: Option<&NormalizedRect>,
    oriented_pixels: [f64; 2],
    roi: Option<PixelRoi>,
) -> NormalizedRect {
    content_window(frame, radians, crop, oriented_pixels, roi.map(pixel_window))
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
        let w = content_window_for_roi([1536.0, 1024.0], 0.0, None, [1536.0, 1024.0], None);
        assert_eq!(w, NormalizedRect::FULL);
        // A floored inscribed bitmap sits centred, slightly inside the ideal rect.
        let r = 3f64.to_radians();
        let ideal = inscribed_size([1536.0, 1024.0], r);
        let px = [ideal[0].floor(), ideal[1].floor()];
        let w = content_window_for_roi([1536.0, 1024.0], r, None, px, None);
        assert!(w.x > 0.0 && w.x < 0.001 && w.width < 1.0 && w.width > 0.998);
        assert!(((w.x * 2.0 + w.width) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn content_window_folds_the_crop_roi() {
        let crop = NormalizedRect::new(0.25, 0.25, 0.5, 0.5);
        let roi = crop.pixel_roi(1536, 1024).unwrap();
        assert_eq!(pixel_window(roi), [384.0, 256.0, 1152.0, 768.0]);
        let w = content_window_for_roi(
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
