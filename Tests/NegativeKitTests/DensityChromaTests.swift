import Foundation
import Testing

@testable import NegativeKit

/// Print Saturation (NegPy 0.45 chroma-stack rebuild; upstream's surviving
/// density-space control, renamed dye_separation in 3fb5ca8): a density-space
/// op after the H&D curve. Property tests on the CPU reference — identity at
/// default, neutral preservation, and direction. (The per-pixel spread-masked
/// Dye Separation ported alongside it was retired the same day upstream
/// deleted it as redundant.)
@Suite struct DensityChromaTests {
    /// A tiny params set with fixed bounds so pixel values are predictable.
    private func params(_ mutate: (inout RenderParams) -> Void = { _ in }) -> RenderParams {
        var p = RenderParams(
            finalBounds: LogNegativeBounds(floors: SIMD3(-2, -2, -2), ceils: SIMD3(-0.5, -0.5, -0.5)),
            slopes: SIMD3(repeating: 3.0), pivots: SIMD3(repeating: 0.4),
            curvatures: .zero, cmyOffsets: .zero,
            toeEff: 0, shoulderEff: 0, toeWidth: 2.5, shoulderWidth: 2.5,
            dMin: 0, vStar: CurveLogic.referenceLinearValue(dMin: 0))
        mutate(&p)
        return p
    }

    /// Curve output for one normalized-log triplet.
    private func print1(_ rgb: SIMD3<Double>, _ p: RenderParams) -> SIMD3<Double> {
        let img = RGBImage(pixels: [Float(rgb.x), Float(rgb.y), Float(rgb.z)], width: 1, height: 1)
        let out = ReferenceCurve.applyPrintCurve(img, params: p)
        return SIMD3(Double(out.pixels[0]), Double(out.pixels[1]), Double(out.pixels[2]))
    }

    /// Print densities (relative, BPC off) for spread measurement.
    private func densities(_ rgb: SIMD3<Double>, _ p: RenderParams) -> SIMD3<Double> {
        let t = print1(rgb, p)
        return SIMD3(-log10(max(t.x, 1e-9)), -log10(max(t.y, 1e-9)), -log10(max(t.z, 1e-9)))
    }

    private func spread(_ d: SIMD3<Double>) -> Double {
        max(d.x, max(d.y, d.z)) - min(d.x, min(d.y, d.z))
    }

    @Test func defaultIsIdentity() {
        let base = params()
        let active = params { $0.printSaturation = 1.0 }
        let px = SIMD3(0.3, 0.45, 0.6)
        #expect(print1(px, base) == print1(px, active))
    }

    /// Neutral pixels (equal channels) never move at any strength.
    @Test func neutralsAreInvariant() {
        let neutral = SIMD3(repeating: 0.5)
        let base = print1(neutral, params())
        for mutate in [{ (p: inout RenderParams) in p.printSaturation = 1.8 },
                       { p in p.printSaturation = 0.3 }] {
            let moved = print1(neutral, params(mutate))
            #expect(abs(moved.x - base.x) < 1e-9 && abs(moved.y - base.y) < 1e-9 && abs(moved.z - base.z) < 1e-9)
        }
    }

    @Test func printSaturationScalesDensitySeparation() {
        let px = SIMD3(0.35, 0.45, 0.55)
        let s0 = spread(densities(px, params()))
        let sUp = spread(densities(px, params { $0.printSaturation = 1.6 }))
        let sDown = spread(densities(px, params { $0.printSaturation = 0.4 }))
        #expect(s0 > 0.01)
        #expect(sUp > s0 * 1.3, "k > 1 must widen dye separation (\(s0) → \(sUp))")
        #expect(sDown < s0 * 0.7, "k < 1 must narrow it (\(s0) → \(sDown))")
        // k = 0 collapses to the achromatic mean.
        let sZero = spread(densities(px, params { $0.printSaturation = 0 }))
        #expect(sZero < 1e-6)
    }

    @Test func sidecarRoundTripAndLegacyDefaults() throws {
        var s = ExposureSettings()
        s.printSaturation = 1.4
        let back = try JSONDecoder().decode(ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back == s)
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.printSaturation == 1.0)
    }

    /// Retired-key compat: sidecars from the day the per-pixel Dye Separation
    /// existed carry a dyeSeparation key; it must be ignored (value dropped),
    /// mirroring NegPy 3fb5ca8's migration.
    @Test func retiredDyeSeparationKeyIsIgnored() throws {
        let old = Data(#"{"printSaturation": 1.2, "dyeSeparation": -0.3}"#.utf8)
        let decoded = try JSONDecoder().decode(ExposureSettings.self, from: old)
        var expected = ExposureSettings()
        expected.printSaturation = 1.2
        #expect(decoded == expected)
    }
}

/// Separation Damping (NegPy d86a5aa): the chroma-selective taper of Print
/// Saturation's push.
@Suite struct SeparationDampingTests {
    @Test func gainClosedForm() {
        let ref = K.separationDampingRefSpread
        // Grey pixel (chroma 0): h = 1 → full k at any damping.
        #expect(abs(CurveLogic.separationDampingGain(k: 1.4, damping: 1.0, chroma: 0) - 1.4) < 1e-12)
        // At the reference spread: h = 0 → k^(1−τ); τ=1 → exactly 1.
        #expect(abs(CurveLogic.separationDampingGain(k: 1.4, damping: 1.0, chroma: ref) - 1.0) < 1e-12)
        #expect(abs(CurveLogic.separationDampingGain(k: 1.4, damping: 0.5, chroma: ref) - pow(1.4, 0.5)) < 1e-12)
        // Far above the reference: h → −1 → 1/k at τ=1 (the reversal).
        let far = CurveLogic.separationDampingGain(k: 1.4, damping: 1.0, chroma: 100)
        #expect(abs(far - 1.0 / 1.4) < 0.01)
        // τ=0 is exactly the flat k, whatever the chroma.
        #expect(CurveLogic.separationDampingGain(k: 1.4, damping: 0, chroma: 0.7) == 1.4)
        // Clamp and the k≤0 guard.
        #expect(CurveLogic.separationDampingGain(k: 2.9, damping: 1.0, chroma: 0) == 2.9)
        #expect(CurveLogic.separationDampingGain(k: 0, damping: 0.5, chroma: 0.1) == 0)
    }

    private func params(_ mutate: (inout RenderParams) -> Void) -> RenderParams {
        var p = RenderParams(
            finalBounds: LogNegativeBounds(floors: SIMD3(-2, -2, -2), ceils: SIMD3(-0.5, -0.5, -0.5)),
            slopes: SIMD3(repeating: 3.0), pivots: SIMD3(repeating: 0.4),
            curvatures: .zero, cmyOffsets: .zero,
            toeEff: 0, shoulderEff: 0, toeWidth: 2.5, shoulderWidth: 2.5,
            dMin: 0, vStar: CurveLogic.referenceLinearValue(dMin: 0))
        mutate(&p)
        return p
    }

    private func densities(_ rgb: SIMD3<Double>, _ p: RenderParams) -> SIMD3<Double> {
        let img = RGBImage(pixels: [Float(rgb.x), Float(rgb.y), Float(rgb.z)], width: 1, height: 1)
        let out = ReferenceCurve.applyPrintCurve(img, params: p)
        return SIMD3(
            -log10(max(Double(out.pixels[0]), 1e-9)),
            -log10(max(Double(out.pixels[1]), 1e-9)),
            -log10(max(Double(out.pixels[2]), 1e-9)))
    }

    private func spread(_ d: SIMD3<Double>) -> Double {
        max(d.x, max(d.y, d.z)) - min(d.x, min(d.y, d.z))
    }

    /// Inert at printSaturation 1 (any damping), and damping 0 is bit-equal
    /// to the flat path.
    @Test func inertAndFlatEquivalence() {
        let px = SIMD3(0.3, 0.45, 0.65)
        let base = densities(px, params { _ in })
        let damped1 = densities(px, params { $0.separationDamping = 1.0 })
        #expect(base == damped1)  // printSaturation 1 → nothing to redistribute
        let flat = densities(px, params { $0.printSaturation = 1.5 })
        let tau0 = densities(px, params { $0.printSaturation = 1.5; $0.separationDamping = 0 })
        #expect(flat == tau0)
    }

    /// At full damping with k > 1: a muted pixel's dye spread still widens,
    /// a vivid pixel's spread NARROWS (the reversal a flat k cannot do).
    @Test func pushRedistributesBetweenPopulations() {
        let muted = SIMD3(0.46, 0.48, 0.50)
        let vivid = SIMD3(0.2, 0.45, 0.75)
        let pFlat = params { $0.printSaturation = 1.5 }
        let pDamped = params { $0.printSaturation = 1.5; $0.separationDamping = 1.0 }

        let mutedBase = spread(densities(muted, params { _ in }))
        let mutedDamped = spread(densities(muted, pDamped))
        #expect(mutedDamped > mutedBase * 1.2, "muted must still widen (\(mutedBase) → \(mutedDamped))")

        let vividBase = spread(densities(vivid, params { _ in }))
        let vividFlat = spread(densities(vivid, pFlat))
        let vividDamped = spread(densities(vivid, pDamped))
        #expect(vividFlat > vividBase, "flat k widens vivid too")
        #expect(vividDamped < vividBase, "damped push must REVERSE on vivid (\(vividBase) → \(vividDamped))")
    }

    @Test func sidecarRoundTripAndLegacyDefault() throws {
        var s = ExposureSettings()
        s.separationDamping = 0.4
        let back = try JSONDecoder().decode(ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back == s)
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.separationDamping == 0)
    }
}
