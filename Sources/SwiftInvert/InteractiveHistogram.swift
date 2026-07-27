import NegativeKit
import SwiftUI

/// The Interactive Histogram window: a large R/G/B histogram where each
/// channel's tonal axis can be grabbed and dragged — the levels remap
/// (`ExposureSettings.levelsRed/Green/Blue`). Grab a horizontal position in
/// the selected channel, drag it: everything left stretches to fill,
/// everything right compresses (endpoints pinned). Because the remap applies
/// in the histogram256/outputEncode kernels, the histogram reshapes live
/// under the drag.
struct InteractiveHistogramView: View {
    /// The window this histogram edits — captured from the key window when
    /// the panel opened (profile editors included), main model as fallback.
    let fallback: AppModel
    @State private var target: AppModel?
    @State private var channel: Channel = .red
    /// The grab's input position (inverse-mapped through the remap in effect
    /// at drag start); nil while not dragging.
    @State private var dragInput: Double?

    enum Channel: Int, CaseIterable, Identifiable {
        case red = 0, green, blue
        var id: Int { rawValue }
        var name: String { ["Red", "Green", "Blue"][rawValue] }
        var color: Color { [.red, .green, .blue][rawValue] }
        var keyPath: WritableKeyPath<ExposureSettings, SIMD2<Double>> {
            [\ExposureSettings.levelsRed, \.levelsGreen, \.levelsBlue][rawValue]
        }
    }

    private var model: AppModel { target ?? fallback }

    private var point: SIMD2<Double> {
        model.settings[keyPath: channel.keyPath]
    }

    private var channelActive: Bool { point.x != point.y }

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
                    model.settings[keyPath: channel.keyPath] = SIMD2(0.5, 0.5)
                }
                .disabled(!channelActive)
                Button("Reset All") {
                    model.pendingHistoryLabel = "Levels reset"
                    var s = model.settings
                    s.levelsRed = SIMD2(0.5, 0.5)
                    s.levelsGreen = SIMD2(0.5, 0.5)
                    s.levelsBlue = SIMD2(0.5, 0.5)
                    model.settings = s
                }
                .disabled(
                    model.settings.levelsRed == SIMD2(0.5, 0.5)
                        && model.settings.levelsGreen == SIMD2(0.5, 0.5)
                        && model.settings.levelsBlue == SIMD2(0.5, 0.5))
            }
            plot
                .frame(minHeight: 220)
            Text("Drag horizontally in the plot: the grabbed tone moves, the left side stretches to fill, the right compresses. The histogram re-bins live.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(14)
        .frame(minWidth: 520, minHeight: 320)
        .onAppear { if target == nil { target = KeyModelTracker.shared.active ?? fallback } }
    }

    private var plot: some View {
        GeometryReader { geo in
            ZStack {
                RoundedRectangle(cornerRadius: 6).fill(Color.black.opacity(0.85))
                Canvas { context, size in
                    guard let bins = model.histogram else { return }
                    // Dim context channels behind the active one.
                    for ch in Channel.allCases where ch != channel {
                        drawChannel(context, size: size, bins: bins, channel: ch, opacity: 0.25)
                    }
                    drawChannel(context, size: size, bins: bins, channel: channel, opacity: 0.9)
                    // The control point's output position (and its input
                    // origin, faint) while the remap is active.
                    if channelActive || dragInput != nil {
                        let p = point
                        var inLine = Path()
                        inLine.move(to: CGPoint(x: p.x * size.width, y: 0))
                        inLine.addLine(to: CGPoint(x: p.x * size.width, y: size.height))
                        context.stroke(inLine, with: .color(.white.opacity(0.25)), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                        var outLine = Path()
                        outLine.move(to: CGPoint(x: p.y * size.width, y: 0))
                        outLine.addLine(to: CGPoint(x: p.y * size.width, y: size.height))
                        context.stroke(outLine, with: .color(channel.color.opacity(0.9)), lineWidth: 1.5)
                    }
                }
                .padding(6)
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { value in
                        let width = max(geo.size.width - 12, 1)
                        let x = min(max((value.location.x - 6) / width, 0), 1)
                        if dragInput == nil {
                            // The grab happens in the DISPLAYED (remapped)
                            // histogram: invert the remap in effect to find
                            // which input tone was grabbed, then drag its
                            // output. Re-grabbing re-anchors the channel's
                            // one control point.
                            model.setControlEditing(true)
                            dragInput = inverseRemap(Double(x), point: point)
                        }
                        guard let input = dragInput else { return }
                        model.settings[keyPath: channel.keyPath] = SIMD2(
                            min(max(input, ExposureKernel.levelsClamp), 1 - ExposureKernel.levelsClamp),
                            min(max(Double(x), ExposureKernel.levelsClamp), 1 - ExposureKernel.levelsClamp))
                    }
                    .onEnded { _ in
                        dragInput = nil
                        model.setControlEditing(false)
                    }
            )
        }
    }

    /// Displayed position → input position under the channel's current remap.
    private func inverseRemap(_ x: Double, point p: SIMD2<Double>) -> Double {
        let a = p.x, b = p.y
        guard a != b, b > 0, b < 1 else { return x }
        return x <= b ? x * a / b : a + (x - b) * (1 - a) / (1 - b)
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
