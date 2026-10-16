//! Contrast Mask — the darkroom's unsharp mask. Port of `ContrastMask.swift` (NegPy 515c1f5:
//! `negpy/features/exposure/normalization.py: contrast_mask_plane`; `logic.py:
//! contrast_mask_scale / expand_mask_plane`).
//!
//! Sandwich the negative with a blurred, low-gamma mask: densities add, so
//! `D' = D − g·blur(D) + const` — positive gamma (a blurred positive) compresses the global
//! range while the blur keeps detail out of the compression; negative gamma stretches instead.
//! The plane is the blurred luma of the NORMALIZED negative on the analysis grid, zero-mean so
//! print density never moves, built from the PRINTED frame only (a bright rebate blurred into
//! the mask prints as a vignette the negative does not have).
//!
//! The plane is always built from the PROXY-scale render source: sigma is a fraction of the
//! analysis grid, never of the render, so preview, HQ tiers and export mask identically (the
//! analysis-on-the-proxy invariant).
//!
//! DIVERGENCE (recorded in CLAUDE.md): the plane normalizes against the BASE bounds
//! (pre-WP/BP-offset), where upstream uses the offset-applied final bounds. Ours keeps
//! white/black-point drags analysis-free — the plane is a blurred zero-mean field of a source
//! property, the same argument as the 2125a34 pre-trim neutral axis. Identical at zero offsets,
//! which is every parity-fixture config.
//!
//! The per-channel scale the kernels multiply the sample by lives in [`crate::derive`]
//! (`mask_val_scale`, folded into `RenderParams` at derive time); it is re-exported here as
//! [`val_scale`] so the feature reads as one module.

use crate::constants::K;
use crate::image::for_each_row_mut;
use crate::reference::{self, MaskSampler};
use crate::{LogNegativeBounds, NegError, RgbImage};

pub use crate::derive::CONTRAST_MASK_GAMMA_LIMIT as GAMMA_LIMIT;
pub use crate::derive::mask_val_scale as val_scale;

/// Spacer slider floor (upstream MASK_SPACER_MIN), percent of the grid's short side. The floor
/// is where the mask stops being unsharp: at the narrow limit blur(v) → v and the operator
/// collapses into a plain (1−g) reduction, i.e. Grade.
pub const SPACER_MIN: f64 = 2.0;
/// Spacer slider ceiling (upstream MASK_SPACER_MAX).
pub const SPACER_MAX: f64 = 6.0;
/// Spacer slider default (upstream MASK_SPACER_DEFAULT).
pub const SPACER_DEFAULT: f64 = 4.0;

/// Single-channel float plane on the analysis grid, row-major, `data.len() == width × height`.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Plane {
    /// A plane from its samples; [`NegError::Shape`] when the count doesn't match the dims,
    /// [`NegError::Empty`] for a zero-sized plane (the sampler needs at least one cell).
    pub fn new(width: usize, height: usize, data: Vec<f32>) -> Result<Self, NegError> {
        if width == 0 || height == 0 {
            return Err(NegError::Empty);
        }
        if data.len() != width * height {
            return Err(NegError::Shape {
                width,
                height,
                len: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// `data[i]` without the panic path (0 when the index math ever runs off the plane).
    #[inline(always)]
    fn at(&self, i: usize) -> f32 {
        self.data.get(i).copied().unwrap_or(0.0)
    }

    /// Bilinear sample at plane coordinates `(px, py)` (already in the plane's half-pixel
    /// frame), taps clamped = edge replication. `f32` throughout — the kernels' arithmetic.
    #[inline]
    fn bilinear(&self, px: f32, py: f32) -> f32 {
        // Boundary casts: the plane dims are small (≤ 1024) and exact in f32.
        let max_x = self.width.saturating_sub(1) as f32;
        let max_y = self.height.saturating_sub(1) as f32;
        let lox = px.floor().clamp(0.0, max_x);
        let loy = py.floor().clamp(0.0, max_y);
        let hix = (lox + 1.0).min(max_x);
        let hiy = (loy + 1.0).min(max_y);
        let fx = (px - lox).clamp(0.0, 1.0);
        let fy = (py - loy).clamp(0.0, 1.0);
        // Non-negative and ≤ max by construction, so the casts are exact truncations.
        let (lox, loy, hix, hiy) = (lox as usize, loy as usize, hix as usize, hiy as usize);
        let i00 = loy * self.width + lox;
        let i10 = loy * self.width + hix;
        let i01 = hiy * self.width + lox;
        let i11 = hiy * self.width + hix;
        let top = self.at(i00) + (self.at(i10) - self.at(i00)) * fx;
        let bot = self.at(i01) + (self.at(i11) - self.at(i01)) * fx;
        top + (bot - top) * fy
    }

    /// Bilinear plane sample for output pixel `(x, y)` of an `out_width × out_height` render
    /// — upstream's `expand_mask_plane` / `contrast_mask_stops` mapping (OpenCV's
    /// half-pixel-centre convention, taps clamped = edge replication). This exact arithmetic
    /// is mirrored in NegPipeline.metal, NegPipeline.comp and the WGSL — keep them in lock step.
    #[inline]
    pub fn sample_at(&self, x: usize, y: usize, out_width: usize, out_height: usize) -> f32 {
        // Boundary casts: pixel coordinates, exact in f32 up to 16 M.
        let px = (x as f32 + 0.5) * (self.width as f32) / (out_width as f32) - 0.5;
        let py = (y as f32 + 0.5) * (self.height as f32) / (out_height as f32) - 0.5;
        self.bilinear(px, py)
    }

    /// Bilinear plane sample at content-normalized `(u, v)` of the printed frame — the
    /// zone-pin path (`u = (x+0.5)/W` collapses the half-pixel mapping above to `u·w − 0.5`).
    #[inline]
    pub fn sample_normalized(&self, u: f64, v: f64) -> f32 {
        // Boundary casts: the Swift narrows u/v to Float before the multiply.
        let px = (u as f32) * (self.width as f32) - 0.5;
        let py = (v as f32) * (self.height as f32) - 0.5;
        self.bilinear(px, py)
    }

    /// Mean of the plane in `f64` (≈ 0 for a built plane — the zero-mean contract).
    pub fn mean(&self) -> f64 {
        if self.data.is_empty() {
            return 0.0;
        }
        self.data.iter().map(|&v| f64::from(v)).sum::<f64>() / self.data.len() as f64
    }
}

impl MaskSampler for Plane {
    #[inline]
    fn sample(&self, x: usize, y: usize, out_width: usize, out_height: usize) -> f32 {
        self.sample_at(x, y, out_width, out_height)
    }
}

/// Build the zero-mean mask plane from the render source at proxy scale (crop and geometry
/// already applied — the plane covers exactly the printed frame the kernels will render).
///
/// Mirrors upstream: resize to the analysis grid (INTER_AREA) → `log10(clip(x, 1e-6, 1))` →
/// per-channel normalize by `bounds` (unclamped stretch) → Rec.709 luma → Gaussian blur at
/// σ = spacer% × grid short side (cv2 kernel: radius round(4σ), replicate border) → subtract
/// the mean.
///
/// `None` when the bounds carry no real range (nothing metered) or the source is degenerate —
/// the caller renders unmasked.
pub fn build_plane(
    render_source: &RgbImage,
    bounds: &LogNegativeBounds,
    spacer_percent: f64,
) -> Option<Plane> {
    if render_source.width < 2 || render_source.height < 2 {
        return None;
    }
    let range = bounds.luminance_density_range();
    if range.is_nan() || range <= 1e-6 {
        return None;
    }

    // To the analysis grid. `downsampled` is the same separable area filter as upstream's
    // INTER_AREA resize; ≤ grid passes through (borrowed).
    let img = render_source.downsampled(K::ANALYSIS_GRID);

    // Normalized-val luma. `reference::normalize` does log10(clip) + per-channel stretch
    // itself (the normalizeLog kernel's CPU mirror, parity-pinned) — upstream's
    // prefilter_log_grid + normalize_log_image collapse to exactly that at grid size (b = 1).
    let norm = reference::normalize(&img, bounds);
    let (w, h) = (norm.width, norm.height);
    // Boundary casts: the luma weights as the kernels hold them (Float).
    let (wr, wg, wb) = (K::LUMA_R as f32, K::LUMA_G as f32, K::LUMA_B as f32);
    let mut lum: Vec<f32> = norm
        .data
        .chunks_exact(3)
        .map(|p| wr * p[0] + wg * p[1] + wb * p[2])
        .collect();
    if lum.len() != w * h {
        return None;
    }

    let sigma = spacer_percent.clamp(SPACER_MIN, SPACER_MAX) * 0.01 * (w.min(h) as f64);
    gaussian_blur_replicate(&mut lum, w, h, sigma);

    // Zero-mean: a real sandwich is denser and the printer opens up for it — the slider must
    // leave print density alone. Summed in f64, subtracted as Float (the Swift's order).
    let mean = lum.iter().map(|&v| f64::from(v)).sum::<f64>() / (w * h) as f64;
    let m = mean as f32;
    for v in &mut lum {
        *v -= m;
    }

    Plane::new(w, h, lum).ok()
}

/// cv2's `getGaussianKernel` construction for a given σ: `ksize = round(8σ+1) | odd`
/// (radius `round(4σ)`), coefficients `exp(−d²/2σ²)` normalized to sum 1 — so the parity
/// fixture can pin the plane against cv2's own output. Empty for σ ≤ 0 or a zero radius.
pub fn gaussian_taps(sigma: f64) -> Vec<f32> {
    if !sigma.is_finite() || sigma <= 0.0 {
        return Vec::new();
    }
    // `round` = Swift `.rounded()` (half away from zero); ksize is small, the cast is exact.
    // Capped: σ comes from a slider (≤ 6 % of a ≤ 1024-px side → ~60 px) but the fn is pub,
    // so an absurd σ must not turn into a multi-GB allocation.
    const MAX_KSIZE: usize = 4097;
    let mut ksize = ((8.0 * sigma + 1.0).round().max(0.0) as usize).min(MAX_KSIZE);
    if ksize.is_multiple_of(2) {
        ksize += 1;
    }
    let radius = (ksize - 1) / 2;
    if radius == 0 {
        return Vec::new();
    }
    let mut taps = Vec::with_capacity(ksize);
    let mut sum = 0.0f64;
    for i in 0..ksize {
        let d = i as f64 - radius as f64;
        let v = (-d * d / (2.0 * sigma * sigma)).exp();
        taps.push(v as f32);
        sum += v;
    }
    let inv = (1.0 / sum) as f32;
    for t in &mut taps {
        *t *= inv;
    }
    taps
}

/// Separable Gaussian blur in place, replicate border, cv2-matched kernel ([`gaussian_taps`]).
///
/// Each line is pre-extended by edge replication so the inner loop is a branch-free MAC
/// (the Swift's scalar path, tap order preserved — the Accelerate path on the Mac
/// correlates the same symmetric taps). Both passes run horizontally over contiguous rows:
/// the vertical pass is a transpose → row pass → transpose, which keeps the MACs
/// cache-friendly and lets `rayon` split rows; the per-sample arithmetic order is identical
/// to the column walk, so the result is bit-for-bit the serial one.
pub fn gaussian_blur_replicate(data: &mut [f32], width: usize, height: usize, sigma: f64) {
    if data.len() != width * height || width == 0 || height == 0 {
        return;
    }
    let taps = gaussian_taps(sigma);
    if taps.is_empty() {
        return;
    }
    let radius = (taps.len() - 1) / 2;

    // Horizontal pass: data → scratch (row-major, same shape).
    let mut scratch = vec![0.0f32; width * height];
    blur_rows(data, &mut scratch, width, &taps, radius);

    // Vertical pass as a transposed horizontal one: scratchᵀ → blurred → transpose back.
    let transposed = transpose(&scratch, width, height);
    blur_rows(&transposed, &mut scratch, height, &taps, radius);
    let back = transpose(&scratch, height, width);
    data.copy_from_slice(&back);
}

/// Blur every `row_len`-wide row of `src` into `dst` with replicate padding.
fn blur_rows(src: &[f32], dst: &mut [f32], row_len: usize, taps: &[f32], radius: usize) {
    if row_len == 0 {
        return;
    }
    for_each_row_mut(dst, row_len, |y, out| {
        let Some(row) = src.get(y * row_len..(y + 1) * row_len) else {
            return;
        };
        let (Some(&first), Some(&last)) = (row.first(), row.last()) else {
            return;
        };
        let mut line = Vec::with_capacity(row_len + 2 * radius);
        line.resize(radius, first);
        line.extend_from_slice(row);
        line.resize(row_len + 2 * radius, last);
        for (i, o) in out.iter_mut().enumerate() {
            let Some(window) = line.get(i..i + taps.len()) else {
                continue;
            };
            let mut acc = 0.0f32;
            for (s, t) in window.iter().zip(taps) {
                acc += s * t;
            }
            *o = acc;
        }
    });
}

/// `src` is `width × height` row-major; returns its `height × width` transpose.
fn transpose(src: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; width * height];
    // Rows of the output are columns of the input.
    for_each_row_mut(&mut out, height, |x, out_row| {
        for (y, o) in out_row.iter_mut().enumerate() {
            *o = src.get(y * width + x).copied().unwrap_or(0.0);
        }
    });
    out
}
