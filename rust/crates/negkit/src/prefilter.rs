//! Log-density prefilter chain shared by all meters — port of `Prefilter.swift` plus the region
//! priority from `ExposureKernel.prepare` (`ExposureSettings.swift`).
//!
//! NegPy: `negpy/features/exposure/normalization.py` (`prefilter_log_grid`, `get_analysis_crop`,
//! `_block_median_grid`) and `resolve_analysis_region`.
//!
//! Pipeline: region priority ([`resolve_analysis_region`]) → centered buffer crop
//! ([`analysis_crop`]) → `log10(clip(x, 1e-6, 1))` ([`log_image`]) → block-median grid
//! ([`block_median_grid`]); [`prefilter_log_grid`] is the last three.

use std::borrow::Cow;

use crate::RgbImage;
use crate::image::for_each_row_mut;
use crate::rect::NormalizedRect;
use crate::stats;

/// The analysis grid's target long edge (`K.analysisGrid`, NegPy `analysis_grid = 1024`): the
/// block side is `b = ceil(maxDim / ANALYSIS_GRID)`.
///
/// Lives here rather than in `constants` (agent C's module) so this module is self-contained;
/// re-export from `constants::K` if the other analysis modules want one home for it.
pub const ANALYSIS_GRID: usize = 1024;

/// Default centered inset: 10 % per side = the middle 80 % of the frame (SwiftInvert default;
/// NegPy ships 5 %). `ExposureKernel.defaultAnalysisBuffer`.
pub const DEFAULT_ANALYSIS_BUFFER: f64 = 0.10;

/// Region priority — NegPy's `resolve_analysis_region`: a freehand `analysis_rect` wins and
/// disables the centered buffer inset; otherwise the output `crop_rect` scopes the meters (so
/// borders outside the crop can't throw off the inversion) with the buffer applied inside it;
/// otherwise the centered buffer alone. Degenerate rects (< 2 px, see
/// [`NormalizedRect::pixel_roi`]) are ignored as if absent.
///
/// Returns the image the meters read (borrowed when no rect applies) and the buffer ratio to
/// pass on to [`prefilter_log_grid`].
pub fn resolve_analysis_region<'a>(
    linear_image: &'a RgbImage,
    crop_rect: Option<&NormalizedRect>,
    analysis_rect: Option<&NormalizedRect>,
    analysis_buffer: f64,
) -> (Cow<'a, RgbImage>, f64) {
    let (w, h) = (linear_image.width, linear_image.height);
    if let Some(roi) = analysis_rect.and_then(|r| r.pixel_roi(w, h)) {
        return (linear_image.cropped_to_pixels(roi), 0.0);
    }
    if let Some(roi) = crop_rect.and_then(|r| r.pixel_roi(w, h)) {
        return (linear_image.cropped_to_pixels(roi), analysis_buffer);
    }
    (Cow::Borrowed(linear_image), analysis_buffer)
}

/// `log10` of the clipped linear image: `log10(clip(x, 1e-6, 1.0))`. Since values are in
/// `(0, 1]`, this holds negative density (−D). Decoded buffers are `u16/65535` by construction,
/// so no NaN/Inf handling is needed. Scalar `log10f` (the Swift non-Accelerate path; the
/// parity suite passes on Linux against the same fixtures, so the scalar path is pinned).
pub fn log_image(image: &RgbImage) -> RgbImage {
    let mut out = image.clone();
    for v in out.data.iter_mut() {
        *v = v.clamp(1e-6, 1.0).log10();
    }
    out
}

/// Centered analysis crop: cut `int(dim · buffer)` from each side (`buffer ≤ 0.3`). Borrowed
/// (unchanged) when the buffer is ≤ 0 or the inset would leave nothing.
pub fn analysis_crop(img: &RgbImage, buffer_ratio: f64) -> Cow<'_, RgbImage> {
    if buffer_ratio <= 0.0 {
        return Cow::Borrowed(img);
    }
    let safe = buffer_ratio.clamp(0.0, 0.3);
    // Truncation like Swift's `Int(Double)`; finite and non-negative by construction.
    let cut_h = (img.height as f64 * safe) as usize;
    let cut_w = (img.width as f64 * safe) as usize;
    let (Some(h), Some(w)) = (
        img.height.checked_sub(2 * cut_h),
        img.width.checked_sub(2 * cut_w),
    ) else {
        return Cow::Borrowed(img);
    };
    if h == 0 || w == 0 {
        return Cow::Borrowed(img);
    }
    let mut out = RgbImage {
        width: w,
        height: h,
        data: vec![0.0; w * h * 3],
    };
    for_each_row_mut(&mut out.data, w * 3, |y, dst| {
        if let Some(src) = img
            .row(y + cut_h)
            .and_then(|r| r.get(cut_w * 3..(cut_w + w) * 3))
        {
            dst.copy_from_slice(src);
        }
    });
    Cow::Owned(out)
}

/// Block-median downfilter to a fixed target grid (`analysis_grid = 1024`):
/// `b = ceil(max(h,w)/grid)`; per-cell per-channel median of `b×b` blocks. Returns the input
/// unchanged when `b ≤ 1` (already at/below grid size).
pub fn block_median_grid(img: RgbImage) -> RgbImage {
    let (h, w) = (img.height, img.width);
    let b = (h.max(w) as f64 / ANALYSIS_GRID as f64).ceil() as usize;
    if b <= 1 || h < b || w < b {
        return img;
    }

    let rows = h / b;
    let cols = w / b;
    if rows == 0 || cols == 0 {
        return img;
    }
    let mut out = RgbImage {
        width: cols,
        height: rows,
        data: vec![0.0; cols * rows * 3],
    };

    if b == 2 {
        // The preview path always lands here (1536px / 1024 grid → b = 2): median of 4 =
        // (sum − min − max) / 2, no sort, no scratch.
        for_each_row_mut(&mut out.data, cols * 3, |gy, dst| {
            let (Some(r0), Some(r1)) = (img.row(gy * 2), img.row(gy * 2 + 1)) else {
                return;
            };
            for ((cell, p0), p1) in dst
                .chunks_exact_mut(3)
                .zip(r0.chunks_exact(6))
                .zip(r1.chunks_exact(6))
            {
                for (c, lane) in cell.iter_mut().enumerate() {
                    let a = p0[c];
                    let b2 = p0[c + 3];
                    let c2 = p1[c];
                    let d = p1[c + 3];
                    let mn = a.min(b2).min(c2.min(d));
                    let mx = a.max(b2).max(c2.max(d));
                    *lane = (a + b2 + c2 + d - mn - mx) / 2.0;
                }
            }
        });
        return out;
    }

    for_each_row_mut(&mut out.data, cols * 3, |gy, dst| {
        let mut scratch = vec![0f32; b * b];
        for (gx, cell) in dst.chunks_exact_mut(3).enumerate() {
            for (c, lane) in cell.iter_mut().enumerate() {
                let mut k = 0usize;
                for by in 0..b {
                    let Some(row) = img.row(gy * b + by) else {
                        continue;
                    };
                    let x0 = gx * b;
                    if let Some(span) = row.get(x0 * 3..(x0 + b) * 3) {
                        for v in span.iter().skip(c).step_by(3) {
                            if let Some(s) = scratch.get_mut(k) {
                                *s = *v;
                            }
                            k += 1;
                        }
                    }
                }
                *lane = stats::median_in_place(&mut scratch).unwrap_or(0.0);
            }
        }
    });
    out
}

/// Full chain: (roi skipped: SwiftInvert meters the whole frame) → centered buffer crop →
/// `log10` → block-median grid.
///
/// The crop runs BEFORE the log even though NegPy logs first: `log10` is elementwise, so the
/// surviving values are identical either way, and the default 0.10 buffer throws away 36 % of
/// the frame — no reason to compute logs for pixels the meters never look at (4.6 → 2.9 ms).
pub fn prefilter_log_grid(image: &RgbImage, analysis_buffer: f64) -> RgbImage {
    let cropped = analysis_crop(image, analysis_buffer);
    block_median_grid(log_image(&cropped))
}
