//! The Contrast Mask plane as the session caches and the renderers sample it
//! (`ContrastMask.Plane` / `ContrastMask.sample` in `Sources/NegativeKit/ContrastMask.swift`,
//! NegPy 515c1f5).
//!
//! The plane is per-image DATA, not a render parameter: a blurred zero-mean normalized-luma field
//! built from the PRINTED proxy frame (crop applied), σ = `maskSpacer` % × the analysis grid's
//! short side. Every tier and the export sample the same plane through the half-pixel bilinear
//! mapping below — that is what makes a tier swap unable to move the mask.
//!
//! `TODO(mask)`: the plane BUILDER (`ContrastMask.buildPlane`: INTER_AREA resize to the analysis
//! grid → log10 → base-bounds normalize → Rec.709 luma → cv2-matched Gaussian → zero-mean) lives
//! in `negkit::contrast_mask`, which wave 3 / agent M is writing concurrently and which is still
//! a stub on this branch. [`build_plane`] is the single seam: it returns `None` today (the session
//! renders unmasked, exactly as the Swift does when `buildPlane` returns nil) and becomes a call
//! into negkit when that module lands. Everything around it — the cache key, the invalidation,
//! the sampler plumbing into `negkit::reference::RenderOptions::mask_plane` — is in place.

use negkit::reference::MaskSampler;
use negkit::{LogNegativeBounds, RgbImage};

/// A mask plane: `values.len() == width * height`, row-major, zero-mean normalized-val luma.
#[derive(Clone, Debug, PartialEq)]
pub struct MaskPlane {
    pub values: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

impl MaskPlane {
    /// Wrap a buffer; `None` when the count doesn't match the dims or either dim is 0.
    pub fn new(values: Vec<f32>, width: usize, height: usize) -> Option<Self> {
        let n = width.checked_mul(height)?;
        (n > 0 && values.len() == n).then_some(Self {
            values,
            width,
            height,
        })
    }

    /// Resident bytes (the session's memory accounting).
    pub fn bytes(&self) -> usize {
        self.values.len() * std::mem::size_of::<f32>()
    }

    #[inline]
    fn at(&self, x: f32, y: f32) -> f32 {
        // `x`/`y` are clamped to [0, dim − 1] by the callers; the truncating casts are exact
        // on those integral floats. `get` keeps the no-panic rule on a hostile plane.
        let i = (y as usize)
            .saturating_mul(self.width)
            .saturating_add(x as usize);
        self.values.get(i).copied().unwrap_or(0.0)
    }

    /// Bilinear sample under output pixel `(x, y)` of an `out_width × out_height` render —
    /// OpenCV's half-pixel-centre convention, taps clamped (edge replication). Mirrored in the
    /// Metal, GLSL and WGSL kernels; keep them in lock step.
    pub fn sample(&self, x: usize, y: usize, out_width: usize, out_height: usize) -> f32 {
        if out_width == 0 || out_height == 0 {
            return 0.0;
        }
        // Pixel coordinates → f32, as the kernels do.
        let px = (x as f32 + 0.5) * self.width as f32 / out_width as f32 - 0.5;
        let py = (y as f32 + 0.5) * self.height as f32 / out_height as f32 - 0.5;
        self.bilinear(px, py)
    }

    /// Bilinear sample at content-normalized `(u, v)` of the printed frame — the zone-pin path
    /// (`u = (x + 0.5) / W` collapses the mapping above to `u·w − 0.5`).
    pub fn sample_normalized(&self, u: f64, v: f64) -> f32 {
        // Normalized coordinates narrowed to the kernels' f32 lane.
        let px = u as f32 * self.width as f32 - 0.5;
        let py = v as f32 * self.height as f32 - 0.5;
        self.bilinear(px, py)
    }

    fn bilinear(&self, px: f32, py: f32) -> f32 {
        let max_x = self.width.saturating_sub(1) as f32;
        let max_y = self.height.saturating_sub(1) as f32;
        // NaN-safe: a NaN coordinate clamps to 0 through `max`/`min`'s NaN handling.
        let lox = px.floor().max(0.0).min(max_x);
        let loy = py.floor().max(0.0).min(max_y);
        let hix = (lox + 1.0).min(max_x);
        let hiy = (loy + 1.0).min(max_y);
        let fx = (px - lox).clamp(0.0, 1.0);
        let fy = (py - loy).clamp(0.0, 1.0);
        let (v00, v10) = (self.at(lox, loy), self.at(hix, loy));
        let (v01, v11) = (self.at(lox, hiy), self.at(hix, hiy));
        let top = v00 + (v10 - v00) * fx;
        let bot = v01 + (v11 - v01) * fx;
        top + (bot - top) * fy
    }
}

impl MaskSampler for MaskPlane {
    fn sample(&self, x: usize, y: usize, out_width: usize, out_height: usize) -> f32 {
        MaskPlane::sample(self, x, y, out_width, out_height)
    }
}

/// Build the zero-mean mask plane from the render source at proxy scale (crop and geometry
/// already applied) against the BASE bounds (pre-WP/BP — the SwiftInvert divergence that keeps
/// white/black-point drags analysis-free). `None` = render unmasked.
///
/// `TODO(mask)`: forwards to `negkit::contrast_mask::build_plane` once that module lands on this
/// branch; until then every input yields `None` (the degenerate-source / no-range guards below
/// are the Swift `buildPlane`'s own first lines and stay as the seam's contract).
pub fn build_plane(
    render_source: &RgbImage,
    bounds: &LogNegativeBounds,
    spacer_percent: f64,
) -> Option<MaskPlane> {
    if render_source.width < 2 || render_source.height < 2 {
        return None;
    }
    let range = bounds.luminance_density_range();
    if range.is_nan() || range <= 1e-6 || !spacer_percent.is_finite() {
        return None;
    }
    log::debug!(
        "contrast mask requested (spacer {spacer_percent} %) — plane builder not wired yet \
         (TODO(mask): negkit::contrast_mask), rendering unmasked"
    );
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn sampling_is_bilinear_and_edge_clamped() {
        let plane = MaskPlane::new(vec![0.0, 1.0, 0.0, 1.0], 2, 2).unwrap();
        // Same-size render: pixel centres hit the texels exactly.
        assert_eq!(plane.sample(0, 0, 2, 2), 0.0);
        assert_eq!(plane.sample(1, 0, 2, 2), 1.0);
        // A 4-wide render samples between the texels: x=1 → px = 0.25 → 0.25.
        assert!((plane.sample(1, 0, 4, 2) - 0.25).abs() < 1e-6);
        // Beyond the last texel clamps.
        assert_eq!(plane.sample(7, 3, 4, 2), 1.0);
        assert!((plane.sample_normalized(0.5, 0.5) - 0.5).abs() < 1e-6);
        assert!(MaskPlane::new(vec![0.0; 3], 2, 2).is_none());
        assert_eq!(plane.sample(0, 0, 0, 0), 0.0);
    }
}
