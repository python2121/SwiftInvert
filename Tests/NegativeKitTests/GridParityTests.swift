import Foundation
import Testing

@testable import NegativeKit

/// synthetic_grid fixture: a 1600×1066 negative-like image regenerated here
/// bit-exactly from the integer-hash formula in scripts/dump_fixtures.py.
/// Exercises the b=2 block-median path and the full neutral-axis quadratic fit.
enum SyntheticGrid {
    static let width = 1600
    static let height = 1066

    /// splitmix32-style avalanche, mirroring hash32() in dump_fixtures.py.
    static func hash32(_ i: UInt32) -> UInt32 {
        var x = i &+ 0x9E37_79B9
        x ^= x >> 16
        x = x &* 0x21F0_AAAD
        x ^= x >> 15
        x = x &* 0x735A_2D97
        x ^= x >> 15
        return x
    }

    static let input: RGBImage = {
        let w = width, h = height
        var img = RGBImage(width: w, height: h)
        let scales: [Float] = [0.85, 0.45, 0.25]
        let invDim = Float(1.0) / Float(w + h - 2)
        img.pixels.withUnsafeMutableBufferPointer { buf in
            for y in 0..<h {
                for x in 0..<w {
                    let base: Float = 0.08 + 0.75 * (Float(x + y) * invDim)
                    let idx = UInt32(y * w + x) &* 3
                    for c in 0..<3 {
                        var n = Float(hash32(idx &+ UInt32(c))) / Float(4294967296.0)
                        n = (n - 0.5) * 0.06
                        let v = (base + n) * scales[c]
                        buf[(y * w + x) * 3 + c] = min(max(v, 1e-4), 1.0)
                    }
                }
            }
        }
        return img
    }()

    static let analysis = ExposureKernel.analyze(linearImage: input, analysisBuffer: 0.05)
}

@Suite struct GridParityTests {
    @Test func generatorMatchesProbes() throws {
        let manifest = try Fixtures.json("synthetic_grid/manifest.json")
        let probes = manifest["input_probe"] as! [String: [Double]]
        for (key, expected) in probes {
            let parts = key.split(separator: ",").map { Int($0)! }
            for c in 0..<3 {
                expectClose(
                    Double(SyntheticGrid.input[parts[0], parts[1], c]), expected[c], accuracy: 1e-7,
                    "probe \(key) ch\(c)")
            }
        }
    }

    @Test func prefilteredGrid() throws {
        let expected = try Fixtures.floats("synthetic_grid/prefiltered.bin")
        let got = Prefilter.prefilterLogGrid(SyntheticGrid.input, analysisBuffer: 0.05)
        #expect(got.pixels.count == expected.count, "grid element count")
        var maxDiff: Float = 0
        for i in 0..<expected.count { maxDiff = max(maxDiff, abs(got.pixels[i] - expected[i])) }
        #expect(maxDiff < 1e-5, "prefiltered grid max diff \(maxDiff)")
    }

    @Test func boundsMetersAndParams() throws {
        let j = try Fixtures.json("synthetic_grid/default/analysis.json")
        let bounds = j["bounds"] as! [String: Any]
        let meters = j["meters"] as! [String: Any]
        let cp = j["curve_params"] as! [String: Any]
        let analysis = SyntheticGrid.analysis

        let baseFloors = bounds["base_floors"] as! [Double]
        let baseCeils = bounds["base_ceils"] as! [Double]
        for ch in 0..<3 {
            expectClose(analysis.baseBounds.floors[ch], baseFloors[ch], accuracy: 1e-4, "floor \(ch)")
            expectClose(analysis.baseBounds.ceils[ch], baseCeils[ch], accuracy: 1e-4, "ceil \(ch)")
        }
        expectClose(analysis.anchor, meters["metered_anchor"] as! Double, accuracy: 1e-4, "anchor")
        expectClose(analysis.texturalRange, meters["textural_range"] as! Double, accuracy: 1e-4, "textural")

        // The full quadratic cast-removal path must engage (nonzero curvatures).
        let curvatures = cp["curvatures"] as! [Double]
        #expect(curvatures[0] != 0 || curvatures[2] != 0, "fixture should exercise the quadratic path")
        let params = ExposureKernel.deriveRenderParams(ExposureSettings(), analysis)
        let slopes = cp["slopes"] as! [Double]
        let pivots = cp["pivots"] as! [Double]
        for ch in 0..<3 {
            expectClose(params.slopes[ch], slopes[ch], accuracy: 1e-4, "slope \(ch)")
            expectClose(params.pivots[ch], pivots[ch], accuracy: 1e-4, "pivot \(ch)")
            expectClose(params.curvatures[ch], curvatures[ch], accuracy: 1e-4, "curv \(ch)")
        }
    }
}
