import Foundation

/// Axis-aligned rect normalized to the image ([0,1] in both axes), used for the
/// output crop and the analysis region (NegPy's manual_crop_rect / analysis_rect).
public struct NormalizedRect: Codable, Equatable, Sendable {
    public var x: Double
    public var y: Double
    public var width: Double
    public var height: Double

    public init(x: Double, y: Double, width: Double, height: Double) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }

    /// Standardized rect from two drag corners, clamped to [0,1].
    public init(from a: (x: Double, y: Double), to b: (x: Double, y: Double)) {
        let x0 = min(max(min(a.x, b.x), 0), 1)
        let y0 = min(max(min(a.y, b.y), 0), 1)
        let x1 = min(max(max(a.x, b.x), 0), 1)
        let y1 = min(max(max(a.y, b.y), 0), 1)
        self.init(x: x0, y: y0, width: x1 - x0, height: y1 - y0)
    }

    /// Pixel ROI (x0, y0, x1, y1) with NegPy's truncating mapping
    /// (resolve_analysis_region: int(min·dim)…int(max·dim)); nil when the rect
    /// is degenerate (< 2 px in either axis), so a stray click can't blank analysis.
    ///
    /// Clamped to the frame BEFORE the degeneracy check: only the drag
    /// initializer guarantees [0,1] — a rect from a hand-edited or corrupt
    /// sidecar can sit off-frame, and checking the unclamped span used to
    /// pass a negative-dimension ROI through to `cropped` (fatal). For
    /// in-frame rects the two orders are identical.
    public func pixelROI(width w: Int, height h: Int) -> (x0: Int, y0: Int, x1: Int, y1: Int)? {
        let x0 = max(Int(x * Double(w)), 0)
        let y0 = max(Int(y * Double(h)), 0)
        let x1 = min(Int((x + width) * Double(w)), w)
        let y1 = min(Int((y + height) * Double(h)), h)
        guard x1 - x0 >= 2, y1 - y0 >= 2 else { return nil }
        return (x0, y0, x1, y1)
    }
}

extension RGBImage {
    /// Axis-aligned crop; returns self when the rect is degenerate.
    public func cropped(to rect: NormalizedRect) -> RGBImage {
        guard let roi = rect.pixelROI(width: width, height: height) else { return self }
        let w = roi.x1 - roi.x0, h = roi.y1 - roi.y0
        var out = RGBImage(width: w, height: h)
        let rowFloats = w * 3
        for y in 0..<h {
            let src = ((y + roi.y0) * width + roi.x0) * 3
            let dst = y * rowFloats
            let dstRange: Range<Int> = dst..<(dst + rowFloats)
            let srcRange: Range<Int> = src..<(src + rowFloats)
            out.pixels.replaceSubrange(dstRange, with: pixels[srcRange])
        }
        return out
    }
}
