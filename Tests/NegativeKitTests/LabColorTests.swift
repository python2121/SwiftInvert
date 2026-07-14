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

    // ── Reds band (chroma-gated color mixer) ──────────────────────────────

    @Test func redBandDefaultsAreIdentity() {
        let px = SIMD3(0.62, 0.21, 0.17)
        #expect(LabColor.applyRedBand(px, hue: 0, saturation: 1.0) == px)
    }

    @Test func redBandLeavesNeutralsUntouched() {
        // The whole point: grays (and faint casts) never move, any strength.
        for gray in [0.02, 0.18, 0.5, 0.95] {
            let px = SIMD3(repeating: gray)
            let out = LabColor.applyRedBand(px, hue: 1.0, saturation: 0.0)
            #expect(simd_length(out - px) < 1e-6)
        }
        // Near-neutral warm cast: chroma below the gate floor → protected.
        let cast = SIMD3(0.52, 0.50, 0.49)
        let lab = LabColor.rgbToLab(cast)
        #expect((lab.y * lab.y + lab.z * lab.z).squareRoot() < LabColor.redChromaGateLow)
        let out = LabColor.applyRedBand(cast, hue: 1.0, saturation: 0.0)
        #expect(simd_length(out - cast) < 1e-6)
    }

    @Test func redBandShiftsSaturatedReds() {
        let red = SIMD3(0.55, 0.10, 0.08)
        let labIn = LabColor.rgbToLab(red)
        let hueIn = atan2(labIn.z, labIn.y)
        let chromaIn = (labIn.y * labIn.y + labIn.z * labIn.z).squareRoot()
        #expect(chromaIn > LabColor.redChromaGateHigh)

        // + hue rotates toward orange (hue angle increases).
        let warmed = LabColor.rgbToLab(LabColor.applyRedBand(red, hue: 1.0, saturation: 1.0))
        #expect(atan2(warmed.z, warmed.y) > hueIn + 0.05)

        // Saturation < 1 pulls chroma down without killing it.
        let tamed = LabColor.rgbToLab(LabColor.applyRedBand(red, hue: 0, saturation: 0.5))
        let chromaTamed = (tamed.y * tamed.y + tamed.z * tamed.z).squareRoot()
        #expect(chromaTamed < chromaIn * 0.85)
        #expect(chromaTamed > chromaIn * 0.3)
    }

    @Test func redBandIgnoresOtherHues() {
        // Saturated blue and green sit far outside the hue window.
        for px in [SIMD3(0.08, 0.12, 0.60), SIMD3(0.10, 0.55, 0.12)] {
            let out = LabColor.applyRedBand(px, hue: 1.0, saturation: 0.2)
            #expect(simd_length(out - px) < 1e-6)
        }
    }
}
