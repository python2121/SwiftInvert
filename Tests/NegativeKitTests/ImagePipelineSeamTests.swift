import Foundation
import Testing

@testable import NegativeKit

/// Seams the parity suites reach only transitively (audit 2026-07-15):
/// the prepare/finalize cache split and the area-average downsampler.
@Suite struct ImagePipelineSeamTests {

    // MARK: - prepare/finalize (ImageSession's two-tier cache contract)

    /// A cached `Prepared` reused later must match a fresh end-to-end
    /// analysis: prepare must be deterministic and finalize must not mutate
    /// its input — the contract behind ImageSession's analysis cache.
    @Test func reusedPreparedMatchesFreshAnalysis() {
        let image = Synthetic64.input
        let prepared = ExposureKernel.prepare(linearImage: image, analysisBuffer: 0.05)
        let cached = ExposureKernel.finalize(prepared)
        let fresh = ExposureKernel.analyze(linearImage: image, analysisBuffer: 0.05)
        #expect(cached == fresh)
        // And finalize is pure: a second call agrees with the first.
        #expect(ExposureKernel.finalize(prepared) == cached)
    }

    /// The 2125a34 semantics: the neutral axis is measured against the
    /// PRE-trim base bounds (the film's cast is a source property), so the
    /// whole analysis is offset-independent — while the white/black-point
    /// handles still reach the RENDER through derive-time finalBounds.
    @Test func neutralAxisIsPreTrimAndOffsetsStillReachTheRender() {
        let prepared = ExposureKernel.prepare(linearImage: Synthetic64.input, analysisBuffer: 0.05)
        let analysis = ExposureKernel.finalize(prepared)

        // The axis is exactly the base-bounds measurement.
        let direct = Meters.neutralAxis(grid: prepared.grid, bounds: prepared.baseBounds)
        #expect(analysis.neutralMid == direct?.mid)
        #expect(analysis.neutralShadow == direct?.shadow)
        #expect(analysis.neutralConfidence == direct?.confidence)

        // Offsets are a render-side fold: finalBounds moves, analysis doesn't.
        var settings = ExposureSettings()
        settings.whitePointOffset = 0.10
        settings.blackPointOffset = -0.08
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        #expect(
            params.finalBounds
                == analysis.baseBounds.applyingOffsets(whitePoint: 0.10, blackPoint: -0.08))
    }

    // MARK: - RGBImage.downsampled

    private func gradient(width: Int, height: Int) -> RGBImage {
        var img = RGBImage(width: width, height: height)
        for y in 0..<height {
            for x in 0..<width {
                let i = (y * width + x) * 3
                img.pixels[i] = Float(x) / Float(max(width - 1, 1))
                img.pixels[i + 1] = Float(y) / Float(max(height - 1, 1))
                img.pixels[i + 2] = 0.25
            }
        }
        return img
    }

    @Test func downsampleDimsPreserveAspect() {
        let out = gradient(width: 64, height: 48).downsampled(maxLongEdge: 32)
        #expect(out.width == 32 && out.height == 24)
        let odd = gradient(width: 100, height: 75).downsampled(maxLongEdge: 64)
        #expect(odd.width == 64 && odd.height == 48)
        let portrait = gradient(width: 48, height: 64).downsampled(maxLongEdge: 32)
        #expect(portrait.width == 24 && portrait.height == 32)
    }

    @Test func downsampleAtOrBelowCapIsIdentity() {
        let img = gradient(width: 64, height: 48)
        #expect(img.downsampled(maxLongEdge: 64).pixels == img.pixels)
        #expect(img.downsampled(maxLongEdge: 128).pixels == img.pixels)
    }

    /// Area averaging over an exact 2× factor: every output pixel is the mean
    /// of its 2×2 block, so the global channel means are preserved exactly
    /// (this is what makes analysis on the proxy match analysis on full res).
    @Test func exactFactorPreservesChannelMeans() {
        let img = gradient(width: 64, height: 48)
        let out = img.downsampled(maxLongEdge: 32)
        func means(_ i: RGBImage) -> SIMD3<Double> {
            var m = SIMD3<Double>()
            for p in stride(from: 0, to: i.pixels.count, by: 3) {
                m += SIMD3(Double(i.pixels[p]), Double(i.pixels[p + 1]), Double(i.pixels[p + 2]))
            }
            return m / Double(i.width * i.height)
        }
        let a = means(img), b = means(out)
        #expect(abs(a.x - b.x) < 1e-5 && abs(a.y - b.y) < 1e-5 && abs(a.z - b.z) < 1e-5)
    }

    @Test func constantImageStaysConstant() {
        var img = RGBImage(width: 50, height: 30)
        for i in 0..<img.pixels.count { img.pixels[i] = 0.42 }
        let out = img.downsampled(maxLongEdge: 16)
        #expect(out.pixels.allSatisfy { abs($0 - 0.42) < 1e-6 })
    }
}
