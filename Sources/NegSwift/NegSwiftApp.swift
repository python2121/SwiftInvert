import AppKit
import SwiftUI

@main
struct NegSwiftApp: App {
    @State private var model = AppModel()

    init() {
        // Running unbundled via `swift run` needs an explicit activation policy
        // for the window to appear and take focus.
        NSApplication.shared.setActivationPolicy(.regular)
        DispatchQueue.main.async { NSApplication.shared.activate(ignoringOtherApps: true) }
    }

    var body: some Scene {
        WindowGroup("NegSwift") {
            ContentView(model: model)
        }
    }
}

struct ContentView: View {
    @Bindable var model: AppModel

    var body: some View {
        NavigationSplitView {
            LibraryView(model: model)
                .navigationSplitViewColumnWidth(min: 200, ideal: 340)
        } detail: {
            HSplitView {
                DetailView(model: model)
                    .frame(minWidth: 400, maxWidth: .infinity, maxHeight: .infinity)
                ControlsSidebar(model: model)
            }
        }
        .frame(minWidth: 1100, minHeight: 700)
    }
}

struct DetailView: View {
    @Bindable var model: AppModel

    var body: some View {
        ZStack {
            Color(nsColor: .underPageBackgroundColor)
            if let image = model.displayImage {
                Image(decorative: image, scale: 1.0)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .padding(12)
            } else if model.selection != nil {
                ProgressView("Developing…")
            } else {
                ContentUnavailableView(
                    "No image selected", systemImage: "film",
                    description: Text("Select a negative from the library."))
            }
        }
    }
}
