import Foundation

/// Neutral-axis measurement: per-channel median raw-log refs at three luma bands
/// over each band's lowest-chroma pixels (measure_neutral_axis_from_log).
public struct NeutralAxisRefs: Equatable, Sendable {
    public var mid: SIMD3<Double>
    public var shadow: SIMD3<Double>
    public var highlight: SIMD3<Double>?  // nil → callers fit a 2-point line
    public var confidence: Double
}

/// Auto-exposure meters, all consuming the shared prefiltered log grid.
/// Ports of the *_from_log meters in negpy/features/exposure/normalization.py.
public enum Meters {
    /// Normalized luma of the grid under `bounds` (shared by anchor + neutral axis).
    static func normalizedLuma(grid: RGBImage, bounds: LogNegativeBounds) -> [Float] {
        let n = grid.width * grid.height
        var luma = [Float](repeating: 0, count: n)
        let eps = 1e-6
        var f = [Double](repeating: 0, count: 3), invD = [Double](repeating: 0, count: 3)
        for ch in 0..<3 {
            f[ch] = bounds.floors[ch]
            var denom = bounds.ceils[ch] - f[ch]
            if abs(denom) < eps { denom = denom >= 0 ? eps : -eps }
            invD[ch] = 1.0 / denom
        }
        grid.pixels.withUnsafeBufferPointer { src in
            for i in 0..<n {
                let r = (Double(src[i * 3]) - f[0]) * invD[0]
                let g = (Double(src[i * 3 + 1]) - f[1]) * invD[1]
                let b = (Double(src[i * 3 + 2]) - f[2]) * invD[2]
                luma[i] = Float(K.lumaR * r + K.lumaG * g + K.lumaB * b)
            }
        }
        return luma
    }

    /// measure_anchor_from_log: P50 of normalized luma, partially pulled toward
    /// assumed_anchor and clamped to ±anchor_meter_band.
    public static func anchor(grid: RGBImage, bounds: LogNegativeBounds) -> Double {
        let lum = normalizedLuma(grid: grid, bounds: bounds)
        let measured = Stats.percentile(lum, K.anchorMeterPercentile)
        let anchor = K.assumedAnchor + K.anchorMeterStrength * (measured - K.assumedAnchor)
        return min(max(anchor, K.assumedAnchor - K.anchorMeterBand), K.assumedAnchor + K.anchorMeterBand)
    }

    /// measure_textural_range_from_log: |P90 − P10| of raw log luma.
    public static func texturalRange(grid: RGBImage) -> Double {
        let n = grid.width * grid.height
        var lum = [Float](repeating: 0, count: n)
        grid.pixels.withUnsafeBufferPointer { src in
            for i in 0..<n {
                lum[i] = Float(
                    K.lumaR * Double(src[i * 3]) + K.lumaG * Double(src[i * 3 + 1]) + K.lumaB * Double(src[i * 3 + 2]))
            }
        }
        lum.sort()
        let lo = Stats.percentileOfSorted(lum, K.texturalRangeClip)
        let hi = Stats.percentileOfSorted(lum, 100.0 - K.texturalRangeClip)
        return abs(hi - lo)
    }

    /// measure_shadow_refs_from_log: P98 per channel of the raw log grid.
    public static func shadowRefs(grid: RGBImage) -> SIMD3<Double> {
        let n = grid.width * grid.height
        var refs = SIMD3<Double>()
        for c in 0..<3 {
            var v = [Float](repeating: 0, count: n)
            grid.pixels.withUnsafeBufferPointer { src in
                for i in 0..<n { v[i] = src[i * 3 + c] }
            }
            refs[c] = Stats.percentile(v, K.shadowNeutralPercentile)
        }
        return refs
    }

    /// measure_neutral_axis_from_log. `bounds` is the PRE-trim base bounds
    /// (NegPy 2125a34: the film's cast is a source property; user WP/BP
    /// trims don't perturb it — their GPU always measured pre-trim, and the
    /// CPU side was standardized to match). Single-pass and allocation-lean.
    public static func neutralAxis(grid: RGBImage, bounds: LogNegativeBounds) -> NeutralAxisRefs? {
        let n = grid.width * grid.height

        // One pass: normalized luma + chroma (max − min of normalized channels).
        var luma = [Float](repeating: 0, count: n)
        var chroma = [Float](repeating: 0, count: n)
        let eps = 1e-6
        var f = [Double](repeating: 0, count: 3), invD = [Double](repeating: 0, count: 3)
        for ch in 0..<3 {
            f[ch] = bounds.floors[ch]
            var denom = bounds.ceils[ch] - f[ch]
            if abs(denom) < eps { denom = denom >= 0 ? eps : -eps }
            invD[ch] = 1.0 / denom
        }
        grid.pixels.withUnsafeBufferPointer { src in
            luma.withUnsafeMutableBufferPointer { lum in
                chroma.withUnsafeMutableBufferPointer { chr in
                    for i in 0..<n {
                        let r = (Double(src[i * 3]) - f[0]) * invD[0]
                        let g = (Double(src[i * 3 + 1]) - f[1]) * invD[1]
                        let b = (Double(src[i * 3 + 2]) - f[2]) * invD[2]
                        lum[i] = Float(K.lumaR * r + K.lumaG * g + K.lumaB * b)
                        chr[i] = Float(max(r, max(g, b)) - min(r, min(g, b)))
                    }
                }
            }
        }

        let cap = K.neutralAxisChromaCap
        func bandRefs(_ lo: Double, _ hi: Double) -> (refs: SIMD3<Double>, medianChroma: Double)? {
            let loF = Float(lo), hiF = Float(hi)
            var indices: [Int] = []
            indices.reserveCapacity(n / 4)
            for i in 0..<n where luma[i] >= loF && luma[i] <= hiF { indices.append(i) }
            guard indices.count >= K.neutralAxisMinPixels else { return nil }
            var bandChroma = [Float](repeating: 0, count: indices.count)
            for (k, i) in indices.enumerated() { bandChroma[k] = chroma[i] }
            let thr = Float(Stats.quantile(bandChroma, K.neutralAxisChromaQuantile))
            // Order-preserving subset (matches np.nonzero(band)[0][band_chroma <= thr]);
            // gather the kept chroma and channel values in the same pass.
            var keptChroma: [Float] = []
            var kept: [[Float]] = [[], [], []]
            keptChroma.reserveCapacity(indices.count / 2)
            for c in 0..<3 { kept[c].reserveCapacity(indices.count / 2) }
            for (k, i) in indices.enumerated() where bandChroma[k] <= thr {
                keptChroma.append(chroma[i])
                kept[0].append(grid.pixels[i * 3])
                kept[1].append(grid.pixels[i * 3 + 1])
                kept[2].append(grid.pixels[i * 3 + 2])
            }
            let nearNeutralChroma = keptChroma.isEmpty ? cap : Stats.median(keptChroma)
            if keptChroma.count < K.neutralAxisMinPixels || nearNeutralChroma > cap { return nil }
            let refs = SIMD3<Double>(
                Stats.median(kept[0]), Stats.median(kept[1]), Stats.median(kept[2]))
            return (refs, nearNeutralChroma)
        }

        guard let mid = bandRefs(K.neutralAxisMidBand.0, K.neutralAxisMidBand.1),
            let shadow = bandRefs(K.neutralAxisShadowBand.0, K.neutralAxisShadowBand.1)
        else { return nil }
        let highlight = bandRefs(K.neutralAxisHighlightBand.0, K.neutralAxisHighlightBand.1)
        let confidence = min(max(1.0 - mid.medianChroma / cap, 0.0), 1.0)
        return NeutralAxisRefs(
            mid: mid.refs, shadow: shadow.refs, highlight: highlight?.refs, confidence: confidence)
    }
}
