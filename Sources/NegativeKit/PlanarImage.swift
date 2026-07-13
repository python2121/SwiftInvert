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

    /// Area-average downsample so the long edge is at most `maxLongEdge`
    /// (stand-in for NegPy's cv2 INTER_AREA preview resize).
    public func downsampled(maxLongEdge: Int) -> RGBImage {
        let long = max(width, height)
        guard long > maxLongEdge else { return self }
        let scale = Double(maxLongEdge) / Double(long)
        let ow = max(1, Int((Double(width) * scale).rounded()))
        let oh = max(1, Int((Double(height) * scale).rounded()))
        var out = RGBImage(width: ow, height: oh)
        let sx = Double(width) / Double(ow)
        let sy = Double(height) / Double(oh)
        out.pixels.withUnsafeMutableBufferPointer { dst in
            pixels.withUnsafeBufferPointer { src in
                for oy in 0..<oh {
                    let y0 = Int(Double(oy) * sy), y1 = min(height, max(y0 + 1, Int(Double(oy + 1) * sy)))
                    for ox in 0..<ow {
                        let x0 = Int(Double(ox) * sx), x1 = min(width, max(x0 + 1, Int(Double(ox + 1) * sx)))
                        var acc: (Float, Float, Float) = (0, 0, 0)
                        for y in y0..<y1 {
                            let row = y * width
                            for x in x0..<x1 {
                                let i = (row + x) * 3
                                acc.0 += src[i]; acc.1 += src[i + 1]; acc.2 += src[i + 2]
                            }
                        }
                        let n = Float((y1 - y0) * (x1 - x0))
                        let o = (oy * ow + ox) * 3
                        dst[o] = acc.0 / n; dst[o + 1] = acc.1 / n; dst[o + 2] = acc.2 / n
                    }
                }
            }
        }
        return out
    }
}
