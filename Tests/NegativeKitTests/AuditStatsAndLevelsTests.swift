import Foundation
import Testing

@testable import NegativeKit

/// Stats order-statistic endpoints (numpy parity corners) and the
/// levels remap / inverse-remap round trip.
@Suite struct AuditStatsAndLevelsTests {

    // MARK: - Stats endpoints

    @Test func percentileEndpointsFloatAndDouble() {
        let f: [Float] = [-2.5, -1.0, 0.25, 3.5]
        let d: [Double] = [-2.5, -1.0, 0.25, 3.5]
        // q = 0: pos = 0, frac = 0 → lower element exactly.
        #expect(Stats.percentileOfSorted(f, 0) == -2.5)
        #expect(Stats.percentileOfSorted(d, 0) == -2.5)
        // q = 100: pos = n−1, lo = n−1, hi = n−1, frac = 0 → last element.
        #expect(Stats.percentileOfSorted(f, 100) == 3.5)
        #expect(Stats.percentileOfSorted(d, 100) == 3.5)
    }

    @Test func singleElementArrays() {
        #expect(Stats.percentileOfSorted([Float(0.75)], 0) == 0.75)
        #expect(Stats.percentileOfSorted([Float(0.75)], 37.2) == 0.75)
        #expect(Stats.percentileOfSorted([Float(0.75)], 100) == 0.75)
        #expect(Stats.percentileOfSorted([Double(-1.25)], 50) == -1.25)
        #expect(Stats.median([Float(0.5)]) == 0.5)
        #expect(Stats.median([Double(0.5)]) == 0.5)
    }

    @Test func evenCountMedianBothOverloads() {
        // n = 4: mean of the two middle values, (2 + 3) / 2 = 2.5.
        #expect(Stats.median([Float(1), 2, 3, 4]) == 2.5)
        #expect(Stats.median([Double(1), 2, 3, 4]) == 2.5)
        // n = 2 (the block-median fallback shape).
        #expect(Stats.median([Float(0.25), 0.75]) == 0.5)
        var scratch: [Float] = [4, 1, 3, 2]
        #expect(Stats.medianInPlace(&scratch, count: 4) == 2.5)
    }

    /// numpy's _lerp two-sided branch: frac ≥ 0.5 computes b − (b−a)(1−t),
    /// NOT a + t(b−a). For sorted [0, 1] both forms are exact; for [0.1, 0.4]
    /// (Double literals) the two forms differ in the last ulp, which pins the
    /// branch bit-for-bit.
    ///
    /// NOTE (pinned differently than specced): for a = 0.1, b = 0.3 the two
    /// forms happen to agree bit-for-bit in Double (verified empirically), so
    /// that pair cannot discriminate the branch. a = 0.1, b = 0.4 does:
    /// b − (b−a)·0.25 = 0.32500000000000007 ≠ a + 0.75·(b−a) = 0.325…01 off
    /// by one ulp. Both the specced pair and the discriminating pair are
    /// asserted against the two-sided form.
    @Test func twoSidedLerpBranchExactness() {
        // Sorted [0, 1], q = 75 → pos = 0.75, frac = 0.75 ≥ 0.5.
        #expect(Stats.percentileOfSorted([Float(0), 1], 75) == 1.0 - (1.0 - 0.0) * 0.25)
        #expect(Stats.percentileOfSorted([Double(0), 1], 75) == 1.0 - (1.0 - 0.0) * 0.25)

        // Specced pair (0.1, 0.3): still must equal the two-sided form
        // exactly (bit comparison) in both overloads.
        do {
            let aF: Float = 0.1, bF: Float = 0.3
            let a = Double(aF), b = Double(bF)
            let expected = b - (b - a) * 0.25
            #expect(Stats.percentileOfSorted([aF, bF], 75) == expected)
        }
        do {
            let a = 0.1, b = 0.3
            #expect(Stats.percentileOfSorted([a, b], 75) == b - (b - a) * 0.25)
        }

        // Discriminating pair (Double overload): the one-sided form differs
        // by one ulp, so this is a true branch pin.
        do {
            let a = 0.1, b = 0.4
            let twoSided = b - (b - a) * 0.25
            let oneSided = a + 0.75 * (b - a)
            #expect(twoSided != oneSided)  // the pair actually discriminates
            #expect(Stats.percentileOfSorted([a, b], 75) == twoSided)
        }
    }

    // MARK: - levelsRemap / levelsInverseRemap round trip

    /// Forward map runs in Float (the shader mirror); the inverse in Double
    /// (the interactive histogram's grab math). Round trip must agree within
    /// 1e-4 for sanitized anchor sets, including a near-vertical segment.
    @Test func levelsInverseRoundTrip() {
        let anchorSets: [[SIMD2<Double>]] = [
            [SIMD2(0.3, 0.6)],  // 1 anchor
            [SIMD2(0.2, 0.3), SIMD2(0.5, 0.55), SIMD2(0.9, 0.95)],  // 3 anchors
            [SIMD2(0.3, 0.3), SIMD2(0.31, 0.9)],  // steep near-vertical segment
        ]
        for raw in anchorSets {
            let pts = ExposureKernel.sanitizeLevels(raw)
            #expect(!pts.isEmpty)  // none of these are identity sets
            let floatPts = pts.map { SIMD2<Float>(Float($0.x), Float($0.y)) }
            for x in stride(from: 0.0, through: 1.0, by: 0.05) {
                let y = ReferenceCurve.levelsRemap(Float(x), floatPts)
                let back = ReferenceCurve.levelsInverseRemap(Double(y), pts)
                #expect(abs(back - x) < 1e-4, "x=\(x) → y=\(y) → back=\(back) for \(pts)")
            }
        }
    }
}

/// LabColor's gamut-aware saturation boost on an input that is already
/// out of gamut before any scaling.
@Suite struct AuditGamutBoostTests {

    @Test func outOfGamutInputYieldsBoundedFiniteBoost() {
        // L 50, a 200, b −150 sits far outside the Adobe RGB cube.
        let lab = SIMD3<Double>(50, 200, -150)
        #expect(!LabColor.inGamutLab(lab.x, lab.y, lab.z))

        for saturation in [1.2, 1.5, 3.0] {
            let boost = LabColor.gamutAwareBoost(lab, saturation: saturation)
            #expect(boost.isFinite)
            #expect(boost >= 1.0)
            #expect(boost <= saturation)
        }
        // An already-out pixel has no headroom: the bisection collapses to
        // sMax = 1 + tolerance, so the boost stays within a hair of 1.0
        // (the knee can't exceed sMax).
        let boost = LabColor.gamutAwareBoost(lab, saturation: 2.0)
        #expect(boost <= 1.0 + LabColor.gamutTolerance + 1e-12)
    }

    /// Sanity contrast: an in-gamut muted pixel takes the full requested push.
    @Test func inGamutPixelTakesFullPush() {
        let lab = LabColor.rgbToLab(SIMD3(0.4, 0.38, 0.36))  // near-neutral
        #expect(LabColor.inGamutLab(lab.x, lab.y * 1.3, lab.z * 1.3))
        #expect(LabColor.gamutAwareBoost(lab, saturation: 1.3) == 1.3)
    }
}
