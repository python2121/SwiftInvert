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
    /// Flat, depth-first file list (drives selection ranges and index numbers).
    var files: [URL] = []
    /// 1-based index badges, in `files` order.
    var fileIndex: [URL: Int] = [:]
    /// Folder hierarchy under `folderURL` (folders without RAWs pruned).
    var folderTree: FolderNode?
    var collapsedFolders: Set<URL> = []
    var isScanning = false

    struct FolderNode: Identifiable, Sendable {
        let id: URL
        let name: String
        var files: [URL]
        var subfolders: [FolderNode]
        var totalCount: Int
    }

    func toggleCollapsed(_ url: URL) {
        if collapsedFolders.contains(url) {
            collapsedFolders.remove(url)
        } else {
            collapsedFolders.insert(url)
        }
    }
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

    /// Transient Straighten drag value: while non-nil the detail view previews
    /// the rotation as a display transform (no settings mutation); commit
    /// happens once on release. Starting a drag on an already-rotated image
    /// kicks ONE re-render at fineRotation 0: the display transform can only
    /// zoom IN (the baked image lacks pixels beyond its inscribed crop), so
    /// rotating back toward zero from a baked angle would wrongly zoom in
    /// too — from a 0° base the cover scale is correct in both directions.
    var straightenDragValue: Double? {
        didSet {
            if oldValue == nil, straightenDragValue != nil, displayedFineRotation != 0 {
                scheduleRender()
            }
        }
    }
    /// The fineRotation the current displayImage was BAKED with. The detail
    /// view rotates the display by (target − baked), so the preview holds
    /// through the post-release re-bake instead of snapping back.
    var displayedFineRotation: Double = 0

    func rotateClockwise() {
        pendingHistoryLabel = "Rotate 90° CW"
        settings.rotation = ((settings.rotation + 90) % 360 + 360) % 360 }
    func rotateCounterclockwise() {
        pendingHistoryLabel = "Rotate 90° CCW"
        settings.rotation = ((settings.rotation - 90) % 360 + 360) % 360 }
    func flipHorizontal() {
        pendingHistoryLabel = "Flip horizontal"
        settings.flipHorizontal.toggle()
    }

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
        case .analysisRegion:
            pendingHistoryLabel = "Analysis region"
            settings.analysisRect = rect
        case .crop:
            pendingHistoryLabel = "Crop"
            settings.cropRect = rect
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

    // MARK: - Edit history (undo/redo)

    struct HistoryEntry: Identifiable, Sendable {
        let id = UUID()
        let label: String
        let settings: ExposureSettings
    }
    private struct HistoryState {
        var entries: [HistoryEntry]
        var index: Int
    }
    /// Per-image histories, kept for the session.
    private var histories: [URL: HistoryState] = [:]
    private var historyURL: URL?
    var historyEntries: [HistoryEntry] = []
    var historyIndex = 0
    private var historyCommitTask: Task<Void, Never>?
    private var isNavigatingHistory = false
    /// Named actions (Rotate, Crop, Reset all…) set this before mutating
    /// settings; the debounced commit prefers it over the field diff.
    var pendingHistoryLabel: String?

    var canUndo: Bool { historyIndex > 0 }
    var canRedo: Bool { historyIndex < historyEntries.count - 1 }

    /// Stash the outgoing image's history, load (or seed) the incoming one.
    private func loadHistory(for url: URL?) {
        if let old = historyURL {
            histories[old] = HistoryState(entries: historyEntries, index: historyIndex)
        }
        historyCommitTask?.cancel()
        pendingHistoryLabel = nil
        historyURL = url
        guard let url else {
            historyEntries = []
            historyIndex = 0
            return
        }
        if let saved = histories[url] {
            historyEntries = saved.entries
            historyIndex = saved.index
        } else {
            historyEntries = [HistoryEntry(label: "Original conversion", settings: settings)]
            historyIndex = 0
        }
    }

    /// Draggable controls report begin/end here: renders keep flowing
    /// mid-drag, but the history commit is held until release — dragging is
    /// preview; letting go is the undoable act. A drag session yields one
    /// entry: the state before interaction -> where the control ended up.
    private var controlEditCount = 0

    func setControlEditing(_ editing: Bool) {
        if editing {
            controlEditCount += 1
            historyCommitTask?.cancel()
        } else {
            controlEditCount = max(0, controlEditCount - 1)
            if controlEditCount == 0 { commitHistory() }
        }
    }

    /// Non-drag changes (toggles, double-click resets, named actions)
    /// coalesce: one entry per settled change (0.7 s after the last tick),
    /// labelled by diffing against the current history state. During a
    /// control drag nothing is scheduled — setControlEditing commits once
    /// on release.
    private func scheduleHistoryCommit() {
        guard selection != nil, controlEditCount == 0 else { return }
        historyCommitTask?.cancel()
        historyCommitTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(700))
            guard !Task.isCancelled else { return }
            self?.commitHistory()
        }
    }

    private func commitHistory() {
        guard !historyEntries.isEmpty, historyIndex < historyEntries.count else { return }
        let current = settings
        guard historyEntries[historyIndex].settings != current else {
            pendingHistoryLabel = nil
            return
        }
        // A new change clears everything ahead of the current state (redo).
        historyEntries.removeSubrange((historyIndex + 1)...)
        let label = pendingHistoryLabel
            ?? historyLabel(from: historyEntries[historyIndex].settings, to: current)
        historyEntries.append(HistoryEntry(label: label, settings: current))
        historyIndex = historyEntries.count - 1
        pendingHistoryLabel = nil
    }

    func undo() {
        guard selection != nil else { return }
        historyCommitTask?.cancel()
        // Flush an in-flight (uncommitted) change so undo steps back exactly one.
        if !historyEntries.isEmpty, historyEntries[historyIndex].settings != settings {
            commitHistory()
        }
        guard canUndo else { return }
        historyIndex -= 1
        applyHistorySettings()
    }

    func redo() {
        guard canRedo else { return }
        historyCommitTask?.cancel()
        historyIndex += 1
        applyHistorySettings()
    }

    func jumpToHistory(_ index: Int) {
        guard index >= 0, index < historyEntries.count, index != historyIndex else { return }
        historyCommitTask?.cancel()
        historyIndex = index
        applyHistorySettings()
    }

    private func applyHistorySettings() {
        isNavigatingHistory = true
        settings = historyEntries[historyIndex].settings
        isNavigatingHistory = false
    }

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
        folderTree = nil
        collapsedFolders = []
        Task { await thumbnails.clear() }
        scanFolder()
    }

    func scanFolder() {
        guard let root = folderURL else { return }
        isScanning = true
        Task.detached(priority: .userInitiated) {
            let tree = Self.buildTree(at: root, depth: 0)
            var flattened: [URL] = []
            Self.flatten(tree, into: &flattened)
            let flat = flattened
            await MainActor.run {
                self.folderTree = tree
                self.files = flat
                self.fileIndex = Dictionary(
                    uniqueKeysWithValues: flat.enumerated().map { ($0.element, $0.offset + 1) })
                self.isScanning = false
                if let selection = self.selection, !flat.contains(selection) {
                    self.selection = flat.first
                }
                if self.selection == nil { self.selection = flat.first }
            }
        }
    }

    /// Recursive scan (VSCode-style tree): RAW files per folder, name-sorted,
    /// hidden files/packages skipped, RAW-less branches pruned, depth-capped.
    nonisolated private static func buildTree(at url: URL, depth: Int) -> FolderNode? {
        guard depth <= 8 else { return nil }
        let keys: [URLResourceKey] = [.isDirectoryKey, .isPackageKey]
        let contents = (try? FileManager.default.contentsOfDirectory(
            at: url, includingPropertiesForKeys: keys, options: .skipsHiddenFiles)) ?? []
        func name(_ u: URL) -> String { u.lastPathComponent }
        let files = contents.filter { RawDecoder.isRawFile($0) }
            .sorted { name($0).localizedStandardCompare(name($1)) == .orderedAscending }
        let dirs = contents.filter { u in
            let values = try? u.resourceValues(forKeys: Set(keys))
            return (values?.isDirectory ?? false) && !(values?.isPackage ?? false)
        }
        .sorted { name($0).localizedStandardCompare(name($1)) == .orderedAscending }
        let subfolders = dirs.compactMap { buildTree(at: $0, depth: depth + 1) }
        if files.isEmpty && subfolders.isEmpty { return nil }
        let total = files.count + subfolders.reduce(0) { $0 + $1.totalCount }
        return FolderNode(
            id: url, name: url.lastPathComponent, files: files, subfolders: subfolders,
            totalCount: total)
    }

    /// Depth-first: a folder's own files first, then its subfolders — the
    /// order behind index numbers and shift-click ranges.
    nonisolated private static func flatten(_ node: FolderNode?, into out: inout [URL]) {
        guard let node else { return }
        out.append(contentsOf: node.files)
        for sub in node.subfolders { flatten(sub, into: &out) }
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
        loadHistory(for: url)
    }

    private func settingsChanged() {
        scheduleRender()
        if !isRestoringSettings { scheduleSave() }
        if !isRestoringSettings && !isNavigatingHistory { scheduleHistoryCommit() }
    }

    /// Preview at full source resolution instead of the 1536px proxy (the HQ
    /// button in the canvas control bar). Session-only by design: a persisted
    /// flag would silently make every launch pay full-res render costs.
    var hqPreview = false {
        didSet { if oldValue != hqPreview { scheduleRender() } }
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
                var snapshot = self.showingBaseline ? self.baselineSettings() : self.settings
                // Mid-straighten-drag renders bake 0° (see straightenDragValue).
                if self.straightenDragValue != nil { snapshot.fineRotation = 0 }
                let uncropped = self.toolMode != .none
                let hq = self.hqPreview
                let midStraightenDrag = self.straightenDragValue != nil
                guard let session = self.session else { break }
                do {
                    // No Analyzing pill for the transient 0° re-base render —
                    // it flashes mid-gesture and adds to the visual churn.
                    if await session.needsPreparation(settings: snapshot, hq: hq), !midStraightenDrag {
                        self.isAnalyzing = true
                    }
                    let output = try await session.render(settings: snapshot, uncropped: uncropped, hq: hq)
                    self.isAnalyzing = false
                    if Task.isCancelled { break }
                    if midStraightenDrag {
                        // The 0° re-base swaps bitmap, fitted frame (different
                        // aspect than the inscribed crop), rotation delta and
                        // cover scale at once — animate so it reads as a
                        // smooth zoom re-base, not a pop.
                        withAnimation(.easeOut(duration: 0.18)) {
                            self.displayImage = output.image
                            self.displayedFineRotation = snapshot.fineRotation
                        }
                    } else {
                        self.displayImage = output.image
                        self.displayedFineRotation = snapshot.fineRotation
                    }
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
        pendingHistoryLabel = "Reset all"
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

    /// File > Export…: the multi-selection when there is one, else the
    /// current image (mirrors the film-strip context menu).
    func requestExportFromMenu() {
        if multiSelection.count > 1 {
            requestExportSelected()
        } else {
            requestExportCurrent()
        }
    }

    func revealSelectionInFinder() {
        guard let selection else { return }
        NSWorkspace.shared.activateFileViewerSelecting([selection])
    }

    // ── Copy/paste adjustments (Lightroom-style, Edit menu) ───────────────
    /// Snapshot for Paste Adjustments; geometry (rotation/flip/straighten/
    /// crops) stays per-frame and is never pasted.
    var copiedAdjustments: ExposureSettings?

    func copyAdjustments() {
        guard selection != nil else { return }
        copiedAdjustments = settings
    }

    func pasteAdjustments() {
        guard let source = copiedAdjustments, selection != nil else { return }
        var next = source
        next.rotation = settings.rotation
        next.flipHorizontal = settings.flipHorizontal
        next.fineRotation = settings.fineRotation
        next.cropRect = settings.cropRect
        next.analysisRect = settings.analysisRect
        pendingHistoryLabel = "Paste adjustments"
        settings = next
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
