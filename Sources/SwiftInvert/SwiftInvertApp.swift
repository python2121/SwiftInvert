import AppKit
import SwiftUI

@main
struct SwiftInvertApp: App {
    @State private var model = AppModel()

    init() {
        // Running unbundled via `swift run` needs an explicit activation policy
        // for the window to appear and take focus.
        NSApplication.shared.setActivationPolicy(.regular)
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

    var body: some View {
        // Plain three-pane layout: the library is a solid panel like the
        // adjustments sidebar (no NavigationSplitView vibrancy overlay).
        HStack(spacing: 0) {
            LibraryView(model: model)
            Divider()
            DetailView(model: model)
                .frame(minWidth: 400, maxWidth: .infinity, maxHeight: .infinity)
            Divider()
            ControlsSidebar(model: model)
        }
        .onExitCommand { model.toolMode = .none }
        .sheet(item: $model.exportRequest) { request in
            ExportSheet(request: request, model: model)
        }
        .frame(minWidth: 1000, minHeight: 700)
    }
}
