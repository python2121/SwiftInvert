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

    /// `downsampled` is a separable area filter (cv2 INTER_AREA's kernel),
    /// parallelized over rows in both passes. Each output sample sums its taps
    /// in a fixed order, so the parallel split must be BYTE-identical to a
    /// serial run — and must survive output heights that aren't a multiple of
    /// the slice count (an unguarded `slice * per` overshoots and traps).
    private func serialSeparable(_ img: RGBImage, maxLongEdge: Int) -> [Float] {
        let long = max(img.width, img.height)
        guard long > maxLongEdge else { return img.pixels }
        let scale = Double(maxLongEdge) / Double(long)
        let ow = max(1, Int((Double(img.width) * scale).rounded()))
        let oh = max(1, Int((Double(img.height) * scale).rounded()))
        let w = img.width, h = img.height
        let hx = RGBImage.areaTaps(src: w, dst: ow)
        let vy = RGBImage.areaTaps(src: h, dst: oh)
        var mid = [Float](repeating: 0, count: ow * h * 3)
        for y in 0..<h {
            for ox in 0..<ow {
                let s0 = hx.start[ox]
                var r: Float = 0, g: Float = 0, b: Float = 0
                for k in 0..<hx.stride {
                    let wt = hx.weight[ox * hx.stride + k]
                    if wt == 0 { continue }
                    let i = (y * w + min(s0 + k, w - 1)) * 3
                    r += img.pixels[i] * wt
                    g += img.pixels[i + 1] * wt
                    b += img.pixels[i + 2] * wt
                }
                let o = (y * ow + ox) * 3
                mid[o] = r
                mid[o + 1] = g
                mid[o + 2] = b
            }
        }
        var out = [Float](repeating: 0, count: ow * oh * 3)
        for oy in 0..<oh {
            let s0 = vy.start[oy]
            for ox in 0..<ow {
                var r: Float = 0, g: Float = 0, b: Float = 0
                for k in 0..<vy.stride {
                    let wt = vy.weight[oy * vy.stride + k]
                    if wt == 0 { continue }
                    let i = (min(s0 + k, h - 1) * ow + ox) * 3
                    r += mid[i] * wt
                    g += mid[i + 1] * wt
                    b += mid[i + 2] * wt
                }
                let o = (oy * ow + ox) * 3
                out[o] = r
                out[o + 1] = g
                out[o + 2] = b
            }
        }
        return out
    }

    @Test func parallelDownsampleMatchesSerialAtEveryRaggedHeight() {
        for (w, h, cap) in [
            (100, 75, 64), (64, 48, 32), (37, 29, 13), (101, 53, 17),
            (200, 13, 10), (19, 200, 10), (64, 48, 1), (9, 9, 2), (1536, 1025, 1536),
            // The preview's real 1.96 ratio, at a size the serial reference can
            // chew through in a debug build.
            (301, 201, 154),
        ] {
            let img = gradient(width: w, height: h)
            #expect(
                img.downsampled(maxLongEdge: cap).pixels == serialSeparable(img, maxLongEdge: cap),
                "\(w)x\(h) -> cap \(cap)")
        }
    }

    /// THE property the area filter exists for, and the one the old
    /// integer-boundary box got wrong: a UNIFORM sample phase.
    ///
    /// Downsampling a linear ramp must give evenly spaced output values —
    /// every sample sits at the centre of its span. Integer box boundaries make
    /// boxes 1 or 2 source pixels wide at the preview's 1.96 ratio, so their
    /// centres drift and the steps come out uneven. That unevenness is a
    /// low-frequency geometric warp of the proxy, which showed up as local
    /// misalignment against the full-resolution tier.
    @Test func areaFilterSamplesOnAUniformGrid() {
        let w = 3012, h = 4, cap = 1536
        var img = RGBImage(width: w, height: h)
        for y in 0..<h {
            for x in 0..<w {
                for c in 0..<3 { img[y, x, c] = Float(x) / Float(w - 1) }
            }
        }
        let out = img.downsampled(maxLongEdge: cap)

        func stepSpread(_ row: [Float]) -> Double {
            let diffs = (0..<(row.count - 1)).map { Double(row[$0 + 1] - row[$0]) }
            let mean = diffs.reduce(0, +) / Double(diffs.count)
            // Expressed in SOURCE pixels, which is the geometric error.
            return diffs.map { abs($0 - mean) }.max()! * Double(w - 1)
        }

        let ours = (0..<out.width).map { out[0, $0, 0] }
        #expect(stepSpread(ours) < 0.05, "sample phase drifted \(stepSpread(ours)) source px")

        // The OLD integer-boundary box, for the same ramp — so this test can't
        // quietly become vacuous if the filter is ever swapped back.
        let sx = Double(w) / Double(out.width)
        var old = [Float]()
        for ox in 0..<out.width {
            let x0 = Int(Double(ox) * sx)
            let x1 = min(w, max(x0 + 1, Int(Double(ox + 1) * sx)))
            var acc: Float = 0
            for x in x0..<x1 { acc += img[0, x, 0] }
            old.append(acc / Float(x1 - x0))
        }
        // ~0.46 source px on this ramp: a box widening from 1 to 2 cells moves
        // its centre by half a cell, and consecutive steps disagree by that.
        #expect(stepSpread(old) > 0.3, "the old box should drift; got \(stepSpread(old))")
        #expect(
            stepSpread(old) > stepSpread(ours) * 5,
            "the fix must be an order of magnitude better, not marginally")
    }

    /// Separability check: the two 1-D passes must equal the naive 2-D area
    /// average (float ordering aside).
    @Test func separablePassesEqualTheNaiveTwoDimensionalAverage() {
        let img = gradient(width: 211, height: 143)
        let cap = 97
        let out = img.downsampled(maxLongEdge: cap)
        let ow = out.width, oh = out.height
        let sx = Double(img.width) / Double(ow), sy = Double(img.height) / Double(oh)
        var worst = 0.0
        for oy in 0..<oh {
            let fy0 = Double(oy) * sy, fy1 = Double(oy + 1) * sy
            for ox in 0..<ow {
                let fx0 = Double(ox) * sx, fx1 = Double(ox + 1) * sx
                var acc = 0.0, wsum = 0.0
                for y in Int(fy0)..<min(img.height, Int(fy1.rounded(.up))) {
                    let wy = min(Double(y + 1), fy1) - max(Double(y), fy0)
                    if wy <= 0 { continue }
                    for x in Int(fx0)..<min(img.width, Int(fx1.rounded(.up))) {
                        let wx = min(Double(x + 1), fx1) - max(Double(x), fx0)
                        if wx <= 0 { continue }
                        acc += wy * wx * Double(img[y, x, 0])
                        wsum += wy * wx
                    }
                }
                worst = max(worst, abs(acc / wsum - Double(out[oy, ox, 0])))
            }
        }
        #expect(worst < 1e-5, "separable vs naive 2-D differed by \(worst)")
    }

    @Test func areaTapWeightsSumToOneAndCoverTheSpan() {
        for (src, dst) in [(3012, 1536), (2010, 1025), (100, 7), (17, 16), (9, 2)] {
            let taps = RGBImage.areaTaps(src: src, dst: dst)
            let s = Double(src) / Double(dst)
            for j in 0..<dst {
                let sum = (0..<taps.stride).reduce(Float(0)) { $0 + taps.weight[j * taps.stride + $1] }
                #expect(abs(sum - 1) < 1e-5, "taps for \(src)->\(dst) row \(j) sum to \(sum)")
                #expect(taps.start[j] >= 0 && taps.start[j] < src)
                // The first tap must be the cell containing the span's start.
                #expect(taps.start[j] == Int((Double(j) * s).rounded(.down)))
            }
        }
    }
}
