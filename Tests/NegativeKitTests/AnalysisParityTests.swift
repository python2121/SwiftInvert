import Foundation
import Testing

@testable import NegativeKit

/// Stage-boundary parity against NegPy on the synthetic64 fixture:
/// prefilter → bounds → meters → curve params → normalized → curve → encoded.
enum Synthetic64 {
    static let input: RGBImage = {
        let pixels = try! Fixtures.floats("synthetic64/input.bin")
        return RGBImage(pixels: pixels, width: 64, height: 64)
    }()
    static let analysis = ExposureKernel.analyze(linearImage: input, analysisBuffer: 0.05)
}

func settingsFrom(_ config: [String: Any]) -> ExposureSettings {
    let e = config["exposure_config"] as! [String: Any]
    var s = ExposureSettings()
    // NegPy fixtures predate pre-saturation (a SwiftInvert-only default of 1.15);
    // parity requires the neutral value.
    s.preSaturation = 1.0
    s.redHue = 0  // Color Mixer red default is +0.5 (SwiftInvert-only); pin off
    s.trueBlack = false  // NegPy default; fixtures dumped without BPC
    s.density = e["density"] as! Double
    s.grade = e["grade"] as! Double
    s.wbCyan = e["wb_cyan"] as! Double
    s.wbMagenta = e["wb_magenta"] as! Double
    s.wbYellow = e["wb_yellow"] as! Double
    s.autoExposure = e["auto_exposure"] as! Bool
    s.autoNormalizeContrast = e["auto_normalize_contrast"] as! Bool
    s.castRemovalStrength = e["cast_removal_strength"] as! Double
    s.toe = e["toe"] as! Double
    s.toeWidth = e["toe_width"] as! Double
    s.shoulder = e["shoulder"] as! Double
    s.shoulderWidth = e["shoulder_width"] as! Double
    s.paperDmin = e["paper_dmin"] as! Bool
    return s
}

func expectImageClose(
    _ got: RGBImage, fixture path: String, tolerance: Float,
    sourceLocation: SourceLocation = #_sourceLocation
) throws {
    let expected = try Fixtures.floats(path)
    #expect(got.pixels.count == expected.count, "\(path) size", sourceLocation: sourceLocation)
    var maxDiff: Float = 0
    var meanDiff: Double = 0
    for i in 0..<expected.count {
        let d = abs(got.pixels[i] - expected[i])
        maxDiff = max(maxDiff, d)
        meanDiff += Double(d)
    }
    meanDiff /= Double(expected.count)
    #expect(maxDiff < tolerance, "\(path): max diff \(maxDiff) (mean \(meanDiff))", sourceLocation: sourceLocation)
}

@Suite struct AnalysisParityTests {
    @Test func prefilteredGrid() throws {
        let expected = try Fixtures.floats("synthetic64/prefiltered.bin")
        let got = Prefilter.prefilterLogGrid(Synthetic64.input, analysisBuffer: 0.05)
        #expect(got.pixels.count == expected.count, "grid size (58x58x3)")
        var maxDiff: Float = 0
        for i in 0..<expected.count { maxDiff = max(maxDiff, abs(got.pixels[i] - expected[i])) }
        #expect(maxDiff < 1e-5, "prefiltered grid max diff \(maxDiff)")
    }

    @Test func boundsAndMeters() throws {
        let j = try Fixtures.json("synthetic64/default/analysis.json")
        let bounds = j["bounds"] as! [String: Any]
        let meters = j["meters"] as! [String: Any]
        let analysis = Synthetic64.analysis
        let baseFloors = bounds["base_floors"] as! [Double]
        let baseCeils = bounds["base_ceils"] as! [Double]
        for ch in 0..<3 {
            expectClose(analysis.baseBounds.floors[ch], baseFloors[ch], accuracy: 1e-4, "floor \(ch)")
            expectClose(analysis.baseBounds.ceils[ch], baseCeils[ch], accuracy: 1e-4, "ceil \(ch)")
        }
        expectClose(analysis.anchor, meters["metered_anchor"] as! Double, accuracy: 1e-4, "anchor")
        expectClose(analysis.texturalRange, meters["textural_range"] as! Double, accuracy: 1e-4, "textural")
        let shadowRefs = meters["shadow_log_refs"] as! [Double]
        for ch in 0..<3 {
            expectClose(analysis.shadowRefs[ch], shadowRefs[ch], accuracy: 1e-4, "shadow ref \(ch)")
        }
        if let na = meters["neutral_axis_refs"] as? [Any], !(na[0] is NSNull) {
            let mid = na[0] as! [Double]
            let shadow = na[1] as! [Double]
            let confidence = na[3] as! Double
            let gotMid = try #require(analysis.neutralMid)
            let gotShadow = try #require(analysis.neutralShadow)
            for ch in 0..<3 {
                expectClose(gotMid[ch], mid[ch], accuracy: 1e-4, "neutral mid \(ch)")
                expectClose(gotShadow[ch], shadow[ch], accuracy: 1e-4, "neutral shadow \(ch)")
            }
            if let hl = na[2] as? [Double] {
                let gotHl = try #require(analysis.neutralHighlight)
                for ch in 0..<3 { expectClose(gotHl[ch], hl[ch], accuracy: 1e-4, "neutral highlight \(ch)") }
            } else {
                #expect(analysis.neutralHighlight == nil)
            }
            expectClose(try #require(analysis.neutralConfidence), confidence, accuracy: 1e-4, "confidence")
        } else {
            #expect(analysis.neutralMid == nil)
        }
    }

    @Test(arguments: ["default", "expo_dark", "expo_cmy"])
    func curveParams(config name: String) throws {
        let j = try Fixtures.json("synthetic64/\(name)/analysis.json")
        let cp = j["curve_params"] as! [String: Any]
        let params = ExposureKernel.deriveRenderParams(settingsFrom(j), Synthetic64.analysis)
        let slopes = cp["slopes"] as! [Double]
        let pivots = cp["pivots"] as! [Double]
        let curvatures = cp["curvatures"] as! [Double]
        let cmyOffsets = cp["cmy_offsets"] as! [Double]
        for ch in 0..<3 {
            expectClose(params.slopes[ch], slopes[ch], accuracy: 1e-4, "\(name) slope \(ch)")
            expectClose(params.pivots[ch], pivots[ch], accuracy: 1e-4, "\(name) pivot \(ch)")
            expectClose(params.curvatures[ch], curvatures[ch], accuracy: 1e-4, "\(name) curv \(ch)")
            expectClose(params.cmyOffsets[ch], cmyOffsets[ch], accuracy: 1e-4, "\(name) cmy \(ch)")
        }
        expectClose(params.toeEff, cp["toe_eff"] as! Double, accuracy: 1e-4, "\(name) toe_eff")
        expectClose(params.shoulderEff, cp["shoulder_eff"] as! Double, accuracy: 1e-4, "\(name) shoulder_eff")
        expectClose(params.vStar, cp["v_star"] as! Double, accuracy: 1e-6, "\(name) v_star")
    }

    @Test(arguments: ["default", "expo_dark", "expo_cmy"])
    func fullChain(config name: String) throws {
        let j = try Fixtures.json("synthetic64/\(name)/analysis.json")
        let params = ExposureKernel.deriveRenderParams(settingsFrom(j), Synthetic64.analysis)
        let normalized = ReferenceCurve.normalize(Synthetic64.input, bounds: params.finalBounds)
        try expectImageClose(normalized, fixture: "synthetic64/\(name)/normalized.bin", tolerance: 1e-4)
        let positive = ReferenceCurve.applyPrintCurve(normalized, params: params)
        try expectImageClose(positive, fixture: "synthetic64/\(name)/curve_linear.bin", tolerance: 1e-4)
        let encoded = ReferenceCurve.encodeOutput(positive)
        try expectImageClose(encoded, fixture: "synthetic64/\(name)/output.bin", tolerance: 1e-4)
    }
}
