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

    /// Two-axis recombination: luma clip drives the mean centre+span, colour clip
    /// the per-channel cast (offset from the median channel).
    public static func analyze(
        grid: RGBImage, lumaRangeClip: Double = 0.0, colorRangeClip: Double = K.baseColorClip
    ) -> LogNegativeBounds {
        // One sort per channel, reused by both clip axes.
        let n = grid.width * grid.height
        var channels: [[Float]] = [[], [], []]
        for c in 0..<3 {
            var v = [Float](repeating: 0, count: n)
            grid.pixels.withUnsafeBufferPointer { src in
                for i in 0..<n { v[i] = src[i * 3 + c] }
            }
            v.sort()
            channels[c] = v
        }

        let (floors, ceils) = sampleLogBounds(channelsSorted: channels, percentileClip: lumaRangeClip, base: K.baseLumaClip)
        let (cFloors, cCeils) = sampleLogBounds(channelsSorted: channels, percentileClip: colorRangeClip, base: 0.0)

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
