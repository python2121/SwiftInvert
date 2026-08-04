import Foundation

/// LSD radix sort for the order statistics behind every meter.
///
/// Sorting dominates analysis: `prepare` runs one sort per channel plus one
/// each for the anchor and textural meters, and `finalize`'s neutral axis
/// sorts every band's chroma and each kept channel. Comparison sorts pay
/// n·log n branchy comparisons for that; a radix sort pays a fixed 4 (Float)
/// or 8 (Double) branch-free counting passes. Measured on the 615×410 preview
/// grid (252k cells/channel): `vDSP_vsort` 21.6 ms, `Array.sort()` 15.8 ms,
/// this 1.9 ms — which took whole-image analysis 162 → 30 ms.
///
/// **This cannot perturb NegPy parity.** A sort is a permutation into a
/// canonical order, and every consumer (`percentileOfSorted`, `median`,
/// `quantile`) only reads the sorted array by index — so any correct sort
/// yields identical bytes downstream. Verified field-by-field on raw bit
/// patterns, not tolerances.
enum RadixSort {
    /// Below this the counting passes lose to a comparison sort's cache
    /// behaviour (the 256-entry histograms cost more than the data).
    /// The small band sets in the neutral axis land here.
    @usableFromInline static let threshold = 512

    // MARK: - Order-preserving float ↔ unsigned keys
    //
    // IEEE-754 is built so that POSITIVE floats compare correctly when their
    // bit patterns are read as unsigned integers. Two things break for the
    // rest: the sign bit makes negatives look enormous, and negatives run
    // backwards among themselves (−2.0's pattern exceeds −1.0's). Flipping
    // every bit of a negative fixes both (reverses their order, clears the
    // top bit); setting the top bit of a positive lifts it above them all.
    // The map is strictly monotonic and a bijection, so `unkey` restores the
    // exact original value.
    //
    // Edge cases, for the record: −0.0 keys just below +0.0 rather than
    // tying. That is still a valid ascending order (they compare equal, so a
    // comparison sort's own relative order is unspecified too), the values
    // round-trip exactly, and the arithmetic downstream is identical either
    // way. NaN is not a concern on the meters' data — the grid is
    // `log10(clip(x, 1e-6, 1.0))`, finite by construction — and unlike a
    // comparison sort, which violates its strict-weak-ordering contract on
    // NaN, this places them deterministically at the ends.

    @inline(__always) static func key(_ f: Float) -> UInt32 {
        let b = f.bitPattern
        return (b & 0x8000_0000) != 0 ? ~b : (b | 0x8000_0000)
    }

    @inline(__always) static func unkey(_ u: UInt32) -> Float {
        Float(bitPattern: (u & 0x8000_0000) != 0 ? (u & 0x7fff_ffff) : ~u)
    }

    @inline(__always) static func key(_ d: Double) -> UInt64 {
        let b = d.bitPattern
        return (b & 0x8000_0000_0000_0000) != 0 ? ~b : (b | 0x8000_0000_0000_0000)
    }

    @inline(__always) static func unkey(_ u: UInt64) -> Double {
        Double(bitPattern: (u & 0x8000_0000_0000_0000) != 0 ? (u & 0x7fff_ffff_ffff_ffff) : ~u)
    }

    // MARK: - Sorts

    /// Ascending sort of a copy. Identical output to `Array.sorted()` for the
    /// NaN-free data the meters produce.
    static func sorted(_ data: [Float]) -> [Float] {
        let n = data.count
        if n < threshold { return data.sorted() }

        var src = [UInt32](repeating: 0, count: n)
        var dst = [UInt32](repeating: 0, count: n)
        data.withUnsafeBufferPointer { input in
            src.withUnsafeMutableBufferPointer { keys in
                for i in 0..<n { keys[i] = key(input[i]) }
            }
        }
        for shift in stride(from: UInt32(0), to: 32, by: 8) {
            src.withUnsafeMutableBufferPointer { from in
                dst.withUnsafeMutableBufferPointer { to in
                    pass(from, to, shift: shift)
                }
            }
            swap(&src, &dst)
        }
        var out = [Float](repeating: 0, count: n)
        src.withUnsafeBufferPointer { keys in
            out.withUnsafeMutableBufferPointer { result in
                for i in 0..<n { result[i] = unkey(keys[i]) }
            }
        }
        return out
    }

    /// Double variant — the same-pixel colour floors run float64 end-to-end
    /// upstream, so their order statistics must too. Eight passes.
    static func sorted(_ data: [Double]) -> [Double] {
        let n = data.count
        if n < threshold { return data.sorted() }

        var src = [UInt64](repeating: 0, count: n)
        var dst = [UInt64](repeating: 0, count: n)
        data.withUnsafeBufferPointer { input in
            src.withUnsafeMutableBufferPointer { keys in
                for i in 0..<n { keys[i] = key(input[i]) }
            }
        }
        for shift in stride(from: UInt64(0), to: 64, by: 8) {
            src.withUnsafeMutableBufferPointer { from in
                dst.withUnsafeMutableBufferPointer { to in
                    pass(from, to, shift: shift)
                }
            }
            swap(&src, &dst)
        }
        var out = [Double](repeating: 0, count: n)
        src.withUnsafeBufferPointer { keys in
            out.withUnsafeMutableBufferPointer { result in
                for i in 0..<n { result[i] = unkey(keys[i]) }
            }
        }
        return out
    }

    /// One stable counting pass over an 8-bit digit: histogram → exclusive
    /// prefix sum → scatter. Stability is what makes least-significant-first
    /// work: elements tied on this digit keep the order the previous digit
    /// left them in, so after the last pass the array is fully sorted.
    @inline(__always)
    private static func pass<T: FixedWidthInteger & UnsignedInteger>(
        _ from: UnsafeMutableBufferPointer<T>,
        _ to: UnsafeMutableBufferPointer<T>,
        shift: T
    ) {
        var offset = [Int](repeating: 0, count: 256)
        offset.withUnsafeMutableBufferPointer { slot in
            for v in from { slot[Int((v >> shift) & 255)] += 1 }
            var running = 0
            for i in 0..<256 {
                let count = slot[i]
                slot[i] = running
                running += count
            }
            for v in from {
                let bucket = Int((v >> shift) & 255)
                to[slot[bucket]] = v
                slot[bucket] += 1
            }
        }
    }
}
