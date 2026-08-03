import NegativeKit
import SwiftUI

/// Zone-placement canvas layer (NegPy 5a095f3/9dff124 port): the solved-look
/// preview drawn over the frame (same overlay trick as the test strip), the
/// draggable pins with measured → target read-outs, and the Apply/Clear
/// panel. Sits in DetailView's image ZStack, so its local coordinates span
/// the displayed bitmap and zoom/pan are inverse-mapped by SwiftUI.
struct ZonePlacementLayer: View {
    let state: AppModel.ZonePlacementState
    let model: AppModel

    /// Live drag position per pin (content-normalized), so the marker tracks
    /// the pointer while the re-sample rounds through the actor.
    @State private var dragPosition: [Int: CGPoint] = [:]

    var body: some View {
        GeometryReader { geo in
            ZStack {
                if let preview = state.previewImage {
                    Image(decorative: preview, scale: 1)
                        .resizable()
                        .interpolation(.high)
                }
                // Click catcher: a tap places (or re-homes) a pin.
                Color.clear
                    .contentShape(Rectangle())
                    .gesture(
                        SpatialTapGesture()
                            .onEnded { tap in
                                model.addZonePin(
                                    u: (tap.location.x / geo.size.width).clamped01,
                                    v: (tap.location.y / geo.size.height).clamped01)
                            })
                ForEach(state.pins.indices, id: \.self) { i in
                    pinMarker(index: i, in: geo.size)
                }
            }
            .coordinateSpace(name: "zonePins")
            .overlay(alignment: .top) {
                if state.armedZone != nil, state.pins.isEmpty {
                    hint("Click the tone to place on Zone "
                        + Densitometry.zoneRoman(state.armedZone ?? 0) + " — Esc cancels")
                }
            }
            .overlay(alignment: .bottom) { panel }
        }
    }

    private func hint(_ text: String) -> some View {
        Text(text)
            .font(.caption)
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .background(.black.opacity(0.65), in: Capsule())
            .foregroundStyle(.white)
            .padding(.top, 10)
    }

    private func pinMarker(index: Int, in size: CGSize) -> some View {
        let pin = state.pins[index]
        let pos = dragPosition[index]
            ?? CGPoint(x: pin.nx * size.width, y: pin.ny * size.height)
        let measured = index < state.labels.count ? state.labels[index] : "–"
        return VStack(spacing: 3) {
            Text("\(measured) → \(Densitometry.zoneRoman(pin.targetZone))")
                .font(.system(size: 10, weight: .semibold, design: .monospaced))
                .padding(.horizontal, 5)
                .padding(.vertical, 2)
                .background(.black.opacity(0.7), in: Capsule())
                .foregroundStyle(.white)
            ZStack {
                Circle().fill(.black.opacity(0.55)).frame(width: 14, height: 14)
                Circle().strokeBorder(.white, lineWidth: 1.5).frame(width: 14, height: 14)
                Circle().fill(.white).frame(width: 3, height: 3)
            }
        }
        // The dot is the anchor; the label floats above it.
        .contentShape(Rectangle())
        .position(x: pos.x, y: pos.y - 10)
        .gesture(
            DragGesture(coordinateSpace: .named("zonePins"))
                .onChanged { g in
                    dragPosition[index] = g.location
                    model.moveZonePin(
                        index: index,
                        u: (g.location.x / size.width).clamped01,
                        v: (g.location.y / size.height).clamped01,
                        final: false)
                }
                .onEnded { g in
                    dragPosition[index] = nil
                    model.moveZonePin(
                        index: index,
                        u: (g.location.x / size.width).clamped01,
                        v: (g.location.y / size.height).clamped01,
                        final: true)
                })
    }

    @ViewBuilder
    private var panel: some View {
        if !state.pins.isEmpty {
            HStack(spacing: 10) {
                if state.solving {
                    ProgressView().controlSize(.small)
                } else if let solution = state.solution {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(solutionSummary(solution))
                            .font(.system(size: 10, design: .monospaced))
                        if solution.clamped {
                            Text("some targets are out of reach — closest shown")
                                .font(.system(size: 9))
                                .foregroundStyle(.orange)
                        }
                    }
                } else if state.pins.count > ZonePlacement.maxPins {
                    Text("too many pins").font(.system(size: 10))
                }
                Button("Apply") { model.applyZonePlacement() }
                    .disabled(state.solution == nil || state.solving)
                    .help("Commit the solved settings as one history entry")
                Button("Clear") { model.clearZonePlacement() }
                    .help("Drop the pins without changing anything (Esc)")
            }
            .controlSize(.small)
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(.regularMaterial, in: Capsule())
            .padding(.bottom, 12)
        }
    }

    private func solutionSummary(_ solution: ZonePlacement.Solution) -> String {
        var parts = [
            String(format: "Brightness %.2f", 2.0 - solution.settings.density)
        ]
        if state.pins.count >= 2 {
            parts.append(String(format: "Grade R%.0f", solution.settings.grade))
        }
        if let knee = solution.kneeLabel {
            parts.append(knee)
        }
        return parts.joined(separator: " · ")
    }
}

extension CGFloat {
    fileprivate var clamped01: Double { Double(Swift.min(Swift.max(self, 0), 1)) }
}
