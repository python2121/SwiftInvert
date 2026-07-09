import Foundation
import Testing

@testable import NegativeKit

/// Property tests for the regional tone controls (exposure, shadows/highlights
/// lift + contrast). These are NegSwift additions with no NegPy fixture — the
/// contract is: zero = identity (fixture tests), each control acts only in its
/// region, in the right direction, and keeps the transfer monotone.
@Suite struct ToneControlsTests {
    /// Default print curve params over a ramp; tone fields injected per test.
    static func params(
        shadows: Double = 0, shadowContrast: Double = 0,
        highlights: Double = 0, highlightContrast: Double = 0
    ) -> RenderParams {
        let slope = CurveLogic.gradeToSlope(115.0, densityRange: 1.3)
        let pivot = CurveLogic.computePivot(slope: slope, density: 1.0, dMin: K.dMin)
        return RenderParams(
            finalBounds: LogNegativeBounds(floors: .zero, ceils: SIMD3(repeating: 1)),
            slopes: SIMD3(repeating: slope), pivots: SIMD3(repeating: pivot), curvatures: .zero,
            cmyOffsets: .zero, toeEff: 0, shoulderEff: 0, toeWidth: 2.5, shoulderWidth: 2.5,
            dMin: K.dMin, vStar: CurveLogic.referenceLinearValue(dMin: K.dMin),
            shadows: shadows, shadowContrast: shadowContrast,
            highlights: highlights, highlightContrast: highlightContrast)
    }

    static func rampEncoded(_ p: RenderParams, n: Int = 257) -> [Float] {
        var ramp = RGBImage(width: n, height: 1)
        for i in 0..<n {
            let x = Float(i) / Float(n - 1)
            for ch in 0..<3 { ramp[0, i, ch] = x }
        }
        let out = ReferenceCurve.encodeOutput(ReferenceCurve.applyPrintCurve(ramp, params: p))
        return (0..<n).map { out[0, $0, 0] }
    }

    // Ramp regions: x near 0 prints bright (highlights), x near 1 prints dark.
    static let hiIdx = 0..<40      // x < ~0.16
    static let shIdx = 215..<257   // x > ~0.84

    @Test func shadowsLiftActsOnlyOnShadows() {
        let base = Self.rampEncoded(Self.params())
        let lifted = Self.rampEncoded(Self.params(shadows: 1.0))
        let shadowDelta = zip(Self.shIdx.map { lifted[$0] }, Self.shIdx.map { base[$0] }).map(-).max()!
        #expect(shadowDelta > 0.05, "shadows +1 should lighten the dark end (got \(shadowDelta))")
        let highlightLeak = Self.hiIdx.map { abs(lifted[$0] - base[$0]) }.max()!
        #expect(highlightLeak < 0.015, "shadows leaked into highlights: \(highlightLeak)")
    }

    @Test func highlightsRecoveryActsOnlyOnHighlights() {
        let base = Self.rampEncoded(Self.params())
        let recovered = Self.rampEncoded(Self.params(highlights: -1.0))
        let highlightDelta = zip(Self.hiIdx.map { base[$0] }, Self.hiIdx.map { recovered[$0] }).map(-).max()!
        #expect(highlightDelta > 0.04, "highlights −1 should darken the bright end (got \(highlightDelta))")
        let shadowLeak = Self.shIdx.map { abs(recovered[$0] - base[$0]) }.max()!
        #expect(shadowLeak < 0.015, "highlights leaked into shadows: \(shadowLeak)")
    }

    /// Ramp index whose straight-line print density is `v` (before knees).
    static func idx(atDensity v: Double, n: Int = 257) -> Int {
        let slope = CurveLogic.gradeToSlope(115.0, densityRange: 1.3)
        let pivot = CurveLogic.computePivot(slope: slope, density: 1.0, dMin: K.dMin)
        let x = pivot + v / slope
        return min(max(Int((x * Double(n - 1)).rounded()), 0), n - 1)
    }

    @Test func shadowContrastExpandsDarkSeparation() {
        let base = Self.rampEncoded(Self.params())
        let contrasty = Self.rampEncoded(Self.params(shadowContrast: 1.0))
        // Separation grows around the shadow anchor (measured between a
        // medium-dark and a deep-shadow tone that isn't yet toe-crushed).
        let a = Self.idx(atDensity: K.shadowToneAnchor - 0.25)
        let b = Self.idx(atDensity: K.shadowToneAnchor + 0.4)
        let baseSpread = base[a] - base[b]
        let newSpread = contrasty[a] - contrasty[b]
        #expect(newSpread > baseSpread + 0.01, "shadow contrast spread \(baseSpread) → \(newSpread)")
        let highlightLeak = Self.hiIdx.map { abs(contrasty[$0] - base[$0]) }.max()!
        #expect(highlightLeak < 0.015, "shadow contrast leaked into highlights: \(highlightLeak)")
    }

    @Test func highlightContrastExpandsBrightSeparation() {
        let base = Self.rampEncoded(Self.params())
        let contrasty = Self.rampEncoded(Self.params(highlightContrast: 1.0))
        let a = Self.idx(atDensity: K.highlightToneAnchor - 0.2)
        let b = Self.idx(atDensity: K.highlightToneAnchor + 0.3)
        let baseSpread = base[a] - base[b]
        let newSpread = contrasty[a] - contrasty[b]
        #expect(newSpread > baseSpread + 0.01, "highlight contrast spread \(baseSpread) → \(newSpread)")
        let shadowLeak = Self.shIdx.map { abs(contrasty[$0] - base[$0]) }.max()!
        #expect(shadowLeak < 0.015, "highlight contrast leaked into shadows: \(shadowLeak)")
    }

    @Test(arguments: [
        [1.0, 0.0, 0.0, 0.0], [-1.0, 0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0, 0.0], [-2.0, 0.0, 0.0, 0.0],  // extended shadows range
        [0.0, 1.0, 0.0, 0.0], [0.0, -1.0, 0.0, 0.0],
        [0.0, 6.0, 0.0, 0.0], [0.0, -3.0, 0.0, 0.0],  // extended shadow-contrast range (kernel floor guards −3)
        [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, -1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, -1.0],
    ])
    func monotoneAtFullDeflection(config: [Double]) {
        let out = Self.rampEncoded(
            Self.params(
                shadows: config[0], shadowContrast: config[1],
                highlights: config[2], highlightContrast: config[3]))
        for i in 1..<out.count {
            #expect(out[i] <= out[i - 1] + 1e-4, "non-monotone at \(i) for \(config)")
        }
    }

    @Test func exposureStopsBrightenGlobally() {
        var brighter = ExposureSettings()
        brighter.exposureStops = 1.0
        let base = ExposureKernel.deriveRenderParams(ExposureSettings(), Synthetic64.analysis)
        let plusOne = ExposureKernel.deriveRenderParams(brighter, Synthetic64.analysis)
        // One stop = −log10(2)/range on every channel's pre-curve offset.
        for ch in 0..<3 {
            let range = abs(base.finalBounds.ceils[ch] - base.finalBounds.floors[ch])
            expectClose(
                plusOne.cmyOffsets[ch] - base.cmyOffsets[ch], -K.log10Two / range,
                accuracy: 1e-9, "offset ch\(ch)")
        }
        // And the rendered frame gets brighter everywhere it isn't clipped.
        let baseOut = ReferenceCurve.render(
            linearImage: Synthetic64.input, settings: ExposureSettings(), analysis: Synthetic64.analysis)
        let brightOut = ReferenceCurve.render(
            linearImage: Synthetic64.input, settings: brighter, analysis: Synthetic64.analysis)
        let meanBase = baseOut.pixels.reduce(0.0) { $0 + Double($1) } / Double(baseOut.pixels.count)
        let meanBright = brightOut.pixels.reduce(0.0) { $0 + Double($1) } / Double(brightOut.pixels.count)
        #expect(meanBright > meanBase + 0.05, "exposure +1: mean \(meanBase) → \(meanBright)")
    }

    @Test func overallContrastRotatesAroundAnchor() throws {
        var contrasty = ExposureSettings()
        contrasty.overallContrast = 1.5
        var flat = ExposureSettings()
        flat.overallContrast = -1.0
        let base = ExposureKernel.deriveRenderParams(ExposureSettings(), Synthetic64.analysis)
        let up = ExposureKernel.deriveRenderParams(contrasty, Synthetic64.analysis)
        let down = ExposureKernel.deriveRenderParams(flat, Synthetic64.analysis)

        // Slopes scale by (1+k); k = slider × overallContrastMax.
        for ch in 0..<3 {
            expectClose(up.slopes[ch], base.slopes[ch] * 1.75, accuracy: 1e-9, "slope up ch\(ch)")
            expectClose(down.slopes[ch], base.slopes[ch] * 0.5, accuracy: 1e-9, "slope down ch\(ch)")
        }

        // The input that hits v* on the base curve still hits v* (green, curv 0):
        // the reference tone's print density is invariant under contrast.
        let vStar = base.vStar
        let uStar = base.pivots.y + vStar / base.slopes.y
        let vUp = up.slopes.y * (uStar - up.pivots.y) + up.curvatures.y * uStar * uStar
        expectClose(vUp, vStar, accuracy: 1e-9, "anchor invariance")

        // End-to-end: rendered spread grows, anchor output stays put, monotone.
        let baseOut = Self.rampEncoded(Self.params())
        var p = Self.params()
        let k = 1.5 * K.overallContrastMax
        for ch in 0..<3 {
            let scaled = p.slopes[ch] * (1 + k)
            p.curvatures[ch] *= (1 + k)
            p.pivots[ch] += k * p.vStar / scaled
            p.slopes[ch] = scaled
        }
        let contrastOut = Self.rampEncoded(p)
        let anchorIdx = Int((0.46 * 256.0).rounded())  // assumed_anchor input
        expectClose(
            Double(contrastOut[anchorIdx]), Double(baseOut[anchorIdx]), accuracy: 0.01, "anchor output")
        let a = Self.idx(atDensity: 0.3), b = Self.idx(atDensity: 1.3)
        #expect(contrastOut[a] - contrastOut[b] > baseOut[a] - baseOut[b] + 0.02, "spread grows")
        for i in 1..<contrastOut.count {
            #expect(contrastOut[i] <= contrastOut[i - 1] + 1e-4, "monotone at \(i)")
        }
    }

    @Test func zeroSettingsAreIdentity() {
        // Belt-and-braces on top of the fixture tests: explicit zero tone fields
        // produce byte-identical output to a params struct without them.
        let base = Self.rampEncoded(Self.params())
        let explicitZero = Self.rampEncoded(
            Self.params(shadows: 0, shadowContrast: 0, highlights: 0, highlightContrast: 0))
        #expect(base == explicitZero)
    }
}
