import Foundation
import Testing

@testable import NegativeKit

/// Scalar oracles dumped from NegPy (Tests/Fixtures/closed_form.json).
@Suite struct ClosedFormTests {
    let fixture: [String: Any]

    init() throws {
        fixture = try Fixtures.json("closed_form.json")
    }

    private func cases(_ key: String) throws -> [[String: Any]] {
        try #require(fixture[key] as? [[String: Any]], "\(key)")
    }

    @Test func percentile() throws {
        for c in try cases("percentile") {
            let data = (c["data"] as! [Double]).map { Float($0) }
            expectClose(Stats.percentile(data, c["q"] as! Double), c["out"] as! Double, accuracy: 1e-6, "\(c)")
        }
    }

    @Test func cmyDensityConversions() throws {
        for c in try cases("cmy_to_density") {
            expectClose(
                CurveLogic.cmyToDensity(c["val"] as! Double, logRange: c["log_range"] as! Double),
                c["out"] as! Double, accuracy: 1e-9, "\(c)")
        }
        for c in try cases("density_to_cmy") {
            expectClose(
                CurveLogic.densityToCmy(c["density"] as! Double, logRange: c["log_range"] as! Double),
                c["out"] as! Double, accuracy: 1e-9, "\(c)")
        }
    }

    @Test func gradeSlope() throws {
        for c in try cases("grade_to_slope") {
            expectClose(
                CurveLogic.gradeToSlope(c["grade"] as! Double, densityRange: c["range"] as? Double),
                c["out"] as! Double, accuracy: 1e-9, "\(c)")
        }
        for c in try cases("slope_to_grade") {
            expectClose(
                CurveLogic.slopeToGrade(c["slope"] as! Double, densityRange: c["range"] as? Double),
                c["out"] as! Double, accuracy: 1e-9, "\(c)")
        }
    }

    @Test func pivotAndReference() throws {
        for c in try cases("compute_pivot") {
            let got = CurveLogic.computePivot(
                slope: c["slope"] as! Double, density: c["density"] as! Double,
                dMin: c["d_min"] as! Double, anchor: c["anchor"] as? Double)
            expectClose(got, c["out"] as! Double, accuracy: 1e-9, "\(c)")
        }
        for c in try cases("reference_linear_value") {
            expectClose(
                CurveLogic.referenceLinearValue(dMin: c["d_min"] as! Double),
                c["out"] as! Double, accuracy: 1e-9, "\(c)")
        }
    }

    @Test func softplusOracles() throws {
        for c in try cases("softplus") {
            expectClose(CurveLogic.softplus(c["x"] as! Double), c["out"] as! Double, accuracy: 1e-12, "\(c)")
        }
        for c in try cases("inv_softplus") {
            expectClose(CurveLogic.invSoftplus(c["y"] as! Double), c["out"] as! Double, accuracy: 1e-9, "\(c)")
        }
    }

    @Test func gradeRangeAndShape() throws {
        for c in try cases("effective_grade_range") {
            let got = CurveLogic.effectiveGradeRange(
                autoNormalizeContrast: c["auto"] as! Bool,
                floorCeilRange: c["floor_ceil"] as? Double,
                texturalRange: c["textural"] as? Double)
            expectClose(try #require(got), c["out"] as! Double, accuracy: 1e-9, "\(c)")
        }
        for c in try cases("grade_coupled_shape") {
            let out = c["out"] as! [Double]
            let got = CurveLogic.gradeCoupledShape(
                slopeG: c["slope_g"] as! Double, toe: c["toe"] as! Double, shoulder: c["shoulder"] as! Double)
            expectClose(got.toe, out[0], accuracy: 1e-9)
            expectClose(got.shoulder, out[1], accuracy: 1e-9)
        }
        for c in try cases("effective_cast_strength") {
            let got = CurveLogic.effectiveCastStrength(
                c["strength"] as! Double, auto: c["auto"] as! Bool, confidence: c["confidence"] as? Double)
            expectClose(got, c["out"] as! Double, accuracy: 1e-9, "\(c)")
        }
    }

    @Test func workingOETF() throws {
        for c in try cases("working_oetf") {
            let x = Float(c["x"] as! Double)
            expectClose(Double(WorkingOETF.encode(x)), c["enc"] as! Double, accuracy: 1e-6, "\(c)")
            expectClose(
                Double(WorkingOETF.decode(WorkingOETF.encode(x))), c["dec_of_enc"] as! Double, accuracy: 1e-6, "\(c)")
        }
    }

    @Test func filtrationOffsetsSample() throws {
        let c = try #require(fixture["filtration_offsets_sample"] as? [String: Any])
        let wb = c["wb_cmy"] as! [Double]
        let floors = c["floors"] as! [Double]
        let ceils = c["ceils"] as! [Double]
        let out = c["out"] as! [Double]
        let bounds = LogNegativeBounds(
            floors: SIMD3(floors[0], floors[1], floors[2]), ceils: SIMD3(ceils[0], ceils[1], ceils[2]))
        let got = CurveLogic.filtrationOffsets(wbCMY: SIMD3(wb[0], wb[1], wb[2]), bounds: bounds)
        for ch in 0..<3 { expectClose(got[ch], out[ch], accuracy: 1e-9) }
    }
}
