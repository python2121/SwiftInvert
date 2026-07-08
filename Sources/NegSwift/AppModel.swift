import AppKit
import Foundation
import MetalRenderKit
import NegativeKit
import Observation
import RawDecodeKit
import UniformTypeIdentifiers

@MainActor
@Observable
final class AppModel {
    var folderURL: URL? {
        didSet { UserDefaults.standard.set(folderURL?.path, forKey: "libraryFolder") }
    }
    var files: [URL] = []
    var selection: URL? {
        didSet { if oldValue != selection { openSelection() } }
    }

    var settings = ExposureSettings() {
        didSet { if oldValue != settings { settingsChanged() } }
    }
    var displayImage: CGImage?
    var histogram: [UInt32]?
    var statusMessage: String?
    var isExporting = false

    let thumbnails = ThumbnailStore()
    private var pipeline: RenderPipeline?
    private var session: ImageSession?
    private var renderTask: Task<Void, Never>?
    private var renderPending = false
    private var saveTask: Task<Void, Never>?
    /// Set while restoring settings from a sidecar so opening a file doesn't
    /// immediately write one back (saves happen only on real user edits).
    private var isRestoringSettings = false

    init() {
        // The pipeline must exist before the folder restore: scanFolder sets the
        // selection, whose didSet immediately kicks the first render.
        do {
            pipeline = try RenderPipeline()
        } catch {
            statusMessage = "Metal unavailable: \(error)"
            NSLog("NegSwift: RenderPipeline init failed: \(error)")
        }
        if let path = UserDefaults.standard.string(forKey: "libraryFolder") {
            let url = URL(fileURLWithPath: path)
            if FileManager.default.fileExists(atPath: url.path) {
                folderURL = url
                scanFolder()
            }
        }
    }

    func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.message = "Choose a folder of camera-scanned negatives"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        folderURL = url
        Task { await thumbnails.clear() }
        scanFolder()
    }

    func scanFolder() {
        guard let folderURL else { return }
        let contents = (try? FileManager.default.contentsOfDirectory(
            at: folderURL, includingPropertiesForKeys: nil, options: .skipsHiddenFiles)) ?? []
        files = contents.filter { RawDecoder.isRawFile($0) }
            .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
        if let selection, !files.contains(selection) { self.selection = files.first }
        if selection == nil { selection = files.first }
    }

    private func openSelection() {
        renderTask?.cancel()
        displayImage = nil
        histogram = nil
        guard let url = selection else { return }
        guard let pipeline else {
            statusMessage = "Cannot render: Metal pipeline unavailable."
            return
        }
        session = ImageSession(url: url, pipeline: pipeline)
        // Loading the sidecar mutates settings, which triggers the first render.
        let restored = SidecarStore.load(for: url) ?? ExposureSettings()
        isRestoringSettings = true
        if restored == settings {
            settingsChanged()  // no mutation → kick the render explicitly
        } else {
            settings = restored
        }
        isRestoringSettings = false
    }

    private func settingsChanged() {
        scheduleRender()
        if !isRestoringSettings { scheduleSave() }
    }

    /// Latest-wins render coalescing: one render in flight; a change during a
    /// render marks it pending and re-renders once with the newest settings.
    private func scheduleRender() {
        guard session != nil else { return }
        if renderTask != nil {
            renderPending = true
            return
        }
        renderTask = Task { [weak self] in
            guard let self else { return }
            repeat {
                self.renderPending = false
                let snapshot = self.settings
                guard let session = self.session else { break }
                do {
                    let output = try await session.render(settings: snapshot)
                    if Task.isCancelled { break }
                    self.displayImage = output.image
                    self.histogram = output.histogram
                    self.statusMessage = nil
                    NSLog("NegSwift: rendered \(self.selection?.lastPathComponent ?? "?") (\(output.image.width)x\(output.image.height))")
                } catch {
                    if !Task.isCancelled {
                        self.statusMessage = "Render failed: \(error)"
                        NSLog("NegSwift: render failed for \(self.selection?.lastPathComponent ?? "?"): \(error)")
                    }
                    break
                }
            } while self.renderPending && !Task.isCancelled
            self.renderTask = nil
            if self.renderPending { self.scheduleRender() }
        }
    }

    private func scheduleSave() {
        guard let url = selection else { return }
        let snapshot = settings
        saveTask?.cancel()
        saveTask = Task {
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled else { return }
            SidecarStore.save(snapshot, for: url)
        }
    }

    func resetSettings() {
        settings = ExposureSettings()
    }

    // MARK: - Export

    func export(format: ExportFormat) {
        guard let session, let url = selection else { return }
        let snapshot = settings
        isExporting = true
        statusMessage = "Exporting…"
        Task {
            do {
                let encoded = try await session.exportRender(settings: snapshot)
                let dest = url.deletingPathExtension().appendingPathExtension(format.fileExtension)
                try Exporter.write(encoded, to: dest, format: format)
                statusMessage = "Exported \(dest.lastPathComponent)"
            } catch {
                statusMessage = "Export failed: \(error)"
            }
            isExporting = false
        }
    }
}
