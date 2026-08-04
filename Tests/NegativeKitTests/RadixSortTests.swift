import Foundation
import Testing

@testable import NegativeKit

/// `Stats.sortedAscending` is a radix sort, and every order statistic in the
/// analysis funnels through it — so its output must be indistinguishable from
/// a comparison sort's on anything the meters can produce. These pin that
/// against `Array.sorted()` on the adversarial shapes (sign boundaries,
/// duplicates, denormals, the small-array threshold) rather than on the happy
/// path the fixtures already cover.
@Suite struct RadixSortTests {

    /// Deterministic pseudo-random log-density values (no Foundation RNG:
    /// the suite must be reproducible).
    private func sample(_ n: Int, seed: UInt64, span: Float = 3.0) -> [Float] {
        var s = seed
        return (0..<n).map { _ in
            s = s &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
            return -Float(s >> 40) / 16_777_216.0 * span
        }
    }

    @Test func matchesStdlibOnRealisticGridSizes() {
        // Straddles the 512 threshold in both directions, and covers the real
        // preview grid (615×410 = 252k cells per channel).
        for n in [1, 2, 7, 511, 512, 513, 4096, 252_150] {
            let data = sample(n, seed: UInt64(n) &+ 99)
            #expect(Stats.sortedAscending(data) == data.sorted(), "n = \(n)")
        }
    }

    @Test func matchesStdlibAcrossTheSignBoundary() {
        // The key transform's whole job: negatives run backwards in raw bit
        // order and the sign bit outranks magnitude. Mix both signs densely.
        var data = sample(2000, seed: 7, span: 4.0)
        for i in stride(from: 0, to: data.count, by: 2) { data[i] = -data[i] }
        #expect(Stats.sortedAscending(data) == data.sorted())

        let doubles = data.map(Double.init)
        #expect(Stats.sortedAscending(doubles) == doubles.sorted())
    }

    @Test func matchesStdlibOnExtremesAndDenormals() {
        let corners: [Float] = [
            0.0, -0.0, .leastNonzeroMagnitude, -.leastNonzeroMagnitude,
            .leastNormalMagnitude, -.leastNormalMagnitude,
            .greatestFiniteMagnitude, -.greatestFiniteMagnitude,
            .infinity, -.infinity, 1.0, -1.0, 1e-6, -6.0,
        ]
        // Padded past the threshold so the radix path (not the fallback) runs.
        let data = corners + sample(600, seed: 31)
        let ours = Stats.sortedAscending(data)
        let theirs = data.sorted()
        // Compare by value: ±0.0 tie, and either order is a valid sort.
        #expect(ours == theirs)

        let doubles = data.map(Double.init)
        #expect(Stats.sortedAscending(doubles) == doubles.sorted())
    }

    @Test func heavyDuplicatesPreserveCounts() {
        // Block-median output is highly quantized — long runs of equal values
        // are the norm, and a miscounted histogram bucket would drop or
        // duplicate elements rather than misorder them.
        var data = [Float]()
        for i in 0..<4000 { data.append(Float(i % 7) * -0.5) }
        let sorted = Stats.sortedAscending(data)
        #expect(sorted == data.sorted())
        #expect(sorted.count == data.count)
        for v in Set(data) {
            #expect(sorted.filter { $0 == v }.count == data.filter { $0 == v }.count)
        }
    }

    @Test func roundTripsValuesExactly() {
        // unkey ∘ key must be the identity: a lossy transform would silently
        // shift percentile VALUES while keeping the order plausible.
        let data = sample(1500, seed: 5150, span: 6.0)
        let sorted = Stats.sortedAscending(data)
        #expect(Set(sorted.map { $0.bitPattern }) == Set(data.map { $0.bitPattern }))

        let doubles = data.map(Double.init)
        let sortedD = Stats.sortedAscending(doubles)
        #expect(Set(sortedD.map { $0.bitPattern }) == Set(doubles.map { $0.bitPattern }))
    }

    @Test func percentilesAgreeWithAComparisonSort() {
        // The property that actually matters downstream: identical order
        // statistics at every clip the meters use.
        let data = sample(50_000, seed: 4242)
        let ours = Stats.sortedAscending(data)
        let theirs = data.sorted()
        for q in [0.0, 0.01, 1.0, 2.0, 10.0, 50.0, 90.0, 98.0, 99.0, 100.0] {
            #expect(Stats.percentileOfSorted(ours, q) == Stats.percentileOfSorted(theirs, q))
        }
        #expect(Stats.median(data) == Stats.median(theirs))
    }
}
