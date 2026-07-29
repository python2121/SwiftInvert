import Foundation

/// Test-strip math (NegPy `analysis.strip_*`, e4bc450/0.45): the frame
/// printed as a 5×5 grid of real renders — Brightness across, Grade down —
/// like a split-filter darkroom test strip. Pure grid math + rgba8 mosaic
/// assembly; the renders themselves come from ImageSession.
///
/// Ladders are ABSOLUTE and centred on the defaults (NegPy's revised spec):
/// a strip printed off one frame is comparable to the next, and the current
/// settings are one of the patches. Deliberate presentation divergence from
/// upstream: columns are ordered so the image BRIGHTENS left → right
/// (density descending), matching this app's "right = brighter" slider
/// convention everywhere; upstream draws density ascending.
public enum TestStrip {
    /// Print density per column, left → right (brightness 0.4 … 1.6).
    public static let densitiesByColumn: [Double] = [1.6, 1.3, 1.0, 0.7, 0.4]
    /// ISO R grade per row, top → bottom (hard → soft, NegPy order).
    public static let gradesByRow: [Double] = [75, 95, 115, 135, 155]

    public static var cols: Int { densitiesByColumn.count }
    public static var rows: Int { gradesByRow.count }

    public struct Cell: Equatable, Sendable {
        public let row: Int
        public let col: Int
        public let density: Double
        public let grade: Double
    }

    /// Every patch, row-major (the render order).
    public static var cells: [Cell] {
        gradesByRow.enumerated().flatMap { r, g in
            densitiesByColumn.enumerated().map { c, d in
                Cell(row: r, col: c, density: d, grade: g)
            }
        }
    }

    /// Integer split of `extent` into `divisions` (NegPy `_strip_bounds`):
    /// exact tiling, no gaps or overlaps whatever the rounding.
    static func bounds(extent: Int, divisions: Int, index: Int) -> (Int, Int) {
        (extent * index / divisions, extent * (index + 1) / divisions)
    }

    /// Pixel rect of one patch inside a width×height frame:
    /// (x0, y0, x1, y1), exclusive upper bounds.
    public static func patchRect(width: Int, height: Int, row: Int, col: Int)
        -> (x0: Int, y0: Int, x1: Int, y1: Int)
    {
        let (x0, x1) = bounds(extent: width, divisions: cols, index: col)
        let (y0, y1) = bounds(extent: height, divisions: rows, index: row)
        return (x0, y0, x1, y1)
    }

    /// Content-normalized position (0…1) → (row, col).
    public static func cell(atX nx: Double, y ny: Double) -> (row: Int, col: Int) {
        (
            min(max(Int(ny * Double(rows)), 0), rows - 1),
            min(max(Int(nx * Double(cols)), 0), cols - 1)
        )
    }

    /// The rung nearest the given settings (the current-settings accent).
    public static func nearestCell(density: Double, grade: Double) -> (row: Int, col: Int) {
        let col = densitiesByColumn.enumerated().min { abs($0.1 - density) < abs($1.1 - density) }!.0
        let row = gradesByRow.enumerated().min { abs($0.1 - grade) < abs($1.1 - grade) }!.0
        return (row, col)
    }

    /// Copy one tile's own patch region into the mosaic (rgba8, shared
    /// bytesPerRow). Called per render so only mosaic + one tile are ever
    /// held; each tile contributes only the frame area its patch occupies,
    /// so the mosaic reads as ONE print (NegPy `strip_mosaic`).
    public static func copyPatch(
        tile: [UInt8], into mosaic: inout [UInt8],
        width: Int, height: Int, bytesPerRow: Int, row: Int, col: Int
    ) {
        let r = patchRect(width: width, height: height, row: row, col: col)
        guard r.x1 > r.x0, r.y1 > r.y0,
            tile.count >= (r.y1 - 1) * bytesPerRow + r.x1 * 4,
            mosaic.count == tile.count
        else { return }
        let rowBytes = (r.x1 - r.x0) * 4
        tile.withUnsafeBufferPointer { src in
            mosaic.withUnsafeMutableBufferPointer { dst in
                for y in r.y0..<r.y1 {
                    let offset = y * bytesPerRow + r.x0 * 4
                    memcpy(dst.baseAddress! + offset, src.baseAddress! + offset, rowBytes)
                }
            }
        }
    }
}
