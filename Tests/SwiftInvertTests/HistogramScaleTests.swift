import Foundation
import Testing

@testable import SwiftInvert

/// The histogram bins raw per-pixel counts, and the HQ preview bins ~15× as
/// many pixels as the 1536px proxy for the same picture. The drawn curve must
/// therefore depend only on the histogram's SHAPE — otherwise toggling HQ
/// visibly redraws it, which is exactly what `log1p(count)/log1p(maxCount)` did.
@Suite struct HistogramScaleTests {

    /// A plausible shape: one peak, a broad midtone body, sparse tails.
    private func shape() -> [Double] {
        (0..<256).map { i in
            let x = Double(i)
            let peak = exp(-pow(x - 90, 2) / (2 * 30 * 30))
            let tail = 0.002 * exp(-pow(x - 240, 2) / (2 * 12 * 12))
            let floorTerm = 0.0004
            return peak + tail + floorTerm
        }
    }

    private func counts(_ shape: [Double], pixels: Double) -> [UInt32] {
        let sum = shape.reduce(0, +)
        return shape.map { UInt32(($0 / sum * pixels).rounded()) }
    }

    /// THE invariant. Proxy ≈1.57M pixels, full resolution ≈24.2M — the same
    /// picture, so the same curve.
    @Test func logCurveIsInvariantToPixelCount() {
        let s = shape()
        let proxy = counts(s, pixels: 1_574_400)
        let hq = counts(s, pixels: 24_216_480)
        let pMax = proxy.max()!, hMax = hq.max()!

        var worst = 0.0
        for i in 0..<256 {
            let a = HistogramView.barHeight(count: proxy[i], maxCount: pMax, log: true)
            let b = HistogramView.barHeight(count: hq[i], maxCount: hMax, log: true)
            worst = max(worst, abs(a - b))
        }
        // Only the rounding of counts to integers survives.
        #expect(worst < 0.005, "log curve moved by \(worst * 100) percentage points")
    }

    @Test func linearCurveIsInvariantToo() {
        let s = shape()
        let proxy = counts(s, pixels: 1_574_400)
        let hq = counts(s, pixels: 24_216_480)
        let pMax = proxy.max()!, hMax = hq.max()!
        for i in 0..<256 {
            let a = HistogramView.barHeight(count: proxy[i], maxCount: pMax, log: false)
            let b = HistogramView.barHeight(count: hq[i], maxCount: hMax, log: false)
            #expect(abs(a - b) < 0.005, "bin \(i)")
        }
    }

    /// The old formula, pinned: it really did lift the sparse end by double
    /// digits, so the invariance test above isn't asserting something trivial.
    @Test func theOldFormulaDidDependOnPixelCount() {
        func old(_ count: UInt32, _ maxCount: UInt32) -> Double {
            log1p(Double(count)) / log1p(Double(max(maxCount, 1)))
        }
        let s = shape()
        let proxy = counts(s, pixels: 1_574_400)
        let hq = counts(s, pixels: 24_216_480)
        let pMax = proxy.max()!, hMax = hq.max()!
        var worst = 0.0
        for i in 0..<256 where proxy[i] > 0 {
            worst = max(worst, abs(old(proxy[i], pMax) - old(hq[i], hMax)))
        }
        #expect(worst > 0.05, "expected the old formula to drift; got \(worst)")
    }

    // MARK: - Shape of the curve itself

    @Test func endpointsAndMonotonicity() {
        for log in [true, false] {
            #expect(HistogramView.barHeight(count: 0, maxCount: 1000, log: log) == 0)
            #expect(abs(HistogramView.barHeight(count: 1000, maxCount: 1000, log: log) - 1) < 1e-12)
            var previous = -1.0
            for c in stride(from: UInt32(0), through: 1000, by: 25) {
                let v = HistogramView.barHeight(count: c, maxCount: 1000, log: log)
                #expect(v >= previous, "must not decrease (log: \(log))")
                #expect(v >= 0 && v <= 1)
                previous = v
            }
        }
    }

    /// Log must lift the sparse end relative to linear — that is the point of
    /// offering it.
    @Test func logLiftsSmallBinsAboveLinear() {
        for c in [UInt32(1), 10, 100, 5000] {
            let lin = HistogramView.barHeight(count: c, maxCount: 100_000, log: false)
            let lg = HistogramView.barHeight(count: c, maxCount: 100_000, log: true)
            #expect(lg > lin, "count \(c): log \(lg) should exceed linear \(lin)")
        }
    }

    @Test func degenerateMaxDoesNotDivideByZero() {
        let v = HistogramView.barHeight(count: 0, maxCount: 0, log: true)
        #expect(v.isFinite && v == 0)
        let w = HistogramView.barHeight(count: 5, maxCount: 0, log: false)
        #expect(w.isFinite)
    }
}
