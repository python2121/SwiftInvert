import NegativeKit
import SwiftUI

/// The Interactive Histogram window (double-click the sidebar histogram):
/// a large R/G/B histogram where each channel's tonal axis is reshaped by
/// dragging. A drag grabs a tone (or an existing anchor) and moves it;
/// releasing PLANTS an anchor that stays fixed — later drags reshape only
/// their own segment between neighbouring anchors. Anchors show as vertical
/// lines with an ✕ below to remove them. The remap applies inside the
/// histogram/encode kernels, so the plot re-bins live under the drag.
struct InteractiveHistogramView: View {
    /// The window this histogram edits — captured from the key window when
    /// the panel opened (profile editors included), main model as fallback.
    /// Held WEAKLY: a closed profile editor's model must not be kept alive
    /// (or silently edited) by this panel.
    final class WeakModel {
        weak var value: AppModel?
        init(_ value: AppModel?) { self.value = value }
    }

    let fallback: AppModel
    @State private var target = WeakModel(nil)
    @State private var channel: Channel = .red
    /// Index (into the channel's sorted anchors) of the anchor being
    /// dragged; nil while idle.
    @State private var dragIndex: Int?

    /// Minimum normalized gap kept between neighbouring anchors on both axes.
    private static let gap = 0.01
    /// Grab tolerance around an existing anchor's line, normalized.
    private static let grabTolerance = 0.015

    enum Channel: Int, CaseIterable, Identifiable {
        case red = 0, green, blue
        var id: Int { rawValue }
        var name: String { ["Red", "Green", "Blue"][rawValue] }
        var color: Color { [.red, .green, .blue][rawValue] }
        var keyPath: WritableKeyPath<ExposureSettings, [SIMD2<Double>]> {
            switch self {
            case .red: return \.levelsRed
            case .green: return \.levelsGreen
            case .blue: return \.levelsBlue
            }
        }
    }

    private var model: AppModel { target.value ?? fallback }

    private var anchors: [SIMD2<Double>] {
        model.settings[keyPath: channel.keyPath]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Picker("Channel", selection: $channel) {
                    ForEach(Channel.allCases) { ch in
                        Text(ch.name).tag(ch)
                    }
                }
                .pickerStyle(.segmented)
                .frame(width: 220)
                .labelsHidden()
                Spacer()
                Button("Reset \(channel.name)") {
                    model.pendingHistoryLabel = "\(channel.name) levels reset"
                    model.settings[keyPath: channel.keyPath] = []
                }
                .disabled(anchors.isEmpty)
                Button("Reset All") {
                    model.pendingHistoryLabel = "Levels reset"
                    var s = model.settings
                    s.levelsRed = []
                    s.levelsGreen = []
                    s.levelsBlue = []
                    model.settings = s
                }
                .disabled(
                    model.settings.levelsRed.isEmpty && model.settings.levelsGreen.isEmpty
                        && model.settings.levelsBlue.isEmpty)
            }
            plot
                .frame(minHeight: 220)
            removalRow
                .frame(height: 18)
            Text("Drag in the plot to move tones: left of the grab stretches, right compresses. Release to plant an anchor — further drags work between anchors; ✕ removes one.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(14)
        .frame(minWidth: 520, minHeight: 340)
        .onAppear { if target.value == nil { target = WeakModel(KeyModelTracker.shared.active ?? fallback) } }
    }

    // MARK: - Plot

    private var plot: some View {
        GeometryReader { geo in
            ZStack {
                RoundedRectangle(cornerRadius: 6).fill(Color.black.opacity(0.85))
                Canvas { context, size in
                    guard let bins = model.histogram else { return }
                    for ch in Channel.allCases where ch != channel {
                        drawChannel(context, size: size, bins: bins, channel: ch, opacity: 0.25)
                    }
                    drawChannel(context, size: size, bins: bins, channel: channel, opacity: 0.9)
                    // Anchor lines at their OUTPUT positions; the dragged
                    // one also shows its input origin dashed.
                    for (i, a) in anchors.enumerated() {
                        var line = Path()
                        line.move(to: CGPoint(x: a.y * size.width, y: 0))
                        line.addLine(to: CGPoint(x: a.y * size.width, y: size.height))
                        context.stroke(
                            line,
                            with: .color(channel.color.opacity(i == dragIndex ? 1.0 : 0.75)),
                            lineWidth: i == dragIndex ? 2 : 1.2)
                        if i == dragIndex {
                            var inLine = Path()
                            inLine.move(to: CGPoint(x: a.x * size.width, y: 0))
                            inLine.addLine(to: CGPoint(x: a.x * size.width, y: size.height))
                            context.stroke(
                                inLine, with: .color(.white.opacity(0.3)),
                                style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                        }
                    }
                }
                .padding(6)
            }
            .contentShape(Rectangle())
            .gesture(dragGesture(plotWidth: max(geo.size.width - 12, 1)))
        }
    }

    private func dragGesture(plotWidth: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { value in
                let x = min(max(Double((value.location.x - 6) / plotWidth), 0), 1)
                if dragIndex == nil {
                    // Enter the editing state ONLY once a grab/plant succeeds:
                    // a failed plant (no room between anchors) must not
                    // increment the edit count — repeated ticks would wedge
                    // it above zero and stop history commits for the session.
                    guard let planted = grabOrPlantAnchor(at: x) else { return }
                    model.setControlEditing(true)
                    dragIndex = planted
                }
                guard let i = dragIndex else { return }
                moveAnchor(i, outputTo: x)
            }
            .onEnded { _ in
                // The anchor stays where it was released — it's already in
                // the settings; ending the drag just commits the history
                // entry ("Red levels"). Only balanced with a successful
                // grab (see onChanged).
                guard dragIndex != nil else { return }
                dragIndex = nil
                model.setControlEditing(false)
            }
    }

    /// Grab an existing anchor when the press lands near its line, else
    /// plant a new one whose input is the tone currently DISPLAYED at the
    /// press position (inverse of the remap in effect). At capacity the
    /// nearest anchor is grabbed instead.
    private func grabOrPlantAnchor(at x: Double) -> Int? {
        let pts = anchors
        if let nearest = pts.enumerated().min(by: { abs($0.1.y - x) < abs($1.1.y - x) }),
            abs(nearest.1.y - x) <= Self.grabTolerance
        {
            return nearest.0
        }
        if pts.count >= ExposureKernel.levelsMaxPoints {
            return pts.enumerated().min(by: { abs($0.1.y - x) < abs($1.1.y - x) })?.0
        }
        let input = inverseRemap(x, pts)
        // Keep a workable gap to the neighbours on the input axis.
        let lo = (pts.last(where: { $0.x < input })?.x ?? 0) + Self.gap
        let hi = (pts.first(where: { $0.x > input })?.x ?? 1) - Self.gap
        guard lo < hi else { return nil }
        let clampedIn = min(max(input, max(lo, ExposureKernel.levelsClamp)), min(hi, 1 - ExposureKernel.levelsClamp))
        var next = pts
        let insertAt = next.firstIndex(where: { $0.x > clampedIn }) ?? next.count
        next.insert(SIMD2(clampedIn, x), at: insertAt)
        model.settings[keyPath: channel.keyPath] = next
        return insertAt
    }

    /// Move anchor `i`'s output, clamped between its neighbours' outputs so
    /// the map stays monotone and other anchors genuinely never move.
    private func moveAnchor(_ i: Int, outputTo x: Double) {
        var pts = anchors
        guard pts.indices.contains(i) else { return }
        let lo = (i > 0 ? pts[i - 1].y : 0) + Self.gap
        let hi = (i < pts.count - 1 ? pts[i + 1].y : 1) - Self.gap
        let y = min(max(x, max(lo, ExposureKernel.levelsClamp)), min(hi, 1 - ExposureKernel.levelsClamp))
        guard pts[i].y != y else { return }
        pts[i].y = y
        model.settings[keyPath: channel.keyPath] = pts
    }

    /// Displayed position → input tone (ReferenceCurve.levelsInverseRemap —
    /// the tested inverse of the kernel's forward map).
    private func inverseRemap(_ y: Double, _ pts: [SIMD2<Double>]) -> Double {
        ReferenceCurve.levelsInverseRemap(y, pts)
    }

    // MARK: - Anchor removal row

    /// One ✕ under each anchor's line (spec: "a little x below all of the
    /// anchor points").
    private var removalRow: some View {
        GeometryReader { geo in
            let plotWidth = max(geo.size.width - 12, 1)
            ForEach(Array(anchors.enumerated()), id: \.offset) { i, a in
                Button {
                    model.pendingHistoryLabel = "\(channel.name) anchor removed"
                    var pts = anchors
                    guard pts.indices.contains(i) else { return }
                    pts.remove(at: i)
                    model.settings[keyPath: channel.keyPath] = pts
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("Remove this anchor")
                .position(x: 6 + a.y * plotWidth, y: 9)
            }
        }
    }

    private func drawChannel(
        _ context: GraphicsContext, size: CGSize, bins: [UInt32], channel: Channel, opacity: Double
    ) {
        let offset = channel.rawValue * 256
        guard bins.count >= offset + 256 else { return }
        let slice = bins[offset..<(offset + 256)]
        let peak = max(slice.max() ?? 1, 1)
        var path = Path()
        path.move(to: CGPoint(x: 0, y: size.height))
        for (i, v) in slice.enumerated() {
            let x = CGFloat(i) / 255 * size.width
            let y = size.height * (1 - CGFloat(v) / CGFloat(peak))
            path.addLine(to: CGPoint(x: x, y: y))
        }
        path.addLine(to: CGPoint(x: size.width, y: size.height))
        path.closeSubpath()
        context.fill(path, with: .color(channel.color.opacity(opacity * 0.55)))
        context.stroke(path, with: .color(channel.color.opacity(opacity)), lineWidth: 1)
    }
}
