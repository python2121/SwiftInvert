import AppKit
import SwiftUI

@main
struct SwiftInvertApp: App {
    @State private var model = AppModel()

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

    var body: some Scene {
        WindowGroup("SwiftInvert") {
            ContentView(model: model)
        }
    }
}

struct ContentView: View {
    @Bindable var model: AppModel
    @AppStorage("libraryWidth") private var libraryWidth = 320.0
    @AppStorage("libraryVisible") private var libraryVisible = true
    @AppStorage("controlsWidth") private var controlsWidth = 260.0
    @State private var dragStartWidth: Double?
    @State private var controlsDragStartWidth: Double?

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
            controlsSplitter
            ControlsSidebar(model: model, width: controlsWidth)
        }
        .animation(.easeOut(duration: 0.15), value: libraryVisible)
        .onExitCommand { model.toolMode = .none }
        .sheet(item: $model.exportRequest) { request in
            ExportSheet(request: request, model: model)
        }
        .frame(minWidth: 1000, minHeight: 700)
    }

    /// Draggable divider between the image and the adjustments sidebar
    /// (width persisted; drag left = wider, mirroring the library splitter).
    private var controlsSplitter: some View {
        Divider()
            .frame(width: 7)
            .contentShape(Rectangle())
            .onHover { inside in
                if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() }
            }
            .gesture(
                DragGesture(minimumDistance: 1, coordinateSpace: .global)
                    .onChanged { g in
                        if controlsDragStartWidth == nil { controlsDragStartWidth = controlsWidth }
                        controlsWidth = (controlsDragStartWidth! - g.translation.width)
                            .rounded()
                            .clamped(to: 235...420)
                    }
                    .onEnded { _ in controlsDragStartWidth = nil }
            )
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
