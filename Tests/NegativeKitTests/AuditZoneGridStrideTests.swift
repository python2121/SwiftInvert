import Foundation
import Testing

@testable import NegativeKit

/// ZoneGrid.compute on a buffer large enough to engage the stride subsample
/// (step > 1) — the path the full-res HQ bitmap takes.
@Suite struct AuditZoneGridStrideTests {

    /// rgba8 horizontal gradient (dark left → bright right), padded rows.
    private func gradientBuffer(width: Int, height: Int, pad: Int) -> ([UInt8], Int) {
        let bpr = width * 4 + pad
        var bytes = [UInt8](repeating: 0, count: bpr * height)
        for y in 0..<height {
            for x in 0..<width {
                let v = UInt8((Double(x) * 255.0 / Double(width - 1)).rounded())
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

    /// 800×600: step = min(800,600)/(4·24) = 6 (stride path engaged);
    /// sampled dims 134×100 → cols = 24, rows = round(100·24/134) = 18.
    /// The gradient is dark→bright left→right, and zone is monotone in
    /// encoded luma, so each row's zones must be non-decreasing with the
    /// column (direction verified empirically: col 0 ≈ zone 0, col 23 = 10).
    @Test func strideSubsampledGradient() throws {
        let (bytes, bpr) = gradientBuffer(width: 800, height: 600, pad: 16)
        let g = try #require(grid(bytes, width: 800, height: 600, bpr: bpr))
        #expect(g.cols == 24)
        #expect(g.rows == 18)
        #expect(g.zones.allSatisfy { (0...10).contains($0) })

        for r in 0..<g.rows {
            var last = -1
            for c in 0..<g.cols {
                let z = g.zone(col: c, row: r)
                #expect(z >= last, "row \(r): zone dropped at col \(c)")
                last = z
            }
            // Direction pin: the gradient's dark edge is the low-zone edge.
            #expect(g.zone(col: 0, row: r) < g.zone(col: g.cols - 1, row: r))
        }
        // Extremes reach the paper ends (mean of the first/last 1/24 of a
        // 0…255 ramp encodes near black / near white).
        #expect(g.zone(col: 0, row: 0) <= 1)
        #expect(g.zone(col: g.cols - 1, row: 0) >= 9)
    }

    /// The same CONTENT at 96×72 runs with step = 1 and produces the same
    /// 24×18 grid shape; a stride subsample of a smooth gradient must land
    /// the corner cells in the same zone as the dense sampling.
    @Test func strideMatchesDenseSamplingAtTheCorners() throws {
        let (bigBytes, bigBpr) = gradientBuffer(width: 800, height: 600, pad: 16)
        let big = try #require(grid(bigBytes, width: 800, height: 600, bpr: bigBpr))
        let (smallBytes, smallBpr) = gradientBuffer(width: 96, height: 72, pad: 0)
        let small = try #require(grid(smallBytes, width: 96, height: 72, bpr: smallBpr))

        #expect(small.cols == big.cols && small.rows == big.rows)
        for (c, r) in [
            (0, 0), (big.cols - 1, 0), (0, big.rows - 1), (big.cols - 1, big.rows - 1),
        ] {
            #expect(
                big.zone(col: c, row: r) == small.zone(col: c, row: r),
                "corner (\(c),\(r)): stride \(big.zone(col: c, row: r)) vs dense \(small.zone(col: c, row: r))")
        }
    }
}
