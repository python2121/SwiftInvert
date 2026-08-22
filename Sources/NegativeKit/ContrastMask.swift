#if canImport(Accelerate)
import Accelerate
#endif
import Foundation

/// Contrast Mask — the darkroom's unsharp mask, NegPy 515c1f5 port
/// (negpy/features/exposure/normalization.py: contrast_mask_plane;
/// logic.py: contrast_mask_scale / expand_mask_plane).
///
/// Sandwich the negative with a blurred, low-gamma mask: densities add, so
/// `D' = D − g·blur(D) + const` — positive gamma (a blurred positive)
/// compresses the global range while the blur keeps detail out of the
/// compression; negative gamma stretches instead. The plane is the blurred
/// luma of the NORMALIZED negative on the analysis grid, zero-mean so print
/// density never moves, built from the PRINTED frame only (a bright rebate
/// blurred into the mask prints as a vignette the negative does not have).
///
/// The plane is always built from the PROXY-scale render source: sigma is a
/// fraction of the analysis grid, never of the render, so preview, HQ tiers
/// and export mask identically (the analysis-on-the-proxy invariant).
///
/// DIVERGENCE (recorded in CLAUDE.md): the plane normalizes against the
/// BASE bounds (pre-WP/BP-offset), where upstream uses the offset-applied
/// final bounds. Ours keeps white/black-point drags analysis-free — the
/// plane is a blurred zero-mean field of a source property, the same
/// argument as the 2125a34 pre-trim neutral axis. Identical at zero
/// offsets, which is every parity-fixture config.
public enum ContrastMask {
    /// Slider limits (upstream MASK_SPACER_MIN/MAX/DEFAULT). The floor is
    /// where the mask stops being unsharp: at the narrow limit blur(v) → v
    /// and the operator collapses into a plain (1−g) reduction, i.e. Grade.
    public static let spacerMin = 2.0
    public static let spacerMax = 6.0
    public static let spacerDefault = 4.0
    public static let gammaLimit = 0.5

    /// Single-channel float plane on the analysis grid.
    public struct Plane: Sendable, Equatable {
        public var values: [Float]  // count == width * height
        public let width: Int
        public let height: Int

        public init(values: [Float], width: Int, height: Int) {
            precondition(values.count == width * height, "plane count mismatch")
            self.values = values
            self.width = width
            self.height = height
        }
    }

    /// Build the zero-mean mask plane from the render source at proxy scale
    /// (crop and geometry already applied — the plane covers exactly the
    /// printed frame the kernels will render).
    ///
    /// Mirrors upstream: resize to the analysis grid (INTER_AREA) →
    /// log10(clip(x, 1e-6, 1)) → per-channel normalize by `bounds`
    /// (unclamped stretch) → Rec.709 luma → Gaussian blur at
    /// σ = spacer% × grid short side (cv2 kernel: radius round(4σ),
    /// replicate border) → subtract the mean.
    ///
    /// Returns nil when the bounds carry no real range (nothing metered) or
    /// the source is degenerate — the caller renders unmasked.
    public static func buildPlane(
        renderSource: RGBImage,
        bounds: LogNegativeBounds,
        spacerPercent: Double
    ) -> Plane? {
        guard renderSource.width >= 2, renderSource.height >= 2 else { return nil }
        guard bounds.luminanceDensityRange > 1e-6 else { return nil }

        // To the analysis grid. `downsampled` is the same separable area
        // filter as upstream's INTER_AREA resize; ≤ grid passes through.
        var img = renderSource
        if max(img.width, img.height) > K.analysisGrid {
            img = img.downsampled(maxLongEdge: K.analysisGrid)
        }

        // Normalized-val luma. ReferenceCurve.normalize does log10(clip) +
        // per-channel stretch itself (the normalizeLog kernel's CPU mirror,
        // parity-pinned) — upstream's prefilter_log_grid + normalize_log_image
        // collapse to exactly that at grid size (b = 1).
        let norm = ReferenceCurve.normalize(img, bounds: bounds)
        let w = norm.width, h = norm.height
        var lum = [Float](unsafeUninitializedCapacity: w * h) { buf, n in
            let wr = Float(K.lumaR), wg = Float(K.lumaG), wb = Float(K.lumaB)
            norm.pixels.withUnsafeBufferPointer { src in
                for i in 0..<(w * h) {
                    buf[i] = wr * src[i * 3] + wg * src[i * 3 + 1] + wb * src[i * 3 + 2]
                }
            }
            n = w * h
        }

        let sigma = min(max(spacerPercent, spacerMin), spacerMax) * 0.01 * Double(min(w, h))
        gaussianBlurReplicate(&lum, width: w, height: h, sigma: sigma)

        // Zero-mean: a real sandwich is denser and the printer opens up for
        // it — the slider must leave print density alone.
        var mean = 0.0
        for v in lum { mean += Double(v) }
        mean /= Double(w * h)
        let m = Float(mean)
        for i in lum.indices { lum[i] -= m }

        return Plane(values: lum, width: w, height: h)
    }

    /// Per-channel pre-curve scale for the plane sample:
    /// upstream's `contrast_mask_scale(gamma, lumRange)` (stops per unit of
    /// plane) folded with the dodge/burn EV scale (`log10(2)/range_ch`, the
    /// same domain `exposureStops` rides), leaving
    /// `val_ch += −gamma · lumRange / range_ch · plane`.
    public static func valScale(
        gamma: Double, lumRange: Double, finalBounds: LogNegativeBounds
    ) -> SIMD3<Double> {
        guard gamma != 0 else { return .zero }
        var out = SIMD3<Double>()
        for ch in 0..<3 {
            let range = max(abs(bounds: finalBounds, channel: ch), 1e-6)
            out[ch] = -gamma * lumRange / range
        }
        return out
    }

    private static func abs(bounds: LogNegativeBounds, channel: Int) -> Double {
        Swift.abs(bounds.ceils[channel] - bounds.floors[channel])
    }

    /// Bilinear plane sample for output pixel (x, y) of an outWidth×outHeight
    /// render — upstream's expand_mask_plane / contrast_mask_stops mapping
    /// (OpenCV's half-pixel-centre convention, taps clamped = edge
    /// replication). This exact arithmetic is mirrored in NegPipeline.metal
    /// and NegPipeline.comp — keep the three in lock step.
    public static func sample(
        _ plane: Plane, x: Int, y: Int, outWidth: Int, outHeight: Int
    ) -> Float {
        let px = (Float(x) + 0.5) * Float(plane.width) / Float(outWidth) - 0.5
        let py = (Float(y) + 0.5) * Float(plane.height) / Float(outHeight) - 0.5
        let maxX = Float(plane.width - 1), maxY = Float(plane.height - 1)
        let lox = min(max(px.rounded(.down), 0), maxX)
        let loy = min(max(py.rounded(.down), 0), maxY)
        let hix = min(lox + 1, maxX)
        let hiy = min(loy + 1, maxY)
        let fx = min(max(px - lox, 0), 1)
        let fy = min(max(py - loy, 0), 1)
        let i00 = Int(loy) * plane.width + Int(lox)
        let i10 = Int(loy) * plane.width + Int(hix)
        let i01 = Int(hiy) * plane.width + Int(lox)
        let i11 = Int(hiy) * plane.width + Int(hix)
        let top = plane.values[i00] + (plane.values[i10] - plane.values[i00]) * fx
        let bot = plane.values[i01] + (plane.values[i11] - plane.values[i01]) * fx
        return top + (bot - top) * fy
    }

    /// Bilinear plane sample at content-normalized (u, v) of the printed
    /// frame — the zone-pin path (u = (x+0.5)/W collapses the half-pixel
    /// mapping above to u·w − 0.5).
    public static func sample(_ plane: Plane, atNormalizedU u: Double, v: Double) -> Float {
        let px = Float(u) * Float(plane.width) - 0.5
        let py = Float(v) * Float(plane.height) - 0.5
        let maxX = Float(plane.width - 1), maxY = Float(plane.height - 1)
        let lox = min(max(px.rounded(.down), 0), maxX)
        let loy = min(max(py.rounded(.down), 0), maxY)
        let hix = min(lox + 1, maxX)
        let hiy = min(loy + 1, maxY)
        let fx = min(max(px - lox, 0), 1)
        let fy = min(max(py - loy, 0), 1)
        let i00 = Int(loy) * plane.width + Int(lox)
        let i10 = Int(loy) * plane.width + Int(hix)
        let i01 = Int(hiy) * plane.width + Int(lox)
        let i11 = Int(hiy) * plane.width + Int(hix)
        let top = plane.values[i00] + (plane.values[i10] - plane.values[i00]) * fx
        let bot = plane.values[i01] + (plane.values[i11] - plane.values[i01]) * fx
        return top + (bot - top) * fy
    }

    /// Separable Gaussian blur, replicate border, cv2-matched kernel:
    /// ksize = round(8σ+1)|odd (i.e. radius round(4σ)), coefficients
    /// exp(−d²/2σ²) normalized to sum 1 — getGaussianKernel's construction,
    /// so the parity fixture can pin the plane against cv2's own output.
    static func gaussianBlurReplicate(
        _ data: inout [Float], width: Int, height: Int, sigma: Double
    ) {
        guard sigma > 0 else { return }
        var ksize = Int((8.0 * sigma + 1.0).rounded())
        if ksize % 2 == 0 { ksize += 1 }
        let radius = (ksize - 1) / 2
        guard radius > 0 else { return }

        var taps = [Float](repeating: 0, count: ksize)
        var sum = 0.0
        for i in 0..<ksize {
            let d = Double(i - radius)
            let v = exp(-d * d / (2.0 * sigma * sigma))
            taps[i] = Float(v)
            sum += v
        }
        let inv = Float(1.0 / sum)
        for i in taps.indices { taps[i] *= inv }

        // Horizontal pass into scratch, then vertical back — each row/column
        // pre-extended by edge replication so the inner loop has no bounds
        // tests (the padded line is small; the win is branch-free MACs).
        var scratch = [Float](repeating: 0, count: width * height)
        var line = [Float](repeating: 0, count: max(width, height) + 2 * radius)

        for y in 0..<height {
            let row = y * width
            for i in 0..<radius { line[i] = data[row] }
            for x in 0..<width { line[radius + x] = data[row + x] }
            let last = data[row + width - 1]
            for i in 0..<radius { line[radius + width + i] = last }
            convolveLine(line, count: width, taps: taps, into: &scratch, at: row, stride: 1)
        }
        for x in 0..<width {
            for i in 0..<radius { line[i] = scratch[x] }
            for y in 0..<height { line[radius + y] = scratch[y * width + x] }
            let last = scratch[(height - 1) * width + x]
            for i in 0..<radius { line[radius + height + i] = last }
            convolveLine(line, count: height, taps: taps, into: &data, at: x, stride: width)
        }
    }

    private static func convolveLine(
        _ line: [Float], count: Int, taps: [Float],
        into out: inout [Float], at base: Int, stride: Int
    ) {
        let k = taps.count
        line.withUnsafeBufferPointer { src in
            taps.withUnsafeBufferPointer { t in
                out.withUnsafeMutableBufferPointer { dst in
                    #if canImport(Accelerate)
                    // vDSP_conv correlates; the taps are symmetric, so
                    // correlation == convolution. The MACs are the O(n·k)
                    // part — always vDSP; a strided (column) output just
                    // scatters the contiguous result afterwards.
                    if stride == 1 {
                        vDSP_conv(
                            src.baseAddress!, 1, t.baseAddress!, 1,
                            dst.baseAddress! + base, 1,
                            vDSP_Length(count), vDSP_Length(k))
                    } else {
                        let col = [Float](unsafeUninitializedCapacity: count) { buf, n in
                            vDSP_conv(
                                src.baseAddress!, 1, t.baseAddress!, 1,
                                buf.baseAddress!, 1,
                                vDSP_Length(count), vDSP_Length(k))
                            n = count
                        }
                        col.withUnsafeBufferPointer { c in
                            for i in 0..<count { dst[base + i * stride] = c[i] }
                        }
                    }
                    #else
                    for i in 0..<count {
                        var acc: Float = 0
                        for j in 0..<k {
                            acc += src[i + j] * t[j]
                        }
                        dst[base + i * stride] = acc
                    }
                    #endif
                }
            }
        }
    }
}
