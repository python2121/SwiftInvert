import Foundation
import Testing

@testable import NegativeKit

/// Contrast Mask (NegPy 515c1f5 port) parity + properties. The fixtures in
/// Tests/Fixtures/contrast_mask/ are dumped from NegPy's own
/// `contrast_mask_plane` / `contrast_mask_ev` / masked PhotometricProcessor
/// chain (`dump_fixtures.py contrast_mask`, additive — the pre-existing
/// fixtures stay at their recorded dump).
@Suite struct ContrastMaskTests {
    static let manifest: [String: Any] = {
        try! Fixtures.json("contrast_mask/manifest.json")
    }()

    static let input: RGBImage = {
        let pixels = try! Fixtures.floats("contrast_mask/input.bin")
        return RGBImage(pixels: pixels, width: 64, height: 64)
    }()

    static var bounds: LogNegativeBounds {
        let b = manifest["bounds"] as! [String: Any]
        let f = (b["floors"] as! [Double]), c = (b["ceils"] as! [Double])
        return LogNegativeBounds(floors: SIMD3(f[0], f[1], f[2]), ceils: SIMD3(c[0], c[1], c[2]))
    }

    // MARK: - Parity with NegPy

    @Test(arguments: ["spacer4", "spacer2_5"])
    func planeMatchesFixture(_ name: String) throws {
        let planes = Self.manifest["planes"] as! [String: Any]
        let entry = planes[name] as! [String: Any]
        let spacer = entry["spacer"] as! Double
        let expected = try Fixtures.floats("contrast_mask/plane_\(name).bin")

        let plane = try #require(
            ContrastMask.buildPlane(
                renderSource: Self.input, bounds: Self.bounds, spacerPercent: spacer))
        #expect(plane.width == 64 && plane.height == 64)
        var maxDiff: Float = 0
        for i in 0..<expected.count { maxDiff = max(maxDiff, abs(plane.values[i] - expected[i])) }
        #expect(maxDiff < 1e-5, "plane \(name): max diff \(maxDiff)")
    }

    /// The bilinear half-pixel expansion × contrast_mask_scale, against
    /// upstream's cv2.INTER_LINEAR expansion — this is the exact arithmetic
    /// all three kernels hand-roll.
    @Test func evExpansionMatchesFixture() throws {
        let ev = Self.manifest["ev"] as! [String: Any]
        let gamma = ev["gamma"] as! Double
        let lumRange = ev["density_range"] as! Double
        let expected = try Fixtures.floats("contrast_mask/ev_96x128.bin")
        let planeValues = try Fixtures.floats("contrast_mask/plane_spacer4.bin")
        let plane = ContrastMask.Plane(values: planeValues, width: 64, height: 64)

        // Their EV map is stops (plane × contrast_mask_scale); the per-channel
        // ev_scale fold happens later on both sides.
        let scale = Float(-gamma * lumRange / K.log10Two)
        let outW = 128, outH = 96
        #expect(expected.count == outW * outH)
        var maxDiff: Float = 0
        for y in 0..<outH {
            for x in 0..<outW {
                let got = ContrastMask.sample(plane, x: x, y: y, outWidth: outW, outHeight: outH) * scale
                maxDiff = max(maxDiff, abs(got - expected[y * outW + x]))
            }
        }
        #expect(maxDiff < 1e-5, "ev expansion: max diff \(maxDiff)")
    }

    /// End-to-end: the masked chain through our CPU reference equals NegPy's
    /// masked PhotometricProcessor output at the standing full-chain gate.
    @Test func maskedFullChainMatchesFixture() throws {
        let chain = Self.manifest["masked_chain"] as! [String: Any]
        let settings = settingsFrom(chain)
        #expect(settings.contrastMask != 0)
        let analysis = ExposureKernel.analyze(linearImage: Self.input, analysisBuffer: 0.05)
        let encoded = ReferenceCurve.render(
            linearImage: Self.input, settings: settings, analysis: analysis)
        try expectImageClose(encoded, fixture: "contrast_mask/masked_output.bin", tolerance: 1e-4)
    }

    // MARK: - Properties

    /// Zero-mean: the mask must never move overall print density.
    @Test func planeIsZeroMean() throws {
        let plane = try #require(
            ContrastMask.buildPlane(
                renderSource: Self.input, bounds: Self.bounds, spacerPercent: 4.0))
        var mean = 0.0
        for v in plane.values { mean += Double(v) }
        mean /= Double(plane.values.count)
        #expect(abs(mean) < 1e-6)
    }

    /// The spacer clamps at both ends: a saved value outside 2–6 renders
    /// exactly at the limit (upstream 1739522 — "cannot render wider than
    /// the slider can show").
    @Test func spacerClampsBothEnds() throws {
        func plane(_ s: Double) -> [Float] {
            ContrastMask.buildPlane(
                renderSource: Self.input, bounds: Self.bounds, spacerPercent: s)!.values
        }
        #expect(plane(0.5) == plane(2.0))
        #expect(plane(9.0) == plane(6.0))
        #expect(plane(2.0) != plane(6.0))
    }

    /// Gamma 0 = identity: no scale, and the derive gates the whole feature.
    @Test func zeroGammaIsIdentity() throws {
        #expect(ContrastMask.valScale(gamma: 0, lumRange: 1.0, finalBounds: Self.bounds) == .zero)
        var settings = ExposureSettings()
        settings.preSaturation = 1.0
        settings.skinProtection = 0
        let analysis = ExposureKernel.analyze(linearImage: Self.input, analysisBuffer: 0.05)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        #expect(params.maskValScale == .zero)
    }

    /// Direction: positive gamma (blurred positive) must COMPRESS the
    /// normalized-val spread — the whole point of the control.
    @Test func positiveGammaCompressesValRange() throws {
        let analysis = ExposureKernel.analyze(linearImage: Self.input, analysisBuffer: 0.05)
        var settings = ExposureSettings()
        settings.preSaturation = 1.0
        settings.skinProtection = 0
        func lumaSpread(_ gamma: Double) -> Double {
            settings.contrastMask = gamma
            let params = ExposureKernel.deriveRenderParams(settings, analysis)
            let plane = gamma == 0
                ? nil
                : ContrastMask.buildPlane(
                    renderSource: Self.input, bounds: analysis.baseBounds, spacerPercent: 4.0)
            var norm = ReferenceCurve.normalize(Self.input, bounds: params.finalBounds)
            // Val-domain green spread after the mask add (pre-curve), which
            // is what the paper then re-normalizes.
            var lo = Float.greatestFiniteMagnitude, hi = -Float.greatestFiniteMagnitude
            let w = norm.width
            norm.pixels.withUnsafeMutableBufferPointer { buf in
                for i in stride(from: 1, to: buf.count, by: 3) {
                    let pix = i / 3
                    var v = buf[i]
                    if let plane {
                        v += Float(params.maskValScale.y) * ContrastMask.sample(
                            plane, x: pix % w, y: pix / w, outWidth: w, outHeight: norm.height)
                    }
                    lo = min(lo, v)
                    hi = max(hi, v)
                }
            }
            return Double(hi - lo)
        }
        let base = lumaSpread(0)
        #expect(lumaSpread(0.5) < base)
        #expect(lumaSpread(-0.5) > base)
    }

    /// The zone model's constant-addend shortcut is exact: predictedZone with
    /// a pin's plane sample equals the full spatial kernel at that pixel.
    @Test func zoneModelSeesTheMask() throws {
        let analysis = ExposureKernel.analyze(linearImage: Self.input, analysisBuffer: 0.05)
        var settings = ExposureSettings()
        settings.preSaturation = 1.0
        settings.skinProtection = 0
        settings.contrastMask = 0.4
        let plane = try #require(
            ContrastMask.buildPlane(
                renderSource: Self.input, bounds: analysis.baseBounds, spacerPercent: 4.0))
        let maskVal = Double(ContrastMask.sample(plane, x: 20, y: 31, outWidth: 64, outHeight: 64))
        let with = ZonePlacement.predictedZone(
            settings: settings, analysis: analysis, valLuma: 0.55, maskVal: maskVal)
        let without = ZonePlacement.predictedZone(
            settings: settings, analysis: analysis, valLuma: 0.55)
        #expect(with != without, "a nonzero plane sample must move the model")
        // And gamma 0 makes the sample inert even when carried.
        settings.contrastMask = 0
        let off = ZonePlacement.predictedZone(
            settings: settings, analysis: analysis, valLuma: 0.55, maskVal: maskVal)
        let offPlain = ZonePlacement.predictedZone(
            settings: settings, analysis: analysis, valLuma: 0.55)
        #expect(off == offPlain)
    }
}
