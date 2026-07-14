import AppKit
import SwiftUI

/// Draggable horizontal divider between stacked sidebar sections: a grabber
/// pill on the divider line, resize cursor on hover. Binds the height of the
/// section BELOW the handle by default (`sectionIsBelow`), clamped and
/// whole-point rounded. Reuse one per section boundary as sections are added.
struct SectionResizeHandle: View {
    @Binding var height: Double
    var range: ClosedRange<Double>
    var sectionIsBelow: Bool = true
    @State private var dragStartHeight: Double?

    var body: some View {
        ZStack {
            Divider()
            Capsule()
                .fill(.secondary.opacity(0.45))
                .frame(width: 36, height: 4)
        }
        .frame(height: 11)
        .contentShape(Rectangle())
        .help("Drag to resize the History list")
        .onHover { inside in
            if inside { NSCursor.resizeUpDown.push() } else { NSCursor.pop() }
        }
        .gesture(
            // Global space — the handle moves with the drag (see the library
            // splitter's jitter lesson).
            DragGesture(minimumDistance: 1, coordinateSpace: .global)
                .onChanged { g in
                    if dragStartHeight == nil { dragStartHeight = height }
                    let delta = sectionIsBelow ? -g.translation.height : g.translation.height
                    height = min(max((dragStartHeight! + delta).rounded(), range.lowerBound), range.upperBound)
                }
                .onEnded { _ in dragStartHeight = nil }
        )
    }
}
