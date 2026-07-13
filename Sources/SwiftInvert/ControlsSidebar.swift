import NegativeKit
import SwiftUI

struct ControlsSidebar: View {
    @Bindable var model: AppModel

    @AppStorage("adjustmentsCollapsed") private var adjustmentsCollapsed = false
    @AppStorage("cropRotationCollapsed") private var cropRotationCollapsed = false
    @AppStorage("historyCollapsed") private var historyCollapsed = false
    @AppStorage("historyHeight") private var historyHeight = 150.0

    var body: some View {
        // GeometryReader + top alignment: stored section heights are clamped to
        // what actually fits, and any residual overflow clips at the BOTTOM —
        // the header can never pan off the top (unclamped overflow in a VStack
        // is centered by SwiftUI, which pushed the top off-screen).
        GeometryReader { geo in
            let historyFit = fitHistory(available: geo.size.height)
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 5) {
                    Button {
                        adjustmentsCollapsed.toggle()
                    } label: {
                        HStack(spacing: 5) {
                            Image(systemName: "chevron.right")
                                .font(.system(size: 9, weight: .semibold))
                                .rotationEffect(.degrees(adjustmentsCollapsed ? 0 : 90))
                                .foregroundStyle(.secondary)
                            Text("Adjustments").font(.headline)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    Spacer()
                    Button("Reset All") { model.resetSettings() }
                        .controlSize(.small)
                        .help("Reset every slider and toggle to its default (keeps crops)")
                }
                .padding(.horizontal, 12)
                .padding(.top, 12)

                // Adjustments is the flexible section: its ScrollView absorbs
                // all slack, so C&R (intrinsic) and History (stored height)
                // pack tightly against the bottom with no dead gaps. Both
                // handles around C&R drag the same boundary — C&R keeps its
                // height, so resizing means resizing the History list.
                if !adjustmentsCollapsed {
                    VStack(alignment: .leading, spacing: 10) {
                        // Pinned: histogram stays visible while controls scroll.
                        HistogramView(model: model)
                            .padding(.horizontal, 12)
                        scrollingControls
                    }
                    .frame(minHeight: 210, maxHeight: .infinity)
                    if historyCollapsed {
                        Divider()
                    } else {
                        SectionResizeHandle(
                            height: $historyHeight, range: 40...600, sectionIsBelow: true)
                    }
                } else {
                    Divider()
                    // Collapsed: nothing flexible above, so the bottom group
                    // sinks to the bottom edge.
                    Spacer(minLength: 0)
                }

                CropRotationSection(model: model)
                if historyCollapsed {
                    Divider()
                } else {
                    // The handle doubles as the separator (it draws a divider).
                    SectionResizeHandle(
                        height: $historyHeight, range: 40...600, sectionIsBelow: true)
                }
                HistoryPanel(model: model, listHeight: historyFit)
            }
            // Matches the inter-section gap (8 section pad + 4 stack
            // spacing) so a collapsed History header sits as far off the
            // bottom edge as collapsed C&R sits off its divider.
            .padding(.bottom, 4)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
        .frame(width: 235)
        .animation(.easeOut(duration: 0.12), value: adjustmentsCollapsed)
    }

    /// History's effective open height: the stored preference (opens at 150,
    /// draggable down to 40 via either handle), clamped so everything above
    /// still fits — C&R at its intrinsic height plus the Adjustments floor
    /// (210, the pinned histogram: squeezing the section below its fixed
    /// content leaves invisible clipped overflow that eats clicks — .clipped()
    /// is visual only, it does not clip hit-testing).
    private func fitHistory(available: CGFloat) -> CGFloat {
        let cropH: CGFloat = cropRotationCollapsed ? 40 : 175
        let above: CGFloat = adjustmentsCollapsed ? 130 : 370
        return min(max(historyHeight, 40), max(available - above - cropH, 40))
    }

    private var scrollingControls: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                GroupBox("Pre-process") {
                    VStack(alignment: .leading, spacing: 8) {
                        toolRow(
                            "Crop for Analysis", mode: .analysisRegion,
                            isSet: model.settings.analysisRect != nil,
                            clear: { model.settings.analysisRect = nil })
                        if model.toolMode != .none {
                            Text("Drag on the image to select the area.")
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                        }
                        HoldButton(
                            label: "View Original", systemImage: "eye",
                            isEnabled: model.selection != nil
                        ) { pressing in
                            model.setBaselinePreview(pressing)
                        }
                        .help("Hold to compare against the stock conversion")
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
                            label: "Shadows", value: $model.settings.shadows, range: -2...2,
                            format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Shadow contrast", value: $model.settings.shadowContrast,
                            range: -3...6, format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Dark shadows", value: $model.settings.darkShadows,
                            range: -2...2, format: "%.2f", defaultValue: 0)
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
                        // C/M/Y trim sliders hidden for now (fields remain in
                        // settings/sidecars; Temp/Tint below are the WB controls).
                        LabeledSlider(
                            label: "Pre-saturation", value: $model.settings.preSaturation,
                            range: 0.5...2.0, format: "%.2f", defaultValue: 1.15)
                        // >1 overcorrects past the measured neutral axis; the
                        // kernel's cast clamps bound it at any strength.
                        LabeledSlider(
                            label: "Cast strength", value: $model.settings.castRemovalStrength,
                            range: 0...2, format: "%.2f", defaultValue: 0.5)
                        Divider()
                        LabeledSlider(
                            label: "Vibrance", value: $model.settings.vibrance,
                            range: 0...2, format: "%.2f", defaultValue: 1.0)
                        LabeledSlider(
                            label: "Saturation", value: $model.settings.saturation,
                            range: 0...2, format: "%.2f", defaultValue: 1.0)
                    }
                    .padding(6)
                }

                ColorGradingSection(model: model)

                GroupBox("Tone") {
                    VStack(alignment: .leading, spacing: 10) {
                        LabeledSlider(
                            label: "Toe", value: $model.settings.toe, range: -1...1,
                            format: "%.2f", defaultValue: 0)
                        LabeledSlider(
                            label: "Shoulder", value: $model.settings.shoulder, range: -1...1,
                            format: "%.2f", defaultValue: 0)
                        Toggle("True black", isOn: $model.settings.trueBlack)
                            .help("Map paper Dmax to display black (black point compensation)")
                        LabeledSlider(
                            label: "White point", value: $model.settings.whitePointOffset,
                            range: -0.3...0.3, format: "%.3f", defaultValue: 0)
                        LabeledSlider(
                            label: "Black point", value: $model.settings.blackPointOffset,
                            range: -0.3...0.3, format: "%.3f", defaultValue: 0)
                    }
                    .padding(6)
                }

                Button("Export…") { model.requestExportCurrent() }
                    .disabled(model.isExporting || model.selection == nil)

                if let status = model.statusMessage {
                    Text(status).font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(12)
        }
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


/// Button that acts while pressed and held (standard buttons fire on release):
/// a ButtonStyle exposes isPressed, which we forward as press/release events.
struct HoldButton: View {
    let label: String
    let systemImage: String
    var isEnabled: Bool = true
    let onPressingChanged: (Bool) -> Void

    var body: some View {
        Button {} label: {
            Label(label, systemImage: systemImage)
                .frame(maxWidth: .infinity)
        }
        .buttonStyle(HoldButtonStyle(onPressingChanged: onPressingChanged))
        .disabled(!isEnabled)
    }
}

private struct HoldButtonStyle: ButtonStyle {
    let onPressingChanged: (Bool) -> Void

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .padding(.vertical, 3)
            .padding(.horizontal, 8)
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(configuration.isPressed ? Color.accentColor.opacity(0.25) : Color.primary.opacity(0.06)))
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .strokeBorder(Color.primary.opacity(0.15), lineWidth: 1))
            .onChange(of: configuration.isPressed) { _, pressed in
                onPressingChanged(pressed)
            }
    }
}
