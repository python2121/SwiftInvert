import Foundation
import Testing
#if canImport(simd)
import simd
#endif

@testable import NegativeKit

/// Saturation/vibrance parity against NegPy's CIELAB chroma ops
/// (Tests/Fixtures/lab_color, dumped from negpy/features/lab/logic.py).
@Suite struct LabColorTests {
    @Test func fixtureParity() throws {
        let manifest = try Fixtures.json("lab_color/manifest.json")
        let inputInfo = manifest["input"] as! [String: Any]
        let shape = inputInfo["shape"] as! [Int]
        let input = RGBImage(
            pixels: try Fixtures.floats("lab_color/input.bin"), width: shape[1], height: shape[0])

        for (name, caseAny) in manifest["cases"] as! [String: [String: Any]] {
            let vibrance = caseAny["vibrance"] as! Double
            let saturation = caseAny["saturation"] as! Double
            let expected = try Fixtures.floats("lab_color/\(name).bin")
            let got = LabColor.apply(input, vibrance: vibrance, saturation: saturation)
            var maxDiff: Float = 0
            for i in 0..<expected.count { maxDiff = max(maxDiff, abs(got.pixels[i] - expected[i])) }
            #expect(maxDiff < 1e-4, "\(name): max diff \(maxDiff)")
        }
    }

    @Test func neutralStayNeutral() {
        // Grays have zero chroma — no op may tint them.
        var img = RGBImage(width: 8, height: 1)
        for i in 0..<8 { for c in 0..<3 { img[0, i, c] = Float(i) / 7.0 } }
        let out = LabColor.apply(img, vibrance: 2.0, saturation: 1.8)
        for i in 0..<img.pixels.count {
            #expect(abs(out.pixels[i] - img.pixels[i]) < 1e-4, "neutral shifted at \(i)")
        }
    }

    @Test func identityAtOne() {
        let img = RGBImage(pixels: [0.2, 0.5, 0.7, 0.9, 0.1, 0.3], width: 2, height: 1)
        let out = LabColor.apply(img, vibrance: 1.0, saturation: 1.0)
        #expect(out.pixels == img.pixels)
    }

    @Test func vibranceProtectsSaturatedColors() {
        // A muted color gains more chroma than an already-saturated one.
        func chromaGain(_ rgb: SIMD3<Double>) -> Double {
            let before = LabColor.rgbToLab(rgb)
            let after = LabColor.rgbToLab(
                LabColor.applyVibranceSaturation(rgb, vibrance: 1.6, saturation: 1.0))
            let c0 = (before.y * before.y + before.z * before.z).squareRoot()
            let c1 = (after.y * after.y + after.z * after.z).squareRoot()
            return c0 > 0 ? c1 / c0 : 1.0
        }
        let mutedGain = chromaGain(SIMD3(0.45, 0.40, 0.38))  // near-neutral skin-ish tone
        let saturatedGain = chromaGain(SIMD3(0.8, 0.1, 0.1))  // strong red
        #expect(mutedGain > saturatedGain + 0.05, "muted \(mutedGain) vs saturated \(saturatedGain)")
        #expect(mutedGain > 1.1)
    }

    // ── Color mixer (chroma-gated R/Y/G/B bands) ──────────────────────────

    static func mixer(
        _ rgb: SIMD3<Double>, red: (Double, Double) = (0, 1), yellow: (Double, Double) = (0, 1),
        green: (Double, Double) = (0, 1), blue: (Double, Double) = (0, 1)
    ) -> SIMD3<Double> {
        LabColor.applyColorMixer(
            rgb,
            hues: SIMD4(red.0, yellow.0, green.0, blue.0),
            saturations: SIMD4(red.1, yellow.1, green.1, blue.1))
    }

    static func chromaHue(_ rgb: SIMD3<Double>) -> (chroma: Double, hueDeg: Double) {
        let lab = LabColor.rgbToLab(rgb)
        return ((lab.y * lab.y + lab.z * lab.z).squareRoot(), atan2(lab.z, lab.y) * 180 / .pi)
    }

    @Test func mixerDefaultsAreIdentity() {
        let px = SIMD3(0.62, 0.21, 0.17)
        #expect(Self.mixer(px) == px)
    }

    @Test func mixerLeavesNeutralsUntouched() {
        // The whole point: grays (and faint casts) never move, any strength.
        for gray in [0.02, 0.18, 0.5, 0.95] {
            let px = SIMD3(repeating: gray)
            let out = Self.mixer(px, red: (1, 0), yellow: (-1, 0), green: (1, 2), blue: (-1, 2))
            #expect(simd_length(out - px) < 1e-6)
        }
        // Near-neutral warm cast: chroma below the gate floor → protected.
        let cast = SIMD3(0.52, 0.50, 0.49)
        #expect(Self.chromaHue(cast).chroma < LabColor.bandChromaGateLow)
        let out = Self.mixer(cast, red: (1, 0))
        #expect(simd_length(out - cast) < 1e-6)
    }

    @Test func mixerShiftsSaturatedReds() {
        let red = SIMD3(0.55, 0.10, 0.08)
        let (chromaIn, hueIn) = Self.chromaHue(red)
        #expect(chromaIn > LabColor.bandChromaGateHigh)

        // + hue rotates toward orange (hue angle increases).
        let warmed = Self.chromaHue(Self.mixer(red, red: (1, 1)))
        #expect(warmed.hueDeg > hueIn + 3)

        // Saturation < 1 pulls chroma down without killing it.
        let tamed = Self.chromaHue(Self.mixer(red, red: (0, 0.5)))
        #expect(tamed.chroma < chromaIn * 0.85)
        #expect(tamed.chroma > chromaIn * 0.3)
    }

    @Test func mixerBandsTargetTheirOwnHues() {
        let red = SIMD3(0.55, 0.10, 0.08)
        let green = SIMD3(0.10, 0.55, 0.12)
        // Hue 264° in the Adobe-D65 Lab (in the blue band 235±65). The old
        // primary-ish (0.08, 0.12, 0.60) lands at 293° — the feather's edge —
        // consistent with the bands being tuned on real content, whose blues
        // (sky ~240-265° here) stay in-band across the space change.
        let blue = SIMD3(0.06, 0.22, 0.55)

        // The blue band ignores saturated red/green (far outside its window)…
        for px in [red, green] {
            #expect(simd_length(Self.mixer(px, blue: (1, 0.2)) - px) < 1e-6)
        }
        // …but moves saturated blue.
        let shifted = Self.chromaHue(Self.mixer(blue, blue: (0, 0.5)))
        #expect(shifted.chroma < Self.chromaHue(blue).chroma * 0.9)

        // The green band moves green but not red.
        #expect(simd_length(Self.mixer(red, green: (1, 0.2)) - red) < 1e-6)
        // Wrapped delta: a full + shift can carry the hue across ±180°.
        let greenShift = Self.chromaHue(Self.mixer(green, green: (1, 1)))
        let delta = (greenShift.hueDeg - Self.chromaHue(green).hueDeg + 540)
            .truncatingRemainder(dividingBy: 360) - 180
        #expect(delta > 3)
    }
}

/// Gamut-aware chroma boost (NegPy 1b900ab) — the properties the fixture
/// can't articulate: hue preservation on clipping pixels, in-gamut
/// byte-identity, and skin-band softening.
@Suite struct GamutAwareChromaTests {
    private func hue(_ rgb: SIMD3<Double>) -> Double {
        let lab = LabColor.rgbToLab(rgb)
        return atan2(lab.z, lab.y) * 180.0 / .pi
    }

    private func hueDelta(_ a: Double, _ b: Double) -> Double {
        var d = a - b
        d -= 360.0 * (d / 360.0).rounded()
        return abs(d)
    }

    /// A vivid pixel that clips under a flat 1.6× push: the old flat+clamp
    /// path shifts its hue; the gamut-aware path must hold it far tighter.
    @Test func huePreservedWherFlatScaleClipped() {
        let vivid = SIMD3(0.85, 0.1, 0.12)  // strong red, near the gamut wall
        let h0 = hue(vivid)

        // The retired behavior, reconstructed: flat scale then per-channel clamp.
        var lab = LabColor.rgbToLab(vivid)
        lab.y *= 1.6
        lab.z *= 1.6
        let flat = simd_clamp(LabColor.labToRgb(lab), SIMD3<Double>(), SIMD3(repeating: 1))
        let flatErr = hueDelta(hue(flat), h0)

        let aware = LabColor.applyVibranceSaturation(vivid, vibrance: 1.0, saturation: 1.6)
        let awareErr = hueDelta(hue(aware), h0)

        #expect(flatErr > 1.0, "premise: the flat path visibly shifts hue (\(flatErr)°)")
        #expect(awareErr < flatErr * 0.5, "gamut-aware must at least halve it (\(flatErr)° → \(awareErr)°)")
        #expect(awareErr < 2.0, "and hold it small in absolute terms (\(awareErr)°)")
    }

    /// Comfortably in-gamut pixels get the exact flat scale — ANY hue: the
    /// skin term was removed from the boost (bfcd90a); Skin Protection is a
    /// separate operator now.
    @Test func inGamutPixelsMatchFlatScale() {
        let muted = SIMD3(0.25, 0.52, 0.60)
        var lab = LabColor.rgbToLab(muted)
        lab.y *= 1.2
        lab.z *= 1.2
        let flat = simd_clamp(LabColor.labToRgb(lab), SIMD3<Double>(), SIMD3(repeating: 1))
        let aware = LabColor.applyVibranceSaturation(muted, vibrance: 1.0, saturation: 1.2)
        #expect(simd_length(aware - flat) < 1e-9)
    }

    /// Desaturation is untouched by the machinery.
    @Test func desaturationStaysFlat() {
        let vivid = SIMD3(0.85, 0.1, 0.12)
        var lab = LabColor.rgbToLab(vivid)
        lab.y *= 0.5
        lab.z *= 0.5
        let flat = simd_clamp(LabColor.labToRgb(lab), SIMD3<Double>(), SIMD3(repeating: 1))
        let aware = LabColor.applyVibranceSaturation(vivid, vibrance: 1.0, saturation: 0.5)
        #expect(simd_length(aware - flat) < 1e-9)
    }

}

/// Skin Protection (NegPy bfcd90a + fb94aed): the rein's contracts — mask
/// selectivity per upstream's measurements, reduce-only, hue/L* invariance.
@Suite struct SkinProtectionTests {
    /// Build a Lab triplet from (L, C, hue°) and get its weight.
    private func weight(L: Double, C: Double, hueDeg: Double) -> Double {
        let h = hueDeg * .pi / 180.0
        return LabColor.skinWeight(L: L, a: C * cos(h), b: C * sin(h))
    }

    @Test func maskMatchesTheMeasuredLocus() {
        // Dead-centre skin: L*65, C*27, hue 52° → near-full weight.
        #expect(weight(L: 65, C: 27, hueDeg: 52) > 0.9)
        // Pure saturated red: right hue-ish but C*≈104 → drops out entirely.
        #expect(weight(L: 55, C: 104, hueDeg: 40) == 0)
        // Sunset-chroma warm (C*57): mostly out via the chroma window.
        #expect(weight(L: 60, C: 57, hueDeg: 50) < 0.15)
        // Wrong hue at skin chroma (teal): out via the hue Gaussian.
        #expect(weight(L: 65, C: 27, hueDeg: -125) < 1e-6)
        // Too dark / too bright: the lightness rolloff is SMOOTH
        // (smoothstep 0…15 and 95…100), so assert attenuation, not zero.
        #expect(weight(L: 2, C: 27, hueDeg: 52) < 0.05)
        #expect(weight(L: 5, C: 27, hueDeg: 52) < 0.3)
        #expect(weight(L: 99, C: 27, hueDeg: 52) < 0.2)
        // Near-neutral: chroma gate.
        #expect(weight(L: 65, C: 1, hueDeg: 52) == 0)
    }

    @Test func reinIsReduceOnlyAndPreservesHueAndL() {
        for (L, C, hue) in [(65.0, 45.0, 52.0), (55.0, 38.0, 60.0), (70.0, 50.0, 45.0)] {
            let h = hue * .pi / 180.0
            let lab = SIMD3(L, C * cos(h), C * sin(h))
            let out = LabColor.skinChromaRein(lab, strength: 1.0)
            let cOut = (out.y * out.y + out.z * out.z).squareRoot()
            #expect(cOut <= C + 1e-9, "chroma may only fall (\(C) → \(cOut))")
            #expect(out.x == L, "L* untouched")
            let hueOut = atan2(out.z, out.y) * 180.0 / .pi
            #expect(abs(hueOut - hue) < 1e-9, "hue untouched")
        }
    }

    @Test func reinBitesSkinButNotWarmObjects() {
        // Skin arriving over-chromatic (C*45 @ 52°): pulled down hard.
        let skinC = 45.0
        let skinLab = SIMD3(65.0, skinC * cos(52.0 * .pi / 180), skinC * sin(52.0 * .pi / 180))
        let reined = LabColor.skinChromaRein(skinLab, strength: 1.0)
        let cReined = (reined.y * reined.y + reined.z * reined.z).squareRoot()
        #expect(cReined < skinC * 0.85, "skin \(skinC) → \(cReined)")

        // A sunset-chroma pixel (C*57, weight ~0.04): essentially untouched.
        let sunC = 57.0
        let sunLab = SIMD3(60.0, sunC * cos(50.0 * .pi / 180), sunC * sin(50.0 * .pi / 180))
        let sun = LabColor.skinChromaRein(sunLab, strength: 1.0)
        let cSun = (sun.y * sun.y + sun.z * sun.z).squareRoot()
        #expect(cSun > sunC * 0.93, "sunset must keep its colour (\(sunC) → \(cSun))")
    }

    @Test func identityBelowKneeAndAtZeroStrength() {
        // Typical skin (C*27) sits BELOW the knee at default 0.5
        // (ceiling 44, start 26.4)? C*27 > 26.4 — just over; use C*24.
        let lab = SIMD3(65.0, 24.0 * cos(52.0 * .pi / 180), 24.0 * sin(52.0 * .pi / 180))
        #expect(LabColor.skinChromaRein(lab, strength: 0.5) == lab)
        let hot = SIMD3(65.0, 45.0, 20.0)
        #expect(LabColor.skinChromaRein(hot, strength: 0) == hot)
    }

    @Test func sidecarDefaultAndRoundTrip() throws {
        var s = ExposureSettings()
        s.skinProtection = 0.8
        let back = try JSONDecoder().decode(ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back == s)
        // Keyless sidecars adopt the on-by-default (upstream's choice).
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.skinProtection == 0.5)
    }
}
