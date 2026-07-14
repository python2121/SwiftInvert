import AppKit
import SwiftUI

@main
struct SwiftInvertApp: App {
    @State private var model = AppModel()
    // Same keys the in-window controls use, so menu toggles stay in sync.
    @AppStorage("libraryVisible") private var libraryVisible = true
    @AppStorage("showGridLines") private var showGridLines = false

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

    private func toolBinding(_ mode: AppModel.ToolMode) -> Binding<Bool> {
        Binding(
            get: { model.toolMode == mode },
            set: { model.toolMode = $0 ? mode : .none })
    }

    var body: some Scene {
        WindowGroup("SwiftInvert") {
            ContentView(model: model)
        }
        .commands {
            // File: the library is folder-based, so Open Folder replaces New.
            CommandGroup(replacing: .newItem) {
                Button("Open Folder…") { model.chooseFolder() }
                    .keyboardShortcut("o")
                Divider()
                Button("Export…") { model.requestExportFromMenu() }
                    .keyboardShortcut("e")
                    .disabled(model.selection == nil || model.isExporting)
                Divider()
                Button("Show in Finder") { model.revealSelectionInFinder() }
                    .keyboardShortcut("r", modifiers: [.command, .shift])
                    .disabled(model.selection == nil)
            }
            // Edit: undo/redo drive the per-image edit history (shortcuts
            // live here, not on the HistoryPanel buttons, so they work
            // whether or not the panel is visible).
            CommandGroup(replacing: .undoRedo) {
                Button("Undo Edit") { model.undo() }
                    .keyboardShortcut("z")
                    .disabled(!model.canUndo)
                Button("Redo Edit") { model.redo() }
                    .keyboardShortcut("z", modifiers: [.command, .shift])
                    .disabled(!model.canRedo)
            }
            // View: panel and display toggles.
            CommandGroup(after: .sidebar) {
                Divider()
                Toggle("Show Library", isOn: $libraryVisible)
                    .keyboardShortcut("l", modifiers: [.command, .shift])
                Toggle("Show Grid Lines", isOn: $showGridLines)
                    .keyboardShortcut("g", modifiers: [.command, .shift])
                Toggle("HQ Preview", isOn: Binding(
                    get: { model.hqPreview },
                    set: { model.hqPreview = $0 }))
                    .keyboardShortcut("p", modifiers: [.command, .shift])
                    .disabled(model.selection == nil)
            }
            // Image: orientation + the two draw-on-image tools (checkmarked
            // while active; Escape also exits them).
            CommandMenu("Image") {
                // Bare-key menu equivalents intercept before responders, so
                // disable while the export sheet (with its text field) is up.
                Button("Previous Image") { model.selectAdjacent(-1) }
                    .keyboardShortcut(.leftArrow, modifiers: [])
                    .disabled(model.files.isEmpty || model.exportRequest != nil)
                Button("Next Image") { model.selectAdjacent(1) }
                    .keyboardShortcut(.rightArrow, modifiers: [])
                    .disabled(model.files.isEmpty || model.exportRequest != nil)
                Divider()
                Button("Rotate Left") { model.rotateCounterclockwise() }
                    .keyboardShortcut("[")
                    .disabled(model.selection == nil)
                Button("Rotate Right") { model.rotateClockwise() }
                    .keyboardShortcut("]")
                    .disabled(model.selection == nil)
                Button("Flip Horizontal") { model.flipHorizontal() }
                    .keyboardShortcut("h", modifiers: [.command, .shift])
                    .disabled(model.selection == nil)
                Divider()
                Toggle("Crop", isOn: toolBinding(.crop))
                    .keyboardShortcut("k")
                    .disabled(model.selection == nil)
                Button("Clear Crop") {
                    model.pendingHistoryLabel = "Crop cleared"
                    model.settings.cropRect = nil
                }
                .disabled(model.settings.cropRect == nil)
                Divider()
                Toggle("Crop for Analysis", isOn: toolBinding(.analysisRegion))
                    .keyboardShortcut("k", modifiers: [.command, .shift])
                    .disabled(model.selection == nil)
                Button("Clear Analysis Region") {
                    model.pendingHistoryLabel = "Analysis region cleared"
                    model.settings.analysisRect = nil
                }
                .disabled(model.settings.analysisRect == nil)
            }
            CommandGroup(after: .pasteboard) {
                Divider()
                Button("Copy Adjustments") { model.copyAdjustments() }
                    .keyboardShortcut("c", modifiers: [.command, .shift])
                    .disabled(model.selection == nil)
                Button("Paste Adjustments") { model.pasteAdjustments() }
                    .keyboardShortcut("v", modifiers: [.command, .shift])
                    .disabled(model.selection == nil || model.copiedAdjustments == nil)
                Button("Paste Adjustments to Selection") { model.pasteAdjustmentsToSelection() }
                    .keyboardShortcut("v", modifiers: [.command, .shift, .option])
                    .disabled(model.multiSelection.isEmpty || model.copiedAdjustments == nil)
                Divider()
                Button("Reset All Adjustments") { model.resetSettings() }
                    .keyboardShortcut("r", modifiers: [.command, .option])
                    .disabled(model.selection == nil)
            }
        }
    }
}

struct ContentView: View {
    @Bindable var model: AppModel
    @AppStorage("libraryWidth") private var libraryWidth = 320.0
    @AppStorage("libraryVisible") private var libraryVisible = true
    @State private var dragStartWidth: Double?
    @State private var escapeMonitor: Any?

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
        .onExitCommand { model.toolMode = .none }
        // onExitCommand needs focus; a local monitor catches Escape anywhere
        // in the window. Pass-through unless a tool mode is active, so sheets
        // and text fields keep their own Escape behavior.
        .onAppear {
            guard escapeMonitor == nil else { return }
            // Escape backs out of a tool mode; Return/Enter accepts (both
            // commit — crop commits on mode exit either way).
            escapeMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
                guard event.keyCode == 53 || event.keyCode == 36 || event.keyCode == 76 else {
                    return event
                }
                // Monitors fire on the main thread; only a Bool crosses the
                // isolation boundary (NSEvent is not Sendable).
                let consumed = MainActor.assumeIsolated { () -> Bool in
                    // The export sheet owns Return (its default button) and
                    // Escape (Cancel) while it's up.
                    guard model.toolMode != .none, model.exportRequest == nil else { return false }
                    model.toolMode = .none
                    return true
                }
                return consumed ? nil : event
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
