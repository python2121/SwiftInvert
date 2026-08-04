import Foundation
import Testing

@testable import NegativeKit

/// Two analysis-stage optimizations changed HOW the work is done without
/// changing WHAT is computed: `sortedChannels` runs its three channels
/// concurrently, and `prefilterLogGrid` crops before the log where NegPy crops
/// after. Both claims are "byte-identical to the obvious implementation", and
/// the fixture suites would only catch a violation that happened to move a
/// percentile. These pin the equivalence directly.
@Suite struct AnalysisEquivalenceTests {

    /// Deterministic pseudo-random log-density grid (the shape the meters see:
    /// negative, finite, heavily duplicated after the block median).
    private func grid(width: Int, height: Int, seed: UInt64 = 4242) -> RGBImage {
        var s = seed
        return RGBImage(width: width, height: height) { dst in
            for i in 0..<(width * height * 3) {
                s = s &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
                dst[i] = -Float(s >> 40) / 16_777_216.0 * 3.0
            }
        }
    }

    /// Linear-domain frame in (0, 1] — what `prefilterLogGrid` actually receives.
    private func linear(width: Int, height: Int, seed: UInt64 = 99) -> RGBImage {
        var s = seed
        return RGBImage(width: width, height: height) { dst in
            for i in 0..<(width * height * 3) {
                s = s &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
                // Spans below the 1e-6 clip and up to 1.0 so both clip branches fire.
                dst[i] = max(1e-9, Float(s >> 40) / 16_777_216.0)
            }
        }
    }

    // MARK: - sortedChannels runs three channels concurrently

    private func serialSortedChannels(_ g: RGBImage) -> [[Float]] {
        let n = g.width * g.height
        var out: [[Float]] = [[], [], []]
        for c in 0..<3 {
            var v = [Float](repeating: 0, count: n)
            for i in 0..<n { v[i] = g.pixels[i * 3 + c] }
            out[c] = v.sorted()
        }
        return out
    }

    @Test func parallelChannelSortMatchesSerial() {
        // Sizes on both sides of the radix sort's 512-element fallback.
        for (w, h) in [(1, 1), (8, 8), (23, 17), (615, 410)] {
            let g = grid(width: w, height: h, seed: UInt64(w * 31 + h))
            #expect(BoundsAnalysis.sortedChannels(grid: g) == serialSortedChannels(g), "\(w)x\(h)")
        }
    }

    /// A data race between the three slots would most likely show up as an
    /// occasional wrong/empty channel rather than a consistent one, so repeat.
    @Test func parallelChannelSortIsDeterministic() {
        let g = grid(width: 615, height: 410)
        let reference = BoundsAnalysis.sortedChannels(grid: g)
        #expect(reference.count == 3)
        #expect(reference.allSatisfy { $0.count == g.width * g.height })
        for run in 0..<25 {
            #expect(BoundsAnalysis.sortedChannels(grid: g) == reference, "run \(run)")
        }
    }

    /// The percentile consumers must agree too — this is what actually reaches
    /// the bounds and the shadow refs.
    @Test func channelPercentilesSurviveTheParallelSort() {
        let g = grid(width: 615, height: 410)
        let parallel = BoundsAnalysis.sortedChannels(grid: g)
        let serial = serialSortedChannels(g)
        for c in 0..<3 {
            for q in [0.01, 1.0, 50.0, 90.0, 98.0, 100.0] {
                #expect(
                    Stats.percentileOfSorted(parallel[c], q) == Stats.percentileOfSorted(serial[c], q),
                    "channel \(c) at q=\(q)")
            }
        }
    }

    // MARK: - prefilterLogGrid crops before the log

    /// NegPy's order is log10 → crop → block median; ours is crop → log10 →
    /// block median. log10 is elementwise, so the pixels that survive the crop
    /// hold identical values either way — this pins that the divergence is
    /// purely about how much work is skipped, never about the result.
    @Test func cropBeforeLogMatchesUpstreamOrdering() {
        for buffer in [0.05, 0.10, 0.3] {
            for (w, h) in [(1536, 1025), (200, 130), (37, 29)] {
                let img = linear(width: w, height: h, seed: UInt64(w &+ h))
                let ours = Prefilter.prefilterLogGrid(img, analysisBuffer: buffer)
                let upstreamOrder = Prefilter.blockMedianGrid(
                    Prefilter.analysisCrop(Prefilter.logImage(img), bufferRatio: buffer))
                #expect(ours.width == upstreamOrder.width && ours.height == upstreamOrder.height)
                #expect(ours.pixels == upstreamOrder.pixels, "\(w)x\(h) buffer \(buffer)")
            }
        }
    }

    /// buffer = 0 skips the crop entirely — the two orderings collapse to the
    /// same call sequence, but the guard should still be exercised.
    @Test func zeroBufferSkipsTheCropInBothOrderings() {
        let img = linear(width: 200, height: 130)
        let ours = Prefilter.prefilterLogGrid(img, analysisBuffer: 0)
        let upstreamOrder = Prefilter.blockMedianGrid(Prefilter.logImage(img))
        #expect(ours.pixels == upstreamOrder.pixels)
    }

    // MARK: - The uninitialized-allocation initializer

    /// `RGBImage(width:height:initializingWith:)` hands out raw uninitialized
    /// memory; the contract is that the body fills every lane and the result is
    /// indistinguishable from a zero-filled buffer the caller overwrote.
    @Test func uninitializedInitMatchesFillThenOverwrite() {
        let w = 37, h = 29
        let viaUninit = RGBImage(width: w, height: h) { dst in
            for i in 0..<(w * h * 3) { dst[i] = Float(i) * 0.5 - 3.0 }
        }
        var viaFill = RGBImage(width: w, height: h)
        for i in 0..<(w * h * 3) { viaFill.pixels[i] = Float(i) * 0.5 - 3.0 }

        #expect(viaUninit.width == w && viaUninit.height == h)
        #expect(viaUninit.pixels.count == w * h * 3)
        #expect(viaUninit.pixels == viaFill.pixels)
    }
}
