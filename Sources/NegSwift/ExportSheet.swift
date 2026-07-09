import SwiftUI

/// Quality modal shared by the sidebar Export button and the library's
/// batch context menu. Defaults are high quality; last-used options stick.
struct ExportSheet: View {
    let request: AppModel.ExportRequest
    @Bindable var model: AppModel
    @State private var options: ExportOptions

    init(request: AppModel.ExportRequest, model: AppModel) {
        self.request = request
        self.model = model
        _options = State(initialValue: model.exportOptions)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(request.urls.count == 1
                ? "Export “\(request.urls[0].deletingPathExtension().lastPathComponent)”"
                : "Export \(request.urls.count) images")
                .font(.headline)

            Picker("Format", selection: $options.format) {
                ForEach(ExportFormat.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.segmented)

            Picker("Color space", selection: $options.colorSpace) {
                ForEach(ExportColorSpace.allCases) { Text($0.label).tag($0) }
            }

            if options.format == .jpeg {
                HStack {
                    Text("Quality")
                    Slider(value: $options.jpegQuality, in: 0.5...1.0)
                    Text(String(format: "%.0f%%", options.jpegQuality * 100))
                        .font(.caption.monospacedDigit())
                        .frame(width: 40, alignment: .trailing)
                }
            }

            Toggle("Resize to fit long edge", isOn: $options.resize)
            if options.resize {
                HStack {
                    Text("Long edge")
                    TextField(
                        "px", value: $options.maxLongEdge, format: .number.grouping(.never))
                        .frame(width: 70)
                    Text("px").foregroundStyle(.secondary)
                    Stepper("", value: $options.maxLongEdge, in: 256...20000, step: 500)
                        .labelsHidden()
                }
            }

            Divider()

            HStack {
                Text("Destination")
                Spacer()
                Picker("", selection: $options.useCustomDestination) {
                    Text("Next to originals").tag(false)
                    Text("Folder…").tag(true)
                }
                .labelsHidden()
                .fixedSize()
            }
            if options.useCustomDestination {
                HStack {
                    Text(options.customDestinationPath.map { URL(fileURLWithPath: $0).lastPathComponent } ?? "No folder chosen")
                        .font(.caption)
                        .foregroundStyle(options.customDestinationPath == nil ? .red : .secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .help(options.customDestinationPath ?? "")
                    Spacer()
                    Button("Choose…") { chooseDestination() }
                }
            }

            Text("Existing exports of the same image are overwritten; the chosen profile is embedded.")
                .font(.caption)
                .foregroundStyle(.secondary)

            HStack {
                Spacer()
                Button("Cancel") { model.exportRequest = nil }
                    .keyboardShortcut(.cancelAction)
                Button("Export") { model.performExport(urls: request.urls, options: options) }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
                    .disabled(options.useCustomDestination && options.customDestinationPath == nil)
            }
        }
        .padding(20)
        .frame(width: 380)
    }

    private func chooseDestination() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.message = "Choose the export destination folder"
        if panel.runModal() == .OK, let url = panel.url {
            options.customDestinationPath = url.path
        }
    }
}
