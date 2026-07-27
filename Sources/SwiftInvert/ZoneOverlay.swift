import NegativeKit
import SwiftUI

/// The zone-system overlay (NegPy ae56f8b): drawn over the fitted image at
/// the displayed bitmap's grid. Same-zone cells merge into regions — the
/// edge between equal neighbours is simply not drawn — with one bold Roman
/// numeral per region; paper black (0) and paper white (X) numerals are
/// flagged red. Grid lines get a dark underlay pass beneath the white lines
/// so they stay readable over blown highlights.
struct ZoneOverlay: View {
    let grid: ZoneGrid.Grid

    var body: some View {
        Canvas { context, size in
            let cellW = size.width / CGFloat(grid.cols)
            let cellH = size.height / CGFloat(grid.rows)

            // Internal edges between differing neighbours only.
            var edges = Path()
            for r in 0..<grid.rows {
                for c in 0..<grid.cols {
                    let zone = grid.zone(col: c, row: r)
                    if c + 1 < grid.cols, grid.zone(col: c + 1, row: r) != zone {
                        let x = CGFloat(c + 1) * cellW
                        edges.move(to: CGPoint(x: x, y: CGFloat(r) * cellH))
                        edges.addLine(to: CGPoint(x: x, y: CGFloat(r + 1) * cellH))
                    }
                    if r + 1 < grid.rows, grid.zone(col: c, row: r + 1) != zone {
                        let y = CGFloat(r + 1) * cellH
                        edges.move(to: CGPoint(x: CGFloat(c) * cellW, y: y))
                        edges.addLine(to: CGPoint(x: CGFloat(c + 1) * cellW, y: y))
                    }
                }
            }
            context.stroke(edges, with: .color(.black.opacity(0.55)), lineWidth: 2.5)
            context.stroke(edges, with: .color(.white.opacity(0.9)), lineWidth: 1)

            let fontSize = min(max(min(cellW, cellH) * 0.55, 9), 22)
            for label in grid.labels {
                let center = CGPoint(
                    x: (CGFloat(label.col) + 0.5) * cellW,
                    y: (CGFloat(label.row) + 0.5) * cellH)
                let paperEnd = label.zone == 0 || label.zone == 10
                let text = Text(Densitometry.zoneRoman(Double(label.zone)))
                    .font(.system(size: fontSize, weight: .bold))
                    .foregroundStyle(paperEnd ? Color.red : Color.white)
                // Poor man's underlay for the numeral: the shadow pass.
                var shadowed = context
                shadowed.addFilter(.shadow(color: .black.opacity(0.8), radius: 1.5))
                shadowed.draw(text, at: center)
            }
        }
        .allowsHitTesting(false)
    }
}
