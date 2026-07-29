import Foundation
import Testing

@testable import NegativeKit

/// Test-strip grid math + mosaic assembly (NegPy e4bc450 port).
@Suite struct TestStripTests {
    @Test func laddersAreCentredOnDefaults() {
        // Centre patch = the stock conversion (density 1.0, grade 115).
        #expect(TestStrip.densitiesByColumn[TestStrip.cols / 2] == 1.0)
        #expect(TestStrip.gradesByRow[TestStrip.rows / 2] == 115)
        // Columns brighten left → right (density descending — this app's
        // "right = brighter" convention; deliberate mirror of upstream).
        #expect(TestStrip.densitiesByColumn == TestStrip.densitiesByColumn.sorted(by: >))
        // Rows soften top → bottom (grade ascending, NegPy order).
        #expect(TestStrip.gradesByRow == TestStrip.gradesByRow.sorted())
        // The exact NegPy ladder values, mirrored.
        #expect(Set(TestStrip.densitiesByColumn) == Set([0.4, 0.7, 1.0, 1.3, 1.6]))
        #expect(TestStrip.gradesByRow == [75, 95, 115, 135, 155])
    }

    @Test func cellsAreRowMajorAndComplete() {
        let cells = TestStrip.cells
        #expect(cells.count == 25)
        #expect(cells[0].row == 0 && cells[0].col == 0)
        #expect(cells[1].col == 1)  // row-major
        #expect(cells[24].row == 4 && cells[24].col == 4)
        #expect(cells[7].density == TestStrip.densitiesByColumn[2])
        #expect(cells[7].grade == TestStrip.gradesByRow[1])
    }

    /// Patch rects tile the frame exactly: disjoint, union = frame — for
    /// dimensions that don't divide evenly.
    @Test(arguments: [(100, 75), (1536, 1025), (7, 11)])
    func patchRectsTileExactly(dims: (Int, Int)) {
        let (w, h) = dims
        var covered = [Bool](repeating: false, count: w * h)
        for cell in TestStrip.cells {
            let r = TestStrip.patchRect(width: w, height: h, row: cell.row, col: cell.col)
            for y in r.y0..<r.y1 {
                for x in r.x0..<r.x1 {
                    #expect(!covered[y * w + x], "overlap at (\(x), \(y))")
                    covered[y * w + x] = true
                }
            }
        }
        #expect(covered.allSatisfy { $0 }, "gaps in tiling \(w)x\(h)")
    }

    /// Hit-testing inverts the rect math: the centre of every patch maps
    /// back to its own (row, col).
    @Test func cellAtInvertsPatchRect() {
        let w = 1536, h = 1025
        for cell in TestStrip.cells {
            let r = TestStrip.patchRect(width: w, height: h, row: cell.row, col: cell.col)
            let nx = (Double(r.x0 + r.x1) / 2) / Double(w)
            let ny = (Double(r.y0 + r.y1) / 2) / Double(h)
            let hit = TestStrip.cell(atX: nx, y: ny)
            #expect(hit.row == cell.row && hit.col == cell.col)
        }
        // Edges clamp.
        #expect(TestStrip.cell(atX: 1.0, y: 1.0) == (TestStrip.rows - 1, TestStrip.cols - 1))
        #expect(TestStrip.cell(atX: 0.0, y: 0.0) == (0, 0))
    }

    @Test func nearestCellFindsDefaultsAndClamps() {
        #expect(TestStrip.nearestCell(density: 1.0, grade: 115) == (2, 2))
        #expect(TestStrip.nearestCell(density: 0.6, grade: 80) == (0, 3))  // 0.6→0.7 (col 3), 80→75 (row 0)
        #expect(TestStrip.nearestCell(density: 5.0, grade: 300) == (4, 0))  // clamps to extremes
    }

    /// Mosaic assembly: tiles filled with distinct values land exactly in
    /// their own patch and nowhere else.
    @Test func mosaicPatchesComeFromTheirOwnTiles() {
        let w = 50, h = 35, bpr = w * 4
        var mosaic = [UInt8](repeating: 0, count: h * bpr)
        for cell in TestStrip.cells {
            let value = UInt8(10 + cell.row * TestStrip.cols + cell.col)
            let tile = [UInt8](repeating: value, count: h * bpr)
            TestStrip.copyPatch(
                tile: tile, into: &mosaic, width: w, height: h, bytesPerRow: bpr,
                row: cell.row, col: cell.col)
        }
        for cell in TestStrip.cells {
            let want = UInt8(10 + cell.row * TestStrip.cols + cell.col)
            let r = TestStrip.patchRect(width: w, height: h, row: cell.row, col: cell.col)
            for y in r.y0..<r.y1 {
                for x in r.x0..<r.x1 {
                    #expect(mosaic[y * bpr + x * 4] == want, "patch (\(cell.row),\(cell.col)) at (\(x),\(y))")
                }
            }
        }
    }
}
