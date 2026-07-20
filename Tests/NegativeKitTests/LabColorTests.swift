import Foundation
import Testing
import simd

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
