import Foundation
import Testing

@testable import NegativeKit
@testable import VulkanRenderKit

// The Vulkan chain is verified against the CPU ReferenceCurve chain — the
// same oracle the Mac suite uses for GPU-vs-CPU parity, at the same gates
// (mean < 0.01, max < 0.04; NegPy's own thresholds). ReferenceCurve itself
// is pinned to the NegPy fixtures by NegativeKitTests, so passing here
// chains the Vulkan renderer back to the same source of truth.

/// Deterministic synthetic scan: smooth ramps + integer-hash noise inside a
/// plausible negative range, channels offset like an orange-masked C-41 frame.
private func syntheticNegative(width: Int = 192, height: Int = 128) -> RGBImage {
    RGBImage(width: width, height: height) { buf in
        for y in 0..<height {
            for x in 0..<width {
                let i = (y * width + x) * 3
                let u = Double(x) / Double(width - 1)
                let v = Double(y) / Double(height - 1)
                var h = UInt32(truncatingIfNeeded: (x &* 374_761_393) &+ (y &* 668_265_263))
                h = (h ^ (h >> 13)) &* 1_274_126_177
                let noise = Double(h ^ (h >> 16)) / Double(UInt32.max)  // 0..1
                let ramp = 0.15 + 0.55 * u + 0.2 * v * (1 - u)
                let n = 0.12 * (noise - 0.5)
                buf[i] = Float(min(max(0.30 * (ramp + n) + 0.18, 0.001), 1.0))
                buf[i + 1] = Float(min(max(0.22 * (ramp + n) + 0.09, 0.001), 1.0))
                buf[i + 2] = Float(min(max(0.16 * (ramp + n) + 0.045, 0.001), 1.0))
            }
        }
    }
}

private struct Gate {
    let mean: Double
    let max: Double
}

private func compare(_ a: RGBImage, _ b: RGBImage, gate: Gate = Gate(mean: 0.01, max: 0.04)) -> (mean: Double, max: Double, pass: Bool) {
    precondition(a.pixels.count == b.pixels.count)
    var sum = 0.0
    var worst = 0.0
    for i in 0..<a.pixels.count {
        let d = abs(Double(a.pixels[i]) - Double(b.pixels[i]))
        sum += d
        worst = max(worst, d)
    }
    let mean = sum / Double(a.pixels.count)
    return (mean, worst, mean < gate.mean && worst < gate.max)
}

@Suite(.serialized)
struct VulkanParityTests {
    static let image = syntheticNegative()
    static let analysis = ExposureKernel.analyze(linearImage: image)
    static let pipeline = try? VulkanRenderPipeline()

    private func requirePipeline() throws -> VulkanRenderPipeline {
        try #require(Self.pipeline, "no Vulkan device — install mesa-vulkan-drivers (lavapipe suffices)")
    }

    private func assertParity(
        _ settings: ExposureSettings, _ comment: Comment, wantLinear: Bool = false
    ) throws {
        let pipeline = try requirePipeline()
        let params = ExposureKernel.deriveRenderParams(settings, Self.analysis)
        let cpu = ReferenceCurve.render(
            linearImage: Self.image, settings: settings, analysis: Self.analysis)
        let gpu = try pipeline.render(image: Self.image, params: params, computeHistogram: false)
        let d = compare(cpu, gpu.encoded)
        #expect(d.pass, "\(comment): mean \(d.mean) max \(d.max)")
    }

    @Test func defaultSettingsMatchCPU() throws {
        try assertParity(ExposureSettings(), "defaults (preSaturation 1.15, skinProtection 0.5)")
    }

    @Test func negPyNeutralMatchesCPU() throws {
        var s = ExposureSettings()
        s.preSaturation = 1.0
        s.skinProtection = 0
        try assertParity(s, "NegPy-neutral (colorPop pass fully off)")
    }

    @Test func exposureControlsMatchCPU() throws {
        var s = ExposureSettings()
        s.density = 1.4
        s.grade = 140
        s.exposureStops = 0.7
        s.wbCyan = 0.3
        s.wbMagenta = -0.2
        s.wbYellow = 0.5
        s.whitePointOffset = 0.05
        s.blackPointOffset = -0.04
        s.temp = 0.4
        s.tint = -0.3
        try assertParity(s, "density/grade/EV/filtration/temp-tint/wp-bp")
    }

    @Test func toneAndBandControlsMatchCPU() throws {
        var s = ExposureSettings()
        s.shadows = 0.8
        s.shadowContrast = -1.2
        s.highlights = 0.5
        s.highlightContrast = 1.5
        s.colorShadows = SIMD3(0.2, -0.1, 0.15)
        s.colorMids = SIMD3(-0.15, 0.1, 0.05)
        s.colorHighs = SIMD3(0.1, 0.05, -0.2)
        s.overallContrast = 0.6
        try assertParity(s, "tone controls + 3-band CMY + overall contrast")
    }

    @Test func colorPopControlsMatchCPU() throws {
        var s = ExposureSettings()
        s.hueTrim = 12
        s.vibrance = 1.5
        s.saturation = 1.6  // exercises the gamut-aware boost + bisection
        s.skinProtection = 0.8
        s.redHue = 0.4
        s.blueSaturation = 1.3
        s.greenHue = -0.5
        try assertParity(s, "hueTrim + mixer bands + vibrance + gamut-aware saturation")
    }

    @Test func printControlsMatchCPU() throws {
        var s = ExposureSettings()
        s.toe = 0.6
        s.shoulder = -0.4
        s.printSaturation = 1.3
        s.separationDamping = 0.6
        s.trueBlack = true
        s.paperDmin = true
        try assertParity(s, "toe/shoulder knees + printSaturation/damping + trueBlack + paperDmin")
    }

    @Test func levelsRemapMatchesCPU() throws {
        var s = ExposureSettings()
        s.levelsRed = [SIMD2(0.3, 0.22), SIMD2(0.7, 0.8)]
        s.levelsGreen = [SIMD2(0.5, 0.42)]
        s.levelsBlue = [SIMD2(0.2, 0.3), SIMD2(0.6, 0.55), SIMD2(0.85, 0.9)]
        try assertParity(s, "per-channel levels anchors")
    }

    @Test func displayPathMatchesFloatPath() throws {
        let pipeline = try requirePipeline()
        let params = ExposureKernel.deriveRenderParams(ExposureSettings(), Self.analysis)
        let source = try pipeline.upload(Self.image)
        let float = try pipeline.render(source: source, params: params, computeHistogram: false)
        let display = try pipeline.renderDisplay(source: source, params: params, computeHistogram: false)
        var worst = 0.0
        for i in 0..<(Self.image.width * Self.image.height) {
            for ch in 0..<3 {
                let f = Double(float.encoded.pixels[i * 3 + ch]) * 255
                let d = Double(display.rgba[i * 4 + ch])
                worst = max(worst, abs(f - d))
            }
            #expect(display.rgba[i * 4 + 3] == 255)
        }
        #expect(worst <= 1.5, "display path deviates \(worst)/255 from float path")
    }

    @Test func srgbDisplayFlagMatchesCPUTransform() throws {
        // The canvas transform: Adobe TRC decode → gamut matrix → clamp →
        // sRGB OETF, computed here scalar-CPU from the float path's output.
        let pipeline = try requirePipeline()
        let params = ExposureKernel.deriveRenderParams(ExposureSettings(), Self.analysis)
        let source = try pipeline.upload(Self.image)
        let float = try pipeline.render(source: source, params: params, computeHistogram: false)
        let display = try pipeline.renderDisplay(
            source: source, params: params, computeHistogram: false, srgbDisplay: true)
        let adobeToXYZ = (
            SIMD3(0.5767309, 0.1855540, 0.1881852),
            SIMD3(0.2973769, 0.6273491, 0.0752741),
            SIMD3(0.0270343, 0.0706872, 0.9911085))
        let xyzToSRGB = (
            SIMD3(3.2404542, -1.5371385, -0.4985314),
            SIMD3(-0.9692660, 1.8760108, 0.0415560),
            SIMD3(0.0556434, -0.2040259, 1.0572252))
        func srgbEncode(_ v: Double) -> Double {
            v <= 0.0031308 ? 12.92 * v : 1.055 * pow(v, 1.0 / 2.4) - 0.055
        }
        var worst = 0.0
        for i in 0..<(Self.image.width * Self.image.height) {
            let e = SIMD3(
                Double(float.encoded.pixels[i * 3]), Double(float.encoded.pixels[i * 3 + 1]),
                Double(float.encoded.pixels[i * 3 + 2]))
            let lin = SIMD3(
                pow(max(e.x, 0), 2.19921875), pow(max(e.y, 0), 2.19921875),
                pow(max(e.z, 0), 2.19921875))
            let xyz = SIMD3(
                simd_dot(adobeToXYZ.0, lin), simd_dot(adobeToXYZ.1, lin),
                simd_dot(adobeToXYZ.2, lin))
            let srgbLin = simd_clamp(
                SIMD3(
                    simd_dot(xyzToSRGB.0, xyz), simd_dot(xyzToSRGB.1, xyz),
                    simd_dot(xyzToSRGB.2, xyz)),
                SIMD3<Double>(), SIMD3(repeating: 1))
            let expected = SIMD3(srgbEncode(srgbLin.x), srgbEncode(srgbLin.y), srgbEncode(srgbLin.z))
            for ch in 0..<3 {
                worst = max(worst, abs(expected[ch] * 255 - Double(display.rgba[i * 4 + ch])))
            }
        }
        #expect(worst <= 1.5, "sRGB display path deviates \(worst)/255 from CPU transform")
    }

    @Test func histogramBinsSumToPixelCount() throws {
        let pipeline = try requirePipeline()
        let params = ExposureKernel.deriveRenderParams(ExposureSettings(), Self.analysis)
        let (_, hist) = try pipeline.render(image: Self.image, params: params)
        let n = UInt32(Self.image.width * Self.image.height)
        for ch in 0..<4 {
            let sum = hist[(ch * 256)..<((ch + 1) * 256)].reduce(0, +)
            #expect(sum == n, "channel \(ch) bins sum \(sum) != \(n)")
        }
    }

    @Test func oddPixelCountDispatchesCleanly() throws {
        // 61×47 is not a multiple of the 256-wide workgroup — pins the
        // bounds check in every kernel.
        let pipeline = try requirePipeline()
        let img = syntheticNegative(width: 61, height: 47)
        let analysis = ExposureKernel.analyze(linearImage: img)
        let params = ExposureKernel.deriveRenderParams(ExposureSettings(), analysis)
        let cpu = ReferenceCurve.render(linearImage: img, settings: ExposureSettings(), analysis: analysis)
        let gpu = try pipeline.render(image: img, params: params, computeHistogram: false)
        let d = compare(cpu, gpu.encoded)
        #expect(d.pass, "odd-size: mean \(d.mean) max \(d.max)")
    }
}

/// The std140 uniform blocks in NegPipeline.comp assume the C layout of the
/// shared ShaderTypes.swift structs — these pins mirror the Mac LayoutTests.
struct VulkanLayoutTests {
    @Test func uniformStridesMatchStd140() {
        #expect(MemoryLayout<NormUniforms>.stride == 48)
        #expect(MemoryLayout<CurveUniforms>.stride == 272)
        #expect(UniformsBuilder.levelsBufferFloats == 51)
        #expect(MemoryLayout<CurveUniforms>.offset(of: \.bandHues) == 224)
        #expect(MemoryLayout<CurveUniforms>.offset(of: \.separationDamping) == 256)
        #expect(MemoryLayout<CurveUniforms>.offset(of: \.hueTrim) == 264)
        #expect(MemoryLayout<CurveUniforms>.offset(of: \.toe) == 128)
    }
}
