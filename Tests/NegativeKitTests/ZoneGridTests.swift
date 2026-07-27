import Foundation
import Testing

@testable import NegativeKit

/// ZoneGrid (the zone-overlay math, NegPy ae56f8b): grid shape, the shared
/// ruler, region merging, and the concave-region label anchor.
@Suite struct ZoneGridTests {
    /// rgba8 buffer helper; bytesPerRow can exceed width*4 (CG rows pad).
    private func buffer(
        width: Int, height: Int, pad: Int = 0, value: (Int, Int) -> UInt8
    ) -> ([UInt8], Int) {
        let bpr = width * 4 + pad
        var bytes = [UInt8](repeating: 0, count: bpr * height)
        for y in 0..<height {
            for x in 0..<width {
                let v = value(x, y)
                let i = y * bpr + x * 4
                bytes[i] = v
                bytes[i + 1] = v
                bytes[i + 2] = v
                bytes[i + 3] = 255
            }
        }
        return (bytes, bpr)
    }

    private func grid(_ bytes: [UInt8], width: Int, height: Int, bpr: Int) -> ZoneGrid.Grid? {
        bytes.withUnsafeBufferPointer {
            ZoneGrid.compute(rgba8: $0.baseAddress!, width: width, height: height, bytesPerRow: bpr)
        }
    }

    /// Every label must sit on a cell of its own zone (the anchor-snapping
    /// contract) — checked for every grid the suite builds.
    private func expectLabelsOwned(_ g: ZoneGrid.Grid) {
        for label in g.labels {
            #expect(g.zone(col: label.col, row: label.row) == label.zone)
        }
    }

    @Test func uniformMidGrayIsOneZoneVRegion() {
        let mid = UInt8((Densitometry.midGrayEncoded * 255).rounded())
        let (bytes, bpr) = buffer(width: 240, height: 160) { _, _ in mid }
        let g = try! #require(grid(bytes, width: 240, height: 160, bpr: bpr))
        #expect(g.cols == 24)
        #expect(g.rows == 16)
        #expect(g.zones.allSatisfy { $0 == 5 })
        #expect(g.labels.count == 1)
        #expect(g.labels[0].zone == 5)
        expectLabelsOwned(g)
    }

    @Test func tallFrameSwapsGridAxes() {
        let (bytes, bpr) = buffer(width: 160, height: 240) { _, _ in 128 }
        let g = try! #require(grid(bytes, width: 160, height: 240, bpr: bpr))
        #expect(g.cols == 16)
        #expect(g.rows == 24)
    }

    @Test func blackWhiteSplitYieldsPaperEndsAndTwoRegions() {
        let (bytes, bpr) = buffer(width: 240, height: 160, pad: 12) { x, _ in x < 120 ? 0 : 255 }
        let g = try! #require(grid(bytes, width: 240, height: 160, bpr: bpr))
        #expect(g.zone(col: 0, row: 8) == 0)
        #expect(g.zone(col: g.cols - 1, row: 8) == 10)
        #expect(g.labels.count == 2)
        #expect(Set(g.labels.map(\.zone)) == [0, 10])
        expectLabelsOwned(g)
    }

    /// A U-shaped region whose centroid falls in the enclosed notch: the
    /// label anchor must still land on a cell the region owns.
    @Test func concaveRegionLabelStaysInside() {
        let mid = UInt8((Densitometry.midGrayEncoded * 255).rounded())
        // Bright U: left column, right column and bottom row bright; the
        // notch (top-centre) mid-gray. 240x160 → 24x16 cells.
        let (bytes, bpr) = buffer(width: 240, height: 160) { x, y in
            let leftArm = x < 40
            let rightArm = x >= 200
            let bottom = y >= 120
            return (leftArm || rightArm || bottom) ? 255 : mid
        }
        let g = try! #require(grid(bytes, width: 240, height: 160, bpr: bpr))
        expectLabelsOwned(g)
        // Two regions: the bright U and the mid notch.
        #expect(g.labels.count == 2)
        let bright = try! #require(g.labels.first { $0.zone == 10 })
        // The U's centroid sits inside the notch (which is zone 5) — the
        // snapped anchor must be a U cell anyway (owned-cell contract above
        // already proves it; this documents the trap being exercised).
        #expect(g.zone(col: bright.col, row: bright.row) == 10)
    }

    /// The overlay's ruler is the densitometer's ruler — same bytes, same
    /// zone (uniform frame: any probe pixel equals the single cell zone).
    @Test func rulerMatchesDensitometer() {
        for v: UInt8 in [0, 40, 96, 128, 200, 255] {
            let (bytes, bpr) = buffer(width: 96, height: 96) { _, _ in v }
            let g = try! #require(grid(bytes, width: 96, height: 96, bpr: bpr))
            let e = Double(v) / 255.0
            let probe = Densitometry.read(encodedRGB: SIMD3(e, e, e))
            #expect(g.zones.allSatisfy { $0 == Int(probe.zone.rounded()) })
        }
    }
}
