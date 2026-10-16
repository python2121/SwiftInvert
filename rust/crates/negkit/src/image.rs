//! Interleaved-RGB float image — port of `PlanarImage.swift` (`RGBImage`) and the `RGBImage`
//! extension in `NormalizedRect.swift` (`cropped`).
//!
//! The struct and constructors are the shared contract (other crates and agents compile against
//! them); the geometry ops mirror the Swift file with its tests (`OrientationTests`,
//! `ImagePipelineSeamTests`, `AuditRectAndGeometryTests` — `tests/geometry.rs`).
//!
//! Allocation note (the "big-buffer discipline"): `vec![0.0f32; n]` is fine in Rust — it goes
//! through `alloc_zeroed`, so fresh pages are never touched until written. What costs is a
//! non-zero fill or a `resize` after the fact. Prefer building with iterators / `extend` or writing
//! every lane of a zeroed buffer exactly once.
//!
//! Ops that may return the input unchanged (`applying_flip(0)`, `fine_rotated(0)`,
//! `downsampled` at or below the cap, `cropped` with a degenerate rect) return a
//! [`Cow`] so the identity case never copies a full-resolution buffer — Swift's `return self`
//! was a free copy-on-write share.
//!
//! Row loops run on `rayon` when the `rayon` feature is on. Every output sample sums its taps in a
//! fixed order, so the parallel split is bit-for-bit the serial result
//! (`parallelDownsampleMatchesSerialAtEveryRaggedHeight`).

use std::borrow::Cow;

use crate::NegError;
use crate::rect::{NormalizedRect, PixelRoi};

/// Interleaved RGB `f32`, row-major, nominally scene-linear in `[0, 1]`.
///
/// Layout is `data[(y * width + x) * 3 + channel]` — the same bytes the GPU storage buffers hold,
/// so uploads and readbacks are memcpys.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

/// Fractional box taps for a 1-D area resample (see [`RgbImage::area_taps`]).
///
/// Output `j` reads source cells `start[j] + k` for `k in 0..stride`, each weighted by
/// `weight[j * stride + k]` (pre-normalized; trailing weights may be zero).
#[derive(Clone, Debug, PartialEq)]
pub struct AreaTaps {
    pub start: Vec<usize>,
    pub weight: Vec<f32>,
    pub stride: usize,
}

/// Run `f(row_index, row)` over every `row_len`-sized chunk of `data`, in parallel when the
/// `rayon` feature is on. No-op when `row_len == 0`.
pub(crate) fn for_each_row_mut<F>(data: &mut [f32], row_len: usize, f: F)
where
    F: Fn(usize, &mut [f32]) + Sync + Send,
{
    if row_len == 0 {
        return;
    }
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        data.par_chunks_exact_mut(row_len)
            .enumerate()
            .for_each(|(y, row)| f(y, row));
    }
    #[cfg(not(feature = "rayon"))]
    {
        data.chunks_exact_mut(row_len)
            .enumerate()
            .for_each(|(y, row)| f(y, row));
    }
}

/// `buf[i]` without the panic path: out-of-range reads yield 0 (never reached when the index
/// math is right; keeps the no-panic crate rule honest on hostile dimensions).
#[inline(always)]
fn at(buf: &[f32], i: usize) -> f32 {
    buf.get(i).copied().unwrap_or(0.0)
}

impl RgbImage {
    /// Wrap an interleaved buffer; the length must be `width * height * 3`.
    pub fn from_vec(width: usize, height: usize, data: Vec<f32>) -> Result<Self, NegError> {
        let want = width.checked_mul(height).and_then(|n| n.checked_mul(3));
        match want {
            Some(n) if n == data.len() && n > 0 => Ok(Self {
                width,
                height,
                data,
            }),
            Some(_) | None if width == 0 || height == 0 => Err(NegError::Empty),
            _ => Err(NegError::Shape {
                width,
                height,
                len: data.len(),
            }),
        }
    }

    /// A zero-filled image (`alloc_zeroed`: pages are not touched until written).
    pub fn zeros(width: usize, height: usize) -> Result<Self, NegError> {
        let n = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(3))
            .ok_or(NegError::Invalid("image too large"))?;
        if n == 0 {
            return Err(NegError::Empty);
        }
        Ok(Self {
            width,
            height,
            data: vec![0.0; n],
        })
    }

    /// Build by filling a zeroed buffer in place (every lane written exactly once by `fill`).
    pub fn new_with(
        width: usize,
        height: usize,
        fill: impl FnOnce(&mut [f32]),
    ) -> Result<Self, NegError> {
        let mut img = Self::zeros(width, height)?;
        fill(&mut img.data);
        Ok(img)
    }

    #[inline]
    pub fn pixel_count(&self) -> usize {
        self.width * self.height
    }

    /// The RGB triple at `(x, y)`; `None` outside the image (never panics on hostile coordinates).
    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> Option<[f32; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y * self.width + x) * 3;
        self.data.get(i..i + 3).map(|p| [p[0], p[1], p[2]])
    }

    /// One image row as an interleaved slice.
    #[inline]
    pub fn row(&self, y: usize) -> Option<&[f32]> {
        let w3 = self.width * 3;
        self.data.get(y * w3..(y + 1) * w3)
    }

    /// Long edge in pixels.
    #[inline]
    pub fn long_edge(&self) -> usize {
        self.width.max(self.height)
    }

    /// Build an `ow × oh` image whose pixel `(ox, oy)` is the source pixel `source(ox, oy)` —
    /// the shared body of the 90° rotations. The source coordinates are bounds-checked, so a
    /// wrong mapping reads black rather than trapping.
    fn remapped(
        &self,
        ow: usize,
        oh: usize,
        source: impl Fn(usize, usize) -> (usize, usize) + Sync,
    ) -> Self {
        let mut out = Self {
            width: ow,
            height: oh,
            data: vec![0.0; ow * oh * 3],
        };
        for_each_row_mut(&mut out.data, ow * 3, |oy, row| {
            for (ox, px) in row.chunks_exact_mut(3).enumerate() {
                let (sx, sy) = source(ox, oy);
                if let Some(p) = self.pixel(sx, sy) {
                    px.copy_from_slice(&p);
                }
            }
        });
        out
    }

    /// Rotate/flip per EXIF-style LibRaw flip codes (dcraw convention):
    /// 0 = none, 3 = 180°, 5 = 90° CCW, 6 = 90° CW. Any other code is a no-op (borrowed).
    pub fn applying_flip(&self, flip: i32) -> Cow<'_, Self> {
        let (w, h) = (self.width, self.height);
        match flip {
            3 => {
                // out[y, x] = self[h−1−y, w−1−x]: the pixel sequence reversed.
                let data: Vec<f32> = self
                    .data
                    .chunks_exact(3)
                    .rev()
                    .flat_map(|p| p.iter().copied())
                    .collect();
                Cow::Owned(Self {
                    width: w,
                    height: h,
                    data,
                })
            }
            // 90° counter-clockwise: out is h×w; out[y, x] = self[x, w−1−y].
            5 => Cow::Owned(self.remapped(h, w, |ox, oy| (w.wrapping_sub(1).wrapping_sub(oy), ox))),
            // 90° clockwise: out is h×w; out[y, x] = self[h−1−x, y].
            6 => Cow::Owned(self.remapped(h, w, |ox, oy| (oy, h.wrapping_sub(1).wrapping_sub(ox)))),
            _ => Cow::Borrowed(self),
        }
    }

    /// Mirror left↔right.
    pub fn flipped_horizontally(&self) -> Self {
        let w3 = self.width * 3;
        let mut out = Self {
            width: self.width,
            height: self.height,
            data: vec![0.0; self.data.len()],
        };
        for_each_row_mut(&mut out.data, w3, |y, row| {
            if let Some(src) = self.row(y) {
                for (dst, p) in row.chunks_exact_mut(3).zip(src.chunks_exact(3).rev()) {
                    dst.copy_from_slice(p);
                }
            }
        });
        out
    }

    /// User orientation: rotation in clockwise 90° steps, then an optional horizontal flip
    /// (applied in display space, after the rotation). Orientation-only — the frame analysis
    /// meters (`ImageSession.meterPreview`); see [`RgbImage::oriented_with_fine_rotation`].
    pub fn oriented(&self, rotation_cw: i32, flip_horizontal: bool) -> Cow<'_, Self> {
        self.oriented_with_fine_rotation(rotation_cw, flip_horizontal, 0.0)
    }

    /// [`RgbImage::oriented`] followed by the straighten angle (clockwise-positive degrees,
    /// skipped below 0.005°) — Swift's `oriented(rotationCW:flipHorizontal:fineRotation:)`.
    pub fn oriented_with_fine_rotation(
        &self,
        rotation_cw: i32,
        flip_horizontal: bool,
        fine_rotation: f64,
    ) -> Cow<'_, Self> {
        let mut img: Cow<'_, Self> = Cow::Borrowed(self);
        match rotation_cw.rem_euclid(360) {
            90 => img = Cow::Owned(img.applying_flip(6).into_owned()),
            180 => img = Cow::Owned(img.applying_flip(3).into_owned()),
            270 => img = Cow::Owned(img.applying_flip(5).into_owned()),
            _ => {}
        }
        if flip_horizontal {
            img = Cow::Owned(img.flipped_horizontally());
        }
        if fine_rotation.abs() > 0.005 {
            img = Cow::Owned(img.fine_rotated(fine_rotation).into_owned());
        }
        img
    }

    /// Largest axis-aligned rectangle (maximal area) inscribed in a `w×h` image rotated by
    /// `radians` — the standard straighten auto-crop. Returns `(width, height)`.
    pub fn inscribed_rect_size(w: f64, h: f64, radians: f64) -> (f64, f64) {
        let sin_a = radians.sin().abs();
        let cos_a = radians.cos().abs();
        let long_side = w.max(h);
        let short_side = w.min(h);
        if short_side <= 2.0 * sin_a * cos_a * long_side || (sin_a - cos_a).abs() < 1e-10 {
            // Half-constrained: opposite corners touch the long sides.
            let x = 0.5 * short_side;
            return if w >= h {
                (x / sin_a, x / cos_a)
            } else {
                (x / cos_a, x / sin_a)
            };
        }
        let cos_2a = cos_a * cos_a - sin_a * sin_a;
        (
            (w * cos_a - h * sin_a) / cos_2a,
            (h * cos_a - w * sin_a) / cos_2a,
        )
    }

    /// Arbitrary-angle rotation (clockwise-positive, display space), bilinear resampled and
    /// auto-cropped to the largest inscribed rectangle so no empty corners exist — keeps the
    /// analysis meters and display clean. The inscribed size is FLOORED on this tier's own
    /// pixel grid (the tier-swap stability story in `CLAUDE.md`). Borrowed below 1e-6 rad.
    pub fn fine_rotated(&self, degrees: f64) -> Cow<'_, Self> {
        let radians = degrees * std::f64::consts::PI / 180.0;
        if radians.abs() <= 1e-6 {
            return Cow::Borrowed(self);
        }
        let (w, h) = (self.width as f64, self.height as f64);
        let (iw, ih) = Self::inscribed_rect_size(w, h, radians);
        // Finite by construction; `as usize` floors (and saturates on garbage).
        let ow = (iw.floor() as usize).max(1);
        let oh = (ih.floor() as usize).max(1);

        // Clockwise visual rotation in y-down coordinates: sample the source through the
        // inverse rotation about both centers.
        let (cs, sn) = (radians.cos(), radians.sin());
        let (cx, cy) = ((w - 1.0) / 2.0, (h - 1.0) / 2.0);
        let (ocx, ocy) = ((ow as f64 - 1.0) / 2.0, (oh as f64 - 1.0) / 2.0);
        let max_x = self.width.saturating_sub(1);
        let max_y = self.height.saturating_sub(1);
        let width = self.width;
        let src = &self.data;

        let mut out = Self {
            width: ow,
            height: oh,
            data: vec![0.0; ow * oh * 3],
        };
        for_each_row_mut(&mut out.data, ow * 3, |oy, row| {
            let dy = oy as f64 - ocy;
            for (ox, px) in row.chunks_exact_mut(3).enumerate() {
                let dx = ox as f64 - ocx;
                let sx = cx + cs * dx + sn * dy;
                let sy = cy - sn * dx + cs * dy;
                // Floor then clamp, like Swift's `min(max(Int(sx.rounded(.down)), 0), maxX)`.
                let x0 = (sx.floor().max(0.0) as usize).min(max_x);
                let y0 = (sy.floor().max(0.0) as usize).min(max_y);
                let x1 = (x0 + 1).min(max_x);
                let y1 = (y0 + 1).min(max_y);
                let fx = (sx - x0 as f64).clamp(0.0, 1.0) as f32;
                let fy = (sy - y0 as f64).clamp(0.0, 1.0) as f32;
                let i00 = (y0 * width + x0) * 3;
                let i10 = (y0 * width + x1) * 3;
                let i01 = (y1 * width + x0) * 3;
                let i11 = (y1 * width + x1) * 3;
                for (c, lane) in px.iter_mut().enumerate() {
                    let top = at(src, i00 + c) * (1.0 - fx) + at(src, i10 + c) * fx;
                    let bottom = at(src, i01 + c) * (1.0 - fx) + at(src, i11 + c) * fx;
                    *lane = top * (1.0 - fy) + bottom * fy;
                }
            }
        });
        Cow::Owned(out)
    }

    /// Fractional box taps for a 1-D area resample: output `j` covers the source span
    /// `[j·s, (j+1)·s)` and each source cell is weighted by how much of it that span actually
    /// covers. Weights are pre-normalized.
    ///
    /// This is cv2's INTER_AREA kernel. The distinction that matters is the SAMPLE PHASE:
    /// integer box boundaries (`Int(j·s)`) give boxes of varying width whose centres drift from
    /// the ideal grid — up to 0.97 source px at the preview's 1.96 ratio, sliding smoothly across
    /// the frame. That is a low-frequency geometric warp, not a blur, and it made the proxy and
    /// the full-resolution tier disagree locally by ~1.5 pt at 4× zoom. Fractional weights put
    /// every sample exactly on the ideal centre.
    pub fn area_taps(src: usize, dst: usize) -> AreaTaps {
        if src == 0 || dst == 0 {
            return AreaTaps {
                start: Vec::new(),
                weight: Vec::new(),
                stride: 0,
            };
        }
        let s = src as f64 / dst as f64;
        let stride = s.ceil() as usize + 1;
        let mut start = vec![0usize; dst];
        let mut weight = vec![0f32; dst * stride];
        for (j, (start_j, weights_j)) in start
            .iter_mut()
            .zip(weight.chunks_exact_mut(stride))
            .enumerate()
        {
            let f0 = j as f64 * s;
            let f1 = (j + 1) as f64 * s;
            let i0 = f0.floor().max(0.0) as usize;
            let i1 = src.min((f1.ceil() as usize).max(i0 + 1));
            *start_j = i0;
            let mut norm = 0.0f64;
            for (k, wk) in weights_j.iter_mut().enumerate().take(i1.saturating_sub(i0)) {
                let i = i0 + k;
                let overlap = ((i + 1) as f64).min(f1) - (i as f64).max(f0);
                if overlap > 0.0 {
                    *wk = overlap as f32;
                    norm += overlap;
                }
            }
            if norm > 0.0 {
                let inv = (1.0 / norm) as f32;
                for wk in weights_j.iter_mut() {
                    *wk *= inv;
                }
            } else if let Some(first) = weights_j.first_mut() {
                *first = 1.0; // degenerate span: take the one cell
            }
        }
        AreaTaps {
            start,
            weight,
            stride,
        }
    }

    /// Area-average downsample so the long edge is at most `max_long_edge` — NegPy's
    /// `cv2.resize(..., INTER_AREA)` preview resize (`services/rendering/image_processor.py`),
    /// which this matches in kernel as well as in intent. Borrowed when already within the cap.
    ///
    /// Separable: a 2-D area box is the product of two 1-D boxes, so a horizontal pass followed
    /// by a vertical one is exactly the 2-D average and costs far less than the naive form.
    pub fn downsampled(&self, max_long_edge: usize) -> Cow<'_, Self> {
        let long = self.long_edge();
        if long <= max_long_edge {
            return Cow::Borrowed(self);
        }
        let scale = max_long_edge as f64 / long as f64;
        // Schoolbook rounding (`f64::round` == Swift `.rounded()`), finite by construction.
        let ow = ((self.width as f64 * scale).round() as usize).max(1);
        let oh = ((self.height as f64 * scale).round() as usize).max(1);
        let (w, h) = (self.width, self.height);
        let hx = Self::area_taps(w, ow);
        let vy = Self::area_taps(h, oh);

        // Horizontal pass into an ow × h intermediate. Rows are independent, and each output
        // sample's taps are summed in a fixed order, so the parallel split is bit-for-bit the
        // serial result.
        let mut mid = vec![0f32; ow * h * 3];
        let src = &self.data;
        for_each_row_mut(&mut mid, ow * 3, |y, mrow| {
            let row = y * w;
            for (ox, px) in mrow.chunks_exact_mut(3).enumerate() {
                let s0 = at_usize(&hx.start, ox);
                let (mut r, mut g, mut b) = (0f32, 0f32, 0f32);
                for k in 0..hx.stride {
                    let weight = at(&hx.weight, ox * hx.stride + k);
                    if weight == 0.0 {
                        continue;
                    }
                    let i = (row + (s0 + k).min(w.saturating_sub(1))) * 3;
                    r += at(src, i) * weight;
                    g += at(src, i + 1) * weight;
                    b += at(src, i + 2) * weight;
                }
                px.copy_from_slice(&[r, g, b]);
            }
        });

        let mut out = Self {
            width: ow,
            height: oh,
            data: vec![0.0; ow * oh * 3],
        };
        let mid = &mid;
        for_each_row_mut(&mut out.data, ow * 3, |oy, orow| {
            let s0 = at_usize(&vy.start, oy);
            for (ox, px) in orow.chunks_exact_mut(3).enumerate() {
                let (mut r, mut g, mut b) = (0f32, 0f32, 0f32);
                for k in 0..vy.stride {
                    let weight = at(&vy.weight, oy * vy.stride + k);
                    if weight == 0.0 {
                        continue;
                    }
                    let i = ((s0 + k).min(h.saturating_sub(1)) * ow + ox) * 3;
                    r += at(mid, i) * weight;
                    g += at(mid, i + 1) * weight;
                    b += at(mid, i + 2) * weight;
                }
                px.copy_from_slice(&[r, g, b]);
            }
        });
        Cow::Owned(out)
    }

    /// Axis-aligned crop; borrowed (unchanged) when the rect is degenerate
    /// (see [`NormalizedRect::pixel_roi`]).
    pub fn cropped(&self, rect: &NormalizedRect) -> Cow<'_, Self> {
        match rect.pixel_roi(self.width, self.height) {
            Some(roi) => self.cropped_to_pixels(roi),
            None => Cow::Borrowed(self),
        }
    }

    /// Crop to an explicit pixel ROI (the shared implementation behind [`RgbImage::cropped`];
    /// clamps out-of-range bounds and returns the input, borrowed, when the window is degenerate).
    pub fn cropped_to_pixels(&self, roi: PixelRoi) -> Cow<'_, Self> {
        let x0 = roi.x0;
        let y0 = roi.y0;
        let x1 = roi.x1.min(self.width);
        let y1 = roi.y1.min(self.height);
        if x1 < x0 + 2 || y1 < y0 + 2 {
            return Cow::Borrowed(self);
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let mut out = Self {
            width: w,
            height: h,
            data: vec![0.0; w * h * 3],
        };
        for_each_row_mut(&mut out.data, w * 3, |y, dst| {
            if let Some(src) = self.row(y + y0).and_then(|r| r.get(x0 * 3..x1 * 3)) {
                dst.copy_from_slice(src);
            }
        });
        Cow::Owned(out)
    }
}

/// `buf[i]` for the tap-start tables, 0 when out of range (see [`at`]).
#[inline(always)]
fn at_usize(buf: &[usize], i: usize) -> usize {
    buf.get(i).copied().unwrap_or(0)
}
