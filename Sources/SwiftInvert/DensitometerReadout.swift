import NegativeKit
import SwiftUI

/// The zone ruler: 11 cells, black (0) to paper white (X), with the probed
/// zone marked. It's the cheat sheet that turns the zone number into something
/// you can see — "III" means little until you see where III sits.
struct ZoneStrip: View {
    /// Probed zone, or nil to show the ruler idle.
    var zone: Double?

    private static let cellWidth: CGFloat = 9
    private static let cellHeight: CGFloat = 10

    /// Each cell's lightness, taken back through the ruler's own hinge so the
    /// steps line up with the zones the read-out names. `Color(white:)` isn't
    /// working-space-encoded, so these are an approximate visual ruler — read the number
    /// for the actual value, not the swatch.
    private func shade(_ z: Int) -> Color {
        let mid = Densitometry.midGrayEncoded
        let e = Double(z) <= 5 ? Double(z) / 5.0 * mid : mid + (Double(z) - 5) / 5.0 * (1 - mid)
        return Color(white: e)
    }

    var body: some View {
        let marked = zone.map { Int(min(max($0, 0), 10).rounded(.down)) }
        HStack(spacing: 1) {
            ForEach(0...10, id: \.self) { z in
                Rectangle()
                    .fill(shade(z))
                    .frame(width: Self.cellWidth, height: Self.cellHeight)
                    .overlay {
                        if z == marked {
                            Rectangle().strokeBorder(Color.accentColor, lineWidth: 1.5)
                        }
                    }
            }
        }
        .overlay {
            RoundedRectangle(cornerRadius: 1).stroke(.secondary.opacity(0.35), lineWidth: 0.5)
        }
        .opacity(zone == nil ? 0.35 : 1)
        .accessibilityLabel("Zone strip")
    }
}

/// Spot read-out for the canvas control bar. Reads `state.reading` and nothing
/// else, so pointer moves invalidate only this label (see `DensitometerState`).
/// Width is reserved so the bar's other controls don't jump as it appears.
struct DensitometerReadout: View {
    var state: DensitometerState

    var body: some View {
        HStack(spacing: 8) {
            ZoneStrip(zone: state.reading?.zone)
            Text(text)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(state.reading == nil ? .tertiary : .secondary)
                .fixedSize()
        }
        .frame(width: 210, alignment: .leading)
        .help(
            "Spot densitometer: reflection density and print zone under the pointer "
                + "(Zone V = 18% grey). Densities are relative to this scan's normalization.")
    }

    private var text: String {
        guard let r = state.reading else { return "hover to meter" }
        return String(format: "D %.2f · Zone %@", r.printDensity, Densitometry.zoneRoman(r.zone))
    }
}
