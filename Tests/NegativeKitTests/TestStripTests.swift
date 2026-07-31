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

/// Ladder rotation (NegPy a2455ab): the 5×5 display grid stays put; the
/// logical (density × grade) assignment rotates beneath it.
@Suite struct TestStripRotationTests {
    /// Every orientation is a bijection onto the full ladder: all 25
    /// (density, grade) pairs appear exactly once.
    @Test(arguments: [0, 1, 2, 3])
    func orientationCoversLadderExactlyOnce(o: Int) {
        let cells = TestStrip.cells(orientation: o)
        #expect(cells.count == 25)
        let pairs = Set(cells.map { "\($0.density)|\($0.grade)" })
        #expect(pairs.count == 25)
    }

    @Test func orientationZeroIsTheBaseline() {
        #expect(TestStrip.cells(orientation: 0) == TestStrip.cells)
        // And 4 wraps to 0, -1 to 3.
        #expect(TestStrip.cells(orientation: 4) == TestStrip.cells(orientation: 0))
        #expect(TestStrip.cells(orientation: -1) == TestStrip.cells(orientation: 3))
    }

    /// One clockwise turn: the top-left corner receives what was at the
    /// bottom-left (standard 90° CW rotation of the logical grid).
    @Test func clockwiseTurnMovesCorners() {
        let base = TestStrip.cell(row: TestStrip.rows - 1, col: 0, orientation: 0)
        let turned = TestStrip.cell(row: 0, col: 0, orientation: 1)
        #expect(turned.density == base.density && turned.grade == base.grade)
    }

    /// Each display axis carries exactly ONE ladder under every orientation
    /// (all cells in a column share density or all share grade — what the
    /// axis labels rely on).
    @Test(arguments: [0, 1, 2, 3])
    func axesStayPure(o: Int) {
        for c in 0..<TestStrip.cols {
            let column = (0..<TestStrip.rows).map { TestStrip.cell(row: $0, col: c, orientation: o) }
            let sameDensity = column.allSatisfy { $0.density == column[0].density }
            let sameGrade = column.allSatisfy { $0.grade == column[0].grade }
            #expect(sameDensity != sameGrade, "column \(c) at o=\(o) must carry exactly one ladder")
        }
        for r in 0..<TestStrip.rows {
            let row = (0..<TestStrip.cols).map { TestStrip.cell(row: r, col: $0, orientation: o) }
            let sameDensity = row.allSatisfy { $0.density == row[0].density }
            let sameGrade = row.allSatisfy { $0.grade == row[0].grade }
            #expect(sameDensity != sameGrade, "row \(r) at o=\(o) must carry exactly one ladder")
        }
    }
}
