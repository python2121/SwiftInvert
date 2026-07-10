import Accelerate
import Foundation

/// Log-density prefilter chain shared by all meters
/// (negpy/features/exposure/normalization.py: prefilter_log_grid,
/// get_analysis_crop, _block_median_grid).
public enum Prefilter {
    /// log10 of the clipped linear image: log10(clip(x, 1e-6, 1.0)) via vForce.
    /// Since values are in (0, 1], this holds negative density (−D). Decoded
    /// buffers are u16/65535 by construction, so no NaN/Inf handling is needed
    /// (vDSP_vclip maps them unpredictably — don't feed it synthetic garbage).
    public static func logImage(_ image: RGBImage) -> RGBImage {
        var out = image
        var lo: Float = 1e-6, hi: Float = 1.0
        var n = Int32(out.pixels.count)
        out.pixels.withUnsafeMutableBufferPointer { buf in
            image.pixels.withUnsafeBufferPointer { src in
                vDSP_vclip(src.baseAddress!, 1, &lo, &hi, buf.baseAddress!, 1, vDSP_Length(src.count))
            }
            vvlog10f(buf.baseAddress!, buf.baseAddress!, &n)
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

        if b == 2 {
            // The preview path always lands here (1536px / 1024 grid → b = 2):
            // median of 4 = (sum − min − max) / 2, no sort, no scratch.
            out.pixels.withUnsafeMutableBufferPointer { dst in
                img.pixels.withUnsafeBufferPointer { src in
                    for gy in 0..<rows {
                        let r0 = (gy * 2) * w * 3, r1 = (gy * 2 + 1) * w * 3
                        for gx in 0..<cols {
                            let base = gx * 6
                            for c in 0..<3 {
                                let a = src[r0 + base + c], b2 = src[r0 + base + 3 + c]
                                let c2 = src[r1 + base + c], d = src[r1 + base + 3 + c]
                                let mn = min(min(a, b2), min(c2, d))
                                let mx = max(max(a, b2), max(c2, d))
                                dst[(gy * cols + gx) * 3 + c] = (a + b2 + c2 + d - mn - mx) / 2
                            }
                        }
                    }
                }
            }
            return out
        }

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

    /// Full chain: log10 → (roi skipped: SwiftInvert meters the whole frame) →
    /// centered buffer crop → block-median grid.
    public static func prefilterLogGrid(_ image: RGBImage, analysisBuffer: Double) -> RGBImage {
        var img = logImage(image)
        if analysisBuffer > 0 { img = analysisCrop(img, bufferRatio: analysisBuffer) }
        return blockMedianGrid(img)
    }
}
