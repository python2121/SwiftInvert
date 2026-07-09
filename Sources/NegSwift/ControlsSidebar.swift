import NegativeKit
import SwiftUI

struct ControlsSidebar: View {
    @Bindable var model: AppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    Text("Adjustments").font(.headline)
                    Spacer()
                    Button("Reset All") { model.resetSettings() }
                        .controlSize(.small)
                        .help("Reset every slider and toggle to its default (keeps crops)")
                }

                HistogramView(bins: model.histogram)

                GroupBox("Pre-process") {
                    VStack(alignment: .leading, spacing: 8) {
                        toolRow(
                            "Crop for Analysis", mode: .analysisRegion,
                            isSet: model.settings.analysisRect != nil,
                            clear: { model.settings.analysisRect = nil })
                        toolRow(
                            "Crop", mode: .crop,
                            isSet: model.settings.cropRect != nil,
                            clear: { model.settings.cropRect = nil })
                        if model.toolMode != .none {
                            Text("Drag on the image to select the area.")
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .padding(6)
                }

                GroupBox("Print") {
                    VStack(alignment: .leading, spacing: 10) {
                        Toggle("Auto exposure", isOn: $model.settings.autoExposure)
                        // Right = brighter (all brightness sliders share that
                        // direction). Internally NegPy's print density: 2 − b.
                        LabeledSlider(
                            label: "Brightness", value: brightnessBinding, range: -3...4,
                            format: "%.2f", defaultValue: 1.0)
                        Toggle("Auto contrast", isOn: $model.settings.autoNormalizeContrast)
                        LabeledSlider(
                            label: "Grade (ISO R)", value: $model.settings.grade, range: 50...180,
                            format: "%.0f", defaultValue: 115)
                    }
                    .padding(6)
                }

                GroupBox("Exposure") {
                    VStack(alignment: .leading, spacing: 10) {
                        LabeledSlider(
                            label: "Exposure (stops)", value: $model.settings.exposureStops,
                            range: -2...2, format: "%+.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Contrast", value: $model.settings.overallContrast,
                            range: -1...2, format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Shadows", value: $model.settings.shadows, range: -1...1,
                            format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Shadow contrast", value: $model.settings.shadowContrast,
                            range: -1...2, format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Highlights", value: $model.settings.highlights, range: -1...1,
                            format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Highlight contrast", value: $model.settings.highlightContrast,
                            range: -1...1, format: "%.2f", defaultValue: 0)
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

                Menu("Export") {
                    ForEach(ExportFormat.allCases) { format in
                        Button(format.label) { model.export(format: format) }
                    }
                }
                .disabled(model.isExporting || model.selection == nil)

                if let status = model.statusMessage {
                    Text(status).font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(12)
        }
        .frame(width: 195)
    }

    /// Brightness ↔ density inversion so dragging right brightens the print.
    private var brightnessBinding: Binding<Double> {
        Binding(
            get: { 2 - model.settings.density },
            set: { model.settings.density = 2 - $0 })
    }

    /// One pre-process tool row: activate-tool button + clear button when set.
    @ViewBuilder
    private func toolRow(
        _ label: String, mode: AppModel.ToolMode, isSet: Bool, clear: @escaping () -> Void
    ) -> some View {
        HStack(spacing: 6) {
            Button {
                model.toolMode = model.toolMode == mode ? .none : mode
            } label: {
                Label(label, systemImage: mode == .crop ? "crop" : "viewfinder.rectangular")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)
            .tint(model.toolMode == mode ? Color.accentColor : nil)
            if isSet {
                Button {
                    clear()
                } label: {
                    Image(systemName: "xmark.circle.fill")
                }
                .buttonStyle(.borderless)
                .help("Clear \(label.lowercased())")
            }
        }
    }
}

struct LabeledSlider: View {
    let label: String
    @Binding var value: Double
    let range: ClosedRange<Double>
    let format: String
    let defaultValue: Double

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
                    .help("Reset \(label.lowercased()) to default")
                }
                Spacer()
                Text(String(format: format, value))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(isChanged ? .primary : .secondary)
            }
            Slider(value: $value, in: range)
                .controlSize(.small)
                .onTapGesture(count: 2) { value = defaultValue }
        }
    }
}
