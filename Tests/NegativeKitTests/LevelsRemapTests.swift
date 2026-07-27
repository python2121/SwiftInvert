import Foundation
import Testing

@testable import NegativeKit

/// The interactive-histogram levels remap (display-domain, one control point
/// per channel): closed-form segment math, identity, isolation to the moved
/// channel, monotonicity, the derive-time endpoint clamp, and sidecars.
@Suite struct LevelsRemapTests {
    /// encodeOutput with a levels point vs the closed-form two-segment map.
    @Test func segmentMathMatchesClosedForm() {
        // Linear values chosen so their encodes land at handy positions.
        let encoded: [Float] = [0.0, 0.1, 0.2, 0.5, 0.8, 1.0]
        let linears = encoded.map { WorkingOETF.decode($0) }
        let source = RGBImage(
            pixels: linears.flatMap { [$0, $0, $0] }, width: encoded.count, height: 1)
        let plain = ReferenceCurve.encodeOutput(source)
        let img = ReferenceCurve.encodeOutput(
            source, levelsIn: SIMD3(0.2, 0.5, 0.5), levelsOut: SIMD3(0.4, 0.5, 0.5))

        func expectRed(_ i: Int, _ want: Float) {
            #expect(abs(img.pixels[i * 3] - want) < 1e-5, "red[\(i)] = \(img.pixels[i * 3]), want \(want)")
        }
        expectRed(0, 0.0)  // endpoint pinned
        expectRed(1, 0.2)  // 0.1 × (0.4/0.2) — left segment stretches
        expectRed(2, 0.4)  // the grabbed point lands at its output
        expectRed(3, 0.625)  // 0.4 + 0.3 × (0.6/0.8) — right segment compresses
        expectRed(4, 0.85)
        expectRed(5, 1.0)  // endpoint pinned

        // Green/blue at identity: untouched (bit-equal to the plain encode of
        // the same image — encode(decode(e)) round-trips within an ulp, so
        // the raw constants aren't the right oracle).
        for i in 0..<encoded.count {
            #expect(img.pixels[i * 3 + 1] == plain.pixels[i * 3 + 1])
            #expect(img.pixels[i * 3 + 2] == plain.pixels[i * 3 + 2])
        }
    }

    @Test func identityDefaultIsPlainEncode() {
        let pixels: [Float] = (0..<48).map { Float($0) / 47.0 }
        let img = RGBImage(pixels: pixels, width: 4, height: 4)
        let plain = ReferenceCurve.encodeOutput(img)
        let dflt = ReferenceCurve.encodeOutput(
            img, levelsIn: SIMD3(repeating: 0.5), levelsOut: SIMD3(repeating: 0.5))
        #expect(plain.pixels == dflt.pixels)
    }

    /// Any in == out pair is identity, wherever the point sits.
    @Test func equalPointAnywhereIsIdentity() {
        let pixels: [Float] = (0..<48).map { Float($0) / 47.0 }
        let img = RGBImage(pixels: pixels, width: 4, height: 4)
        let plain = ReferenceCurve.encodeOutput(img)
        for x in [0.1, 0.3, 0.7, 0.95] {
            let moved = ReferenceCurve.encodeOutput(
                img, levelsIn: SIMD3(repeating: x), levelsOut: SIMD3(repeating: x))
            #expect(moved.pixels == plain.pixels, "point at \(x)")
        }
    }

    @Test func remapIsMonotone() {
        let ramp = (0...256).map { Float($0) / 256.0 }
        let img = RGBImage(pixels: ramp.flatMap { [WorkingOETF.decode($0), 0, 0] }, width: ramp.count, height: 1)
        for (a, b) in [(0.1, 0.6), (0.8, 0.3), (0.5, 0.05), (0.02, 0.98)] {
            let out = ReferenceCurve.encodeOutput(
                img, levelsIn: SIMD3(a, 0.5, 0.5), levelsOut: SIMD3(b, 0.5, 0.5))
            for i in 1...256 {
                #expect(out.pixels[i * 3] >= out.pixels[(i - 1) * 3] - 1e-6,
                    "non-monotone at \(i) for (\(a), \(b))")
            }
        }
    }

    /// Derive clamps the point off the endpoints (finite segment slopes) and
    /// passes the identity through untouched.
    @Test func deriveClampsEndpoints() {
        let analysis = ExposureAnalysis(
            baseBounds: LogNegativeBounds(
                floors: SIMD3(-2, -2, -2), ceils: SIMD3(-0.5, -0.5, -0.5)),
            anchor: 0.46, texturalRange: 1.0, shadowRefs: SIMD3(-0.8, -0.8, -0.8),
            neutralMid: nil, neutralShadow: nil, neutralHighlight: nil, neutralConfidence: nil)
        var s = ExposureSettings()
        s.levelsRed = SIMD2(0.001, 0.9999)
        let p = ExposureKernel.deriveRenderParams(s, analysis)
        #expect(p.levelsIn.x == ExposureKernel.levelsClamp)
        #expect(p.levelsOut.x == 1.0 - ExposureKernel.levelsClamp)
        // Untouched channels stay at the identity default.
        #expect(p.levelsIn.y == 0.5 && p.levelsOut.y == 0.5)
        #expect(p.levelsIn.z == 0.5 && p.levelsOut.z == 0.5)
    }

    @Test func sidecarRoundTripAndLegacyDefault() throws {
        var s = ExposureSettings()
        s.levelsRed = SIMD2(0.25, 0.4)
        s.levelsBlue = SIMD2(0.7, 0.6)
        let back = try JSONDecoder().decode(ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back == s)
        // Sidecars written before the control existed decode to identity.
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.levelsRed == SIMD2(0.5, 0.5))
        #expect(legacy.levelsGreen == SIMD2(0.5, 0.5))
        #expect(legacy.levelsBlue == SIMD2(0.5, 0.5))
    }
}
