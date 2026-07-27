import Foundation
import Testing

@testable import NegativeKit

/// The interactive-histogram levels remap (display-domain, piecewise-linear
/// anchors per channel): closed-form segment math, identity, channel
/// isolation, monotonicity, the derive-time sanitizer, and sidecars.
@Suite struct LevelsRemapTests {
    /// One anchor: the classic two-segment stretch/compress.
    @Test func singleAnchorMatchesClosedForm() {
        let encoded: [Float] = [0.0, 0.1, 0.2, 0.5, 0.8, 1.0]
        let linears = encoded.map { WorkingOETF.decode($0) }
        let source = RGBImage(
            pixels: linears.flatMap { [$0, $0, $0] }, width: encoded.count, height: 1)
        let plain = ReferenceCurve.encodeOutput(source)
        let img = ReferenceCurve.encodeOutput(
            source, levels: [[SIMD2(0.2, 0.4)], [], []])

        func expectRed(_ i: Int, _ want: Float) {
            #expect(abs(img.pixels[i * 3] - want) < 1e-5, "red[\(i)] = \(img.pixels[i * 3]), want \(want)")
        }
        expectRed(0, 0.0)  // endpoint pinned
        expectRed(1, 0.2)  // 0.1 × (0.4/0.2) — left segment stretches
        expectRed(2, 0.4)  // the anchor lands at its output
        expectRed(3, 0.625)  // 0.4 + 0.3 × (0.6/0.8) — right segment compresses
        expectRed(4, 0.85)
        expectRed(5, 1.0)  // endpoint pinned

        // Untouched channels: bit-equal to the plain encode.
        for i in 0..<encoded.count {
            #expect(img.pixels[i * 3 + 1] == plain.pixels[i * 3 + 1])
            #expect(img.pixels[i * 3 + 2] == plain.pixels[i * 3 + 2])
        }
    }

    /// Two anchors: three independent segments; each anchor pins its output
    /// and tones between interpolate linearly.
    @Test func twoAnchorsPiecewise() {
        let pts: [SIMD2<Float>] = [SIMD2(0.25, 0.4), SIMD2(0.75, 0.8)]
        #expect(abs(ReferenceCurve.levelsRemap(0.25, pts) - 0.4) < 1e-6)
        #expect(abs(ReferenceCurve.levelsRemap(0.75, pts) - 0.8) < 1e-6)
        // Midpoint of the middle segment: (0.25..0.75)→(0.4..0.8) is linear.
        #expect(abs(ReferenceCurve.levelsRemap(0.5, pts) - 0.6) < 1e-6)
        // Left segment: 0.125 → 0.2; right segment: 0.875 → 0.9.
        #expect(abs(ReferenceCurve.levelsRemap(0.125, pts) - 0.2) < 1e-6)
        #expect(abs(ReferenceCurve.levelsRemap(0.875, pts) - 0.9) < 1e-6)
        // Endpoints pinned.
        #expect(ReferenceCurve.levelsRemap(0.0, pts) == 0.0)
        #expect(abs(ReferenceCurve.levelsRemap(1.0, pts) - 1.0) < 1e-6)
        // An anchor moved later only bends ITS segments: tones left of the
        // untouched first anchor are identical under both maps.
        let moved: [SIMD2<Float>] = [SIMD2(0.25, 0.4), SIMD2(0.75, 0.6)]
        for e: Float in [0.05, 0.15, 0.25] {
            #expect(ReferenceCurve.levelsRemap(e, pts) == ReferenceCurve.levelsRemap(e, moved))
        }
    }

    @Test func emptyIsIdentity() {
        let pixels: [Float] = (0..<48).map { Float($0) / 47.0 }
        let img = RGBImage(pixels: pixels, width: 4, height: 4)
        let plain = ReferenceCurve.encodeOutput(img)
        let dflt = ReferenceCurve.encodeOutput(img, levels: [[], [], []])
        #expect(plain.pixels == dflt.pixels)
    }

    @Test func remapIsMonotone() {
        let ramp = (0...256).map { Float($0) / 256.0 }
        let anchorSets: [[SIMD2<Float>]] = [
            [SIMD2(0.1, 0.6)],
            [SIMD2(0.8, 0.3)],
            [SIMD2(0.2, 0.3), SIMD2(0.5, 0.55), SIMD2(0.9, 0.95)],
            [SIMD2(0.3, 0.3), SIMD2(0.31, 0.9)],  // near-vertical segment
        ]
        for pts in anchorSets {
            var last: Float = -1
            for e in ramp {
                let y = ReferenceCurve.levelsRemap(e, pts)
                #expect(y >= last - 1e-6, "non-monotone at \(e) for \(pts)")
                last = y
            }
        }
    }

    /// The sanitizer: sorts by input, clamps off the endpoints, drops
    /// duplicate inputs, forces outputs monotone, caps at 8, and collapses
    /// all-identity sets to empty.
    @Test func sanitizerContract() {
        let clamp = ExposureKernel.levelsClamp
        let dirty: [SIMD2<Double>] = [
            SIMD2(0.9, 0.95),
            SIMD2(0.001, 0.3),  // clamps to (clamp, 0.3)
            SIMD2(0.5, 0.1),  // output below its left neighbour → raised
            SIMD2(0.5, 0.6),  // duplicate input → dropped
        ]
        let clean = ExposureKernel.sanitizeLevels(dirty)
        #expect(clean.count == 3)
        #expect(clean.map(\.x) == clean.map(\.x).sorted())
        #expect(clean[0] == SIMD2(clamp, 0.3))
        #expect(clean[1].x == 0.5 && clean[1].y == 0.3)  // raised to monotone
        #expect(clean[2] == SIMD2(0.9, 0.95))

        // Cap.
        let many: [SIMD2<Double>] = (0..<12).map { i in
            let x: Double = 0.05 + 0.08 * Double(i)
            let y: Double = 0.05 + 0.081 * Double(i)
            return SIMD2<Double>(x, y)
        }
        #expect(ExposureKernel.sanitizeLevels(many).count == ExposureKernel.levelsMaxPoints)

        // Identity-only sets vanish.
        #expect(ExposureKernel.sanitizeLevels([SIMD2(0.3, 0.3), SIMD2(0.7, 0.7)]) == [])
        #expect(ExposureKernel.sanitizeLevels([]) == [])
    }

    @Test func deriveSanitizesPerChannel() {
        let analysis = ExposureAnalysis(
            baseBounds: LogNegativeBounds(
                floors: SIMD3(-2, -2, -2), ceils: SIMD3(-0.5, -0.5, -0.5)),
            anchor: 0.46, texturalRange: 1.0, shadowRefs: SIMD3(-0.8, -0.8, -0.8),
            neutralMid: nil, neutralShadow: nil, neutralHighlight: nil, neutralConfidence: nil)
        var s = ExposureSettings()
        s.levelsRed = [SIMD2(0.7, 0.6), SIMD2(0.2, 0.3)]  // unsorted
        s.levelsBlue = [SIMD2(0.4, 0.4)]  // identity → empty
        let p = ExposureKernel.deriveRenderParams(s, analysis)
        #expect(p.levelsPoints[0] == [SIMD2(0.2, 0.3), SIMD2(0.7, 0.6)])
        #expect(p.levelsPoints[1] == [])
        #expect(p.levelsPoints[2] == [])
    }

    @Test func sidecarRoundTripAndLegacyDecodes() throws {
        var s = ExposureSettings()
        s.levelsRed = [SIMD2(0.25, 0.4), SIMD2(0.6, 0.7)]
        s.levelsBlue = [SIMD2(0.7, 0.6)]
        let back = try JSONDecoder().decode(ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back == s)
        // Pre-levels sidecars decode to identity.
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.levelsRed == [] && legacy.levelsGreen == [] && legacy.levelsBlue == [])
        // The brief single-point era ([x, y] scalar pair) decodes as one
        // anchor; identity pairs drop.
        let single = Data(#"{"levelsRed": [0.2, 0.45], "levelsGreen": [0.5, 0.5]}"#.utf8)
        let migrated = try JSONDecoder().decode(ExposureSettings.self, from: single)
        #expect(migrated.levelsRed == [SIMD2(0.2, 0.45)])
        #expect(migrated.levelsGreen == [])
    }
}
