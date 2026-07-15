import Foundation
import Testing

@testable import NegativeKit

/// Spot densitometer + negative-character diagnostic, against closed-form
/// oracles derived from NegPy's `densitometer.py` / `stats.py` semantics.
@Suite struct DensitometryTests {

    // MARK: - Zone ruler

    @Test func zoneVIsMidGray() {
        // The ruler's defining property: 18% reflectance prints as Zone V.
        let enc = Double(WorkingOETF.encode(Float(Densitometry.zoneMidReflectance)))
        #expect(abs(Densitometry.zone(ofEncoded: enc) - 5.0) < 1e-9)
    }

    @Test func zoneEndpoints() {
        #expect(abs(Densitometry.zone(ofEncoded: 0.0) - 0.0) < 1e-9)
        #expect(abs(Densitometry.zone(ofEncoded: 1.0) - 10.0) < 1e-9)
    }

    @Test func zoneIsMonotonicAndClamped() {
        var previous = -1.0
        for i in 0...200 {
            let z = Densitometry.zone(ofEncoded: Double(i) / 200.0)
            #expect(z >= previous)
            #expect(z >= 0 && z <= 10)
            previous = z
        }
        // Out-of-range input clamps rather than extrapolating.
        #expect(Densitometry.zone(ofEncoded: -0.5) == 0.0)
        #expect(Densitometry.zone(ofEncoded: 1.5) == 10.0)
    }

    @Test func zonePiecewiseLinearHalves() {
        // Below the hinge the ruler is linear from 0 to V, above it from V to X
        // (NegPy zone_of_encoded) — check a midpoint of each leg.
        let mid = Densitometry.midGrayEncoded
        #expect(abs(Densitometry.zone(ofEncoded: mid / 2) - 2.5) < 1e-9)
        #expect(abs(Densitometry.zone(ofEncoded: (mid + 1) / 2) - 7.5) < 1e-9)
    }

    // MARK: - Print density

    @Test func printDensityOfPaperWhiteIsZero() {
        #expect(abs(Densitometry.printDensity(ofEncoded: 1.0)) < 1e-9)
    }

    @Test func printDensityClampsAtPaperBlack() {
        #expect(Densitometry.printDensity(ofEncoded: 0.0) == Densitometry.printDensityMax)
    }

    @Test func printDensityMatchesNegativeLog10OfDecodedTone() {
        // D = −log10(OETF⁻¹(enc)) over the range where the clamp is inactive.
        for enc in [0.2, 0.4, 0.6, 0.8, 1.0] {
            let t = Double(WorkingOETF.decode(Float(enc)))
            #expect(abs(Densitometry.printDensity(ofEncoded: enc) - (-log10(t))) < 1e-6)
        }
    }

    @Test func printDensityDecreasesWithLightness() {
        var previous = Double.infinity
        for i in 1...100 {
            let d = Densitometry.printDensity(ofEncoded: Double(i) / 100.0)
            #expect(d <= previous)
            previous = d
        }
    }

    @Test func midGrayPrintsNearDensityPointSevenFour() {
        // 18% reflectance ⇒ D = −log10(0.18) ≈ 0.745, the paper's mid-tone.
        let enc = Densitometry.midGrayEncoded
        #expect(abs(Densitometry.printDensity(ofEncoded: enc) - 0.7447) < 1e-3)
    }

    // MARK: - Reading

    @Test func readTakesLumaOnEncodedValuesBeforeDecoding() {
        // NegPy computes luma on the ENCODED triplet, then decodes; decoding
        // per channel first would give a different (wrong) density.
        let rgb = SIMD3<Double>(0.2, 0.6, 0.9)
        let reading = Densitometry.read(encodedRGB: rgb)
        let lum = K.lumaR * 0.2 + K.lumaG * 0.6 + K.lumaB * 0.9
        #expect(abs(reading.printDensity - Densitometry.printDensity(ofEncoded: lum)) < 1e-9)
        #expect(abs(reading.zone - Densitometry.zone(ofEncoded: lum)) < 1e-9)
        #expect(reading.rgb == rgb)
    }

    @Test func neutralGrayReadingIsSelfConsistent() {
        let enc = Densitometry.midGrayEncoded
        let reading = Densitometry.read(encodedRGB: SIMD3(enc, enc, enc))
        #expect(abs(reading.zone - 5.0) < 1e-9)
    }

    // MARK: - Zone roman numerals

    @Test func zoneRomanFormatsThirds() {
        #expect(Densitometry.zoneRoman(0) == "0")
        #expect(Densitometry.zoneRoman(3) == "III")
        #expect(Densitometry.zoneRoman(4.33) == "IV⅓")  // NegPy's own example
        #expect(Densitometry.zoneRoman(4.67) == "IV⅔")
        #expect(Densitometry.zoneRoman(5) == "V")
        #expect(Densitometry.zoneRoman(10) == "X")
    }

    @Test func zoneRomanClampsAndNeverOverflows() {
        #expect(Densitometry.zoneRoman(-3) == "0")
        #expect(Densitometry.zoneRoman(99) == "X")
        // 9.9 rounds to 30 thirds = base 10: must yield "X", not index past it.
        #expect(Densitometry.zoneRoman(9.9) == "X")
        for i in 0...1000 {
            _ = Densitometry.zoneRoman(Double(i) / 100.0)  // no trap
        }
    }

    // MARK: - Negative character

    @Test func characterThresholds() {
        let normal = CurveLogic.defaultGradeRange
        #expect(Densitometry.character(densityRange: normal) == .normal)
        // Ratios sit either side of NegPy's 0.80 / 1.25 gates.
        #expect(Densitometry.character(densityRange: normal * 0.79) == .flat)
        #expect(Densitometry.character(densityRange: normal * 0.81) == .normal)
        #expect(Densitometry.character(densityRange: normal * 1.24) == .normal)
        #expect(Densitometry.character(densityRange: normal * 1.26) == .contrasty)
    }

    @Test func characterBoundariesAreInclusiveOfNormal() {
        // NegPy: `< 0.80` is flat and `> 1.25` is contrasty, so both exact
        // ratios are normal.
        let normal = CurveLogic.defaultGradeRange
        #expect(Densitometry.character(densityRange: normal * 0.80) == .normal)
        #expect(Densitometry.character(densityRange: normal * 1.25) == .normal)
    }

    @Test func characterIsNilWithoutAMeasurement() {
        #expect(Densitometry.character(densityRange: 0) == nil)
        #expect(Densitometry.character(densityRange: -1) == nil)
        #expect(Densitometry.character(densityRange: .nan) == nil)
        #expect(Densitometry.character(densityRange: .infinity) == nil)
    }

    @Test func characterLabelsCarryTheGradeHint() {
        #expect(Densitometry.NegativeCharacter.flat.label == "flat (≈N−1)")
        #expect(Densitometry.NegativeCharacter.normal.label == "normal")
        #expect(Densitometry.NegativeCharacter.contrasty.label == "contrasty (≈N+1)")
    }

    /// The diagnostic must read the same range the curve does — a drift here
    /// would have it describe a negative the render isn't printing.
    @Test func characterReadsTheSameRangeTheCurveDoes() {
        let analysis = Synthetic64.analysis
        let range = analysis.baseBounds.luminanceDensityRange
        #expect(range > 0)
        #expect(Densitometry.character(densityRange: range) != nil)
        // Same value deriveRenderParams hands effectiveGradeRange as floorCeilRange.
        let viaCurve = CurveLogic.effectiveGradeRange(
            autoNormalizeContrast: false, floorCeilRange: range, texturalRange: analysis.texturalRange)
        #expect(viaCurve == range)
    }
}
