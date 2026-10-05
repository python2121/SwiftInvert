import Foundation
import Testing

@testable import NegativeKit

/// The cast-removal quadratic must not fold back past its vertex (NegPy
/// d4dc3e49). The NegPy fixtures can't cover this: their fitted vertices sit
/// far outside the data, so the hold is pinned here by property.
@Suite struct QuadraticCoreTests {
    static let slope = CurveLogic.gradeToSlope(115.0, densityRange: 1.3)
    static let pivot = CurveLogic.computePivot(slope: slope, density: 1.0, dMin: K.dMin)
    /// R at the positive curvature clamp, G straight, B at the negative one.
    static let curvatures = SIMD3(K.neutralAxisCurvMaxRatio * slope, 0, -K.neutralAxisCurvMaxRatio * slope)

    @Test func identityInsideTheVertexAndHeldBeyond() {
        let k = Self.slope, p = Self.pivot
        for curv in [Self.curvatures.x, Self.curvatures.z] {
            let vertex = -k / (2 * curv)
            let atVertex = k * (vertex - p) + curv * vertex * vertex
            // Toward the data from the vertex: the plain quadratic.
            let inside = vertex + (curv > 0 ? 0.5 : -0.5)
            #expect(ReferenceCurve.quadraticCore(slope: k, pivot: p, curvature: curv, inside)
                == k * (inside - p) + curv * inside * inside)
            // Past it: held, however far out.
            for far in [0.01, 1.0, 5.0] {
                let u = vertex - (curv > 0 ? far : -far)
                #expect(ReferenceCurve.quadraticCore(slope: k, pivot: p, curvature: curv, u) == atVertex)
            }
        }
        #expect(ReferenceCurve.quadraticCore(slope: k, pivot: p, curvature: 0, -9) == k * (-9 - p))
    }

    /// Far-out-of-range input (holder edges and dust below 0, bare light
    /// above 1) must keep printing monotonically in every channel.
    @Test func printCurveStaysMonotoneFarOutsideTheFrame() {
        let p = RenderParams(
            finalBounds: LogNegativeBounds(floors: .zero, ceils: SIMD3(repeating: 1)),
            slopes: SIMD3(repeating: Self.slope), pivots: SIMD3(repeating: Self.pivot),
            curvatures: Self.curvatures,
            cmyOffsets: .zero, toeEff: 0, shoulderEff: 0, toeWidth: 2.5, shoulderWidth: 2.5,
            dMin: K.dMin, vStar: CurveLogic.referenceLinearValue(dMin: K.dMin))
        let n = 601
        var ramp = RGBImage(width: n, height: 1)
        for i in 0..<n {
            let u = -4.0 + 6.0 * Double(i) / Double(n - 1)
            for ch in 0..<3 { ramp[0, i, ch] = Float(u) }
        }
        let out = ReferenceCurve.applyPrintCurve(ramp, params: p)
        for ch in 0..<3 {
            for i in 1..<n {
                #expect(out[0, i, ch] <= out[0, i - 1, ch] + 1e-6,
                    "channel \(ch) reverses at u = \(-4.0 + 6.0 * Double(i) / Double(n - 1))")
            }
            // The dense end prints the straight channel's paper white, not a
            // folded-back tone.
            #expect(abs(out[0, 0, ch] - out[0, 0, 1]) < 0.02)
        }
    }
}
