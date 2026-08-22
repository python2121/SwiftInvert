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
        didSet {
            UserDefaults.standard.set(folderURL?.path, forKey: "libraryFolder")
            // Retained sessions belong to the old library — holding their
            // decodes would strand tens of MB per frame nobody can navigate to.
            if oldValue != folderURL { sessionLRU.removeAll() }
        }
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

    /// Arrow-key navigation in film-strip (discovery) order.
    func selectAdjacent(_ offset: Int) {
        guard !files.isEmpty else { return }
        guard let current = selection, let index = files.firstIndex(of: current) else {
            select(files[0], additive: false)
            return
        }
        let next = min(max(index + offset, 0), files.count - 1)
        guard next != index else { return }
        select(files[next], additive: false)
    }

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
        didSet {
            guard oldValue != toolMode else { return }
            clearTestStrip()
            clearZonePlacement()
            refreshHQActive(debounced: false)
            if toolMode == .crop {
                // Snapshot for Escape-cancel: mid-mode straighten commits
                // mutate settings live, so cancel restores this pair.
                cropModeBackup = (settings.fineRotation, settings.cropRect)
            }
            scheduleRender()
        }
    }

    private var cropModeBackup: (fineRotation: Double, cropRect: NormalizedRect?)?
    /// Set by cancelCropMode so DetailView's mode-exit handler skips the
    /// crop-box commit (it resets the flag).
    var cropModeCancelled = false

    /// Escape in Crop & Straighten: restore the angle and crop to their
    /// values at mode entry, then exit without committing the box.
    func cancelCropMode() {
        guard toolMode == .crop else { return }
        straightenDragValue = nil
        if let backup = cropModeBackup,
            backup.fineRotation != settings.fineRotation || backup.cropRect != settings.cropRect
        {
            var restored = settings
            restored.fineRotation = backup.fineRotation
            restored.cropRect = backup.cropRect
            pendingHistoryLabel = "Cancel crop"
            settings = restored
        }
        cropModeCancelled = true
        toolMode = .none
    }

    // MARK: - Orientation & canvas

    /// Transient Straighten drag value: while non-nil the detail view previews
    /// the rotation as a display transform (no settings mutation); commit
    /// happens once on release. Starting a drag on an already-rotated image
    /// re-bases the display on a fineRotation-0 render: the display transform
    /// can only zoom IN (the baked image lacks pixels beyond its inscribed
    /// crop), so rotating back toward zero from a baked angle would wrongly
    /// zoom in too — from a 0° base the cover scale is correct both ways.
    /// The 0° base is PRECOMPUTED (`straightenBase`) so the re-base swaps in
    /// instantly at the first touch — when target still equals the baked
    /// angle and the two presentations nearly coincide — instead of landing
    /// mid-gesture; the render here is only the cache-miss fallback.
    var straightenDragValue: Double? {
        didSet {
            guard oldValue == nil, straightenDragValue != nil, displayedFineRotation != 0
            else { return }
            var zeroed = settings
            zeroed.fineRotation = 0
            if toolMode == .none, !showingBaseline, !hqActive,
                let base = straightenBase, base.settings == zeroed {
                displayImage = base.output.image
                histogram = base.output.histogram
                displayAspect = base.output.displayAspect
                contentWindow = base.output.contentWindow
                displayedFineRotation = 0
            } else {
                scheduleRender()
            }
        }
    }

    /// Precomputed 0° render (image + histogram) keyed by the exact settings
    /// it was rendered with; refreshed ~350ms after edits settle and on
    /// straighten-slider hover, cleared on image switch.
    private var straightenBase: (settings: ExposureSettings, output: ImageSession.RenderOutput)?
    private var straightenBaseTask: Task<Void, Never>?

    func prepareStraightenBase(afterMilliseconds delay: Int = 0) {
        guard selection != nil, settings.fineRotation != 0, toolMode == .none,
            !showingBaseline, !hqActive, straightenDragValue == nil
        else { return }
        var zeroed = settings
        zeroed.fineRotation = 0
        if straightenBase?.settings == zeroed { return }
        straightenBaseTask?.cancel()
        straightenBaseTask = Task { [weak self] in
            if delay > 0 {
                try? await Task.sleep(for: .milliseconds(delay))
            }
            guard !Task.isCancelled, let self, let session = self.session else { return }
            guard let output = try? await session.renderDetached(settings: zeroed) else { return }
            guard !Task.isCancelled else { return }
            self.straightenBase = (zeroed, output)
        }
    }
    /// The fineRotation the current displayImage was BAKED with. The detail
    /// view rotates the display by (target − baked), so the preview holds
    /// through the post-release re-bake instead of snapping back.
    var displayedFineRotation: Double = 0
    /// Unrotated (orientation-only) frame dims of the current image, from the
    /// last render — the base for CropGeometry's rotated-space math.
    var frameSize: CGSize = .zero
    /// Aspect the canvas lays out at — resolution-independent, so swapping the
    /// HQ tier in cannot nudge the picture. See `ImageSession.RenderOutput`.
    var displayAspect: Double = 0
    /// Where the current bitmap sits inside that laid-out rect (nominally the
    /// unit rect, off it by a fraction of a pixel). See `ImageSession.contentWindow`.
    var contentWindow = NormalizedRect(x: 0, y: 0, width: 1, height: 1)
    /// Measured pre-offset density range of the current negative, from the last
    /// render — input to the Negative-character read-out beside the Grade
    /// slider. 0 until the first render lands.
    var densityRange: Double = 0

    /// Commit a straighten angle, remapping any committed crop so it keeps
    /// showing the same content (center-preserved, shrunk only if the new
    /// angle demands it) instead of drifting with the inscribed auto-crop.
    func commitFineRotation(_ degrees: Double) {
        var next = settings
        next.fineRotation = degrees
        if let crop = settings.cropRect, frameSize != .zero {
            next.cropRect = CropGeometry.remapCrop(
                crop,
                from: settings.fineRotation * .pi / 180,
                to: degrees * .pi / 180,
                frame: SIMD2(frameSize.width, frameSize.height))
        }
        settings = next
    }

    /// The ⨯ next to the Crop button and the Image-menu item: one action
    /// resets both geometry edits (no crop remap needed — the crop goes too).
    func clearCropAndStraighten() {
        let hadCrop = settings.cropRect != nil
        let hadAngle = abs(settings.fineRotation) > 1e-9
        guard hadCrop || hadAngle else { return }
        pendingHistoryLabel =
            hadCrop && hadAngle
            ? "Crop & straighten cleared"
            : hadCrop ? "Crop cleared" : "Straighten cleared"
        var next = settings
        next.cropRect = nil
        next.fineRotation = 0
        settings = next
    }

    func rotateClockwise() {
        if testStripOwnsRotation {
            rotateTestStripLadder(clockwise: true)
            return
        }
        pendingHistoryLabel = "Rotate 90° CW"
        settings.rotation = ((settings.rotation + 90) % 360 + 360) % 360 }
    func rotateCounterclockwise() {
        if testStripOwnsRotation {
            rotateTestStripLadder(clockwise: false)
            return
        }
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
            // Drawn on the (possibly straightened) display: meter at this angle.
            settings.analysisRectFineRotation = settings.fineRotation
        case .crop:
            pendingHistoryLabel = "Crop"
            settings.cropRect = rect
        case .none: break
        }
        toolMode = .none  // settings didSet already re-renders (and re-analyzes)
    }

    /// The NSWindow hosting this model's ContentView (set by
    /// WindowKeyObserver): the key monitor only consumes events from its
    /// own window now that profile editors make the app multi-window.
    @ObservationIgnored weak var hostWindow: NSWindow?

    let thumbnails = ThumbnailStore()

    /// Process-wide render pipeline (see init) — compiled once, shared by
    /// the main window and every profile-editor window.
    private static let sharedPipelineResult: Result<RenderPipeline, Error> = Result {
        try RenderPipeline()
    }
    private static var sharedPipeline: RenderPipeline? {
        try? sharedPipelineResult.get()
    }
    private static var sharedPipelineError: String? {
        if case .failure(let e) = sharedPipelineResult { return "\(e)" }
        return nil
    }

    private var pipeline: RenderPipeline?
    private var session: ImageSession?
    /// Recently-visited sessions, oldest first, including the active one.
    ///
    /// Dropping a session on navigation threw away its whole cache tower —
    /// decode, oriented preview, the ~150 ms `prepare`, the uploaded textures —
    /// so arrowing back to a frame you were just on paid the full cost again to
    /// reproduce pixels that were on screen seconds ago. Retention is enough on
    /// its own here: every tier is keyed (`MeterKey`/`OrientKey`/`PreparedKey`/
    /// `AnalysisKey`), so a returning session re-validates against the current
    /// settings and can't serve stale pixels. NegPy needs a `RenderMemo` of the
    /// last displayed frame on top of its cache for the same effect; with the
    /// tower warm our re-render is derive+GPU (~5 ms), so the memo would buy
    /// nothing.
    ///
    /// Budget 2 (matching NegPy's `preview_cache_max_full_res_entries`): the
    /// active frame plus the one navigated from, which is the A/B compare move.
    private var sessionLRU: [(url: URL, session: ImageSession)] = []
    private static let sessionLRULimit = 2
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

    /// True when an edit is newer than the current history entry (the 0.7 s
    /// debounce window) — undo can flush-and-revert it, so the menu item
    /// must stay enabled even at history index 0.
    var hasUncommittedEdit: Bool {
        !historyEntries.isEmpty && historyEntries.indices.contains(historyIndex)
            && historyEntries[historyIndex].settings != settings
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
        guard selection != nil else { return }
        historyCommitTask?.cancel()
        // An in-flight (uncommitted) edit truncates the redo tail — flushing
        // it first is what makes that true (silently redoing over it would
        // discard the edit AND resurrect a tail the edit invalidated).
        if !historyEntries.isEmpty, historyEntries[historyIndex].settings != settings {
            commitHistory()
            return
        }
        guard canRedo else { return }
        historyIndex += 1
        applyHistorySettings()
    }

    func jumpToHistory(_ index: Int) {
        guard index >= 0, index < historyEntries.count, index != historyIndex else { return }
        pendingHistoryLabel = nil  // an armed named label must not mislabel a later edit
        historyCommitTask?.cancel()
        historyIndex = index
        applyHistorySettings()
    }

    private func applyHistorySettings() {
        isNavigatingHistory = true
        settings = historyEntries[historyIndex].settings
        isNavigatingHistory = false
    }

    /// Profile-editor mode (the Create/Edit window of File → Choose Default
    /// Settings…): one shared adjustments draft applies to EVERY frame —
    /// navigate anywhere and the same look follows; adjust further and it
    /// follows back. Geometry stays per-frame (read from sidecars, shown but
    /// never written), and NO sidecar is ever written in this mode.
    let isProfileEditor: Bool

    /// The shared adjustments being edited (geometry always stock). Kept in
    /// sync from `settings` on every change; what Accept saves to the profile.
    private(set) var profileDraft = ExposureSettings()

    /// Set by the key monitor when Escape lands in a profile-editor window
    /// with no tool mode active (tool Escapes keep their exit/cancel
    /// meaning); ProfileEditorView watches it and runs save/discard/close.
    var profileEditorEscape = false

    init(profileEditor: Bool = false, profileSeed: ExposureSettings = ExposureSettings()) {
        isProfileEditor = profileEditor
        profileDraft = profileSeed.adjustmentsOnly
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
        // selection, whose didSet immediately kicks the first render. ONE
        // pipeline serves every AppModel (profile editors included):
        // RenderPipeline is built for sharing — render() is lock-serialized
        // and returns read-back buffers only — and a per-window pipeline
        // meant recompiling the shaders and duplicating the texture caches
        // on every editor open.
        pipeline = Self.sharedPipeline
        if pipeline == nil {
            statusMessage = "Metal unavailable: \(Self.sharedPipelineError ?? "unknown")"
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
        clearTestStrip()
        clearZonePlacement()
        // Route a live crop mode through the CANCEL path: committing here
        // would apply the OLD image's crop box to the NEW image's settings
        // (DetailView's mode-exit handler runs after the switch).
        if toolMode == .crop { cropModeCancelled = true }
        toolMode = .none
        showingBaseline = false
        displayImage = nil
        histogram = nil
        densityRange = 0
        if let url = selection, !multiSelection.contains(url) { multiSelection = [url] }
        guard let url = selection else { return }
        guard let pipeline else {
            statusMessage = "Cannot render: Metal pipeline unavailable."
            return
        }
        straightenBaseTask?.cancel()
        straightenBase = nil
        // A new frame always arrives fitted (DetailView resets its gesture
        // state too). Resolve HQ BEFORE the sidecar load kicks off the first
        // render, or an `.auto` frame opened while the previous one was zoomed
        // in would decode full resolution for a picture shown at fit.
        canvasZoom = 1
        hqSwapTask?.cancel()
        hqActive = hqShouldBeActive
        session = retainedSession(for: url, pipeline: pipeline)
        // Loading the sidecar mutates settings, which triggers the first render.
        // A frame with no sidecar gets the house default profile. The profile
        // editor instead composes its shared draft over the frame's geometry.
        let frame = SidecarStore.load(for: url) ?? DefaultProfile.settings
        let restored = isProfileEditor ? profileDraft.keepingGeometry(of: frame) : frame
        isRestoringSettings = true
        if restored == settings {
            settingsChanged()  // no mutation → kick the render explicitly
        } else {
            settings = restored
        }
        isRestoringSettings = false
        loadHistory(for: url)
    }

    /// The session for `url`, reusing a retained one when we've been here
    /// recently (see `sessionLRU`). The returning session's cache tower is
    /// re-validated by its own keys, so nothing here has to reason about
    /// whether the settings moved while we were away.
    private func retainedSession(for url: URL, pipeline: RenderPipeline) -> ImageSession {
        let found: ImageSession
        if let i = sessionLRU.firstIndex(where: { $0.url == url }) {
            found = sessionLRU.remove(at: i).session  // re-append: most recent last
        } else {
            found = ImageSession(url: url, pipeline: pipeline)
        }
        sessionLRU.append((url, found))
        while sessionLRU.count > Self.sessionLRULimit { sessionLRU.removeFirst() }
        // Only the frame on screen may hold the full-res tier; a retained
        // session keeping its HQ decode would cost hundreds of MB for pixels
        // nobody is looking at.
        for entry in sessionLRU where entry.url != url {
            Task { await entry.session.releaseEnhancedTiers() }
        }
        return found
    }

    private func settingsChanged() {
        clearTestStrip()  // the patches no longer reflect the settings
        clearZonePlacement()  // the pins' samples/solution no longer do either
        // Every settings edit in the profile editor updates the shared draft
        // (geometry stripped — crop/rotate in the editor stays view-local).
        if isProfileEditor { profileDraft = settings.adjustmentsOnly }
        scheduleRender()
        if !isRestoringSettings { scheduleSave() }
        if !isRestoringSettings && !isNavigatingHistory { scheduleHistoryCommit() }
    }

    /// Preview at full source resolution instead of the 1536px proxy (the HQ
    /// button in the canvas control bar, cycling off → auto → on).
    ///
    /// Session-only by design: a persisted `.on` would silently make every
    /// launch pay full-res render costs. `.auto` is the default and is cheap
    /// at rest — it decodes nothing until you actually magnify past the point
    /// where the proxy runs out of pixels.
    enum HQMode: String, CaseIterable, Identifiable, Sendable {
        /// Always the 1536px proxy.
        case off
        /// Proxy when fitted, full resolution once zoomed past
        /// `hqAutoZoomThreshold` — rendered in the background and swapped in.
        case auto
        /// Always full resolution.
        case on

        var id: String { rawValue }

        var label: String {
            switch self {
            case .off: return "Off"
            case .auto: return "Auto"
            case .on: return "On"
            }
        }

        var next: HQMode {
            switch self {
            case .off: return .auto
            case .auto: return .on
            case .on: return .off
            }
        }

        /// Pure resolution of "mode + canvas context → render full resolution?".
        /// Split out of `AppModel` so it is testable without standing up a
        /// render pipeline and a decoded session.
        ///
        /// - `canvasOwnedByProxyTool`: a test strip or zone placement is up.
        ///   Both render from the proxy tower by construction, so HQ would be a
        ///   lie there — this is what the old boolean's explicit force-off did.
        /// - `cropToolActive`: Crop & Straighten pins the canvas to fit
        ///   (`scaleEffect(1)`) whatever the zoom state holds, so magnification
        ///   is not a reason to decode while it owns the view. It does not
        ///   suppress an explicit `.on`.
        ///
        /// Baseline hold is deliberately absent: a press-and-hold compare must
        /// show both states at the SAME resolution, or part of the difference
        /// you see is the proxy rather than the edit.
        nonisolated func resolve(
            zoom: CGFloat,
            threshold: CGFloat,
            hasSelection: Bool,
            canvasOwnedByProxyTool: Bool,
            cropToolActive: Bool
        ) -> Bool {
            guard hasSelection, !canvasOwnedByProxyTool else { return false }
            switch self {
            case .off: return false
            case .on: return true
            case .auto: return !cropToolActive && zoom >= threshold
            }
        }
    }

    /// Magnification at which `.auto` switches to the full-resolution tier.
    ///
    /// `zoom == 1` is fit-to-window, so on a Retina display the 1536px proxy is
    /// already being upsampled before you touch the trackpad — a
    /// "proxy has run out of real pixels" threshold would fire at rest and make
    /// `.auto` indistinguishable from `.on`. 2× is comfortably past the proxy's
    /// useful resolution at any sane window size, and it keeps the trigger a
    /// property of YOUR gesture rather than of the window's dimensions.
    nonisolated static let hqAutoZoomThreshold: CGFloat = 2.0

    /// Which source the render should read from.
    ///
    /// `.on` means "show me what export produces", so it always pays for the
    /// full decode. `.auto` is a convenience — it takes whatever costs nothing,
    /// which is the half-size buffer the preview decode already produced (the
    /// session falls through to `.full` on bodies that have no such buffer).
    /// So Auto is instant but not export-truth; On is export-truth but waits.
    nonisolated static func renderTier(mode: HQMode, active: Bool) -> ImageSession.RenderTier {
        guard active else { return .proxy }
        return mode == .on ? .full : .mediumIfFree
    }

    var hqMode: HQMode = .auto {
        didSet {
            guard oldValue != hqMode else { return }
            // Changing the mode by hand invalidates the canvas-owning states,
            // exactly as toggling the old boolean did.
            clearTestStrip()
            clearZonePlacement()
            if hqMode == .off { Task { [session] in await session?.releaseEnhancedTiers() } }
            refreshHQActive(debounced: false)
        }
    }

    /// Canvas magnification, reported by DetailView so `.auto` can follow it
    /// (the gesture state itself stays view-local).
    var canvasZoom: CGFloat = 1 {
        didSet {
            guard oldValue != canvasZoom, hqMode == .auto else { return }
            // A pinch reports continuously; only a CROSSING can change the
            // answer, so ignore the rest of the gesture rather than cancelling
            // and rescheduling a task per frame.
            let was = oldValue >= Self.hqAutoZoomThreshold
            let now = canvasZoom >= Self.hqAutoZoomThreshold
            guard was != now else { return }
            // Debounced upward: sweeping past 2× mid-pinch shouldn't commit to
            // a ~700 ms decode until the gesture settles there.
            refreshHQActive(debounced: true)
        }
    }

    /// Whether the CURRENT render should use the full-resolution tier — the
    /// mode resolved against zoom and the canvas-owning states.
    private(set) var hqActive = false

    private var hqShouldBeActive: Bool {
        hqMode.resolve(
            zoom: canvasZoom,
            threshold: Self.hqAutoZoomThreshold,
            hasSelection: selection != nil,
            canvasOwnedByProxyTool: testStrip != nil || testStripTask != nil || zonePlacement != nil,
            cropToolActive: toolMode != .none)
    }

    @ObservationIgnored private var hqSwapTask: Task<Void, Never>?

    /// Re-resolve `hqActive` and re-render if it moved. The swap is deliberately
    /// NOT accompanied by clearing `displayImage`: the proxy stays on screen
    /// until the full-resolution frame is ready, so crossing the threshold reads
    /// as the picture sharpening rather than as a reload.
    func refreshHQActive(debounced: Bool) {
        hqSwapTask?.cancel()
        let target = hqShouldBeActive
        guard target != hqActive else { return }
        // Dropping BACK to the proxy is instant (its tower is warm), so only
        // the expensive direction waits out the gesture.
        guard debounced, target else {
            applyHQActive(target)
            return
        }
        hqSwapTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(350))
            guard !Task.isCancelled, let self else { return }
            guard self.hqShouldBeActive != self.hqActive else { return }
            self.applyHQActive(self.hqShouldBeActive)
        }
    }

    /// Deliberately does NOT clear the test strip / zone placement: those states
    /// force `hqShouldBeActive` false while they own the canvas, so clearing
    /// them here would re-enable HQ and bounce straight back. Clearing on an
    /// explicit mode change is `hqMode`'s job.
    private func applyHQActive(_ value: Bool) {
        guard hqActive != value else { return }
        hqActive = value
        scheduleRender()
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
                // Mid-straighten-drag renders bake 0° (see straightenDragValue),
                // and so does the unified Crop & Straighten mode: the image
                // rotates behind the axis-aligned crop box as a display
                // transform over the full unrotated frame.
                if self.straightenDragValue != nil || self.toolMode == .crop {
                    snapshot.fineRotation = 0
                }
                let uncropped = self.toolMode != .none
                let tier = Self.renderTier(mode: self.hqMode, active: self.hqActive)
                let midStraightenDrag = self.straightenDragValue != nil
                guard let session = self.session else { break }
                do {
                    // No Analyzing pill for the transient 0° re-base render —
                    // it flashes mid-gesture and adds to the visual churn.
                    if await session.needsPreparation(settings: snapshot, tier: tier),
                        !midStraightenDrag
                    {
                        self.isAnalyzing = true
                    }
                    let output = try await session.render(
                        settings: snapshot, uncropped: uncropped, tier: tier,
                        retainFull: self.hqMode != .off)
                    self.isAnalyzing = false
                    if Task.isCancelled { break }
                    self.frameSize = output.frameSize
                    self.displayAspect = output.displayAspect
                    self.contentWindow = output.contentWindow
                    self.densityRange = output.densityRange
                    if midStraightenDrag {
                        // Cache-miss fallback: the 0° re-base swaps bitmap,
                        // fitted frame aspect, rotation delta and cover scale
                        // at once — animate so it reads as a smooth zoom
                        // re-base, not a pop.
                        withAnimation(.easeOut(duration: 0.18)) {
                            self.displayImage = output.image
                            self.displayedFineRotation = snapshot.fineRotation
                        }
                    } else {
                        self.displayImage = output.image
                        self.displayedFineRotation = snapshot.fineRotation
                        // Keep the precomputed 0° base fresh once edits settle.
                        self.prepareStraightenBase(afterMilliseconds: 350)
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
        guard !isProfileEditor, let url = selection else { return }
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
    /// Back to "first time we see this image": the default profile AND stock
    /// geometry (crop, analysis region, orientation, straighten) — the frame
    /// re-analyzes and renders exactly as on first open with no sidecar.
    func resetSettings() {
        let before = settings
        pendingHistoryLabel = "Reset all"
        settings = DefaultProfile.settings
        // No-op (already at defaults): didSet never fired, so nothing will
        // consume the label — disarm it or the NEXT edit commits as "Reset
        // all". (When the reset DID change something, the label must
        // survive until the debounced commit reads it.)
        if before == settings { pendingHistoryLabel = nil }
    }

    /// Cancel the default profile too: the untouched stock (NegPy-neutral)
    /// conversion, for starting an edit from scratch.
    func resetToStock() {
        let before = settings
        pendingHistoryLabel = "Start from scratch"
        settings = ExposureSettings()
        if before == settings { pendingHistoryLabel = nil }  // no-op: disarm
    }

    // MARK: - Test strip (NegPy e4bc450 port)

    /// The assembled 5×5 proof over the current frame, or nil. Session-only
    /// display state: any real edit, navigation, tool mode or baseline peek
    /// clears it (the patches would no longer reflect the settings).
    /// Two-stage picking: single-click a patch → full-frame PREVIEW of that
    /// patch's settings (click again to return to the grid, click other
    /// patches to compare); double-click → commit.
    struct TestStripState {
        let image: CGImage
        /// Settings the strip was built from (its non-density/grade context).
        let baseSettings: ExposureSettings
        /// Ladder orientation the mosaic was assembled with (0–3 × 90° CW).
        let orientation: Int
        /// Full-frame preview of one patch (nil = the grid is showing).
        var preview: (row: Int, col: Int, image: CGImage)?
    }
    var testStrip: TestStripState?
    @ObservationIgnored private var testStripTask: Task<Void, Never>?
    /// Guards against a stale build finishing after a newer toggle: only the
    /// generation that started a task may publish or reset its state.
    @ObservationIgnored private var testStripGeneration = 0

    /// True while the strip is building (the toggle shows active immediately).
    var testStripBuilding = false

    /// Ladder orientation (0–3 × 90° CW; a2455ab): while a strip is up the
    /// rotate buttons and ⌘[/⌘] turn the LADDER, not the image — the
    /// dense/hard end lands on a different part of the frame. Sticky for the
    /// session; the mosaic rebuilds from the warm tower (~a strip build).
    private(set) var testStripOrientation = 0

    /// True when the rotate commands should turn the ladder instead of the
    /// image (a strip is showing or building).
    var testStripOwnsRotation: Bool { testStrip != nil || testStripTask != nil }

    func rotateTestStripLadder(clockwise: Bool) {
        testStripOrientation = ((testStripOrientation + (clockwise ? 1 : 3)) % 4)
        clearTestStrip()
        toggleTestStrip()
    }

    func toggleTestStrip() {
        if testStrip != nil || testStripTask != nil {
            clearTestStrip()
            return
        }
        guard let session, selection != nil, toolMode == .none, !showingBaseline else { return }
        clearZonePlacement()  // strip and pins both own the canvas
        // The strip and its patch previews are proxy-renders by design
        // (25 full-res renders would take ages). `hqShouldBeActive` already
        // reports false while a strip owns the canvas; demote an explicit
        // `.on` too, so the button can't read "always HQ" over a proxy strip.
        if hqMode == .on { hqMode = .off }
        refreshHQActive(debounced: false)
        let snapshot = settings
        testStripGeneration += 1
        let generation = testStripGeneration
        testStripBuilding = true
        let orientation = testStripOrientation
        testStripTask = Task { [weak self] in
            let image = try? await session.renderTestStrip(settings: snapshot, orientation: orientation)
            guard let self, self.testStripGeneration == generation else { return }
            self.testStripBuilding = false
            self.testStripTask = nil
            // Publish only if nothing changed underneath the build.
            guard !Task.isCancelled, let image, self.settings == snapshot,
                self.toolMode == .none, !self.showingBaseline
            else { return }
            self.testStrip = TestStripState(image: image, baseSettings: snapshot, orientation: orientation)
        }
    }

    func clearTestStrip() {
        let owned = testStrip != nil || testStripTask != nil
        testStripGeneration += 1
        testStripTask?.cancel()
        testStripTask = nil
        if testStripBuilding { testStripBuilding = false }
        if testStrip != nil { testStrip = nil }
        // The strip suppressed HQ while it owned the canvas; releasing it may
        // let `.auto` apply again at the current magnification.
        if owned { refreshHQActive(debounced: false) }
    }

    /// Single click on a patch: render THAT look full-frame (the preview
    /// stage) so candidates can be compared at full size before committing.
    /// One derive+render on the warm tower (~a slider tick).
    func previewTestStripCell(row: Int, col: Int) {
        guard let strip = testStrip, let session,
            TestStrip.gradesByRow.indices.contains(row),
            TestStrip.densitiesByColumn.indices.contains(col)
        else { return }
        let cell = TestStrip.cell(row: row, col: col, orientation: strip.orientation)
        var s = strip.baseSettings
        s.density = cell.density
        s.grade = cell.grade
        let generation = testStripGeneration
        Task { [weak self] in
            guard let output = try? await session.render(settings: s) else { return }
            guard let self, self.testStripGeneration == generation,
                self.testStrip != nil
            else { return }
            self.testStrip?.preview = (row, col, output.image)
        }
    }

    /// Click while previewing: back to the grid to try another patch.
    func returnToTestStripGrid() {
        guard testStrip?.preview != nil else { return }
        testStrip?.preview = nil
    }

    /// Double click (grid or preview): commit the patch's density+grade as
    /// one history entry. Auto exposure / auto contrast stay untouched — the
    /// patches were rendered under them (upstream rule), so toggling them
    /// would render something other than the clicked patch.
    func confirmTestStripCell(row: Int, col: Int) {
        guard let strip = testStrip,
            TestStrip.gradesByRow.indices.contains(row),
            TestStrip.densitiesByColumn.indices.contains(col)
        else { return }
        let cell = TestStrip.cell(row: row, col: col, orientation: strip.orientation)
        pendingHistoryLabel = "Test strip pick"
        var s = settings
        s.density = cell.density
        s.grade = cell.grade
        let before = settings
        settings = s  // didSet clears the strip; explicit clear below covers
        clearTestStrip()  // picking the patch that IS the current settings
        if before == settings { pendingHistoryLabel = nil }  // no-op pick: disarm
    }

    // MARK: - Zone placement (NegPy 5a095f3/9dff124 port)

    /// Session-only canvas state, like the test strip: pins over the frame,
    /// the live solved-look preview, and the strip-armed target. Any real
    /// edit, navigation, tool mode, baseline peek or HQ change clears it.
    struct ZonePlacementState {
        var pins: [ZonePlacement.Pin] = []
        /// Measured roman label per pin (what it prints at under the CURRENT
        /// settings), refreshed with every re-solve.
        var labels: [String] = []
        var solution: ZonePlacement.Solution?
        /// Full-frame render of the solution, drawn over the canvas while
        /// pins are up (the same overlay trick as the test strip's preview).
        var previewImage: CGImage?
        /// Zone picked on the strip, consumed by the next canvas click.
        var armedZone: Double?
        var solving = false
    }
    var zonePlacement: ZonePlacementState?
    /// Fences stale async sample/solve completions (same pattern as the
    /// test-strip generation counter).
    @ObservationIgnored private var zonePlacementGeneration = 0

    /// Zone-strip cell click: arm that zone for the next canvas click
    /// (upstream: "click a cell in the Analysis zone strip, click that spot
    /// on the photo"). Clicking the armed cell again disarms.
    func armZonePlacementTarget(_ zone: Double) {
        guard selection != nil, toolMode == .none, !showingBaseline else { return }
        clearTestStrip()
        // Placement samples and previews on the proxy tower (like the test
        // strip); leaving HQ nominally on would lie. Demote BEFORE creating the
        // state — applyHQActive clears zone placement.
        if hqMode == .on { hqMode = .off }
        refreshHQActive(debounced: false)
        var state = zonePlacement ?? ZonePlacementState()
        state.armedZone = state.armedZone == zone ? nil : zone
        zonePlacement = state
    }

    /// Canvas click while placement is active. Armed: the pin takes the zone
    /// picked on the strip. Unarmed: it takes the zone it already reads, so a
    /// bare click meters without moving the print (upstream rule). Beyond
    /// maxPins the nearest pin is replaced.
    func addZonePin(u: Double, v: Double) {
        guard zonePlacement != nil, let session else { return }
        let armed = zonePlacement?.armedZone
        let snapshot = settings
        zonePlacementGeneration += 1
        let generation = zonePlacementGeneration
        Task { [weak self] in
            guard let sample = try? await session.sampleZonePin(settings: snapshot, u: u, v: v)
            else { return }
            var target = armed
            if target == nil {
                guard
                    let z = try? await session.predictedZone(
                        settings: snapshot, valLuma: sample.valLuma,
                        maskVal: sample.maskVal)
                else { return }
                target = (z * 3).rounded() / 3  // snap the metered read to thirds
            }
            guard let self, self.zonePlacementGeneration == generation,
                var state = self.zonePlacement
            else { return }
            let pin = ZonePlacement.Pin(
                nx: u, ny: v, valRGB: sample.valRGB, valLuma: sample.valLuma,
                targetZone: target!, retargeted: armed != nil,
                maskVal: sample.maskVal)
            if state.pins.count < ZonePlacement.maxPins {
                state.pins.append(pin)
            } else {
                let nearest = state.pins.indices.min(by: {
                    let a = state.pins[$0], b = state.pins[$1]
                    return (a.nx - u) * (a.nx - u) + (a.ny - v) * (a.ny - v)
                        < (b.nx - u) * (b.nx - u) + (b.ny - v) * (b.ny - v)
                })!
                state.pins[nearest] = pin
            }
            state.armedZone = nil
            self.zonePlacement = state
            self.resolveZonePlacement()
        }
    }

    /// Drag a pin: re-samples the tone under it. An untargeted pin re-snaps
    /// to the new reading, a retargeted one keeps its zone; the solve waits
    /// for `final` (upstream `move_zone_pin`).
    func moveZonePin(index: Int, u: Double, v: Double, final: Bool) {
        guard let state = zonePlacement, state.pins.indices.contains(index), let session
        else { return }
        let snapshot = settings
        zonePlacementGeneration += 1
        let generation = zonePlacementGeneration
        Task { [weak self] in
            guard let sample = try? await session.sampleZonePin(settings: snapshot, u: u, v: v)
            else { return }
            guard let self, self.zonePlacementGeneration == generation,
                var st = self.zonePlacement, st.pins.indices.contains(index)
            else { return }
            var pin = st.pins[index]
            pin.nx = u
            pin.ny = v
            pin.valRGB = sample.valRGB
            pin.valLuma = sample.valLuma
            pin.maskVal = sample.maskVal
            if !pin.retargeted {
                let z = (try? await session.predictedZone(
                    settings: snapshot, valLuma: sample.valLuma,
                    maskVal: sample.maskVal)) ?? pin.targetZone
                pin.targetZone = (z * 3).rounded() / 3
            }
            guard self.zonePlacementGeneration == generation,
                var latest = self.zonePlacement, latest.pins.indices.contains(index)
            else { return }
            latest.pins[index] = pin
            self.zonePlacement = latest
            if final { self.resolveZonePlacement() }
        }
    }

    func removeZonePin(index: Int) {
        guard var state = zonePlacement, state.pins.indices.contains(index) else { return }
        state.pins.remove(at: index)
        if state.pins.isEmpty {
            state.solution = nil
            state.previewImage = nil
            state.labels = []
        }
        zonePlacement = state
        if !state.pins.isEmpty { resolveZonePlacement() }
    }

    /// Re-solve and refresh: pin labels (what each tone prints at NOW), the
    /// solution, and the solved-look preview render (one derive+render on the
    /// warm tower, like a test-strip patch preview).
    private func resolveZonePlacement() {
        guard let state = zonePlacement, !state.pins.isEmpty, let session else { return }
        let snapshot = settings
        let pins = state.pins
        zonePlacementGeneration += 1
        let generation = zonePlacementGeneration
        zonePlacement?.solving = true
        Task { [weak self] in
            var labels: [String] = []
            for pin in pins {
                let z = (try? await session.predictedZone(settings: snapshot, valLuma: pin.valLuma))
                labels.append(z.map { Densitometry.zoneRoman($0) } ?? "–")
            }
            let solution = (try? await session.solveZonePlacement(settings: snapshot, pins: pins))
                .flatMap { $0 }
            var preview: CGImage?
            if let solution {
                preview = try? await session.render(settings: solution.settings).image
            }
            guard let self, self.zonePlacementGeneration == generation,
                var st = self.zonePlacement
            else { return }
            st.labels = labels
            st.solution = solution ?? nil
            st.previewImage = preview
            st.solving = false
            self.zonePlacement = st
        }
    }

    /// Apply the solution as ONE history entry (upstream: "Apply is one undo
    /// step"). The settings didSet clears the placement state.
    func applyZonePlacement() {
        guard let solution = zonePlacement?.solution else { return }
        pendingHistoryLabel = "Zone placement"
        let before = settings
        settings = solution.settings
        clearZonePlacement()
        if before == settings { pendingHistoryLabel = nil }  // no-op: disarm
    }

    func clearZonePlacement() {
        let owned = zonePlacement != nil
        zonePlacementGeneration += 1
        if zonePlacement != nil { zonePlacement = nil }
        if owned { refreshHQActive(debounced: false) }
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
        if on {
            clearTestStrip()
            clearZonePlacement()
        }
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

    /// Context-menu copy from any frame: the open image's live settings, or
    /// another frame's sidecar (defaults if it has never been edited).
    func copyAdjustments(from url: URL) {
        copiedAdjustments = url == selection ? settings : (SidecarStore.load(for: url) ?? DefaultProfile.settings)
    }

    /// Paste to explicit targets: the open image via the normal history path,
    /// others by rewriting their sidecars (picked up when opened; thumbnails
    /// show the raw negative, so nothing to invalidate).
    func pasteAdjustments(to urls: [URL]) {
        guard !isProfileEditor, let source = copiedAdjustments else { return }
        for url in urls {
            if url == selection {
                pasteAdjustments()
            } else {
                let target = SidecarStore.load(for: url) ?? DefaultProfile.settings
                SidecarStore.save(mergedAdjustments(source, keepingGeometryOf: target), for: url)
            }
        }
    }

    /// The copied adjustments with `target`'s geometry kept in place —
    /// rotation/flip/straighten/crops (and the analysis region's pinned
    /// angle) are per-frame and never pasted.
    private func mergedAdjustments(
        _ source: ExposureSettings, keepingGeometryOf target: ExposureSettings
    ) -> ExposureSettings {
        source.keepingGeometry(of: target)
    }

    func pasteAdjustments() {
        guard let source = copiedAdjustments, selection != nil else { return }
        let before = settings
        pendingHistoryLabel = "Paste adjustments"
        settings = mergedAdjustments(source, keepingGeometryOf: settings)
        if before == settings { pendingHistoryLabel = nil }  // no-op paste: disarm
    }

    /// Edit > Paste Adjustments to Selection: apply to every multi-selected
    /// frame.
    func pasteAdjustmentsToSelection() {
        pasteAdjustments(to: files.filter { multiSelection.contains($0) })
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
        if let selection, !isProfileEditor { SidecarStore.save(settings, for: selection) }

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
                let fileSettings = url == liveURL ? liveSettings : (SidecarStore.load(for: url) ?? DefaultProfile.settings)
                let session = url == liveURL && liveSession != nil
                    ? liveSession! : ImageSession(url: url, pipeline: pipeline)
                do {
                    let encoded = try await session.exportRender(settings: fileSettings)
                    if Task.isCancelled { break }
                    try Exporter.write(
                        encoded, to: options.destinationURL(for: url), options: options,
                        source: url)
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
