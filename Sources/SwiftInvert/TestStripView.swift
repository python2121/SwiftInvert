import NegativeKit
import SwiftUI

/// The test-strip presentation (NegPy e4bc450): the assembled mosaic drawn
/// over the canvas at the image's own frame, with NO gridlines — it reads as
/// one print, the way a real test strip does. Only the hovered patch is
/// outlined (so it stays obvious what a click would take); Brightness labels
/// run along the top (brighter →, this app's slider direction), Grade labels
/// down the left; the rung pair matching the strip's base settings is
/// accented. Click a patch to commit its Brightness+Grade.
struct TestStripLayer: View {
    let strip: AppModel.TestStripState
    let model: AppModel

    @State private var hovered: (row: Int, col: Int)?

    var body: some View {
        GeometryReader { geo in
            ZStack {
                Image(decorative: strip.image, scale: 1.0)
                    .resizable()
                    .interpolation(.high)
                chrome(size: geo.size)
            }
            .contentShape(Rectangle())
            .onContinuousHover(coordinateSpace: .local) { phase in
                switch phase {
                case .active(let p):
                    let cell = TestStrip.cell(
                        atX: min(max(p.x / geo.size.width, 0), 1),
                        y: min(max(p.y / geo.size.height, 0), 1))
                    if hovered == nil || hovered! != cell { hovered = cell }
                case .ended:
                    hovered = nil
                }
            }
            .gesture(
                SpatialTapGesture(coordinateSpace: .local).onEnded { value in
                    let cell = TestStrip.cell(
                        atX: min(max(value.location.x / geo.size.width, 0), 1),
                        y: min(max(value.location.y / geo.size.height, 0), 1))
                    model.pickTestStripCell(row: cell.row, col: cell.col)
                }
            )
        }
    }

    private func chrome(size: CGSize) -> some View {
        Canvas { context, _ in
            let cellW = size.width / CGFloat(TestStrip.cols)
            let cellH = size.height / CGFloat(TestStrip.rows)
            let accent = TestStrip.nearestCell(
                density: strip.baseSettings.density, grade: strip.baseSettings.grade)
            let fontSize = min(max(min(cellW, cellH) * 0.14, 10), 16)

            // Axis labels: Brightness along the top edge of each column,
            // Grade down the left edge of each row (bold, dark underlay via
            // shadow — same treatment as the zone overlay numerals).
            var shadowed = context
            shadowed.addFilter(.shadow(color: .black.opacity(0.85), radius: 1.5))
            for (c, density) in TestStrip.densitiesByColumn.enumerated() {
                let accented = c == accent.col
                let text = Text(String(format: "%.1f", 2.0 - density))
                    .font(.system(size: fontSize, weight: .bold))
                    .foregroundStyle(accented ? Color.accentColor : .white)
                shadowed.draw(
                    text, at: CGPoint(x: (CGFloat(c) + 0.5) * cellW, y: fontSize * 0.9))
            }
            for (r, grade) in TestStrip.gradesByRow.enumerated() {
                let accented = r == accent.row
                let text = Text("R\(Int(grade))")
                    .font(.system(size: fontSize, weight: .bold))
                    .foregroundStyle(accented ? Color.accentColor : .white)
                shadowed.draw(
                    text, at: CGPoint(x: fontSize * 1.2, y: (CGFloat(r) + 0.5) * cellH))
            }

            // Hovered patch outline — the only gridwork drawn.
            if let hovered {
                let rect = CGRect(
                    x: CGFloat(hovered.col) * cellW, y: CGFloat(hovered.row) * cellH,
                    width: cellW, height: cellH)
                context.stroke(
                    Path(rect.insetBy(dx: 1, dy: 1)), with: .color(.black.opacity(0.6)),
                    lineWidth: 3)
                context.stroke(
                    Path(rect.insetBy(dx: 1, dy: 1)), with: .color(.white), lineWidth: 1.5)
            }
        }
        .allowsHitTesting(false)
    }
}
