import Foundation
import Testing

@testable import NegativeKit

/// ramp257 fixture: the raw print curve (no analysis) over a 257-sample ramp,
/// mirroring NegPy's tests/test_characteristic_curve.py::_curve.
@Suite struct RampCurveTests {
    @Test func rampCases() throws {
        let j = try Fixtures.json("ramp257.json")
        let xs = (j["x"] as! [Double]).map { Float($0) }
        let dMin = j["d_min"] as! Double
        let cases = j["cases"] as! [String: [String: Any]]

        for (name, c) in cases {
            let slope = c["slope"] as! Double
            let pivot = c["pivot"] as! Double
            // The fixture passes raw toe/shoulder into apply_characteristic_curve
            // (no grade coupling), so RenderParams carries them unmodified.
            let params = RenderParams(
                finalBounds: LogNegativeBounds(floors: .zero, ceils: SIMD3(repeating: 1)),
                slopes: SIMD3(repeating: slope),
                pivots: SIMD3(repeating: pivot),
                curvatures: .zero,
                cmyOffsets: .zero,
                toeEff: c["toe"] as! Double,
                shoulderEff: c["shoulder"] as! Double,
                toeWidth: 2.5,
                shoulderWidth: 2.5,
                dMin: dMin,
                vStar: CurveLogic.referenceLinearValue(dMin: dMin)
            )
            var ramp = RGBImage(width: xs.count, height: 1)
            for (i, x) in xs.enumerated() { for ch in 0..<3 { ramp[0, i, ch] = x } }

            let linear = ReferenceCurve.applyPrintCurve(ramp, params: params)
            let encoded = ReferenceCurve.encodeOutput(linear)

            let expLinear = (c["output_linear"] as! [Double]).map { Float($0) }
            let expEncoded = (c["output_encoded"] as! [Double]).map { Float($0) }
            for i in 0..<xs.count {
                expectClose(Double(linear[0, i, 0]), Double(expLinear[i]), accuracy: 1e-5, "\(name) linear[\(i)]")
                expectClose(Double(encoded[0, i, 0]), Double(expEncoded[i]), accuracy: 1e-5, "\(name) encoded[\(i)]")
            }
        }
    }
}
