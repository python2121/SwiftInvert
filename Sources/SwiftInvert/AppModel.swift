import AppKit
import Foundation
import MetalRenderKit
import NegativeKit
import Observation
import RawDecodeKit
import SwiftUI
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
    /// Library multi-selection (⌘-click). Always contains `selection` when set.
    var multiSelection: Set<URL> = []

    func select(_ url: URL, additive: Bool) {
        if additive {
            if multiSelection.contains(url) && multiSelection.count > 1 {
                multiSelection.remove(url)
                if selection == url { selection = multiSelection.first }
            } else {
                multiSelection.insert(url)
                selection = url
            }
        } else {
            multiSelection = [url]
            selection = url
        }
    }

    /// Shift-click: select the contiguous range from the current image to the
    /// clicked one (replacing the multi-selection, Finder-style).
    func selectRange(to url: URL) {
        guard let anchor = selection,
            let a = files.firstIndex(of: anchor),
            let b = files.firstIndex(of: url)
        else { return select(url, additive: false) }
        multiSelection = Set(files[min(a, b)...max(a, b)])
        selection = url
    }

    var settings = ExposureSettings() {
        didSet { if oldValue != settings { settingsChanged() } }
    }
    var displayImage: CGImage?
    var histogram: [UInt32]?
    var statusMessage: String?
    var isExporting = false
    /// True while the session is decoding or re-running the base analysis
    /// (drives the bottom-left indicator; plain slider renders don't set it).
    var isAnalyzing = false

    /// Active pre-process selection tool. While a tool is on, the detail view
    /// shows the uncropped frame and drag draws the selection rect.
    enum ToolMode { case none, analysisRegion, crop }
    var toolMode: ToolMode = .none {
        didSet { if oldValue != toolMode { scheduleRender() } }
    }

    // MARK: - Orientation & canvas

    func rotateClockwise() { settings.rotation = ((settings.rotation + 90) % 360 + 360) % 360 }
    func rotateCounterclockwise() { settings.rotation = ((settings.rotation - 90) % 360 + 360) % 360 }
    func flipHorizontal() { settings.flipHorizontal.toggle() }

    enum CanvasColor: String, CaseIterable, Identifiable {
        case gray, veryDarkGray, black
        var id: String { rawValue }
        var color: Color {
            switch self {
            case .gray: return Color(white: 0.5)
            case .veryDarkGray: return Color(white: 0.12)
            case .black: return .black
            }
        }
        var label: String {
            switch self {
            case .gray: return "Gray"
            case .veryDarkGray: return "Very dark gray"
            case .black: return "Black"
            }
        }
    }
    var canvasColor: CanvasColor = CanvasColor(
        rawValue: UserDefaults.standard.string(forKey: "canvasColor") ?? "") ?? .veryDarkGray
    {
        didSet { UserDefaults.standard.set(canvasColor.rawValue, forKey: "canvasColor") }
    }

    func commitSelection(_ rect: NormalizedRect) {
        switch toolMode {
        case .analysisRegion: settings.analysisRect = rect
        case .crop: settings.cropRect = rect
        case .none: break
        }
        toolMode = .none  // settings didSet already re-renders (and re-analyzes)
    }

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
        // One-time migration from the pre-rename defaults domain ("NegSwift"):
        // unbundled binaries key their preferences by process name.
        if UserDefaults.standard.object(forKey: "libraryFolder") == nil,
            let legacy = UserDefaults(suiteName: "NegSwift")
        {
            for key in ["libraryFolder", "canvasColor", "exportOptions"] {
                if let value = legacy.object(forKey: key) {
                    UserDefaults.standard.set(value, forKey: key)
                }
            }
        }
        // The pipeline must exist before the folder restore: scanFolder sets the
        // selection, whose didSet immediately kicks the first render.
        do {
            pipeline = try RenderPipeline()
        } catch {
            statusMessage = "Metal unavailable: \(error)"
            NSLog("SwiftInvert: RenderPipeline init failed: \(error)")
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
        toolMode = .none
        showingBaseline = false
        displayImage = nil
        histogram = nil
        if let url = selection, !multiSelection.contains(url) { multiSelection = [url] }
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
                let snapshot = self.showingBaseline ? self.baselineSettings() : self.settings
                let uncropped = self.toolMode != .none
                guard let session = self.session else { break }
                do {
                    if await session.needsPreparation(settings: snapshot) {
                        self.isAnalyzing = true
                    }
                    let output = try await session.render(settings: snapshot, uncropped: uncropped)
                    self.isAnalyzing = false
                    if Task.isCancelled { break }
                    self.displayImage = output.image
                    self.histogram = output.histogram
                    self.statusMessage = nil
                    NSLog("SwiftInvert: rendered \(self.selection?.lastPathComponent ?? "?") (\(output.image.width)x\(output.image.height))")
                } catch {
                    self.isAnalyzing = false
                    if !Task.isCancelled {
                        self.statusMessage = "Render failed: \(error)"
                        NSLog("SwiftInvert: render failed for \(self.selection?.lastPathComponent ?? "?"): \(error)")
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

    /// Reset every slider/toggle to defaults; the pre-process rects are
    /// geometry, not adjustments, and survive the reset.
    func resetSettings() {
        var fresh = ExposureSettings()
        fresh.analysisRect = settings.analysisRect
        fresh.cropRect = settings.cropRect
        settings = fresh
    }

    // MARK: - Baseline (press-and-hold "before") preview

    /// True while the long-press comparison shows the stock conversion.
    var showingBaseline = false

    /// The "where you came from" settings: stock adjustments with the current
    /// geometry (crop/analysis region/orientation) so the comparison aligns.
    private func baselineSettings() -> ExposureSettings {
        var base = ExposureSettings()
        base.analysisRect = settings.analysisRect
        base.cropRect = settings.cropRect
        base.rotation = settings.rotation
        base.flipHorizontal = settings.flipHorizontal
        return base
    }

    func setBaselinePreview(_ on: Bool) {
        guard on != showingBaseline else { return }
        showingBaseline = on
        scheduleRender()
    }

    // MARK: - Export

    /// A pending export request drives the quality modal.
    struct ExportRequest: Identifiable {
        let id = UUID()
        let urls: [URL]
    }
    var exportRequest: ExportRequest?
    var exportOptions = ExportOptions.loadSticky()

    struct ExportProgress {
        var done: Int
        var total: Int
        var currentName: String
    }
    var exportProgress: ExportProgress?
    private var exportTask: Task<Void, Never>?

    func cancelExport() {
        exportTask?.cancel()
    }

    /// Open the quality modal for the current image (sidebar button).
    func requestExportCurrent() {
        guard let selection else { return }
        exportRequest = ExportRequest(urls: [selection])
    }

    /// Open the quality modal for the library multi-selection (context menu).
    func requestExportSelected() {
        let urls = files.filter { multiSelection.contains($0) }
        guard !urls.isEmpty else { return }
        exportRequest = ExportRequest(urls: urls)
    }

    /// Sequential batch export with the chosen options. Uses live settings for
    /// the open image and each file's sidecar (or defaults) otherwise.
    func performExport(urls: [URL], options: ExportOptions) {
        guard let pipeline, !isExporting else { return }
        exportOptions = options
        options.saveSticky()
        exportRequest = nil
        // Flush the debounced sidecar so the open image exports what's on screen.
        if let selection { SidecarStore.save(settings, for: selection) }

        isExporting = true
        let liveURL = selection
        let liveSettings = settings
        let liveSession = session
        exportTask = Task {
            var failures = 0
            var completed = 0
            for (index, url) in urls.enumerated() {
                if Task.isCancelled { break }
                exportProgress = ExportProgress(
                    done: index, total: urls.count, currentName: url.lastPathComponent)
                let fileSettings = url == liveURL ? liveSettings : (SidecarStore.load(for: url) ?? ExposureSettings())
                let session = url == liveURL && liveSession != nil
                    ? liveSession! : ImageSession(url: url, pipeline: pipeline)
                do {
                    let encoded = try await session.exportRender(settings: fileSettings)
                    if Task.isCancelled { break }
                    try Exporter.write(encoded, to: options.destinationURL(for: url), options: options)
                    completed += 1
                } catch {
                    failures += 1
                    NSLog("SwiftInvert: export failed for \(url.lastPathComponent): \(error)")
                }
            }
            if Task.isCancelled {
                statusMessage = "Export cancelled (\(completed) of \(urls.count) done)"
            } else {
                statusMessage = failures == 0
                    ? "Exported \(urls.count) image\(urls.count == 1 ? "" : "s")"
                    : "Exported \(urls.count - failures) of \(urls.count) (\(failures) failed)"
            }
            exportProgress = nil
            isExporting = false
            exportTask = nil
        }
    }
}
