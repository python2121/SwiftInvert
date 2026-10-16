//! Normalized rectangles (0…1 of a frame) — port of `NormalizedRect.swift`.
//!
//! Axis-aligned rect normalized to the image (`[0,1]` in both axes), used for the output crop
//! and the analysis region (NegPy's `manual_crop_rect` / `analysis_rect`). Struct + serde keys are
//! the shared contract: sidecars carry these as `{"x":…,"y":…,"width":…,"height":…}`.
//!
//! The pixel mapping ([`NormalizedRect::pixel_roi`]) is NegPy's truncating
//! `resolve_analysis_region` rule, and the "< 2 px is degenerate" guard lives HERE — every
//! consumer (crop, region priority in `ExposureKernel.prepare`) asks this one function.

use serde::{Deserialize, Serialize};

/// A rectangle in normalized frame coordinates, origin top-left, all fields in `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NormalizedRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A pixel window `[x0, x1) × [y0, y1)` inside a frame, as returned by
/// [`NormalizedRect::pixel_roi`]: always in-frame and at least 2 px in each axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelRoi {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl PixelRoi {
    /// `x1 − x0`.
    #[inline]
    pub fn width(&self) -> usize {
        self.x1.saturating_sub(self.x0)
    }

    /// `y1 − y0`.
    #[inline]
    pub fn height(&self) -> usize {
        self.y1.saturating_sub(self.y0)
    }
}

impl NormalizedRect {
    /// The full frame.
    pub const FULL: NormalizedRect = NormalizedRect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Standardized rect from two drag corners, clamped to `[0,1]` (Swift `init(from:to:)`).
    pub fn from_corners(a: (f64, f64), b: (f64, f64)) -> Self {
        let x0 = a.0.min(b.0).clamp(0.0, 1.0);
        let y0 = a.1.min(b.1).clamp(0.0, 1.0);
        let x1 = a.0.max(b.0).clamp(0.0, 1.0);
        let y1 = a.1.max(b.1).clamp(0.0, 1.0);
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Pixel ROI with NegPy's truncating mapping (`resolve_analysis_region`:
    /// `int(min·dim)…int(max·dim)`); `None` when the rect is degenerate (< 2 px in either
    /// axis), so a stray click can't blank analysis.
    ///
    /// Clamped to the frame BEFORE the degeneracy check: only the drag initializer guarantees
    /// `[0,1]` — a rect from a hand-edited or corrupt sidecar can sit off-frame, and checking the
    /// unclamped span used to pass a negative-dimension ROI through to `cropped` (fatal in the
    /// Swift app). For in-frame rects the two orders are identical.
    pub fn pixel_roi(&self, width: usize, height: usize) -> Option<PixelRoi> {
        let w = i64::try_from(width).ok()?;
        let h = i64::try_from(height).ok()?;
        // `as i64` truncates toward zero like Swift's `Int(Double)`; a NaN (which would trap in
        // Swift) becomes 0 and an overflow saturates — both land on a rejected or clamped span.
        let x0 = ((self.x * w as f64) as i64).max(0);
        let y0 = ((self.y * h as f64) as i64).max(0);
        let x1 = (((self.x + self.width) * w as f64) as i64).min(w);
        let y1 = (((self.y + self.height) * h as f64) as i64).min(h);
        if x1 - x0 < 2 || y1 - y0 < 2 {
            return None;
        }
        Some(PixelRoi {
            x0: usize::try_from(x0).ok()?,
            y0: usize::try_from(y0).ok()?,
            x1: usize::try_from(x1).ok()?,
            y1: usize::try_from(y1).ok()?,
        })
    }
}
