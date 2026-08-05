import Dispatch
import Foundation

/// Interleaved RGB float32 image buffer, row-major, values scene-linear in [0, 1]
/// (the direct equivalent of NegPy's `(H, W, 3) float32` numpy buffers).
public struct RGBImage: @unchecked Sendable {
    public var pixels: [Float]  // count == width * height * 3
    public let width: Int
    public let height: Int

    public init(pixels: [Float], width: Int, height: Int) {
        precondition(pixels.count == width * height * 3, "pixel count mismatch")
        self.pixels = pixels
        self.width = width
        self.height = height
    }

    public init(width: Int, height: Int, fill: Float = 0) {
        self.pixels = [Float](repeating: fill, count: width * height * 3)
        self.width = width
        self.height = height
    }

    /// Allocate WITHOUT the zero-fill, for buffers whose every lane the caller
    /// writes. `repeating:` costs a full pass over the buffer that the write
    /// then immediately discards — 290 MB of it at export size. `body` must
    /// initialize all `width * height * 3` elements.
    public init(
        width: Int, height: Int,
        initializingWith body: (UnsafeMutableBufferPointer<Float>) -> Void
    ) {
        self.width = width
        self.height = height
        self.pixels = [Float](unsafeUninitializedCapacity: width * height * 3) { buffer, count in
            body(buffer)
            count = width * height * 3
        }
    }

    @inlinable
    public subscript(y: Int, x: Int, c: Int) -> Float {
        get { pixels[(y * width + x) * 3 + c] }
        set { pixels[(y * width + x) * 3 + c] = newValue }
    }

    /// Rotate/flip per EXIF-style LibRaw flip codes (dcraw convention):
    /// 0 = none, 3 = 180°, 5 = 90° CCW, 6 = 90° CW.
    public func applyingFlip(_ flip: Int32) -> RGBImage {
        switch flip {
        case 3:
            var out = RGBImage(width: width, height: height)
            for y in 0..<height {
                for x in 0..<width {
                    for c in 0..<3 { out[y, x, c] = self[height - 1 - y, width - 1 - x, c] }
                }
            }
            return out
        case 5:  // 90° counter-clockwise
            var out = RGBImage(width: height, height: width)
            for y in 0..<width {
                for x in 0..<height {
                    for c in 0..<3 { out[y, x, c] = self[x, width - 1 - y, c] }
                }
            }
            return out
        case 6:  // 90° clockwise
            var out = RGBImage(width: height, height: width)
            for y in 0..<width {
                for x in 0..<height {
                    for c in 0..<3 { out[y, x, c] = self[height - 1 - x, y, c] }
                }
            }
            return out
        default:
            return self
        }
    }

    /// Mirror left↔right.
    public func flippedHorizontally() -> RGBImage {
        var out = RGBImage(width: width, height: height)
        for y in 0..<height {
            for x in 0..<width {
                for c in 0..<3 { out[y, x, c] = self[y, width - 1 - x, c] }
            }
        }
        return out
    }

    /// User orientation: rotation in clockwise 90° steps, then an optional
    /// horizontal flip (applied in display space, after the rotation).
    public func oriented(rotationCW: Int, flipHorizontal: Bool, fineRotation: Double = 0) -> RGBImage {
        var img = self
        switch ((rotationCW % 360) + 360) % 360 {
        case 90: img = img.applyingFlip(6)
        case 180: img = img.applyingFlip(3)
        case 270: img = img.applyingFlip(5)
        default: break
        }
        if flipHorizontal { img = img.flippedHorizontally() }
        if abs(fineRotation) > 0.005 { img = img.fineRotated(degrees: fineRotation) }
        return img
    }

    /// Largest axis-aligned rectangle (maximal area) inscribed in a w×h image
    /// rotated by `radians` — the standard straighten auto-crop.
    public static func inscribedRectSize(width w: Double, height h: Double, radians: Double)
        -> (width: Double, height: Double)
    {
        let sinA = abs(sin(radians)), cosA = abs(cos(radians))
        let longSide = max(w, h), shortSide = min(w, h)
        if shortSide <= 2 * sinA * cosA * longSide || abs(sinA - cosA) < 1e-10 {
            // Half-constrained: opposite corners touch the long sides.
            let x = 0.5 * shortSide
            return w >= h ? (x / sinA, x / cosA) : (x / cosA, x / sinA)
        }
        let cos2A = cosA * cosA - sinA * sinA
        return ((w * cosA - h * sinA) / cos2A, (h * cosA - w * sinA) / cos2A)
    }

    /// Arbitrary-angle rotation (clockwise-positive, display space), bilinear
    /// resampled and auto-cropped to the largest inscribed rectangle so no
    /// empty corners exist — keeps the analysis meters and display clean.
    public func fineRotated(degrees: Double) -> RGBImage {
        let radians = degrees * .pi / 180
        guard abs(radians) > 1e-6 else { return self }
        let w = Double(width), h = Double(height)
        let inscribed = Self.inscribedRectSize(width: w, height: h, radians: radians)
        let ow = max(Int(inscribed.width.rounded(.down)), 1)
        let oh = max(Int(inscribed.height.rounded(.down)), 1)

        var out = RGBImage(width: ow, height: oh)
        // Clockwise visual rotation in y-down coordinates: sample the source
        // through the inverse rotation about both centers.
        let cs = cos(radians), sn = sin(radians)
        let cx = (w - 1) / 2, cy = (h - 1) / 2
        let ocx = (Double(ow) - 1) / 2, ocy = (Double(oh) - 1) / 2
        let maxX = width - 1, maxY = height - 1

        out.pixels.withUnsafeMutableBufferPointer { dst in
            pixels.withUnsafeBufferPointer { src in
                for oy in 0..<oh {
                    let dy = Double(oy) - ocy
                    for ox in 0..<ow {
                        let dx = Double(ox) - ocx
                        let sx = cx + cs * dx + sn * dy
                        let sy = cy - sn * dx + cs * dy
                        let x0 = min(max(Int(sx.rounded(.down)), 0), maxX)
                        let y0 = min(max(Int(sy.rounded(.down)), 0), maxY)
                        let x1 = min(x0 + 1, maxX)
                        let y1 = min(y0 + 1, maxY)
                        let fx = Float(min(max(sx - Double(x0), 0), 1))
                        let fy = Float(min(max(sy - Double(y0), 0), 1))
                        let i00 = (y0 * width + x0) * 3
                        let i10 = (y0 * width + x1) * 3
                        let i01 = (y1 * width + x0) * 3
                        let i11 = (y1 * width + x1) * 3
                        let o = (oy * ow + ox) * 3
                        for c in 0..<3 {
                            let top = src[i00 + c] * (1 - fx) + src[i10 + c] * fx
                            let bottom = src[i01 + c] * (1 - fx) + src[i11 + c] * fx
                            dst[o + c] = top * (1 - fy) + bottom * fy
                        }
                    }
                }
            }
        }
        return out
    }

    /// Fractional box taps for a 1-D area resample: output `j` covers the
    /// source span `[j·s, (j+1)·s)` and each source cell is weighted by how
    /// much of it that span actually covers. Weights are pre-normalized.
    ///
    /// This is cv2's INTER_AREA kernel. The distinction that matters is the
    /// SAMPLE PHASE: integer box boundaries (`Int(j·s)`) give boxes of varying
    /// width whose centres drift from the ideal grid — up to 0.97 source px at
    /// the preview's 1.96 ratio, sliding smoothly across the frame. That is a
    /// low-frequency geometric warp, not a blur, and it made the proxy and the
    /// full-resolution tier disagree locally by ~1.5 pt at 4× zoom. Fractional
    /// weights put every sample exactly on the ideal centre.
    static func areaTaps(src: Int, dst: Int) -> (start: [Int], weight: [Float], stride: Int) {
        let s = Double(src) / Double(dst)
        let stride = Int(s.rounded(.up)) + 1
        var start = [Int](repeating: 0, count: dst)
        var weight = [Float](repeating: 0, count: dst * stride)
        for j in 0..<dst {
            let f0 = Double(j) * s, f1 = Double(j + 1) * s
            let i0 = max(Int(f0.rounded(.down)), 0)
            let i1 = min(src, max(Int(f1.rounded(.up)), i0 + 1))
            start[j] = i0
            var norm = 0.0
            for k in 0..<min(stride, i1 - i0) {
                let i = i0 + k
                let overlap = min(Double(i + 1), f1) - max(Double(i), f0)
                if overlap > 0 {
                    weight[j * stride + k] = Float(overlap)
                    norm += overlap
                }
            }
            if norm > 0 {
                let inv = Float(1.0 / norm)
                for k in 0..<stride { weight[j * stride + k] *= inv }
            } else {
                weight[j * stride] = 1  // degenerate span: take the one cell
            }
        }
        return (start, weight, stride)
    }

    /// Area-average downsample so the long edge is at most `maxLongEdge` —
    /// NegPy's `cv2.resize(..., INTER_AREA)` preview resize
    /// (`services/rendering/image_processor.py`), which this now matches in
    /// kernel as well as in intent.
    ///
    /// Separable: a 2-D area box is the product of two 1-D boxes, so a
    /// horizontal pass followed by a vertical one is exactly the 2-D average
    /// and costs far less than the naive form.
    public func downsampled(maxLongEdge: Int) -> RGBImage {
        let long = max(width, height)
        guard long > maxLongEdge else { return self }
        let scale = Double(maxLongEdge) / Double(long)
        let ow = max(1, Int((Double(width) * scale).rounded()))
        let oh = max(1, Int((Double(height) * scale).rounded()))
        let w = width, h = height
        let hx = Self.areaTaps(src: w, dst: ow)
        let vy = Self.areaTaps(src: h, dst: oh)

        // Horizontal pass into an ow × h intermediate. Rows are independent,
        // and each output sample's taps are summed in a fixed order, so the
        // parallel split is bit-for-bit the serial result.
        let mid = [Float](unsafeUninitializedCapacity: ow * h * 3) { buf, count in
            let m = buf.baseAddress!
            pixels.withUnsafeBufferPointer { src in
                let sp = src.baseAddress!
                let slices = min(8, h)
                let per = (h + slices - 1) / slices
                DispatchQueue.concurrentPerform(iterations: slices) { slice in
                    let first = min(slice * per, h), last = min(h, first + per)
                    for y in first..<last {
                        let row = y * w
                        for ox in 0..<ow {
                            let s0 = hx.start[ox]
                            var r: Float = 0, g: Float = 0, b: Float = 0
                            for k in 0..<hx.stride {
                                let weight = hx.weight[ox * hx.stride + k]
                                if weight == 0 { continue }
                                let i = (row + min(s0 + k, w - 1)) * 3
                                r += sp[i] * weight
                                g += sp[i + 1] * weight
                                b += sp[i + 2] * weight
                            }
                            let o = (y * ow + ox) * 3
                            m[o] = r
                            m[o + 1] = g
                            m[o + 2] = b
                        }
                    }
                }
            }
            count = ow * h * 3
        }

        return RGBImage(width: ow, height: oh) { dst in
            let out = dst.baseAddress!
            mid.withUnsafeBufferPointer { src in
                let mp = src.baseAddress!
                let slices = min(8, oh)
                let per = (oh + slices - 1) / slices
                DispatchQueue.concurrentPerform(iterations: slices) { slice in
                    let first = min(slice * per, oh), last = min(oh, first + per)
                    for oy in first..<last {
                        let s0 = vy.start[oy]
                        for ox in 0..<ow {
                            var r: Float = 0, g: Float = 0, b: Float = 0
                            for k in 0..<vy.stride {
                                let weight = vy.weight[oy * vy.stride + k]
                                if weight == 0 { continue }
                                let i = (min(s0 + k, h - 1) * ow + ox) * 3
                                r += mp[i] * weight
                                g += mp[i + 1] * weight
                                b += mp[i + 2] * weight
                            }
                            let o = (oy * ow + ox) * 3
                            out[o] = r
                            out[o + 1] = g
                            out[o + 2] = b
                        }
                    }
                }
            }
        }
    }
}
