import Foundation

/// Per-channel D-min/D-max container (floors < ceils for negatives).
public struct LogNegativeBounds: Equatable, Codable, Sendable {
    public var floors: SIMD3<Double>
    public var ceils: SIMD3<Double>

    public init(floors: SIMD3<Double>, ceils: SIMD3<Double>) {
        self.floors = floors
        self.ceils = ceils
    }

    /// Rec.709-luma-weighted density range (luminance_density_range).
    public var luminanceDensityRange: Double {
        K.lumaR * abs(ceils.x - floors.x) + K.lumaG * abs(ceils.y - floors.y) + K.lumaB * abs(ceils.z - floors.z)
    }

    /// White/black-point offsets applied on top of analyzed bounds
    /// (NormalizationProcessor: floors + wp, ceils + bp).
    public func applyingOffsets(whitePoint: Double, blackPoint: Double) -> LogNegativeBounds {
        LogNegativeBounds(
            floors: floors + SIMD3(repeating: whitePoint),
            ceils: ceils + SIMD3(repeating: blackPoint)
        )
    }
}

/// Auto-exposure bounds analysis (analyze_log_exposure_bounds_from_log,
/// C-41 branch only — no E6 reversal, no margin-mode negative sliders in v1,
/// though the margin path is kept since it's part of _sample_log_bounds).
public enum BoundsAnalysis {
    /// _sample_log_bounds: per-channel (floors, ceils) at one clip level.
    static func sampleLogBounds(
        channelsSorted: [[Float]], percentileClip: Double, base: Double
    ) -> (floors: [Double], ceils: [Double]) {
        let clip: Double
        var margin = 0.0
        if percentileClip >= 0 {
            clip = max(0.00001, min(50.0, percentileClip + base))
        } else {
            clip = base
            margin = -percentileClip
        }
        let pLow = clip, pHigh = 100.0 - clip

        var floors = [Double](), ceils = [Double]()
        for ch in 0..<3 {
            floors.append(Stats.percentileOfSorted(channelsSorted[ch], pLow))
            ceils.append(Stats.percentileOfSorted(channelsSorted[ch], pHigh))
        }
        if margin > 0 {
            for ch in 0..<3 {
                if ceils[ch] >= floors[ch] {
                    floors[ch] -= margin
                    ceils[ch] += margin
                } else {
                    floors[ch] += margin
                    ceils[ch] -= margin
                }
            }
        }
        return (floors, ceils)
    }

    /// One ascending sort per channel — the expensive half of the percentile
    /// sampling. Public so `prepare` can share the sorts with the other
    /// percentile meters (shadowRefs): the sorted arrays are a pure function
    /// of the grid, so every consumer sees identical bytes.
    public static func sortedChannels(grid: RGBImage) -> [[Float]] {
        let n = grid.width * grid.height
        var channels: [[Float]] = [[], [], []]
        for c in 0..<3 {
            var v = [Float](repeating: 0, count: n)
            grid.pixels.withUnsafeBufferPointer { src in
                v.withUnsafeMutableBufferPointer { dst in
                    for i in 0..<n { dst[i] = src[i * 3 + c] }
                }
            }
            channels[c] = Stats.sortedAscending(v)
        }
        return channels
    }

    /// _same_pixel_color_floor_refs: dense-end (print-white) colour refs from
    /// one shared pixel set — the luma-extreme band's lowest-chroma subset,
    /// chroma measured base-anchored (offsets from the thin-end refs,
    /// per-channel span as provisional gamma, refined once from the band
    /// medians). Independent per-channel percentiles read a different scene
    /// object per channel, so coloured highlight content masquerades as film
    /// cast; a shared chroma-gated set cannot. The thin end needs no such
    /// treatment — density on real film is bounded below by base, anchoring
    /// per-channel ceils. nil when the band's neutral set is too small or too
    /// chromatic (caller falls back to the percentile pass).
    ///
    /// Runs in float64 end to end (the one analysis path that does upstream —
    /// it casts the grid to float64 on entry), so this is bit-comparable to
    /// the numpy reference.
    static func samePixelColorFloorRefs(
        grid: RGBImage,
        lumaFloors: [Double], lumaCeils: [Double],
        baseRefs: [Double],
        colorClip: Double
    ) -> [Double]? {
        let eps = 1e-6
        let n = grid.width * grid.height
        let minPx = K.neutralAxisMinPixels
        let width = K.colorBoundsBandWidth

        var denoms = [Double](repeating: 0, count: 3)
        for ch in 0..<3 {
            var denom = lumaCeils[ch] - lumaFloors[ch]
            if abs(denom) < eps { denom = denom >= 0 ? eps : -eps }
            denoms[ch] = denom
        }
        var luma = [Double](repeating: 0, count: n)
        grid.pixels.withUnsafeBufferPointer { src in
            for i in 0..<n {
                let r = (Double(src[i * 3]) - lumaFloors[0]) / denoms[0]
                let g = (Double(src[i * 3 + 1]) - lumaFloors[1]) / denoms[1]
                let b = (Double(src[i * 3 + 2]) - lumaFloors[2]) / denoms[2]
                luma[i] = K.lumaR * r + K.lumaG * g + K.lumaB * b
            }
        }

        let clip = max(0.00001, min(50.0 - width, colorClip))
        let sortedLuma = Stats.sortedAscending(luma)
        let lo = Stats.percentileOfSorted(sortedLuma, clip)
        let hi = Stats.percentileOfSorted(sortedLuma, clip + width)

        // Band offsets from the thin-end base refs, order-preserving triplets.
        var d: [Double] = []
        d.reserveCapacity(n / 8 * 3)
        grid.pixels.withUnsafeBufferPointer { src in
            for i in 0..<n where luma[i] >= lo && luma[i] <= hi {
                d.append(Double(src[i * 3]) - baseRefs[0])
                d.append(Double(src[i * 3 + 1]) - baseRefs[1])
                d.append(Double(src[i * 3 + 2]) - baseRefs[2])
            }
        }
        let m = d.count / 3
        guard m >= minPx else { return nil }

        func select(_ gamma: [Double]) -> (kept: [Int], medianChroma: Double)? {
            let g = gamma.map { abs($0) < eps ? eps : $0 }
            var chroma = [Double](repeating: 0, count: m)
            for j in 0..<m {
                chroma[j] = Meters.rmsChroma(d[j * 3] / g[0], d[j * 3 + 1] / g[1], d[j * 3 + 2] / g[2])
            }
            let thr = Stats.quantile(chroma, K.neutralAxisChromaQuantile)
            var kept: [Int] = []
            var keptChroma: [Double] = []
            kept.reserveCapacity(m / 2)
            keptChroma.reserveCapacity(m / 2)
            for j in 0..<m where chroma[j] <= thr {
                kept.append(j)
                keptChroma.append(chroma[j])
            }
            guard kept.count >= minPx else { return nil }
            return (kept, Stats.median(keptChroma))
        }

        func channelMedians(_ rows: [Int]) -> [Double] {
            var scratch = [Double](repeating: 0, count: rows.count)
            var out = [Double](repeating: 0, count: 3)
            for ch in 0..<3 {
                for (k, j) in rows.enumerated() { scratch[k] = d[j * 3 + ch] }
                out[ch] = Stats.median(scratch)
            }
            return out
        }

        let spans = (0..<3).map { lumaFloors[$0] - baseRefs[$0] }
        // Pass-1 loose cap: a homogeneous coloured cluster would otherwise be
        // self-normalized to zero chroma by pass 2 and read as neutral.
        guard let first = select(spans), first.medianChroma <= K.neutralAxisFirstPassCap
        else { return nil }
        let provisional = channelMedians(first.kept)
        guard provisional.allSatisfy({ abs($0) >= eps }) else { return nil }
        guard let second = select(provisional), second.medianChroma <= K.neutralAxisChromaCap
        else { return nil }
        let med = channelMedians(second.kept)
        return [baseRefs[0] + med[0], baseRefs[1] + med[1], baseRefs[2] + med[2]]
    }

    /// Two-axis recombination: luma clip drives the mean centre+span, colour clip
    /// the per-channel cast (offset from the median channel). Colour floors
    /// (dense end, scene content) prefer the same-pixel chroma-gated band refs
    /// (NegPy 0.43), falling back to the percentile pass when the band holds no
    /// trustworthy neutrals (and always for margin-mode clips).
    public static func analyze(
        grid: RGBImage, lumaRangeClip: Double = 0.0, colorRangeClip: Double = K.baseColorClip
    ) -> LogNegativeBounds {
        analyze(grid: grid, channelsSorted: sortedChannels(grid: grid),
            lumaRangeClip: lumaRangeClip, colorRangeClip: colorRangeClip)
    }

    /// Same analysis with the caller's pre-sorted channels (shared with the
    /// other percentile meters — one sort pass per prepare, not two).
    public static func analyze(
        grid: RGBImage, channelsSorted channels: [[Float]],
        lumaRangeClip: Double = 0.0, colorRangeClip: Double = K.baseColorClip
    ) -> LogNegativeBounds {
        let (floors, ceils) = sampleLogBounds(channelsSorted: channels, percentileClip: lumaRangeClip, base: K.baseLumaClip)
        let colour = sampleLogBounds(channelsSorted: channels, percentileClip: colorRangeClip, base: 0.0)
        var cFloors = colour.floors
        let cCeils = colour.ceils
        if colorRangeClip >= 0,
            let sp = samePixelColorFloorRefs(
                grid: grid, lumaFloors: floors, lumaCeils: ceils, baseRefs: cCeils, colorClip: colorRangeClip)
        {
            cFloors = sp
        }

        let meanLF = (floors[0] + floors[1] + floors[2]) / 3.0
        let meanLC = (ceils[0] + ceils[1] + ceils[2]) / 3.0
        let meanCF = cFloors.sorted()[1]
        let meanCC = cCeils.sorted()[1]

        return LogNegativeBounds(
            floors: SIMD3(meanLF + (cFloors[0] - meanCF), meanLF + (cFloors[1] - meanCF), meanLF + (cFloors[2] - meanCF)),
            ceils: SIMD3(meanLC + (cCeils[0] - meanCC), meanLC + (cCeils[1] - meanCC), meanLC + (cCeils[2] - meanCC))
        )
    }
}
