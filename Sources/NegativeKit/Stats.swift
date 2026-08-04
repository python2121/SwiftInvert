import Foundation

/// numpy-compatible order statistics (linear-interpolation percentile, even/odd
/// median). NegPy's bounds analysis is built on np.percentile/np.median, so these
/// must match exactly.
public enum Stats {
    /// Ascending sort — same ordering as Array.sorted() for the NaN-free float
    /// data the meters see. Radix, not comparison: sorting is the bulk of
    /// analysis and this is ~11× faster than vDSP_vsort here (see RadixSort).
    /// Every percentile/median/quantile below funnels through this one call.
    public static func sortedAscending(_ data: [Float]) -> [Float] {
        RadixSort.sorted(data)
    }

    /// np.percentile(data, q) with the default "linear" interpolation:
    /// pos = q/100 * (n-1); result = lower + frac * (upper - lower).
    /// Sorts a copy; q in [0, 100].
    public static func percentile(_ data: [Float], _ q: Double) -> Double {
        precondition(!data.isEmpty)
        return percentileOfSorted(sortedAscending(data), q)
    }

    /// Percentile over already-sorted data (for multiple qs on one sort).
    public static func percentileOfSorted(_ sorted: [Float], _ q: Double) -> Double {
        let n = sorted.count
        if n == 1 { return Double(sorted[0]) }
        let pos = q / 100.0 * Double(n - 1)
        let lo = Int(pos.rounded(.down))
        let hi = min(lo + 1, n - 1)
        let frac = pos - Double(lo)
        let a = Double(sorted[lo]), b = Double(sorted[hi])
        // numpy's _lerp mirrors the interpolation above t = 0.5 (last-ulp parity).
        return frac >= 0.5 ? b - (b - a) * (1.0 - frac) : a + frac * (b - a)
    }

    /// np.quantile — percentile with q in [0, 1].
    public static func quantile(_ data: [Float], _ q: Double) -> Double {
        percentile(data, q * 100.0)
    }

    /// np.median — middle value, or the mean of the two middle values.
    public static func median(_ data: [Float]) -> Double {
        precondition(!data.isEmpty)
        let sorted = sortedAscending(data)
        let n = sorted.count
        if n % 2 == 1 { return Double(sorted[n / 2]) }
        return (Double(sorted[n / 2 - 1]) + Double(sorted[n / 2])) / 2.0
    }

    /// In-place median of a small scratch slice (block-median inner loop; avoids
    /// per-cell allocation).
    @inlinable
    public static func medianInPlace(_ scratch: inout [Float], count: Int) -> Float {
        scratch[0..<count].sort()
        if count % 2 == 1 { return scratch[count / 2] }
        return (scratch[count / 2 - 1] + scratch[count / 2]) / 2.0
    }

    // Double-precision variants: the same-pixel colour-floor refs run in
    // float64 upstream (the one analysis path that does), so their order
    // statistics must too.

    public static func sortedAscending(_ data: [Double]) -> [Double] {
        RadixSort.sorted(data)
    }

    public static func percentileOfSorted(_ sorted: [Double], _ q: Double) -> Double {
        let n = sorted.count
        if n == 1 { return sorted[0] }
        let pos = q / 100.0 * Double(n - 1)
        let lo = Int(pos.rounded(.down))
        let hi = min(lo + 1, n - 1)
        let frac = pos - Double(lo)
        let a = sorted[lo], b = sorted[hi]
        // numpy's _lerp mirrors the interpolation above t = 0.5 (last-ulp parity).
        return frac >= 0.5 ? b - (b - a) * (1.0 - frac) : a + frac * (b - a)
    }

    public static func quantile(_ data: [Double], _ q: Double) -> Double {
        precondition(!data.isEmpty)
        return percentileOfSorted(sortedAscending(data), q * 100.0)
    }

    public static func median(_ data: [Double]) -> Double {
        precondition(!data.isEmpty)
        let sorted = sortedAscending(data)
        let n = sorted.count
        if n % 2 == 1 { return sorted[n / 2] }
        return (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}
