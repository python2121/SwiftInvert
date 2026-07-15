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
