import Foundation
import Testing

@testable import NegativeKit

/// The 8532dd92 automatic helpers: the textured-cell activity gate, Shadow
/// Reach's slope floor and Highlight Hold's burn. Numerical parity with
/// upstream lives in ClosedFormTests (dumped oracle vectors) and the
/// synthetic64 chain (expo_dark drives the burn to its cap); these pin the
/// PROPERTIES the port must keep under any constants retune.
@Suite struct AutoHelpersTests {
    // MARK: - Textured-cell gate

    /// A grid smaller than one sector (2×2 blocks of activityBlock cells)
    /// can't vote — every cell passes through.
    @Test func gateFallsBackOnTinyGrids() {
        let lum = [Float](repeating: 0.5, count: 8 * 8)
        #expect(Meters.texturedCells(lum, width: 8, height: 8).count == lum.count)
    }

    /// A flat frame has no active sectors — the min-fraction fallback keeps
    /// every cell voting instead of metering an empty set.
    @Test func gateFallsBackOnFlatFrames() {
        let lum = [Float](repeating: 0.7, count: 32 * 32)
        #expect(Meters.texturedCells(lum, width: 32, height: 32).count == lum.count)
    }

    /// Half the frame carries texture, half is a flat wall: only the
    /// textured sectors' cells survive, and they keep their values.
    @Test func gateDropsFlatSectors() {
        let w = 32, h = 32
        var lum = [Float](repeating: 0.2, count: w * h)
        // Left two sectors (x < 16): a checkerboard whose 8×8 block means
        // differ well past the 0.05 gate.
        for y in 0..<h {
            for x in 0..<16 {
                lum[y * w + x] = ((x / 8) + (y / 8)) % 2 == 0 ? 0.1 : 0.9
            }
        }
        let kept = Meters.texturedCells(lum, width: w, height: h)
        #expect(kept.count == 16 * 32)
        #expect(kept.allSatisfy { $0 != 0.2 })
    }

    // MARK: - Shadow Reach

    /// The reach only ever raises: a slope already past the required line is
    /// returned untouched, a flat one is lifted onto it, and the lift stays
    /// under the global slope clamp.
    @Test func reachNeverLowersAndClamps() {
        let raised = CurveLogic.shadowReachSlope(2.0, anchor: 0.46, shadowPoint: 0.95)
        #expect(raised > 2.0)
        #expect(CurveLogic.shadowReachSlope(raised + 1, anchor: 0.46, shadowPoint: 0.95) == raised + 1)
        // A hair past the anchor needs an absurd slope — the clamp holds it.
        #expect(CurveLogic.shadowReachSlope(2.0, anchor: 0.46, shadowPoint: 0.4601) == K.slopeMax)
        // Degenerate / inverted spans change nothing.
        #expect(CurveLogic.shadowReachSlope(2.0, anchor: 0.46, shadowPoint: 0.46) == 2.0)
        #expect(CurveLogic.shadowReachSlope(2.0, anchor: 0.6, shadowPoint: 0.5) == 2.0)
    }

    /// The raised slope really does print the dark tail at the reach density:
    /// the line through the anchor's v* evaluated at the tail lands on
    /// referenceLinearValue(target: shadowReachDensity).
    @Test func reachLandsTheTail() {
        let anchor = 0.46, tail = 0.95
        let slope = CurveLogic.shadowReachSlope(2.0, anchor: anchor, shadowPoint: tail)
        let vAtTail = CurveLogic.referenceLinearValue() + slope * (tail - anchor)
        expectClose(
            vAtTail, CurveLogic.referenceLinearValue(target: K.shadowReachDensity),
            accuracy: 1e-9, "tail straight-line value")
    }

    // MARK: - Highlight Hold

    /// The hold never lifts (0 when the tail already prints deep enough),
    /// burns when the line would print it brighter, and caps.
    @Test func holdNeverLiftsAndCaps() {
        #expect(CurveLogic.highlightHoldOffset(slope: 3.0, pivot: 0.22, highlightPoint: 0.30) == 0)
        let burn = CurveLogic.highlightHoldOffset(slope: 3.0, pivot: 0.22, highlightPoint: 0.05)
        #expect(burn > 0)
        #expect(burn <= K.highlightHoldMax)
        #expect(CurveLogic.highlightHoldOffset(slope: 3.0, pivot: 0.55, highlightPoint: 0.10) == K.highlightHoldMax)
    }

    /// deriveRenderParams gates the burn on Auto Grade and the measured
    /// bright tail, and routes it through RenderParams.autoHighlight — never
    /// a settings field.
    @Test func deriveGatesTheBurn() {
        var analysis = ExposureAnalysis(
            baseBounds: LogNegativeBounds(floors: SIMD3(-2, -2, -2), ceils: SIMD3(-0.5, -0.5, -0.5)),
            anchor: 0.46, texturalRange: 1.0,
            shadowPoint: 0.9, highlightPoint: 0.02,
            shadowRefs: SIMD3(-0.8, -0.8, -0.8))
        var s = ExposureSettings()
        let on = ExposureKernel.deriveRenderParams(s, analysis)
        #expect(on.autoHighlight > 0)
        expectClose(
            on.autoHighlight,
            CurveLogic.highlightHoldOffset(
                slope: on.slopes.y, pivot: on.pivots.y, highlightPoint: 0.02,
                dMin: on.dMin),
            accuracy: 1e-12, "burn from the rendered green line")

        s.autoNormalizeContrast = false
        #expect(ExposureKernel.deriveRenderParams(s, analysis).autoHighlight == 0)

        s.autoNormalizeContrast = true
        analysis.highlightPoint = nil  // fixture-built analyses: unmeasured → inert
        #expect(ExposureKernel.deriveRenderParams(s, analysis).autoHighlight == 0)
    }

    /// The kernel term only ever adds density (the print gets darker,
    /// reflectance drops) and its weight lives in the highlight zone: a deep
    /// shadow tone moves by orders of magnitude less than a highlight tone.
    @Test func burnDarkensHighlightsOnly() {
        let analysis = ExposureAnalysis(
            baseBounds: LogNegativeBounds(floors: SIMD3(-2, -2, -2), ceils: SIMD3(-0.5, -0.5, -0.5)),
            anchor: 0.46, texturalRange: 1.0,
            shadowRefs: SIMD3(-0.8, -0.8, -0.8))
        var params = ExposureKernel.deriveRenderParams(ExposureSettings(), analysis)
        // A luma ramp from print highlights to shadows (normalized-log in).
        let n = 64
        var pixels = [Float](repeating: 0, count: n * 3)
        for i in 0..<n {
            let t = Float(i) / Float(n - 1) * 1.2 - 0.1
            pixels[i * 3] = t
            pixels[i * 3 + 1] = t
            pixels[i * 3 + 2] = t
        }
        let ramp = RGBImage(pixels: pixels, width: n, height: 1)
        let base = ReferenceCurve.applyPrintCurve(ramp, params: params)
        params.autoHighlight = 0.3
        let burned = ReferenceCurve.applyPrintCurve(ramp, params: params)
        var maxHighlightDrop: Float = 0
        var maxShadowDrop: Float = 0
        for i in 0..<n {
            let drop = base.pixels[i * 3 + 1] - burned.pixels[i * 3 + 1]
            #expect(drop >= -1e-6, "burn must never brighten (i=\(i))")
            if base.pixels[i * 3 + 1] > 0.5 { maxHighlightDrop = max(maxHighlightDrop, drop) }
            if base.pixels[i * 3 + 1] < 0.02 { maxShadowDrop = max(maxShadowDrop, drop) }
        }
        #expect(maxHighlightDrop > 0.05, "the burn must land in the highlights")
        #expect(maxShadowDrop < maxHighlightDrop / 20, "deep shadows must be spared")
    }
}
