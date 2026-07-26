import AppKit
import SwiftUI

/// Which window's AppModel owns the menu bar and the app-global key
/// monitors. With profile-editor windows the app is multi-window; every
/// ContentView registers itself on window-become-key (WindowKeyObserver),
/// and the App's commands act on `active ?? mainModel`.
@MainActor @Observable
final class KeyModelTracker {
    static let shared = KeyModelTracker()
    weak var active: AppModel?
}

/// Invisible helper view: captures the hosting NSWindow onto the model
/// (`hostWindow`, so the key monitor only consumes events from its own
/// window) and updates KeyModelTracker when that window becomes key.
struct WindowKeyObserver: NSViewRepresentable {
    let model: AppModel

    final class Observer: NSView {
        var model: AppModel?
        private var token: NSObjectProtocol?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let token { NotificationCenter.default.removeObserver(token) }
            token = nil
            guard let window, let model else { return }
            model.hostWindow = window
            if window.isKeyWindow { KeyModelTracker.shared.active = model }
            token = NotificationCenter.default.addObserver(
                forName: NSWindow.didBecomeKeyNotification, object: window, queue: .main
            ) { [weak model] _ in
                MainActor.assumeIsolated { KeyModelTracker.shared.active = model }
            }
        }

        deinit {
            if let token { NotificationCenter.default.removeObserver(token) }
        }
    }

    func makeNSView(context: Context) -> Observer {
        let view = Observer()
        view.model = model
        return view
    }

    func updateNSView(_ view: Observer, context: Context) {
        view.model = model
    }
}

/// Payload for the profile-editor window: which profile to edit (nil =
/// create new) and which one seeds the starting look.
struct ProfileEditRequest: Codable, Hashable {
    var editID: UUID?
    var seedID: UUID
}

/// File → Choose Default Settings…: pick the profile applied to fresh
/// frames (built-in first), or create/edit profiles in a live editor.
struct ProfilePickerView: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openWindow) private var openWindow
    @State private var selectedID: UUID = ProfileStore.shared.activeID

    private var store: ProfileStore { ProfileStore.shared }
    private var selectionIsBuiltIn: Bool { ProfileStore.reservedIDs.contains(selectedID) }

    var body: some View {
        VStack(spacing: 0) {
            List(store.all, selection: Binding(get: { selectedID }, set: { selectedID = $0 ?? selectedID })) { profile in
                HStack {
                    Text(profile.name)
                    if ProfileStore.reservedIDs.contains(profile.id) {
                        Text("built-in").font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if profile.id == store.activeID {
                        Image(systemName: "checkmark.circle.fill").foregroundStyle(.tint)
                            .help("Current default")
                    }
                }
                .tag(profile.id)
            }
            Divider()
            HStack {
                Button("Create New…") {
                    openWindow(value: ProfileEditRequest(editID: nil, seedID: selectedID))
                }
                .help("New profile starting from the selected one")
                Button("Edit…") {
                    openWindow(value: ProfileEditRequest(editID: selectedID, seedID: selectedID))
                }
                .disabled(selectionIsBuiltIn)
                .help(selectionIsBuiltIn ? "The built-in profile evolves with the app and can't be edited — create a new one from it" : "Edit the selected profile in a live editor")
                Button("Delete") {
                    store.delete(selectedID)
                    selectedID = store.activeID
                }
                .disabled(selectionIsBuiltIn)
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Accept") {
                    store.setActive(selectedID)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .help("Use the selected profile as the default for fresh frames")
            }
            .padding(12)
        }
        .frame(minWidth: 420, minHeight: 300)
    }
}

/// The Create/Edit window: the full app UI, but every adjustment applies to
/// EVERY photo (AppModel profile-editor mode — one shared draft follows
/// across frames; geometry stays per-frame and nothing touches sidecars).
/// Accept saves the profile; Cancel discards.
struct ProfileEditorView: View {
    let request: ProfileEditRequest
    @State private var model: AppModel
    @State private var name: String
    @Environment(\.dismiss) private var dismiss

    init(request: ProfileEditRequest) {
        self.request = request
        let seed = ProfileStore.shared.all.first { $0.id == (request.editID ?? request.seedID) }
            ?? ProfileStore.builtIn
        _model = State(initialValue: AppModel(profileEditor: true, profileSeed: seed.settings))
        _name = State(initialValue: request.editID != nil ? seed.name : "\(seed.name) Copy")
    }

    var body: some View {
        ContentView(model: model)
            .navigationTitle("Default Settings Editor — adjustments apply to every photo")
            .toolbar {
                ToolbarItem(placement: .principal) {
                    HStack(spacing: 6) {
                        Text("Profile name:")
                            .foregroundStyle(.secondary)
                        TextField("My default look", text: $name)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 200)
                            .help("The name this profile is listed under in Choose Default Settings")
                    }
                }
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", role: .cancel) { dismiss() }
                        .help("Close without saving the profile")
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Accept") {
                        let trimmed = name.trimmingCharacters(in: .whitespaces)
                        ProfileStore.shared.upsert(
                            SettingsProfile(
                                id: request.editID ?? UUID(),
                                name: trimmed.isEmpty ? "Untitled Profile" : trimmed,
                                settings: model.profileDraft))
                        dismiss()
                    }
                    .buttonStyle(.borderedProminent)
                    .help("Save this profile with the current adjustments")
                }
            }
    }
}
