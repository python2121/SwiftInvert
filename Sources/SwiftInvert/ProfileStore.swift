import Foundation
import NegativeKit
import Observation

/// A named default-settings profile: adjustments only — geometry (crop,
/// rotation, straighten, analysis region) is a per-frame fact and is
/// stripped on save (`adjustmentsOnly`).
struct SettingsProfile: Codable, Identifiable, Equatable, Sendable {
    var id: UUID
    var name: String
    var settings: ExposureSettings
}

extension ExposureSettings {
    /// Copy with `target`'s geometry kept in place — the single list of
    /// what counts as geometry (shared by Paste Adjustments, the profile
    /// editor's per-frame compose, and profile stripping).
    func keepingGeometry(of target: ExposureSettings) -> ExposureSettings {
        var next = self
        next.rotation = target.rotation
        next.flipHorizontal = target.flipHorizontal
        next.fineRotation = target.fineRotation
        next.cropRect = target.cropRect
        next.analysisRect = target.analysisRect
        next.analysisRectFineRotation = target.analysisRectFineRotation
        return next
    }

    /// Adjustments with stock geometry — what a profile stores.
    var adjustmentsOnly: ExposureSettings { keepingGeometry(of: ExposureSettings()) }
}

/// The profile library and which profile is the app's default look.
///
/// The built-in profile (`DefaultProfile.builtIn`, "SwiftInvert Default") is
/// always first, resolved from code each launch — never persisted, never
/// editable, so it evolves with the app. User profiles + the active choice
/// persist as JSON in UserDefaults (`settingsProfiles`/`activeProfileID`,
/// same pattern as `exportOptions`). Everything that asks "what does a
/// fresh frame get?" reads `DefaultProfile.settings`, which resolves
/// through `ProfileStore.shared.active` — so Accept in the picker changes
/// the app's default look immediately, everywhere.
@MainActor @Observable
final class ProfileStore {
    static let shared = ProfileStore()

    static let builtInID = UUID(uuidString: "00000000-0000-0000-0000-000000000001")!
    static var builtIn: SettingsProfile {
        SettingsProfile(id: builtInID, name: "SwiftInvert Default", settings: DefaultProfile.builtIn)
    }

    private let defaults: UserDefaults
    private(set) var userProfiles: [SettingsProfile] = []
    private(set) var activeID: UUID = ProfileStore.builtInID

    /// Built-in first, then user profiles in creation order.
    var all: [SettingsProfile] { [Self.builtIn] + userProfiles }

    var active: SettingsProfile {
        all.first { $0.id == activeID } ?? Self.builtIn
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        if let data = defaults.data(forKey: "settingsProfiles"),
            let decoded = try? JSONDecoder().decode([SettingsProfile].self, from: data)
        {
            userProfiles = decoded
        }
        if let raw = defaults.string(forKey: "activeProfileID"), let id = UUID(uuidString: raw),
            all.contains(where: { $0.id == id })
        {
            activeID = id
        }
    }

    func setActive(_ id: UUID) {
        guard all.contains(where: { $0.id == id }) else { return }
        activeID = id
        persist()
    }

    /// Insert or update a user profile (geometry stripped here, so no caller
    /// can persist a crop into a profile). The built-in id is refused.
    func upsert(_ profile: SettingsProfile) {
        guard profile.id != Self.builtInID else { return }
        var stripped = profile
        stripped.settings = profile.settings.adjustmentsOnly
        if let i = userProfiles.firstIndex(where: { $0.id == stripped.id }) {
            userProfiles[i] = stripped
        } else {
            userProfiles.append(stripped)
        }
        persist()
    }

    /// Remove a user profile; deleting the active one falls back to built-in.
    func delete(_ id: UUID) {
        guard id != Self.builtInID else { return }
        userProfiles.removeAll { $0.id == id }
        if activeID == id { activeID = Self.builtInID }
        persist()
    }

    private func persist() {
        if let data = try? JSONEncoder().encode(userProfiles) {
            defaults.set(data, forKey: "settingsProfiles")
        }
        defaults.set(activeID.uuidString, forKey: "activeProfileID")
    }
}
