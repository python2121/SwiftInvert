import Foundation
import Testing

@testable import NegativeKit

/// Pre-saturation (NLP-style pre-curve density-deviation gain).
@Suite struct PreSaturationTests {
    @Test func identityAtOne() {
        var s = ExposureSettings()
        s.preSaturation = 1.0
        let p = ExposureKernel.deriveRenderParams(s, Synthetic64.analysis)
        #expect(p.preSaturation == 1.0)
        // Fixture parity tests already pin the full default output.
    }

    @Test func neutralPixelsUnchanged() {
        // Equal channels have zero deviation — pre-saturation must not touch them.
        var gray = RGBImage(width: 4, height: 1)
        for i in 0..<4 { for c in 0..<3 { gray[0, i, c] = 0.2 + 0.2 * Float(i) } }
        var p = ToneControlsTests.params()
        let base = ReferenceCurve.applyPrintCurve(gray, params: p)
        p.preSaturation = 1.5
        let boosted = ReferenceCurve.applyPrintCurve(gray, params: p)
        for i in 0..<base.pixels.count {
            expectClose(Double(boosted.pixels[i]), Double(base.pixels[i]), accuracy: 1e-6, "gray \(i)")
        }
    }

    @Test func increasesOutputChroma() {
        // On the synthetic64 image (colored corners), pre-saturation > 1 must
        // raise the mean CIELAB chroma of the rendered positive.
        var base = ExposureSettings()
        var boosted = ExposureSettings()
        boosted.preSaturation = 1.3
        let outBase = ReferenceCurve.render(
            linearImage: Synthetic64.input, settings: base, analysis: Synthetic64.analysis)
        let outBoost = ReferenceCurve.render(
            linearImage: Synthetic64.input, settings: boosted, analysis: Synthetic64.analysis)

        func meanChroma(_ img: RGBImage) -> Double {
            var total = 0.0
            let n = img.width * img.height
            for i in 0..<n {
                // Decode the working TRC back to linear for the Lab measure.
                let rgb = SIMD3(
                    Double(WorkingOETF.decode(img.pixels[i * 3])),
                    Double(WorkingOETF.decode(img.pixels[i * 3 + 1])),
                    Double(WorkingOETF.decode(img.pixels[i * 3 + 2])))
                let lab = LabColor.rgbToLab(rgb)
                total += (lab.y * lab.y + lab.z * lab.z).squareRoot()
            }
            return total / Double(n)
        }
        let cBase = meanChroma(outBase)
        let cBoost = meanChroma(outBoost)
        #expect(cBoost > cBase * 1.05, "chroma \(cBase) → \(cBoost)")
        _ = base
    }

    @Test func sidecarRoundTrip() throws {
        var s = ExposureSettings()
        s.preSaturation = 1.35
        let back = try JSONDecoder().decode(ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back == s)
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.preSaturation == 1.0)
    }
}
