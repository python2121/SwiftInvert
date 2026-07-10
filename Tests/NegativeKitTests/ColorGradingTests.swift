import Foundation
import Testing

@testable import NegativeKit

/// Per-band color balance + Temp/Tint properties (SwiftInvert additions on the
/// tone-region masks; zero = identity is covered by the untouched fixtures).
@Suite struct ColorGradingTests {
    /// Ramp through the print curve with band CMY injected.
    static func rampEncoded(
        shadowCMY: SIMD3<Double> = .zero, midCMY: SIMD3<Double> = .zero,
        highlightCMY: SIMD3<Double> = .zero, n: Int = 257
    ) -> [[Float]] {
        var p = ToneControlsTests.params()
        p.shadowCMY = shadowCMY * K.cmyMaxDensity
        p.midCMY = midCMY * K.cmyMaxDensity
        p.highlightCMY = highlightCMY * K.cmyMaxDensity
        var ramp = RGBImage(width: n, height: 1)
        for i in 0..<n {
            let x = Float(i) / Float(n - 1)
            for ch in 0..<3 { ramp[0, i, ch] = x }
        }
        let out = ReferenceCurve.encodeOutput(ReferenceCurve.applyPrintCurve(ramp, params: p))
        return (0..<n).map { i in (0..<3).map { out[0, i, $0] } }
    }

    @Test func shadowBandAffectsOnlyShadows() {
        let base = Self.rampEncoded()
        // Shadows R→C full: adds red-channel density in the dark band → less red.
        let graded = Self.rampEncoded(shadowCMY: SIMD3(1, 0, 0))
        let sIdx = ToneControlsTests.idx(atDensity: K.shadowToneAnchor)
        let hIdx = ToneControlsTests.idx(atDensity: K.highlightToneAnchor)
        // ~0.018 encoded delta for a 0.2 D shift: dark tones are compressed by
        // the paper toe and the OETF, so the gate is calibrated, not round.
        #expect(base[sIdx][0] - graded[sIdx][0] > 0.015, "red should drop in shadows")
        expectClose(Double(graded[sIdx][1]), Double(base[sIdx][1]), accuracy: 1e-5, "green untouched")
        #expect(abs(graded[hIdx][0] - base[hIdx][0]) < 0.012, "highlights leak")
    }

    @Test func highBandAffectsOnlyHighs() {
        let base = Self.rampEncoded()
        // Highs B→Y full: adds blue-channel density in the bright band → yellow.
        let graded = Self.rampEncoded(highlightCMY: SIMD3(0, 0, 1))
        let sIdx = ToneControlsTests.idx(atDensity: K.shadowToneAnchor + 0.3)
        let hIdx = ToneControlsTests.idx(atDensity: K.highlightToneAnchor)
        #expect(base[hIdx][2] - graded[hIdx][2] > 0.02, "blue should drop in highs")
        #expect(abs(graded[sIdx][2] - base[sIdx][2]) < 0.012, "shadows leak")
    }

    @Test func midBandCentered() {
        let base = Self.rampEncoded()
        let graded = Self.rampEncoded(midCMY: SIMD3(0, 1, 0))
        let mIdx = ToneControlsTests.idx(atDensity: 0.85)  // between the anchors
        let sIdx = ToneControlsTests.idx(atDensity: K.shadowToneAnchor + 0.45)
        #expect(base[mIdx][1] - graded[mIdx][1] > 0.02, "green should drop in mids")
        #expect(abs(graded[sIdx][1] - base[sIdx][1]) < abs(base[mIdx][1] - graded[mIdx][1]) / 2, "mids dominate")
    }

    @Test func tempTintFoldIntoOffsets() {
        var warm = ExposureSettings()
        warm.temp = 0.8
        var tinted = ExposureSettings()
        tinted.tint = -0.5
        let base = ExposureKernel.deriveRenderParams(ExposureSettings(), Synthetic64.analysis)
        let warmP = ExposureKernel.deriveRenderParams(warm, Synthetic64.analysis)
        let tintP = ExposureKernel.deriveRenderParams(tinted, Synthetic64.analysis)

        // Temp: yellow axis fully + Planckian-coupled magenta; cyan untouched.
        let ratio = 0.0029 / 0.0057
        for (params, wb) in [(warmP, SIMD3(0.0, 0.8 * ratio, 0.8)), (tintP, SIMD3(0.0, -0.5, 0.0))] {
            for ch in 0..<3 {
                let range = abs(base.finalBounds.ceils[ch] - base.finalBounds.floors[ch])
                let expected = base.cmyOffsets[ch] + wb[ch] * K.cmyMaxDensity / range
                expectClose(params.cmyOffsets[ch], expected, accuracy: 1e-9, "ch\(ch)")
            }
        }
    }

    @Test func castStrengthBeyondOneStaysBounded() {
        // Strength 2 overcorrects harder than 1 but the kernel clamps keep the
        // per-channel curve sane (slope range, finite pivots/curvatures).
        var strong = ExposureSettings()
        strong.castRemovalStrength = 2.0
        strong.autoCastRemoval = false  // bypass confidence scaling for determinism
        var mild = ExposureSettings()
        mild.castRemovalStrength = 0.5
        mild.autoCastRemoval = false
        let pStrong = ExposureKernel.deriveRenderParams(strong, SyntheticGrid.analysis)
        let pMild = ExposureKernel.deriveRenderParams(mild, SyntheticGrid.analysis)
        var strongerSomewhere = false
        for ch in [0, 2] {  // red/blue tilt vs green
            #expect(pStrong.slopes[ch] >= K.slopeMin && pStrong.slopes[ch] <= K.slopeMax)
            #expect(pStrong.pivots[ch].isFinite && pStrong.curvatures[ch].isFinite)
            if abs(pStrong.slopes[ch] - pStrong.slopes.y) > abs(pMild.slopes[ch] - pMild.slopes.y) + 1e-9 {
                strongerSomewhere = true
            }
        }
        #expect(strongerSomewhere, "strength 2 should tilt R/B harder than 0.5")
    }

    @Test func sidecarRoundTripsGrading() throws {
        var s = ExposureSettings()
        s.temp = 0.3
        s.tint = -0.2
        s.colorShadows = SIMD3(0.1, -0.4, 0.25)
        s.colorHighs = SIMD3(-0.15, 0.05, 0.6)
        s.vibrance = 1.4
        s.saturation = 0.9
        let back = try JSONDecoder().decode(
            ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back == s)
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.colorShadows == .zero && legacy.temp == 0 && legacy.vibrance == 1.0)
    }
}
