import SwiftUI

/// 4×256-bin histogram (R, G, B filled, luma outline), log-scaled counts —
/// the post-curve display-encoded distribution, matching NegPy's chart.
struct HistogramView: View {
    let bins: [UInt32]?  // 1024 = R,G,B,L × 256

    var body: some View {
        Canvas { context, size in
            guard let bins, bins.count == 1024 else { return }
            let maxCount = max(bins.max() ?? 1, 1)
            let logMax = log1p(Double(maxCount))

            func path(channel: Int, closed: Bool) -> Path {
                var p = Path()
                let w = size.width, h = size.height
                p.move(to: CGPoint(x: 0, y: h))
                for i in 0..<256 {
                    let v = log1p(Double(bins[channel * 256 + i])) / logMax
                    let pt = CGPoint(x: w * Double(i) / 255.0, y: h * (1.0 - v))
                    p.addLine(to: pt)
                }
                if closed {
                    p.addLine(to: CGPoint(x: size.width, y: h))
                    p.closeSubpath()
                }
                return p
            }

            let colors: [Color] = [.red, .green, .blue]
            for ch in 0..<3 {
                context.fill(path(channel: ch, closed: true), with: .color(colors[ch].opacity(0.35)))
            }
            context.stroke(path(channel: 3, closed: false), with: .color(.white.opacity(0.85)), lineWidth: 1)
        }
        .background(.black.opacity(0.85), in: RoundedRectangle(cornerRadius: 6))
        .frame(height: 110)
        .accessibilityLabel("Histogram")
    }
}
