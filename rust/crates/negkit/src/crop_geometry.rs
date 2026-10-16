//! Geometry for the unified Crop & Straighten mode — port of `CropGeometry.swift` (the
//! Lightroom/Photos model: the image rotates behind an axis-aligned crop box). No NegPy
//! counterpart: upstream has no straighten tool; this is SwiftInvert's own.
//!
//! Coordinates — "rotated space": screen-aligned axes, origin at the frame center, y-down, in
//! pixels of the unrotated frame. The frame content occupies its w×h rectangle rotated by θ
//! (clockwise-positive, matching [`RgbImage::fine_rotated`] and SwiftUI's `rotationEffect`).
//! The baked pipeline output at θ is the inscribed rect, centered; a [`NormalizedRect`] crop is
//! normalized over that inscribed rect.
//!
//! Points and sizes are `[f64; 2]` (Swift `SIMD2<Double>`): `[x, y]` / `[width, height]`.

use crate::{NormalizedRect, RgbImage, V2};

/// An axis-aligned box in rotated space: its center and half extents, in frame pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotatedBox {
    pub center: V2,
    pub half_extents: V2,
}

/// A pixel window `(x0, y0, x1, y1)` of a bitmap, as `f64` (Swift `SIMD4<Double>`).
pub type PixelWindow = [f64; 4];

/// Clockwise rotation in y-down coordinates (display map: screen = R(θ)·frame).
pub fn rotate(p: V2, radians: f64) -> V2 {
    let (s, c) = radians.sin_cos();
    [c * p[0] - s * p[1], s * p[0] + c * p[1]]
}

/// Inscribed (baked output) rect dimensions at θ.
pub fn inscribed_size(frame: V2, radians: f64) -> V2 {
    if radians.abs() <= 1e-9 {
        return frame;
    }
    let (w, h) = RgbImage::inscribed_rect_size(frame[0], frame[1], radians);
    [w, h]
}

/// Width÷height the canvas should LAY OUT the render at, in continuous space: the
/// orientation-only frame, through the straighten inscribed rect, through the crop — nothing
/// rounded to a pixel grid.
///
/// The rendered bitmap's own pixel ratio can't serve this. `pixel_roi` truncates the crop to
/// whole pixels and `fine_rotated` floors the inscribed rect, so the same settings produce
/// aspects that differ by up to ~0.2% between the 1536px proxy and the full-resolution tier.
/// Laying out from the bitmap therefore nudged the picture whenever HQ swapped in — and the
/// canvas `scaleEffect` multiplies that error by the zoom, which is precisely when the swap
/// happens. Both tiers describe the SAME continuous rectangle; lay out from that and let the
/// bitmap fill it.
///
/// `frame` is the orientation-only (90° steps + flip) size. Pass `crop: None` for the
/// uncropped presentations the tools use.
pub fn display_aspect(frame: V2, radians: f64, crop: Option<&NormalizedRect>) -> f64 {
    let mut size = inscribed_size(frame, radians);
    if let Some(crop) = crop {
        size[0] *= crop.width;
        size[1] *= crop.height;
    }
    if !(size[0] > 0.0 && size[1] > 0.0 && frame[1] > 0.0) {
        return (frame[0] / frame[1].max(1.0)).max(1e-6);
    }
    size[0] / size[1]
}

/// Where a rendered bitmap actually lands inside the ideal display rectangle, in units of
/// that rectangle — nominally (0, 0, 1, 1), off it by a fraction of a pixel.
///
/// Two stages quantize, and they quantize differently at every resolution: `fine_rotated`
/// FLOORS the inscribed rect (the bitmap covers slightly less than ideal, centred), and the
/// crop `pixel_roi` floors onto that bitmap's grid. So a 1536px proxy and a 6024px full-res
/// render of the same settings cover slightly different windows and can't both fill one rect
/// without disagreeing — about half a full-res pixel, ~1 pt of visible slide at 4× zoom.
/// Placing each bitmap at the window it really covers removes the disagreement exactly,
/// because both then resolve to the same affine map from frame coordinates to screen.
///
/// `oriented_pixels` is the fine-rotated bitmap's size; `roi` its crop window in that bitmap's
/// pixels (`None` = uncropped), as `(x0, y0, x1, y1)`. Measure a tier's bitmap against ITS
/// OWN frame — the proxy's frame for the HQ bitmap yields a window ~3.9× the ideal rect.
pub fn content_window(
    frame: V2,
    radians: f64,
    crop: Option<&NormalizedRect>,
    oriented_pixels: V2,
    roi: Option<PixelWindow>,
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
        origin[0] += size[0] * (roi[0] / oriented_pixels[0]);
        origin[1] += size[1] * (roi[1] / oriented_pixels[1]);
        size[0] *= (roi[2] - roi[0]) / oriented_pixels[0];
        size[1] *= (roi[3] - roi[1]) / oriented_pixels[1];
        origin[0] = (origin[0] - crop.x) / crop.width;
        origin[1] = (origin[1] - crop.y) / crop.height;
        size[0] /= crop.width;
        size[1] /= crop.height;
    }
    NormalizedRect::new(origin[0], origin[1], size[0], size[1])
}

/// The four signed corner offsets of a box, `(±hx, ±hy)`.
fn corners(half_extents: V2) -> [V2; 4] {
    let [hx, hy] = half_extents;
    [[-hx, -hy], [-hx, hy], [hx, -hy], [hx, hy]]
}

/// Does the axis-aligned box lie inside the θ-rotated frame?
pub fn box_fits(center: V2, half_extents: V2, radians: f64, frame: V2, tolerance: f64) -> bool {
    let bound = [frame[0] / 2.0 + tolerance, frame[1] / 2.0 + tolerance];
    corners(half_extents).iter().all(|c| {
        let q = rotate([center[0] + c[0], center[1] + c[1]], -radians);
        q[0].abs() <= bound[0] && q[1].abs() <= bound[1]
    })
}

/// Largest scale (≤ `max_scale`) of the box about its own center that keeps it inside the
/// θ-rotated frame. 0 when the center itself is outside.
pub fn fit_scale(center: V2, half_extents: V2, radians: f64, frame: V2, max_scale: f64) -> f64 {
    let u = rotate(center, -radians);
    let bound = [frame[0] / 2.0, frame[1] / 2.0];
    let mut k = max_scale;
    for c in corners(half_extents) {
        let t = rotate(c, -radians);
        for ((ta, ba), ua) in t.iter().zip(&bound).zip(&u) {
            if ta.abs() <= 1e-12 {
                continue;
            }
            // u + k·t must stay within ±bound on this axis.
            let limit = ((if *ta > 0.0 { *ba } else { -*ba }) - ua) / ta;
            k = k.min(limit.max(0.0));
        }
    }
    k
}

/// Clamp the center into the rotated frame, then scale the box about it to fit — the
/// Lightroom "constrain to image" resolution.
pub fn constrain(
    center: V2,
    half_extents: V2,
    radians: f64,
    frame: V2,
    max_scale: f64,
) -> RotatedBox {
    let mut u = rotate(center, -radians);
    let bound = [frame[0] / 2.0, frame[1] / 2.0];
    u[0] = u[0].clamp(-bound[0], bound[0]);
    u[1] = u[1].clamp(-bound[1], bound[1]);
    let c = rotate(u, radians);
    let k = fit_scale(c, half_extents, radians, frame, max_scale);
    RotatedBox {
        center: c,
        half_extents: [half_extents[0] * k, half_extents[1] * k],
    }
}

/// [`NormalizedRect`] over the inscribed rect at θ → rotated-space box (Swift `box(from:)`).
pub fn box_from_rect(rect: &NormalizedRect, frame: V2, radians: f64) -> RotatedBox {
    let ins = inscribed_size(frame, radians);
    RotatedBox {
        center: [
            (rect.x + rect.width / 2.0 - 0.5) * ins[0],
            (rect.y + rect.height / 2.0 - 0.5) * ins[1],
        ],
        half_extents: [rect.width * ins[0] / 2.0, rect.height * ins[1] / 2.0],
    }
}

/// Rotated-space box → [`NormalizedRect`] over the inscribed rect at θ.
pub fn normalized_rect(center: V2, half_extents: V2, frame: V2, radians: f64) -> NormalizedRect {
    let ins = inscribed_size(frame, radians);
    let min_corner = (
        (center[0] - half_extents[0]) / ins[0] + 0.5,
        (center[1] - half_extents[1]) / ins[1] + 0.5,
    );
    let max_corner = (
        (center[0] + half_extents[0]) / ins[0] + 0.5,
        (center[1] + half_extents[1]) / ins[1] + 0.5,
    );
    NormalizedRect::from_corners(min_corner, max_corner)
}

/// Content-preserving remap of a committed crop between straighten angles: the frame point
/// under the crop's center stays under it, the size is kept (shrunk only as far as the new
/// rotation demands), and the result re-normalizes over the new angle's inscribed rect.
pub fn remap_crop(
    rect: &NormalizedRect,
    old_radians: f64,
    new_radians: f64,
    frame: V2,
) -> NormalizedRect {
    let old = box_from_rect(rect, frame, old_radians);
    let content_center = rotate(old.center, -old_radians);
    let fitted = constrain(
        rotate(content_center, new_radians),
        old.half_extents,
        new_radians,
        frame,
        1.0,
    );
    normalized_rect(fitted.center, fitted.half_extents, frame, new_radians)
}
