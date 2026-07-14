import NegativeKit
import SwiftUI

/// Negative Lab Pro-style color grading: overall Temp/Tint plus per-band
/// (shadows/mids/highs) R↔C, G↔M, B↔Y balance, all on gradient tracks.
struct ColorGradingSection: View {
    @Bindable var model: AppModel

    enum Band: String, CaseIterable, Identifiable {
        case shadows = "Shadows"
        case mids = "Mids"
        case highs = "Highs"
        var id: String { rawValue }

        var keyPath: WritableKeyPath<ExposureSettings, SIMD3<Double>> {
            switch self {
            case .shadows: return \.colorShadows
            case .mids: return \.colorMids
            case .highs: return \.colorHighs
            }
        }
    }

    @State private var band: Band = .mids

    static let tempColors = [Color(red: 0.25, green: 0.35, blue: 0.85), Color(white: 0.4), Color(red: 0.75, green: 0.7, blue: 0.15)]
    static let tintColors = [Color(red: 0.2, green: 0.75, blue: 0.25), Color(white: 0.4), Color(red: 0.7, green: 0.2, blue: 0.75)]
    static let rcColors = [Color(red: 0.85, green: 0.2, blue: 0.2), Color(white: 0.4), Color(red: 0.1, green: 0.75, blue: 0.8)]
    static let gmColors = [Color(red: 0.2, green: 0.75, blue: 0.25), Color(white: 0.4), Color(red: 0.8, green: 0.2, blue: 0.7)]
    static let byColors = [Color(red: 0.25, green: 0.35, blue: 0.85), Color(white: 0.4), Color(red: 0.8, green: 0.75, blue: 0.2)]

    var body: some View {
        GroupBox("Color Grading") {
            VStack(alignment: .leading, spacing: 10) {
                GradientSlider(
                    label: "Temp", value: $model.settings.temp, range: -1...1,
                    defaultValue: 0, colors: Self.tempColors,
                    help: "Overall white balance along blue ↔ yellow (right = warmer). Moves every pixel, neutrals included.")
                GradientSlider(
                    label: "Tint", value: $model.settings.tint, range: -1...1,
                    defaultValue: 0, colors: Self.tintColors,
                    help: "Overall white balance along green ↔ magenta. Moves every pixel, neutrals included.")

                Picker("", selection: $band) {
                    ForEach(Band.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .help("Which tonal region the three color-balance sliders below act on")

                GradientSlider(
                    label: "R ↔ C", value: bandBinding(0), range: -1...1,
                    defaultValue: 0, colors: Self.rcColors,
                    help: "Balance red ↔ cyan in the selected tonal region (enlarger filtration).")
                GradientSlider(
                    label: "G ↔ M", value: bandBinding(1), range: -1...1,
                    defaultValue: 0, colors: Self.gmColors,
                    help: "Balance green ↔ magenta in the selected tonal region (enlarger filtration).")
                GradientSlider(
                    label: "B ↔ Y", value: bandBinding(2), range: -1...1,
                    defaultValue: 0, colors: Self.byColors,
                    help: "Balance blue ↔ yellow in the selected tonal region (enlarger filtration).")
            }
            .padding(6)
        }
    }

    private func bandBinding(_ component: Int) -> Binding<Double> {
        let keyPath = band.keyPath
        return Binding(
            get: { model.settings[keyPath: keyPath][component] },
            set: { model.settings[keyPath: keyPath][component] = $0 })
    }
}

/// Slider with a gradient capsule track (NLP-style), plus the standard label /
/// value / reset-x affordances of LabeledSlider.
struct GradientSlider: View {
    let label: String
    @Binding var value: Double
    let range: ClosedRange<Double>
    let defaultValue: Double
    let colors: [Color]
    /// Hover tooltip describing what the control does.
    var help: String = ""
    @Environment(\.controlEditingChanged) private var controlEditingChanged
    @State private var isDragging = false

    private var isChanged: Bool { abs(value - defaultValue) > 1e-9 }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                Text(label).font(.caption)
                if isChanged {
                    Button {
                        value = defaultValue
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(.system(size: 9))
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.borderless)
                    .help("Reset \(label) to default")
                }
                Spacer()
                Text(String(format: "%.2f", value))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(isChanged ? .primary : .secondary)
            }
            GeometryReader { geo in
                let fraction = (value - range.lowerBound) / (range.upperBound - range.lowerBound)
                let knobX = CGFloat(fraction) * (geo.size.width - 14) + 7
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(LinearGradient(colors: colors, startPoint: .leading, endPoint: .trailing))
                        .frame(height: 8)
                        .frame(maxHeight: .infinity, alignment: .center)
                    Circle()
                        .fill(.white)
                        .frame(width: 14, height: 14)
                        .shadow(radius: 1, y: 0.5)
                        .position(x: knobX, y: geo.size.height / 2)
                }
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { g in
                            if !isDragging {
                                isDragging = true
                                controlEditingChanged(true)
                            }
                            let f = min(max((g.location.x - 7) / (geo.size.width - 14), 0), 1)
                            value = range.lowerBound + Double(f) * (range.upperBound - range.lowerBound)
                        }
                        .onEnded { _ in
                            isDragging = false
                            controlEditingChanged(false)
                        }
                )
                .onTapGesture(count: 2) { value = defaultValue }
            }
            .frame(height: 16)
        }
        .help(help)
    }
}
