import NegativeKit
import SwiftUI

/// Chroma-gated color mixer (LabColor.applyColorMixer): per-band Hue and
/// Saturation on gradient tracks. Only pixels saturated enough to read as
/// the band's color move; the neutral axis (whites, grays, faint casts) is
/// untouched by construction — the complement of the WB sliders.
struct ColorMixerSection: View {
    @Bindable var model: AppModel

    enum Band: String, CaseIterable, Identifiable {
        case red = "Red"
        case yellow = "Yellow"
        case green = "Green"
        case blue = "Blue"
        var id: String { rawValue }

        var hueKeyPath: WritableKeyPath<ExposureSettings, Double> {
            switch self {
            case .red: return \.redHue
            case .yellow: return \.yellowHue
            case .green: return \.greenHue
            case .blue: return \.blueHue
            }
        }
        var saturationKeyPath: WritableKeyPath<ExposureSettings, Double> {
            switch self {
            case .red: return \.redSaturation
            case .yellow: return \.yellowSaturation
            case .green: return \.greenSaturation
            case .blue: return \.blueSaturation
            }
        }

        /// Track colors: [− direction, band color, + direction] — the actual
        /// hue destinations (+ rotates ccw in Lab: red→orange, yellow→green,
        /// green→teal, blue→purple), matching LabColor.bandCentersDeg order.
        var hueColors: [Color] {
            switch self {
            case .red:
                return [
                    Color(red: 0.85, green: 0.2, blue: 0.5),
                    Color(red: 0.85, green: 0.2, blue: 0.2),
                    Color(red: 0.9, green: 0.55, blue: 0.15),
                ]
            case .yellow:
                return [
                    Color(red: 0.9, green: 0.55, blue: 0.15),
                    Color(red: 0.88, green: 0.8, blue: 0.2),
                    Color(red: 0.6, green: 0.82, blue: 0.25),
                ]
            case .green:
                return [
                    Color(red: 0.6, green: 0.82, blue: 0.25),
                    Color(red: 0.25, green: 0.72, blue: 0.3),
                    Color(red: 0.15, green: 0.72, blue: 0.6),
                ]
            case .blue:
                return [
                    Color(red: 0.15, green: 0.68, blue: 0.8),
                    Color(red: 0.25, green: 0.45, blue: 0.88),
                    Color(red: 0.55, green: 0.3, blue: 0.85),
                ]
            }
        }
        /// Saturation track: gray → the band color.
        var saturationColors: [Color] { [Color(white: 0.4), hueColors[1]] }
    }

    @State private var band: Band = .red

    var body: some View {
        GroupBox("Color Mixer") {
            VStack(alignment: .leading, spacing: 10) {
                Picker("", selection: $band) {
                    ForEach(Band.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()

                GradientSlider(
                    label: "Hue", value: binding(band.hueKeyPath), range: -1.5...1.5,
                    defaultValue: 0, colors: band.hueColors)
                GradientSlider(
                    label: "Saturation", value: binding(band.saturationKeyPath), range: 0...2,
                    defaultValue: 1.0, colors: band.saturationColors)
            }
            .padding(6)
        }
    }

    private func binding(_ keyPath: WritableKeyPath<ExposureSettings, Double>) -> Binding<Double> {
        Binding(
            get: { model.settings[keyPath: keyPath] },
            set: { model.settings[keyPath: keyPath] = $0 })
    }
}
