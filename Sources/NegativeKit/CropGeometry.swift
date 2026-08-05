import Foundation
import simd

/// Geometry for the unified Crop & Straighten mode (the Lightroom/Photos
/// model: the image rotates behind an axis-aligned crop box).
///
/// Coordinates — "rotated space": screen-aligned axes, origin at the frame
/// center, y-down, in pixels of the unrotated frame. The frame content
/// occupies its w×h rectangle rotated by θ (clockwise-positive, matching
/// `RGBImage.fineRotated` and SwiftUI's `rotationEffect`). The baked pipeline
/// output at θ is the inscribed rect, centered; a `NormalizedRect` crop is
/// normalized over that inscribed rect.
public enum CropGeometry {
    /// Clockwise rotation in y-down coordinates (display map: screen = R(θ)·frame).
    public static func rotate(_ p: SIMD2<Double>, by radians: Double) -> SIMD2<Double> {
        let c = cos(radians)
        let s = sin(radians)
        return SIMD2(c * p.x - s * p.y, s * p.x + c * p.y)
    }

    /// Inscribed (baked output) rect dimensions at θ.
    public static func inscribedSize(frame: SIMD2<Double>, radians: Double) -> SIMD2<Double> {
        guard abs(radians) > 1e-9 else { return frame }
        let r = RGBImage.inscribedRectSize(width: frame.x, height: frame.y, radians: radians)
        return SIMD2(r.width, r.height)
    }

    /// Width÷height the canvas should LAY OUT the render at, in continuous
    /// space: the orientation-only frame, through the straighten inscribed
    /// rect, through the crop — nothing rounded to a pixel grid.
    ///
    /// The rendered bitmap's own pixel ratio can't serve this. `pixelROI`
    /// truncates the crop to whole pixels and `fineRotated` floors the
    /// inscribed rect, so the same settings produce aspects that differ by up
    /// to ~0.2% between the 1536px proxy and the full-resolution tier. Laying
    /// out from the bitmap therefore nudged the picture whenever HQ swapped in
    /// — and the canvas `scaleEffect` multiplies that error by the zoom, which
    /// is precisely when the swap happens. Both tiers describe the SAME
    /// continuous rectangle; lay out from that and let the bitmap fill it.
    ///
    /// `frame` is the orientation-only (90° steps + flip) size. Pass
    /// `crop: nil` for the uncropped presentations the tools use.
    public static func displayAspect(
        frame: SIMD2<Double>, radians: Double, crop: NormalizedRect?
    ) -> Double {
        var size = inscribedSize(frame: frame, radians: radians)
        if let crop {
            size.x *= crop.width
            size.y *= crop.height
        }
        guard size.x > 0, size.y > 0, frame.y > 0 else { return max(frame.x / max(frame.y, 1), 1e-6) }
        return size.x / size.y
    }

    /// Where a rendered bitmap actually lands inside the ideal display
    /// rectangle, in units of that rectangle — nominally (0, 0, 1, 1), off it
    /// by a fraction of a pixel.
    ///
    /// Two stages quantize, and they quantize differently at every resolution:
    /// `fineRotated` FLOORS the inscribed rect (the bitmap covers slightly less
    /// than ideal, centred), and the crop `pixelROI` floors onto that bitmap's
    /// grid. So a 1536px proxy and a 6024px full-res render of the same
    /// settings cover slightly different windows and can't both fill one rect
    /// without disagreeing — about half a full-res pixel, ~1 pt of visible
    /// slide at 4× zoom. Placing each bitmap at the window it really covers
    /// removes the disagreement exactly, because both then resolve to the same
    /// affine map from frame coordinates to screen.
    ///
    /// `orientedPixels` is the fine-rotated bitmap's size; `roi` its crop
    /// window in that bitmap's pixels (nil = uncropped), as (x0, y0, x1, y1).
    public static func contentWindow(
        frame: SIMD2<Double>, radians: Double, crop: NormalizedRect?,
        orientedPixels: SIMD2<Double>, roi: SIMD4<Double>?
    ) -> NormalizedRect {
        let inscribed = inscribedSize(frame: frame, radians: radians)
        guard inscribed.x > 0, inscribed.y > 0, orientedPixels.x > 0, orientedPixels.y > 0 else {
            return NormalizedRect(x: 0, y: 0, width: 1, height: 1)
        }
        // Stage 1, in units of the ideal inscribed rect.
        var origin = SIMD2(
            (1 - orientedPixels.x / inscribed.x) / 2, (1 - orientedPixels.y / inscribed.y) / 2)
        var size = SIMD2(orientedPixels.x / inscribed.x, orientedPixels.y / inscribed.y)
        // Stage 2: fold in the crop ROI, then re-express in units of the ideal
        // CROPPED rect — which is what the canvas fits.
        if let crop, let roi, crop.width > 0, crop.height > 0 {
            origin.x += size.x * (roi.x / orientedPixels.x)
            origin.y += size.y * (roi.y / orientedPixels.y)
            size.x *= (roi.z - roi.x) / orientedPixels.x
            size.y *= (roi.w - roi.y) / orientedPixels.y
            origin.x = (origin.x - crop.x) / crop.width
            origin.y = (origin.y - crop.y) / crop.height
            size.x /= crop.width
            size.y /= crop.height
        }
        return NormalizedRect(x: origin.x, y: origin.y, width: size.x, height: size.y)
    }

    /// Does the axis-aligned box lie inside the θ-rotated frame?
    public static func boxFits(
        center: SIMD2<Double>, halfExtents: SIMD2<Double>, radians: Double,
        frame: SIMD2<Double>, tolerance: Double = 1e-6
    ) -> Bool {
        let bound = frame / 2 + SIMD2(repeating: tolerance)
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                let corner = center + SIMD2(sx * halfExtents.x, sy * halfExtents.y)
                let q = rotate(corner, by: -radians)
                if abs(q.x) > bound.x || abs(q.y) > bound.y { return false }
            }
        }
        return true
    }

    /// Largest scale (≤ maxScale) of the box about its own center that keeps
    /// it inside the θ-rotated frame. 0 when the center itself is outside.
    public static func fitScale(
        center: SIMD2<Double>, halfExtents: SIMD2<Double>, radians: Double,
        frame: SIMD2<Double>, maxScale: Double = 1.0
    ) -> Double {
        let u = rotate(center, by: -radians)
        let bound = frame / 2
        var k = maxScale
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                let t = rotate(SIMD2(sx * halfExtents.x, sy * halfExtents.y), by: -radians)
                for axis in 0..<2 {
                    let ta = t[axis]
                    guard abs(ta) > 1e-12 else { continue }
                    // u + k·t must stay within ±bound on this axis.
                    let limit = ((ta > 0 ? bound[axis] : -bound[axis]) - u[axis]) / ta
                    k = min(k, max(limit, 0))
                }
            }
        }
        return k
    }

    /// Clamp the center into the rotated frame, then scale the box about it
    /// to fit — the Lightroom "constrain to image" resolution.
    public static func constrain(
        center: SIMD2<Double>, halfExtents: SIMD2<Double>, radians: Double,
        frame: SIMD2<Double>, maxScale: Double = 1.0
    ) -> (center: SIMD2<Double>, halfExtents: SIMD2<Double>) {
        var u = rotate(center, by: -radians)
        let bound = frame / 2
        u.x = min(max(u.x, -bound.x), bound.x)
        u.y = min(max(u.y, -bound.y), bound.y)
        let c = rotate(u, by: radians)
        let k = fitScale(
            center: c, halfExtents: halfExtents, radians: radians, frame: frame, maxScale: maxScale)
        return (c, halfExtents * k)
    }

    /// NormalizedRect over the inscribed rect at θ → rotated-space box.
    public static func box(
        from rect: NormalizedRect, frame: SIMD2<Double>, radians: Double
    ) -> (center: SIMD2<Double>, halfExtents: SIMD2<Double>) {
        let ins = inscribedSize(frame: frame, radians: radians)
        let halfExtents = SIMD2(rect.width * ins.x / 2, rect.height * ins.y / 2)
        let center = SIMD2(
            (rect.x + rect.width / 2 - 0.5) * ins.x,
            (rect.y + rect.height / 2 - 0.5) * ins.y)
        return (center, halfExtents)
    }

    /// Rotated-space box → NormalizedRect over the inscribed rect at θ.
    public static func normalizedRect(
        center: SIMD2<Double>, halfExtents: SIMD2<Double>, frame: SIMD2<Double>, radians: Double
    ) -> NormalizedRect {
        let ins = inscribedSize(frame: frame, radians: radians)
        let minCorner = (center - halfExtents) / ins + SIMD2(repeating: 0.5)
        let maxCorner = (center + halfExtents) / ins + SIMD2(repeating: 0.5)
        return NormalizedRect(
            from: (x: minCorner.x, y: minCorner.y), to: (x: maxCorner.x, y: maxCorner.y))
    }

    /// Content-preserving remap of a committed crop between straighten angles:
    /// the frame point under the crop's center stays under it, the size is
    /// kept (shrunk only as far as the new rotation demands), and the result
    /// re-normalizes over the new angle's inscribed rect.
    public static func remapCrop(
        _ rect: NormalizedRect, from oldRadians: Double, to newRadians: Double,
        frame: SIMD2<Double>
    ) -> NormalizedRect {
        let old = box(from: rect, frame: frame, radians: oldRadians)
        let contentCenter = rotate(old.center, by: -oldRadians)
        let (center, halfExtents) = constrain(
            center: rotate(contentCenter, by: newRadians), halfExtents: old.halfExtents,
            radians: newRadians, frame: frame)
        return normalizedRect(
            center: center, halfExtents: halfExtents, frame: frame, radians: newRadians)
    }
}
