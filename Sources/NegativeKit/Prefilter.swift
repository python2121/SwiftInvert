import Foundation

/// Log-density prefilter chain shared by all meters
/// (negpy/features/exposure/normalization.py: prefilter_log_grid,
/// get_analysis_crop, _block_median_grid).
public enum Prefilter {
    /// log10 of the clipped linear image: log10(clip(nan_to_num(x), 1e-6, 1.0)).
    /// Since values are in (0, 1], this holds negative density (−D).
    public static func logImage(_ image: RGBImage) -> RGBImage {
        var out = image
        let eps: Float = 1e-6
        out.pixels.withUnsafeMutableBufferPointer { buf in
            for i in 0..<buf.count {
                var v = buf[i]
                if v.isNaN { v = eps }
                if v.isInfinite { v = v > 0 ? 1.0 : eps }
                v = min(max(v, eps), 1.0)
                buf[i] = log10(v)
            }
        }
        return out
    }

    /// Centered analysis crop: cut int(dim * buffer) from each side (buffer ≤ 0.3).
    public static func analysisCrop(_ img: RGBImage, bufferRatio: Double) -> RGBImage {
        guard bufferRatio > 0 else { return img }
        let safe = min(max(bufferRatio, 0.0), 0.3)
        let cutH = Int(Double(img.height) * safe)
        let cutW = Int(Double(img.width) * safe)
        let h = img.height - 2 * cutH, w = img.width - 2 * cutW
        guard h > 0, w > 0 else { return img }
        var out = RGBImage(width: w, height: h)
        let rowFloats = w * 3
        for y in 0..<h {
            let src = ((y + cutH) * img.width + cutW) * 3
            let dst = y * rowFloats
            let dstRange: Range<Int> = dst..<(dst + rowFloats)
            let srcRange: Range<Int> = src..<(src + rowFloats)
            out.pixels.replaceSubrange(dstRange, with: img.pixels[srcRange])
        }
        return out
    }

    /// Block-median downfilter to a fixed target grid (analysis_grid = 1024):
    /// b = ceil(max(h,w)/grid); per-cell per-channel median of b×b blocks.
    /// Early-returns the input when b ≤ 1 (already at/below grid size).
    public static func blockMedianGrid(_ img: RGBImage) -> RGBImage {
        let h = img.height, w = img.width
        let grid = K.analysisGrid
        let b = Int((Double(max(h, w)) / Double(grid)).rounded(.up))
        if b <= 1 || h < b || w < b { return img }

        let hb = (h / b) * b, wb = (w / b) * b
        let rows = hb / b, cols = wb / b
        var out = RGBImage(width: cols, height: rows)
        var scratch = [Float](repeating: 0, count: b * b)
        img.pixels.withUnsafeBufferPointer { src in
            for gy in 0..<rows {
                for gx in 0..<cols {
                    for c in 0..<3 {
                        var k = 0
                        for by in 0..<b {
                            let rowBase = ((gy * b + by) * w + gx * b) * 3 + c
                            for bx in 0..<b {
                                scratch[k] = src[rowBase + bx * 3]
                                k += 1
                            }
                        }
                        out[gy, gx, c] = Stats.medianInPlace(&scratch, count: b * b)
                    }
                }
            }
        }
        return out
    }

    /// Full chain: log10 → (roi skipped: NegSwift meters the whole frame) →
    /// centered buffer crop → block-median grid.
    public static func prefilterLogGrid(_ image: RGBImage, analysisBuffer: Double) -> RGBImage {
        var img = logImage(image)
        if analysisBuffer > 0 { img = analysisCrop(img, bufferRatio: analysisBuffer) }
        return blockMedianGrid(img)
    }
}
