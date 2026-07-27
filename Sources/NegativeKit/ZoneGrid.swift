import Foundation

/// Zone-system overlay math (NegPy `analysis.zone_grid` / `zone_region_labels`,
/// ae56f8b): the integer Adams zone of each cell of a fixed grid over the
/// display-encoded frame, plus one label anchor per contiguous same-zone
/// region. Pure measurement on the displayed bitmap — the same ruler as the
/// zone strip and the spot densitometer (`Densitometry.zone(ofEncoded:)` on
/// Rec.709 luma of the ENCODED triplet), so the three read-outs can never
/// disagree. No render involvement, no parity surface.
public enum ZoneGrid {
    /// Cells along the frame's long edge (NegPy ZONE_GRID_CELLS).
    public static let cellsLongEdge = 24

    public struct Grid: Equatable, Sendable {
        public let cols: Int
        public let rows: Int
        /// Integer zone 0…10 per cell, row-major.
        public let zones: [Int]
        /// One numeral anchor per contiguous same-zone region.
        public let labels: [Label]

        public struct Label: Equatable, Sendable {
            public let col: Int
            public let row: Int
            public let zone: Int

            public init(col: Int, row: Int, zone: Int) {
                self.col = col
                self.row = row
                self.zone = zone
            }
        }

        public func zone(col: Int, row: Int) -> Int { zones[row * cols + col] }
    }

    /// Grid over an rgba8 display-encoded bitmap (the densitometer's cached
    /// bytes). Cells are square-ish whatever the aspect; the area average
    /// over a cell is what damps grain — no extra smoothing needed.
    ///
    /// Like NegPy, the frame is stride-subsampled first (a full-res HQ
    /// buffer is millions of pixels; ~4 samples per cell edge is plenty),
    /// then box-averaged per cell. NegPy uses INTER_AREA for the second
    /// step; integer-boundary box means differ by at most the fractional
    /// edge samples — well under the zone rounding step, and this is a
    /// read-out, not a parity surface.
    public static func compute(
        rgba8 base: UnsafePointer<UInt8>, width: Int, height: Int, bytesPerRow: Int,
        cells: Int = cellsLongEdge
    ) -> Grid? {
        guard width >= 2, height >= 2, cells >= 1 else { return nil }

        // Stride subsample (NegPy: step = max(1, min(h, w) // (4 * cells))).
        let step = max(1, min(width, height) / (4 * cells))
        let sw = (width + step - 1) / step
        let sh = (height + step - 1) / step

        // Grid dims from the sampled aspect: long edge = `cells`, short edge
        // proportional (NegPy: scale = cells / max(dims), rounded).
        let scale = Double(cells) / Double(max(sw, sh))
        let cols = max(1, Int((Double(sw) * scale).rounded()))
        let rows = max(1, Int((Double(sh) * scale).rounded()))

        // Per-cell box average of the ENCODED rgb, then luma → zone → rint.
        var zones = [Int](repeating: 0, count: cols * rows)
        for r in 0..<rows {
            let y0 = r * sh / rows, y1 = max(y0 + 1, (r + 1) * sh / rows)
            for c in 0..<cols {
                let x0 = c * sw / cols, x1 = max(x0 + 1, (c + 1) * sw / cols)
                var sum = SIMD3<Double>()
                for sy in y0..<y1 {
                    let rowBase = min(sy * step, height - 1) * bytesPerRow
                    for sx in x0..<x1 {
                        let i = rowBase + min(sx * step, width - 1) * 4
                        sum += SIMD3(Double(base[i]), Double(base[i + 1]), Double(base[i + 2]))
                    }
                }
                let n = Double((y1 - y0) * (x1 - x0)) * 255.0
                let zone = Densitometry.zone(ofEncoded: Densitometry.luma(sum / n))
                zones[r * cols + c] = min(max(Int(zone.rounded()), 0), 10)
            }
        }
        return Grid(cols: cols, rows: rows, zones: zones, labels: regionLabels(zones: zones, cols: cols, rows: rows))
    }

    /// One label anchor per 4-connected same-zone region. The anchor is the
    /// region cell nearest its centroid, so a concave region's numeral can't
    /// land outside it (its centroid can) — NegPy `zone_region_labels`.
    static func regionLabels(zones: [Int], cols: Int, rows: Int) -> [Grid.Label] {
        var seen = [Bool](repeating: false, count: zones.count)
        var labels: [Grid.Label] = []
        var stack: [Int] = []
        var region: [Int] = []
        for start in 0..<zones.count where !seen[start] {
            let zone = zones[start]
            region.removeAll(keepingCapacity: true)
            stack.removeAll(keepingCapacity: true)
            stack.append(start)
            seen[start] = true
            while let i = stack.popLast() {
                region.append(i)
                let c = i % cols, r = i / cols
                if c > 0, !seen[i - 1], zones[i - 1] == zone { seen[i - 1] = true; stack.append(i - 1) }
                if c < cols - 1, !seen[i + 1], zones[i + 1] == zone { seen[i + 1] = true; stack.append(i + 1) }
                if r > 0, !seen[i - cols], zones[i - cols] == zone { seen[i - cols] = true; stack.append(i - cols) }
                if r < rows - 1, !seen[i + cols], zones[i + cols] == zone { seen[i + cols] = true; stack.append(i + cols) }
            }
            var cx = 0.0, cy = 0.0
            for i in region {
                cx += Double(i % cols)
                cy += Double(i / cols)
            }
            cx /= Double(region.count)
            cy /= Double(region.count)
            var best = region[0]
            var bestD = Double.infinity
            for i in region {
                let dx = Double(i % cols) - cx, dy = Double(i / cols) - cy
                let d = dx * dx + dy * dy
                if d < bestD {
                    bestD = d
                    best = i
                }
            }
            labels.append(Grid.Label(col: best % cols, row: best / cols, zone: zone))
        }
        return labels
    }
}
