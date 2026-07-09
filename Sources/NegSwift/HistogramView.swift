import NegativeKit
import SwiftUI

/// The histogram, built from the best of the tools we admire:
/// - Lightroom: filled RGB channels drawn additively (screen blend), so
///   R+G overlap reads yellow, all three read white — you can see *which*
///   channels occupy a region at a glance.
/// - Capture One: crisp luma outline over the color mass.
/// - darktable: corner clipping indicators that light up per channel, with
///   exact percentages on hover.
/// - NegPy: draggable black/white-point edge handles wired straight to the
///   normalization offsets (drag inward to clip, double-click to reset).
/// Plus a hover readout (level + per-channel bin share) and a lin/log toggle.
struct HistogramView: View {
    @Bindable var model: AppModel

    @State private var logScale = true
    @State private var hoverBin: Int?
    // Offset value at drag start (handles accumulate from there).
    @State private var dragStartOffset: Double?

    private let handleZone: CGFloat = 0.35  // fraction of width each handle can travel
    private let offsetFullScale = 0.3  // D at full handle travel (Tone slider range)

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            chart
                .frame(height: 128)
            footer
        }
    }

    private var bins: [UInt32]? {
        model.histogram?.count == 1024 ? model.histogram : nil
    }

    // MARK: - Chart

    private var chart: some View {
        GeometryReader { geo in
            ZStack {
                RoundedRectangle(cornerRadius: 6).fill(.black.opacity(0.88))
                if let bins {
                    canvas(bins: bins, size: geo.size)
                    clippingIndicators(bins: bins)
                    handles(size: geo.size)
                    if let hoverBin { hoverReadout(bins: bins, bin: hoverBin) }
                } else {
                    Text("—").foregroundStyle(.tertiary)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .onContinuousHover { phase in
                switch phase {
                case .active(let p):
                    hoverBin = min(max(Int(p.x / geo.size.width * 255), 0), 255)
                case .ended:
                    hoverBin = nil
                }
            }
        }
    }

    private func canvas(bins: [UInt32], size: CGSize) -> some View {
        Canvas { context, size in
            let maxCount = max(bins.max() ?? 1, 1)

            func height(_ count: UInt32) -> Double {
                let v = Double(count)
                return logScale ? log1p(v) / log1p(Double(maxCount)) : v / Double(maxCount)
            }

            func path(channel: Int, closed: Bool) -> Path {
                var p = Path()
                let w = size.width, h = size.height
                p.move(to: CGPoint(x: 0, y: h))
                for i in 0..<256 {
                    let y = h * (1.0 - height(bins[channel * 256 + i]))
                    p.addLine(to: CGPoint(x: w * Double(i) / 255.0, y: y))
                }
                if closed {
                    p.addLine(to: CGPoint(x: size.width, y: h))
                    p.closeSubpath()
                }
                return p
            }

            // Quarter-tone gridlines behind the data.
            for q in 1..<4 {
                let x = size.width * Double(q) / 4.0
                context.stroke(
                    Path { $0.move(to: CGPoint(x: x, y: 0)); $0.addLine(to: CGPoint(x: x, y: size.height)) },
                    with: .color(.white.opacity(0.08)), lineWidth: 1)
            }

            // Additive RGB: pure channels, screen-blended — overlaps become
            // their secondary colors, triple overlap goes white.
            context.blendMode = .screen
            context.fill(path(channel: 0, closed: true), with: .color(Color(red: 0.85, green: 0.1, blue: 0.1).opacity(0.85)))
            context.fill(path(channel: 1, closed: true), with: .color(Color(red: 0.1, green: 0.8, blue: 0.15).opacity(0.85)))
            context.fill(path(channel: 2, closed: true), with: .color(Color(red: 0.15, green: 0.25, blue: 0.95).opacity(0.85)))
            context.blendMode = .normal

            // Luma outline on top.
            context.stroke(path(channel: 3, closed: false), with: .color(.white.opacity(0.75)), lineWidth: 1)
        }
    }

    // MARK: - Clipping indicators

    private func clipFractions(bins: [UInt32], end: Bool) -> SIMD3<Double> {
        let total = max(Double(bins[0..<256].reduce(0) { $0 + UInt64($1) }), 1)
        var out = SIMD3<Double>()
        for c in 0..<3 { out[c] = Double(bins[c * 256 + (end ? 255 : 0)]) / total }
        return out
    }

    private func clippingIndicators(bins: [UInt32]) -> some View {
        let black = clipFractions(bins: bins, end: false)
        let white = clipFractions(bins: bins, end: true)
        func color(_ f: SIMD3<Double>) -> Color {
            Color(
                red: f.x > 0.001 ? 1 : 0.25,
                green: f.y > 0.001 ? 1 : 0.25,
                blue: f.z > 0.001 ? 1 : 0.25)
        }
        func active(_ f: SIMD3<Double>) -> Bool { f.max() > 0.001 }
        return VStack {
            HStack {
                if active(black) {
                    Image(systemName: "arrowtriangle.left.fill")
                        .font(.system(size: 8))
                        .foregroundStyle(color(black))
                        .help(String(
                            format: "Shadow clipping — R %.1f%%  G %.1f%%  B %.1f%%",
                            black.x * 100, black.y * 100, black.z * 100))
                }
                Spacer()
                if active(white) {
                    Image(systemName: "arrowtriangle.right.fill")
                        .font(.system(size: 8))
                        .foregroundStyle(color(white))
                        .help(String(
                            format: "Highlight clipping — R %.1f%%  G %.1f%%  B %.1f%%",
                            white.x * 100, white.y * 100, white.z * 100))
                }
            }
            .padding(4)
            Spacer()
        }
    }

    // MARK: - Black/white-point handles (NegPy's histogram edge handles)

    private func handles(size: CGSize) -> some View {
        let travel = size.width * handleZone
        // Inward travel maps to the clipping direction of each offset:
        // left handle → negative blackPointOffset, right → positive whitePoint.
        let blackX = min(max(-model.settings.blackPointOffset / offsetFullScale, 0), 1) * travel
        let whiteX = size.width - min(max(model.settings.whitePointOffset / offsetFullScale, 0), 1) * travel
        return ZStack {
            handle(x: blackX, size: size, isBlack: true, travel: travel)
            handle(x: whiteX, size: size, isBlack: false, travel: travel)
        }
    }

    private func handle(x: CGFloat, size: CGSize, isBlack: Bool, travel: CGFloat) -> some View {
        ZStack {
            Rectangle()
                .fill(.white.opacity(0.55))
                .frame(width: 1.5, height: size.height)
            Image(systemName: isBlack ? "arrowtriangle.right.fill" : "arrowtriangle.left.fill")
                .font(.system(size: 7))
                .foregroundStyle(.white.opacity(0.8))
                .offset(y: size.height / 2 - 6)
        }
        .position(x: x, y: size.height / 2)
        .contentShape(Rectangle().inset(by: -6))
        .gesture(
            DragGesture(minimumDistance: 1)
                .onChanged { g in
                    if dragStartOffset == nil {
                        dragStartOffset =
                            isBlack ? model.settings.blackPointOffset : model.settings.whitePointOffset
                    }
                    let delta = Double(g.translation.width / travel) * offsetFullScale
                    if isBlack {
                        model.settings.blackPointOffset = min(max(dragStartOffset! - delta, -offsetFullScale), 0)
                    } else {
                        model.settings.whitePointOffset = min(max(dragStartOffset! - delta, 0), offsetFullScale)
                    }
                }
                .onEnded { _ in dragStartOffset = nil }
        )
        .onTapGesture(count: 2) {
            if isBlack { model.settings.blackPointOffset = 0 } else { model.settings.whitePointOffset = 0 }
        }
        .help(isBlack ? "Black point (drag right to deepen blacks)" : "White point (drag left to brighten whites)")
    }

    // MARK: - Hover readout & footer

    private func hoverReadout(bins: [UInt32], bin: Int) -> some View {
        let total = max(Double(bins[0..<256].reduce(0) { $0 + UInt64($1) }), 1)
        func pct(_ c: Int) -> Double { Double(bins[c * 256 + bin]) / total * 100 }
        return VStack {
            HStack {
                Spacer()
                Text(String(format: "L %d  ·  R %.1f  G %.1f  B %.1f", bin, pct(0), pct(1), pct(2)))
                    .font(.system(size: 8).monospacedDigit())
                    .padding(.horizontal, 5)
                    .padding(.vertical, 2)
                    .background(.black.opacity(0.7), in: Capsule())
                    .foregroundStyle(.white.opacity(0.9))
            }
            .padding(.horizontal, 16)
            .padding(.top, 3)
            Spacer()
        }
        .allowsHitTesting(false)
    }

    private var footer: some View {
        HStack {
            Text("Shadows").font(.system(size: 8)).foregroundStyle(.tertiary)
            Spacer()
            Button(logScale ? "log" : "lin") { logScale.toggle() }
                .buttonStyle(.plain)
                .font(.system(size: 8).monospacedDigit())
                .foregroundStyle(.secondary)
                .help("Toggle count scale")
            Spacer()
            Text("Highlights").font(.system(size: 8)).foregroundStyle(.tertiary)
        }
    }
}
