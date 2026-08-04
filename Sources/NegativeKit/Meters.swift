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
        // Through Stats so this shares the radix sort with every other meter
        // (a bare lum.sort() left this the slowest line in prepare).
        let sortedLum = Stats.sortedAscending(lum)
        let lo = Stats.percentileOfSorted(sortedLum, K.texturalRangeClip)
        let hi = Stats.percentileOfSorted(sortedLum, 100.0 - K.texturalRangeClip)
        return abs(hi - lo)
    }

    /// measure_shadow_refs_from_log: P98 per channel of the raw log grid.
    public static func shadowRefs(grid: RGBImage) -> SIMD3<Double> {
        shadowRefs(channelsSorted: BoundsAnalysis.sortedChannels(grid: grid))
    }

    /// Same refs from pre-sorted channels (bit-identical: the percentile is
    /// deterministic on the sorted array, which is a pure function of the
    /// grid — shared with BoundsAnalysis so prepare sorts once).
    public static func shadowRefs(channelsSorted: [[Float]]) -> SIMD3<Double> {
        SIMD3(
            Stats.percentileOfSorted(channelsSorted[0], K.shadowNeutralPercentile),
            Stats.percentileOfSorted(channelsSorted[1], K.shadowNeutralPercentile),
            Stats.percentileOfSorted(channelsSorted[2], K.shadowNeutralPercentile))
    }

    /// Pairwise-RMS distance from the neutral axis (rotation-symmetric around
    /// grey, so the near-neutral ranking is hue-uniform — max−min scores an
    /// opposed R/B split double a same-side deviation). _rms_chroma.
    @inlinable
    static func rmsChroma(_ r: Double, _ g: Double, _ b: Double) -> Double {
        (((r - g) * (r - g) + (g - b) * (g - b) + (r - b) * (r - b)) / 3.0).squareRoot()
    }

    /// measure_neutral_axis_from_log. `bounds` is the PRE-trim base bounds
    /// (NegPy 2125a34: the film's cast is a source property; user WP/BP
    /// trims don't perturb it — their GPU always measured pre-trim, and the
    /// CPU side was standardized to match).
    ///
    /// Two passes (NegPy 0.43): pass 1 selects through the residual cast under
    /// a loose cap (a strong but correctable cast must not collapse the axis;
    /// saturated content still fails it), then the affine R/B→G correction
    /// implied by its mid+shadow refs re-ranks chroma so pass 2 selects true
    /// neutrals under the strict cap. Confidence combines the grey sets'
    /// corrected tightness, the midtone sample size and mid↔shadow deviation
    /// agreement.
    public static func neutralAxis(grid: RGBImage, bounds: LogNegativeBounds) -> NeutralAxisRefs? {
        let n = grid.width * grid.height

        // One pass: normalized triplets (Float, mirroring NegPy's float32
        // normalize_log_image output), luma, and pass-1 RMS chroma.
        var norm = [Float](repeating: 0, count: n * 3)
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
            norm.withUnsafeMutableBufferPointer { nrm in
                luma.withUnsafeMutableBufferPointer { lum in
                    chroma.withUnsafeMutableBufferPointer { chr in
                        for i in 0..<n {
                            nrm[i * 3] = Float((Double(src[i * 3]) - f[0]) * invD[0])
                            nrm[i * 3 + 1] = Float((Double(src[i * 3 + 1]) - f[1]) * invD[1])
                            nrm[i * 3 + 2] = Float((Double(src[i * 3 + 2]) - f[2]) * invD[2])
                            // Luma/chroma read the ROUNDED norm values — upstream
                            // computes them from the float32 normalized image.
                            let r = Double(nrm[i * 3])
                            let g = Double(nrm[i * 3 + 1])
                            let b = Double(nrm[i * 3 + 2])
                            lum[i] = Float(K.lumaR * r + K.lumaG * g + K.lumaB * b)
                            chr[i] = Float(rmsChroma(r, g, b))
                        }
                    }
                }
            }
        }

        // Band membership depends only on the (immutable) luma — compute
        // each band's indices ONCE and reuse across both passes (pass 2
        // used to rescan all n pixels per band for identical sets).
        func bandIndices(_ lo: Double, _ hi: Double) -> [Int] {
            let loF = Float(lo), hiF = Float(hi)
            var indices: [Int] = []
            indices.reserveCapacity(n / 4)
            for i in 0..<n where luma[i] >= loF && luma[i] <= hiF { indices.append(i) }
            return indices
        }

        func bandRefs(
            _ indices: [Int], _ chromaVals: [Float], _ capVal: Double
        ) -> (refs: SIMD3<Double>, medianChroma: Double, count: Int)? {
            guard indices.count >= K.neutralAxisMinPixels else { return nil }
            var bandChroma = [Float](repeating: 0, count: indices.count)
            for (k, i) in indices.enumerated() { bandChroma[k] = chromaVals[i] }
            let thr = Float(Stats.quantile(bandChroma, K.neutralAxisChromaQuantile))
            // Order-preserving subset (matches np.nonzero(band)[0][band_chroma <= thr]);
            // gather the kept chroma and channel values in the same pass.
            var keptChroma: [Float] = []
            var kept: [[Float]] = [[], [], []]
            keptChroma.reserveCapacity(indices.count / 2)
            for c in 0..<3 { kept[c].reserveCapacity(indices.count / 2) }
            for (k, i) in indices.enumerated() where bandChroma[k] <= thr {
                keptChroma.append(chromaVals[i])
                kept[0].append(grid.pixels[i * 3])
                kept[1].append(grid.pixels[i * 3 + 1])
                kept[2].append(grid.pixels[i * 3 + 2])
            }
            let nearNeutralChroma = keptChroma.isEmpty ? capVal : Stats.median(keptChroma)
            if keptChroma.count < K.neutralAxisMinPixels || nearNeutralChroma > capVal { return nil }
            let refs = SIMD3<Double>(
                Stats.median(kept[0]), Stats.median(kept[1]), Stats.median(kept[2]))
            return (refs, nearNeutralChroma, keptChroma.count)
        }

        // Raw-log refs → normalized space under the same bounds (_norm_ref).
        func normRef(_ refs: SIMD3<Double>) -> SIMD3<Double> {
            var out = SIMD3<Double>()
            for ch in 0..<3 {
                var denom = bounds.ceils[ch] - bounds.floors[ch]
                if abs(denom) < eps { denom = denom >= 0 ? eps : -eps }
                out[ch] = (refs[ch] - bounds.floors[ch]) / denom
            }
            return out
        }

        let mb = K.neutralAxisMidBand, sb = K.neutralAxisShadowBand, hb = K.neutralAxisHighlightBand
        let mbIdx = bandIndices(mb.0, mb.1)
        let sbIdx = bandIndices(sb.0, sb.1)
        guard let mid1 = bandRefs(mbIdx, chroma, K.neutralAxisFirstPassCap),
            let sh1 = bandRefs(sbIdx, chroma, K.neutralAxisFirstPassCap)
        else { return nil }

        // Affine R/B→G correction implied by the pass-1 mid+shadow refs, then
        // re-ranked chroma (corrected channels stored at Float, matching the
        // float32 array upstream writes into).
        let nm = normRef(mid1.refs), ns = normRef(sh1.refs)
        var aC = [1.0, 1.0, 1.0], bC = [0.0, 0.0, 0.0]
        for ch in [0, 2] {
            let du = nm[ch] - ns[ch]
            if abs(du) < eps {
                aC[ch] = 1.0
                bC[ch] = nm[1] - nm[ch]
            } else {
                aC[ch] = (nm[1] - ns[1]) / du
                bC[ch] = nm[1] - aC[ch] * nm[ch]
            }
        }
        var chroma2 = [Float](repeating: 0, count: n)
        norm.withUnsafeBufferPointer { nrm in
            chroma2.withUnsafeMutableBufferPointer { chr in
                for i in 0..<n {
                    let r = Float(aC[0] * Double(nrm[i * 3]) + bC[0])
                    let g = nrm[i * 3 + 1]
                    let b = Float(aC[2] * Double(nrm[i * 3 + 2]) + bC[2])
                    chr[i] = Float(rmsChroma(Double(r), Double(g), Double(b)))
                }
            }
        }

        let cap = K.neutralAxisChromaCap
        guard let mid = bandRefs(mbIdx, chroma2, cap),
            let shadow = bandRefs(sbIdx, chroma2, cap)
        else { return nil }
        let highlight = bandRefs(bandIndices(hb.0, hb.1), chroma2, cap)

        // Confidence: corrected tightness of the grey sets × midtone sample
        // size × mid↔shadow deviation agreement (a dead zone passes plausible
        // crossover).
        let tight = min(max(1.0 - max(mid.medianChroma, shadow.medianChroma) / cap, 0.0), 1.0)
        let sizeTerm = Double(mid.count) / (Double(mid.count) + K.neutralAxisConfidenceN0)
        let dm = normRef(mid.refs), ds = normRef(shadow.refs)
        let spread = max(
            abs((dm.x - dm.y) - (ds.x - ds.y)),
            abs((dm.z - dm.y) - (ds.z - ds.y)))
        let agree = 1.0 - min(max(spread - K.neutralAxisAgreementDeadzone, 0.0) / K.neutralAxisAgreementScale, 1.0)
        let confidence = min(max(tight * sizeTerm * agree, 0.0), 1.0)
        return NeutralAxisRefs(
            mid: mid.refs, shadow: shadow.refs, highlight: highlight?.refs, confidence: confidence)
    }
}
