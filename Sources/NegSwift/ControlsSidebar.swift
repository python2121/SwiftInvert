import NegativeKit
import SwiftUI

struct ControlsSidebar: View {
    @Bindable var model: AppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HistogramView(bins: model.histogram)

                GroupBox("Exposure") {
                    VStack(alignment: .leading, spacing: 10) {
                        Toggle("Auto exposure", isOn: $model.settings.autoExposure)
                        LabeledSlider(
                            label: "Density", value: $model.settings.density, range: -3...4,
                            format: "%.2f", defaultValue: 1.0)
                        Toggle("Auto contrast", isOn: $model.settings.autoNormalizeContrast)
                        LabeledSlider(
                            label: "Grade (ISO R)", value: $model.settings.grade, range: 50...180,
                            format: "%.0f", defaultValue: 115)
                    }
                    .padding(6)
                }

                GroupBox("Color") {
                    VStack(alignment: .leading, spacing: 10) {
                        LabeledSlider(
                            label: "Cyan", value: $model.settings.wbCyan, range: -1...1,
                            format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Magenta", value: $model.settings.wbMagenta, range: -1...1,
                            format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Yellow", value: $model.settings.wbYellow, range: -1...1,
                            format: "%.2f", defaultValue: 0)
                        Divider()
                        Toggle("Auto cast removal", isOn: $model.settings.autoCastRemoval)
                        LabeledSlider(
                            label: "Cast strength", value: $model.settings.castRemovalStrength,
                            range: 0...1, format: "%.2f", defaultValue: 0.5)
                    }
                    .padding(6)
                }

                GroupBox("Tone") {
                    VStack(alignment: .leading, spacing: 10) {
                        LabeledSlider(
                            label: "Toe", value: $model.settings.toe, range: -1...1,
                            format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Shoulder", value: $model.settings.shoulder, range: -1...1,
                            format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "White point", value: $model.settings.whitePointOffset,
                            range: -0.3...0.3, format: "%.3f", defaultValue: 0)
                        LabeledSlider(
                            label: "Black point", value: $model.settings.blackPointOffset,
                            range: -0.3...0.3, format: "%.3f", defaultValue: 0)
                    }
                    .padding(6)
                }

                HStack {
                    Button("Reset") { model.resetSettings() }
                    Spacer()
                    Menu("Export") {
                        ForEach(ExportFormat.allCases) { format in
                            Button(format.label) { model.export(format: format) }
                        }
                    }
                    .disabled(model.isExporting || model.selection == nil)
                    .fixedSize()
                }

                if let status = model.statusMessage {
                    Text(status).font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(12)
        }
        .frame(minWidth: 260, idealWidth: 300)
    }
}

struct LabeledSlider: View {
    let label: String
    @Binding var value: Double
    let range: ClosedRange<Double>
    let format: String
    let defaultValue: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(label).font(.caption)
                Spacer()
                Text(String(format: format, value))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            Slider(value: $value, in: range)
                .controlSize(.small)
                .onTapGesture(count: 2) { value = defaultValue }
        }
    }
}
