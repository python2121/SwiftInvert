import Foundation
import Testing

@testable import NegativeKit

/// Analysis edge paths: neutral-axis nil routes, margin-mode bounds, the
/// same-pixel colour-floor fallback, and the b ≥ 3 block-median prefilter.
@Suite struct AuditAnalysisEdgeTests {

    // MARK: - Meters.neutralAxis nil paths

    /// (a) A 6×6 grid holds 36 pixels total — no luma band can ever reach
    /// K.neutralAxisMinPixels (64), so the mid-band guard fails → nil.
    @Test func tinyGridYieldsNilNeutralAxis() {
        var grid = RGBImage(width: 6, height: 6)
        for i in 0..<36 {
            let t = Float(i) / 35.0
            grid.pixels[i * 3] = -2.0 + 1.5 * t
            grid.pixels[i * 3 + 1] = -2.1 + 1.5 * t
            grid.pixels[i * 3 + 2] = -1.9 + 1.5 * t
        }
        let bounds = LogNegativeBounds(
            floors: SIMD3(-2.1, -2.2, -2.0), ceils: SIMD3(-0.4, -0.5, -0.3))
        #expect(Meters.neutralAxis(grid: grid, bounds: bounds) == nil)
    }

    /// (b) Two-tone fully saturated 64×64 (half pure red 0.8/0.05/0.05, half
    /// pure blue) → nil.
    ///
    /// Pinned empirically: nil here comes from an EMPTY mid band, not the
    /// chroma caps. Under the recombined bounds the two tones normalize to
    /// lumas ≈ 0.32 (red half) and ≈ 0.11 (blue half) — the mid band
    /// (0.40–0.60) captures no pixels at all, so the min-pixel guard fires
    /// before any chroma cap is consulted. Either way the contract holds:
    /// a frame with no neutral content must not produce a neutral axis.
    @Test func twoToneSaturatedGridYieldsNilNeutralAxis() {
        var grid = RGBImage(width: 64, height: 64)
        let red = (Float(log10(0.8)), Float(log10(0.05)), Float(log10(0.05)))
        let blue = (Float(log10(0.05)), Float(log10(0.05)), Float(log10(0.8)))
        for i in 0..<(64 * 64) {
            let c = i < 64 * 32 ? red : blue
            grid.pixels[i * 3] = c.0
            grid.pixels[i * 3 + 1] = c.1
            grid.pixels[i * 3 + 2] = c.2
        }
        let bounds = BoundsAnalysis.analyze(grid: grid)
        #expect(Meters.neutralAxis(grid: grid, bounds: bounds) == nil)
    }

    // MARK: - sampleLogBounds margin mode

    /// A negative percentileClip switches to margin mode: percentiles are
    /// taken at the BASE clip and the (positive) margin is added outward.
    /// Expected values recomputed here from Stats percentiles directly.
    @Test func negativeClipIsBasePercentilesPlusMargin() {
        // Three distinct, known sorted channels (floors < ceils).
        let channels: [[Float]] = [
            (0...100).map { Float(-2.0) + Float($0) * 0.01 },  // −2.0 … −1.0
            (0...100).map { Float(-1.8) + Float($0) * 0.012 },  // −1.8 … −0.6
            (0...100).map { Float(-2.4) + Float($0) * 0.02 },  // −2.4 … −0.4
        ]
        let base = K.baseLumaClip  // 0.01
        let margin = 0.5
        let got = BoundsAnalysis.sampleLogBounds(
            channelsSorted: channels, percentileClip: -margin, base: base)
        for ch in 0..<3 {
            let floor = Stats.percentileOfSorted(channels[ch], base)
            let ceil = Stats.percentileOfSorted(channels[ch], 100.0 - base)
            // floors < ceils here, so margin pushes floors down, ceils up.
            #expect(got.floors[ch] == floor - margin, "ch \(ch)")
            #expect(got.ceils[ch] == ceil + margin, "ch \(ch)")
        }
    }

    // MARK: - analyze percentile fallback (samePixelColorFloorRefs nil)

    /// A two-patch saturated grid (90% one colour, 10% another) whose
    /// luma-extreme band holds no trustworthy neutrals: the same-pixel refs
    /// return nil (verified directly), and analyze must then equal the
    /// percentile-colour-floor recombination, recomputed here from
    /// sampleLogBounds + the recombination formula.
    @Test func analyzeFallsBackToPercentileColourFloors() {
        var grid = RGBImage(width: 64, height: 64)
        let a = (Float(log10(0.7)), Float(log10(0.2)), Float(log10(0.1)))
        let b = (Float(log10(0.05)), Float(log10(0.3)), Float(log10(0.6)))
        let n = 64 * 64
        for i in 0..<n {
            let c = i < n / 10 ? b : a
            grid.pixels[i * 3] = c.0
            grid.pixels[i * 3 + 1] = c.1
            grid.pixels[i * 3 + 2] = c.2
        }

        let channels = BoundsAnalysis.sortedChannels(grid: grid)
        let luma = BoundsAnalysis.sampleLogBounds(
            channelsSorted: channels, percentileClip: 0.0, base: K.baseLumaClip)
        let colour = BoundsAnalysis.sampleLogBounds(
            channelsSorted: channels, percentileClip: K.baseColorClip, base: 0.0)

        // The chroma-gated band refs reject this frame (saturated two-tone
        // content, no neutral subset) — the fallback precondition.
        let sp = BoundsAnalysis.samePixelColorFloorRefs(
            grid: grid, lumaFloors: luma.floors, lumaCeils: luma.ceils,
            baseRefs: colour.ceils, colorClip: K.baseColorClip)
        #expect(sp == nil)

        // Recombination (BoundsAnalysis.analyze): luma axis sets the mean
        // centre, colour axis the per-channel offset from the median channel.
        let meanLF = (luma.floors[0] + luma.floors[1] + luma.floors[2]) / 3.0
        let meanLC = (luma.ceils[0] + luma.ceils[1] + luma.ceils[2]) / 3.0
        let medCF = colour.floors.sorted()[1]
        let medCC = colour.ceils.sorted()[1]
        let expected = LogNegativeBounds(
            floors: SIMD3(
                meanLF + (colour.floors[0] - medCF),
                meanLF + (colour.floors[1] - medCF),
                meanLF + (colour.floors[2] - medCF)),
            ceils: SIMD3(
                meanLC + (colour.ceils[0] - medCC),
                meanLC + (colour.ceils[1] - medCC),
                meanLC + (colour.ceils[2] - medCC)))

        #expect(BoundsAnalysis.analyze(grid: grid) == expected)
    }

    // MARK: - Prefilter b ≥ 3 block-median path

    /// 2500×40 column gradient → b = ceil(2500/1024) = 3; grid is
    /// (2499/3)×(39/3) = 833×13. Each 3×3 block spans 3 columns × 3 equal
    /// rows, so its per-channel median is exactly the middle column's
    /// log10 value — recomputed here by sorting the block explicitly.
    @Test func blockMedianThreeWideBlocks() {
        let w = 2500, h = 40
        var img = RGBImage(width: w, height: h)
        func value(_ col: Int) -> Float { 0.1 + 0.8 * Float(col) / Float(w - 1) }
        let chScale: [Float] = [1.0, 0.9, 0.8]
        for y in 0..<h {
            for x in 0..<w {
                for c in 0..<3 { img[y, x, c] = value(x) * chScale[c] }
            }
        }
        let grid = Prefilter.prefilterLogGrid(img, analysisBuffer: 0)
        #expect(grid.width == 833)
        #expect(grid.height == 13)

        for (gy, gx) in [(0, 0), (0, 1), (12, 832), (6, 400), (3, 511)] {
            for c in 0..<3 {
                // Independent expectation: gather the 3×3 block, log10 with
                // the prefilter's clip, sort, take the middle of 9.
                var block: [Float] = []
                for by in 0..<3 {
                    for bx in 0..<3 {
                        let v = img[gy * 3 + by, gx * 3 + bx, c]
                        block.append(log10(min(max(v, 1e-6), 1.0)))
                    }
                }
                block.sort()
                let expected = block[4]
                #expect(abs(grid[gy, gx, c] - expected) < 1e-5, "cell (\(gy),\(gx)) ch \(c)")
                // And that middle element IS the block's centre column.
                let centre = log10(min(max(value(gx * 3 + 1) * chScale[c], 1e-6), 1.0))
                #expect(abs(expected - centre) < 1e-6)
            }
        }
    }

    /// The analysis buffer is clamped at 0.3 per side: a 0.45 request must
    /// produce byte-identical output to 0.3 (both through the public chain
    /// and the internal crop).
    @Test func analysisBufferClampsAtPointThree() {
        var img = RGBImage(width: 200, height: 120)
        for i in 0..<img.pixels.count {
            img.pixels[i] = 0.05 + 0.9 * Float((i * 2654435761) % 1000) / 999.0
        }
        let a = Prefilter.prefilterLogGrid(img, analysisBuffer: 0.45)
        let b = Prefilter.prefilterLogGrid(img, analysisBuffer: 0.3)
        #expect(a.width == b.width && a.height == b.height)
        #expect(a.pixels == b.pixels)

        let ca = Prefilter.analysisCrop(img, bufferRatio: 0.45)
        let cb = Prefilter.analysisCrop(img, bufferRatio: 0.3)
        #expect(ca.width == cb.width && ca.height == cb.height)
        #expect(ca.pixels == cb.pixels)
        // 0.3 of each side actually cut: 200 − 2·60 = 80, 120 − 2·36 = 48.
        #expect(cb.width == 80 && cb.height == 48)
    }
}
