import AppKit
import SwiftUI

@main
struct SwiftInvertApp: App {
    @State private var model = AppModel()
    // Same keys the in-window controls use, so menu toggles stay in sync.
    @AppStorage("libraryVisible") private var libraryVisible = true
    @AppStorage("showGridLines") private var showGridLines = false
    @AppStorage("showZoneOverlay") private var showZoneOverlay = false
    @Environment(\.openWindow) private var openWindow

    init() {
        // Running unbundled via `swift run` needs an explicit activation policy
        // for the window to appear and take focus.
        NSApplication.shared.setActivationPolicy(.regular)
        // Unbundled (swift run) apps have no Info.plist icon; set it directly.
        if let url = Bundle.module.url(forResource: "AppIcon", withExtension: "png", subdirectory: "Resources"),
            let icon = NSImage(contentsOf: url)
        {
            NSApplication.shared.applicationIconImage = icon
        }
        DispatchQueue.main.async { NSApplication.shared.activate(ignoringOtherApps: true) }
    }

    /// Menu commands act on the key window's model: profile-editor windows
    /// are full app copies, so ⌘Z/arrows/Reset must hit the frontmost one.
    private var keyModel: AppModel { KeyModelTracker.shared.active ?? model }

    private func toolBinding(_ mode: AppModel.ToolMode) -> Binding<Bool> {
        Binding(
            get: { keyModel.toolMode == mode },
            set: { keyModel.toolMode = $0 ? mode : .none })
    }

    var body: some Scene {
        WindowGroup("SwiftInvert") {
            ContentView(model: model)
        }
        .commands {
            // File: the library is folder-based, so Open Folder replaces New.
            CommandGroup(replacing: .newItem) {
                Button("Open Folder…") { keyModel.chooseFolder() }
                    .keyboardShortcut("o")
                Divider()
                Button("Choose Default Settings…") { openWindow(id: "choose-default-settings") }
                Divider()
                Button("Export…") { keyModel.requestExportFromMenu() }
                    .keyboardShortcut("e")
                    .disabled(keyModel.selection == nil || keyModel.isExporting)
                Divider()
                Button("Show in Finder") { keyModel.revealSelectionInFinder() }
                    .keyboardShortcut("r", modifiers: [.command, .shift])
                    .disabled(keyModel.selection == nil)
            }
            // Edit: undo/redo drive the per-image edit history (shortcuts
            // live here, not on the HistoryPanel buttons, so they work
            // whether or not the panel is visible).
            CommandGroup(replacing: .undoRedo) {
                Button("Undo Edit") { keyModel.undo() }
                    .keyboardShortcut("z")
                    .disabled(!keyModel.canUndo && !keyModel.hasUncommittedEdit)
                Button("Redo Edit") { keyModel.redo() }
                    .keyboardShortcut("z", modifiers: [.command, .shift])
                    .disabled(!keyModel.canRedo)
            }
            // View: panel and display toggles.
            CommandGroup(after: .sidebar) {
                Divider()
                Toggle("Show Library", isOn: $libraryVisible)
                    .keyboardShortcut("l", modifiers: [.command, .shift])
                Toggle("Show Grid Lines", isOn: $showGridLines)
                    .keyboardShortcut("g", modifiers: [.command, .shift])
                // ⇧Z is handled by the window key monitor (a bare-letter
                // menu equivalent would fire while typing in text fields).
                Toggle("Zone Overlay", isOn: $showZoneOverlay)
                Toggle("HQ Preview", isOn: Binding(
                    get: { keyModel.hqPreview },
                    set: { keyModel.hqPreview = $0 }))
                    .keyboardShortcut("p", modifiers: [.command, .shift])
                    .disabled(keyModel.selection == nil)
            }
            // Image: orientation + the two draw-on-image tools (checkmarked
            // while active; Escape also exits them).
            CommandMenu("Image") {
                // Bare-key menu equivalents intercept before responders, so
                // disable while the export sheet (with its text field) is up.
                // ←/→ (like ↑/↓) live in the window key monitor, which can
                // check the first responder — a bare menu equivalent fires
                // even while typing in a text field.
                Button("Previous Image") { keyModel.selectAdjacent(-1) }
                    .disabled(keyModel.files.isEmpty || keyModel.exportRequest != nil)
                Button("Next Image") { keyModel.selectAdjacent(1) }
                    .disabled(keyModel.files.isEmpty || keyModel.exportRequest != nil)
                Divider()
                Button("Rotate Left") { keyModel.rotateCounterclockwise() }
                    .keyboardShortcut("[")
                    .disabled(keyModel.selection == nil)
                Button("Rotate Right") { keyModel.rotateClockwise() }
                    .keyboardShortcut("]")
                    .disabled(keyModel.selection == nil)
                Button("Flip Horizontal") { keyModel.flipHorizontal() }
                    .keyboardShortcut("h", modifiers: [.command, .shift])
                    .disabled(keyModel.selection == nil)
                Divider()
                Toggle("Crop or Straighten", isOn: toolBinding(.crop))
                    .keyboardShortcut("k")
                    .disabled(keyModel.selection == nil)
                Button("Clear Crop & Straighten") {
                    keyModel.clearCropAndStraighten()
                }
                .disabled(
                    keyModel.settings.cropRect == nil
                        && abs(keyModel.settings.fineRotation) < 1e-9)
                Divider()
                Toggle("Crop for Analysis", isOn: toolBinding(.analysisRegion))
                    .keyboardShortcut("k", modifiers: [.command, .shift])
                    .disabled(keyModel.selection == nil)
                Button("Clear Analysis Region") {
                    keyModel.pendingHistoryLabel = "Analysis region cleared"
                    keyModel.settings.analysisRect = nil
                }
                .disabled(keyModel.settings.analysisRect == nil)
            }
            CommandGroup(after: .pasteboard) {
                Divider()
                Button("Copy Adjustments") { keyModel.copyAdjustments() }
                    .keyboardShortcut("c", modifiers: [.command, .shift])
                    .disabled(keyModel.selection == nil)
                Button("Paste Adjustments") { keyModel.pasteAdjustments() }
                    .keyboardShortcut("v", modifiers: [.command, .shift])
                    .disabled(keyModel.selection == nil || keyModel.copiedAdjustments == nil)
                Button("Paste Adjustments to Selection") { keyModel.pasteAdjustmentsToSelection() }
                    .keyboardShortcut("v", modifiers: [.command, .shift, .option])
                    .disabled(keyModel.multiSelection.isEmpty || keyModel.copiedAdjustments == nil)
                Divider()
                Button("Reset All") { keyModel.resetSettings() }
                    .keyboardShortcut("r", modifiers: [.command, .option])
                    .disabled(keyModel.selection == nil)
            }
        }

        // Sidebar histogram's expand button: per-channel levels remap panel
        // (edits the key window's model, captured at open).
        Window("Interactive Histogram", id: "interactive-histogram") {
            InteractiveHistogramView(fallback: model)
        }

        // File → Choose Default Settings…: profile picker + live editors.
        Window("Choose Default Settings", id: "choose-default-settings") {
            ProfilePickerView()
        }
        .windowResizability(.contentSize)

        WindowGroup("Default Settings Editor", for: ProfileEditRequest.self) { $request in
            if let request {
                ProfileEditorView(request: request)
            }
        }
    }
}

struct ContentView: View {
    @Bindable var model: AppModel
    @AppStorage("libraryWidth") private var libraryWidth = 320.0
    @AppStorage("libraryVisible") private var libraryVisible = true
    @AppStorage("showZoneOverlay") private var showZoneOverlay = false
    @State private var dragStartWidth: Double?
    /// Local keyDown monitor: Escape/Return for the tool modes, ↑/↓ for frame
    /// navigation (menu items hold the ←/→ equivalents; an item takes only
    /// one shortcut, so the vertical pair lives here).
    @State private var keyMonitor: Any?

    var body: some View {
        // Plain three-pane layout: the library is a solid panel like the
        // adjustments sidebar (no NavigationSplitView vibrancy overlay).
        HStack(spacing: 0) {
            if libraryVisible {
                LibraryView(model: model, onToggleVisibility: { libraryVisible = false })
                    .frame(width: libraryWidth)
                librarySplitter
            }
            DetailView(model: model)
                .frame(minWidth: 400, maxWidth: .infinity, maxHeight: .infinity)
                .overlay(alignment: .topLeading) {
                    if !libraryVisible {
                        Button {
                            libraryVisible = true
                        } label: {
                            Image(systemName: "sidebar.left")
                        }
                        .buttonStyle(.borderless)
                        .padding(8)
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6))
                        .padding(10)
                        .help("Show library")
                    }
                }
            Divider()
            ControlsSidebar(model: model)
        }
        .animation(.easeOut(duration: 0.15), value: libraryVisible)
        .background(WindowKeyObserver(model: model))
        .onExitCommand {
            // Same semantics as the key monitor's Escape: crop mode CANCELS
            // (restores angle + box); committing is Return's job.
            if model.toolMode == .crop {
                model.cancelCropMode()
            } else {
                model.toolMode = .none
            }
        }
        // onExitCommand needs focus; a local monitor catches Escape anywhere
        // in the window. Pass-through unless a tool mode is active, so sheets
        // and text fields keep their own Escape behavior.
        .onAppear {
            guard keyMonitor == nil else { return }
            // Escape CANCELS a tool mode (crop: angle + box restored to
            // mode-entry values); Return/Enter accepts. ↑/↓ walk the film
            // strip like the ←/→ menu equivalents (↑ = previous, ↓ = next —
            // the strip is a vertical column, so both axes should read).
            keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak model] event in
                guard let model else { return event }
                let isEscape = event.keyCode == 53
                let isAccept = event.keyCode == 36 || event.keyCode == 76
                let isUp = event.keyCode == 126
                let isDown = event.keyCode == 125
                let isLeft = event.keyCode == 123
                let isRight = event.keyCode == 124
                // ⇧Z zone overlay lives HERE, not as a menu equivalent: a
                // bare-letter equivalent intercepts before responders and
                // would fire while typing in any text field — the monitor
                // can check the first responder instead.
                let mods = event.modifierFlags.intersection([.shift, .command, .option, .control])
                let isZoneToggle = event.keyCode == 6 && mods == .shift
                let isStripToggle = event.keyCode == 17 && mods == .shift
                let isEditingText = event.window?.firstResponder is NSTextView
                guard isEscape || isAccept || isUp || isDown || isLeft || isRight
                    || isZoneToggle || isStripToggle
                else { return event }
                // Monitors fire on the main thread; only Sendable values
                // cross the isolation boundary (NSEvent is not).
                let eventWindow = event.window
                let consumed = MainActor.assumeIsolated { () -> Bool in
                    // Local monitors are app-global and every window's
                    // ContentView installs one — act only on events from
                    // THIS view's window (profile editors are full copies).
                    guard let host = model.hostWindow, eventWindow === host else { return false }
                    // The export sheet owns all of these while it's up
                    // (Return = default button, Escape = Cancel, arrows =
                    // its text fields).
                    guard model.exportRequest == nil else { return false }
                    if isZoneToggle {
                        guard !isEditingText, model.selection != nil else { return false }
                        showZoneOverlay.toggle()
                        return true
                    }
                    if isStripToggle {
                        guard !isEditingText, model.selection != nil else { return false }
                        model.toggleTestStrip()
                        return true
                    }
                    // Escape clears a showing/building test strip before its
                    // other meanings (tool modes can't coexist with it).
                    if isEscape, model.testStrip != nil || model.testStripBuilding {
                        model.clearTestStrip()
                        return true
                    }
                    if isUp || isDown || isLeft || isRight {
                        // Frame navigation — but never while a text field has
                        // focus (arrows must move the caret in the profile
                        // name / export fields, not switch images).
                        guard !isEditingText, !model.files.isEmpty else { return false }
                        model.selectAdjacent((isUp || isLeft) ? -1 : 1)
                        return true
                    }
                    guard model.toolMode != .none else {
                        // Editor windows: bare Escape (no tool active) starts
                        // the save/discard/close flow in ProfileEditorView.
                        if isEscape, model.isProfileEditor {
                            model.profileEditorEscape = true
                            return true
                        }
                        return false
                    }
                    if isEscape, model.toolMode == .crop {
                        model.cancelCropMode()
                    } else {
                        model.toolMode = .none
                    }
                    return true
                }
                return consumed ? nil : event
            }
        }
        .onDisappear {
            // Local monitors are app-global: every window's ContentView adds
            // one, so a closed profile editor must remove its own or every
            // keypress runs through dead monitors (and the closure would pin
            // the editor's AppModel forever).
            if let keyMonitor {
                NSEvent.removeMonitor(keyMonitor)
                self.keyMonitor = nil
            }
        }
        .sheet(item: $model.exportRequest) { request in
            ExportSheet(request: request, model: model)
        }
        .frame(minWidth: 1000, minHeight: 700)
    }

    /// Draggable divider between the library and the image (width persisted).
    private var librarySplitter: some View {
        Divider()
            .frame(width: 7)
            .contentShape(Rectangle())
            .onHover { inside in
                if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() }
            }
            .gesture(
                // Global space: the splitter itself moves during the drag, so a
                // local-space translation oscillates (measured against a frame
                // that shifts under the cursor) and the layout jitters.
                DragGesture(minimumDistance: 1, coordinateSpace: .global)
                    .onChanged { g in
                        if dragStartWidth == nil { dragStartWidth = libraryWidth }
                        libraryWidth = (dragStartWidth! + g.translation.width)
                            .rounded()
                            .clamped(to: 200...560)
                    }
                    .onEnded { _ in dragStartWidth = nil }
            )
    }
}


extension Double {
    fileprivate func clamped(to range: ClosedRange<Double>) -> Double {
        Swift.min(Swift.max(self, range.lowerBound), range.upperBound)
    }
}
