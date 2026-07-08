import Foundation
import Metal
import Testing

@testable import MetalRenderKit
@testable import NegativeKit

/// GPU parity against the NegPy fixtures and the Swift CPU reference.
/// Tolerances follow NegPy's own GPU/CPU gates (test_gpu_curve_parity.py):
/// mean abs diff < 0.01, max abs diff < 0.04.
enum GPU {
    static let pipeline: RenderPipeline? = try? RenderPipeline()
}

enum Fixtures2 {
    static let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Fixtures")

    static func json(_ path: String) throws -> [String: Any] {
        let data = try Data(contentsOf: root.appendingPathComponent(path))
        return try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    static func floats(_ path: String) throws -> [Float] {
        let data = try Data(contentsOf: root.appendingPathComponent(path))
        return data.withUnsafeBytes { Array($0.bindMemory(to: Float.self)) }
    }
}

@Suite struct LayoutTests {
    @Test func uniformStrides() {
        // Must match the MSL structs in NegPipeline.metal.
        #expect(MemoryLayout<NormUniforms>.stride == 48)
        #expect(MemoryLayout<CurveUniforms>.stride == 192)
        #expect(MemoryLayout<CurveUniforms>.offset(of: \.toe) == 112)
        #expect(MemoryLayout<CurveUniforms>.offset(of: \.gammaWidth) == 172)
        #expect(MemoryLayout<CurveUniforms>.offset(of: \.shadowsLift) == 176)
        #expect(MemoryLayout<CurveUniforms>.offset(of: \.highlightContrast) == 188)
    }
}

@Suite struct GPUParityTests {
    static func settings(_ config: [String: Any]) -> ExposureSettings {
        let e = config["exposure_config"] as! [String: Any]
        var s = ExposureSettings()
        s.density = e["density"] as! Double
        s.grade = e["grade"] as! Double
        s.wbCyan = e["wb_cyan"] as! Double
        s.wbMagenta = e["wb_magenta"] as! Double
        s.wbYellow = e["wb_yellow"] as! Double
        s.autoExposure = e["auto_exposure"] as! Bool
        s.autoNormalizeContrast = e["auto_normalize_contrast"] as! Bool
        s.castRemovalStrength = e["cast_removal_strength"] as! Double
        s.autoCastRemoval = e["auto_cast_removal"] as! Bool
        s.toe = e["toe"] as! Double
        s.toeWidth = e["toe_width"] as! Double
        s.shoulder = e["shoulder"] as! Double
        s.shoulderWidth = e["shoulder_width"] as! Double
        s.paperDmin = e["paper_dmin"] as! Bool
        return s
    }

    static func diffStats(_ got: [Float], _ expected: [Float]) -> (mean: Double, maxV: Double) {
        var maxDiff = 0.0, meanDiff = 0.0
        for i in 0..<expected.count {
            let d = Double(abs(got[i] - expected[i]))
            maxDiff = max(maxDiff, d)
            meanDiff += d
        }
        return (meanDiff / Double(expected.count), maxDiff)
    }

    @Test(arguments: ["default", "expo_dark", "expo_cmy"])
    func fixtureParity(config name: String) throws {
        let pipeline = try #require(GPU.pipeline, "Metal unavailable")
        let pixels = try Fixtures2.floats("synthetic64/input.bin")
        let input = RGBImage(pixels: pixels, width: 64, height: 64)
        let analysis = ExposureKernel.analyze(linearImage: input, analysisBuffer: 0.05)

        let j = try Fixtures2.json("synthetic64/\(name)/analysis.json")
        let params = ExposureKernel.deriveRenderParams(Self.settings(j), analysis)

        let source = try pipeline.upload(input)
        let result = try pipeline.render(source: source, params: params)

        let linear = pipeline.readback(result.linear)
        let encoded = pipeline.readback(result.encoded)

        let expLinear = try Fixtures2.floats("synthetic64/\(name)/curve_linear.bin")
        let expEncoded = try Fixtures2.floats("synthetic64/\(name)/output.bin")

        let (meanL, maxL) = Self.diffStats(linear.pixels, expLinear)
        #expect(meanL < 0.01 && maxL < 0.04, "\(name) linear: mean \(meanL), max \(maxL)")
        let (meanE, maxE) = Self.diffStats(encoded.pixels, expEncoded)
        #expect(meanE < 0.01 && maxE < 0.04, "\(name) encoded: mean \(meanE), max \(maxE)")
    }

    @Test func toneControlsParityWithCPU() throws {
        // The regional tone controls have no NegPy fixture; the GPU must match
        // the Swift CPU reference at the standard parity tolerances.
        let pipeline = try #require(GPU.pipeline, "Metal unavailable")
        let pixels = try Fixtures2.floats("synthetic64/input.bin")
        let input = RGBImage(pixels: pixels, width: 64, height: 64)
        let analysis = ExposureKernel.analyze(linearImage: input, analysisBuffer: 0.05)

        var settings = ExposureSettings()
        settings.exposureStops = 0.7
        settings.shadows = 0.8
        settings.shadowContrast = -0.5
        settings.highlights = -0.9
        settings.highlightContrast = 0.6
        let params = ExposureKernel.deriveRenderParams(settings, analysis)

        let cpu = ReferenceCurve.encodeOutput(
            ReferenceCurve.applyPrintCurve(
                ReferenceCurve.normalize(input, bounds: params.finalBounds), params: params))
        let source = try pipeline.upload(input)
        let gpu = pipeline.readback(try pipeline.render(source: source, params: params).encoded)

        let (mean, maxV) = Self.diffStats(gpu.pixels, cpu.pixels)
        #expect(mean < 0.01 && maxV < 0.04, "tone controls GPU/CPU: mean \(mean), max \(maxV)")
    }

    @Test func histogramMatchesCPU() throws {
        let pipeline = try #require(GPU.pipeline, "Metal unavailable")
        let pixels = try Fixtures2.floats("synthetic64/input.bin")
        let input = RGBImage(pixels: pixels, width: 64, height: 64)
        let analysis = ExposureKernel.analyze(linearImage: input, analysisBuffer: 0.05)
        let params = ExposureKernel.deriveRenderParams(ExposureSettings(), analysis)

        let source = try pipeline.upload(input)
        let result = try pipeline.render(source: source, params: params)

        // CPU histogram over the GPU's own linear output → binning must agree.
        let linear = pipeline.readback(result.linear)
        var cpuBins = [UInt32](repeating: 0, count: 1024)
        let n = linear.width * linear.height
        for i in 0..<n {
            let r = WorkingOETF.encode(linear.pixels[i * 3])
            let g = WorkingOETF.encode(linear.pixels[i * 3 + 1])
            let b = WorkingOETF.encode(linear.pixels[i * 3 + 2])
            let l = Float(K.lumaR) * r + Float(K.lumaG) * g + Float(K.lumaB) * b
            cpuBins[Int(min(max(r * 255, 0), 255))] += 1
            cpuBins[256 + Int(min(max(g * 255, 0), 255))] += 1
            cpuBins[512 + Int(min(max(b * 255, 0), 255))] += 1
            cpuBins[768 + Int(min(max(l * 255, 0), 255))] += 1
        }
        // Allow a small fraction of edge-of-bin disagreements from float rounding.
        var l1 = 0
        for i in 0..<1024 { l1 += abs(Int(result.histogram[i]) - Int(cpuBins[i])) }
        #expect(l1 <= n / 25, "histogram L1 diff \(l1) of \(n * 4) samples")
        #expect(result.histogram.reduce(0) { $0 + Int($1) } == n * 4, "total count")
    }
}
