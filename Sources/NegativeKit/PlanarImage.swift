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
