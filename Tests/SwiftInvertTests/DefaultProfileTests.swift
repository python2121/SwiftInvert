import Foundation
import Testing

@testable import NegativeKit
@testable import SwiftInvert

/// The built-in house profile's contract (see DefaultProfile.swift): plain
/// settings, adjustments only, cancellable back to stock. When the profile
/// evolves, update ONLY the zeroing list here to match its documented
/// fields — the equality check then proves nothing else drifted (geometry
/// included, since any stray field breaks it).
@Suite struct DefaultProfileTests {
    @Test func profileTouchesOnlyItsDocumentedFields() {
        var s = DefaultProfile.builtIn
        s.exposureStops = 0
        s.highlights = 0
        s.shadows = 0
        s.blackPointOffset = 0
        s.overallContrast = 0
        s.shadowContrast = 0
        #expect(s == ExposureSettings())
    }

    /// The profile must never carry geometry: a fresh frame's crop, rotation
    /// and analysis region are per-frame facts. (Subsumed by the equality
    /// test, but pinned separately so a violation names the actual problem.)
    @Test func profileCarriesNoGeometry() {
        let p = DefaultProfile.builtIn
        #expect(p.cropRect == nil)
        #expect(p.analysisRect == nil)
        #expect(p.rotation == 0)
        #expect(p.flipHorizontal == false)
        #expect(p.fineRotation == 0)
    }

    /// Start-from-scratch semantics: the profile is cancellable by plain
    /// stock settings, which stay NegPy-parity-neutral.
    @Test func profileDiffersFromStock() {
        #expect(DefaultProfile.builtIn != ExposureSettings())
    }

    /// The active resolution: with no user choice, DefaultProfile.settings
    /// IS the built-in.
    @MainActor @Test func activeDefaultsToBuiltIn() {
        let store = ProfileStore(defaults: UserDefaults(suiteName: "test-dp-\(UUID().uuidString)")!)
        #expect(store.active.settings == DefaultProfile.builtIn)
    }
}

/// ProfileStore: persistence, ordering, geometry stripping, built-in
/// protection, active fallback.
@MainActor @Suite struct ProfileStoreTests {
    private func freshDefaults() -> UserDefaults {
        UserDefaults(suiteName: "test-profiles-\(UUID().uuidString)")!
    }

    @Test func builtInIsFirstAndActiveByDefault() {
        let store = ProfileStore(defaults: freshDefaults())
        #expect(store.all.first?.id == ProfileStore.builtInID)
        #expect(store.activeID == ProfileStore.builtInID)
        #expect(store.active.name == "SwiftInvert Default")
    }

    @Test func upsertStripsGeometryAndRoundTrips() {
        let defaults = freshDefaults()
        let store = ProfileStore(defaults: defaults)
        var s = DefaultProfile.builtIn
        s.vibrance = 1.4
        s.cropRect = NormalizedRect(x: 0.1, y: 0.1, width: 0.5, height: 0.5)
        s.rotation = 90
        let id = UUID()
        store.upsert(SettingsProfile(id: id, name: "Warm", settings: s))
        store.setActive(id)

        // Geometry stripped on save; adjustments kept.
        let saved = store.active.settings
        #expect(saved.vibrance == 1.4)
        #expect(saved.cropRect == nil)
        #expect(saved.rotation == 0)

        // A new store over the same defaults sees the same state.
        let reloaded = ProfileStore(defaults: defaults)
        #expect(reloaded.activeID == id)
        #expect(reloaded.active.settings == saved)
        #expect(reloaded.all.count == 2)
    }

    @Test func builtInCannotBeUpsertedOrDeleted() {
        let store = ProfileStore(defaults: freshDefaults())
        var hijack = ProfileStore.builtIn
        hijack.settings.vibrance = 9
        store.upsert(hijack)
        store.delete(ProfileStore.builtInID)
        #expect(store.all.count == 1)
        #expect(store.active.settings == DefaultProfile.builtIn)
    }

    @Test func deletingActiveFallsBackToBuiltIn() {
        let store = ProfileStore(defaults: freshDefaults())
        let id = UUID()
        store.upsert(SettingsProfile(id: id, name: "Temp", settings: ExposureSettings()))
        store.setActive(id)
        store.delete(id)
        #expect(store.activeID == ProfileStore.builtInID)
        #expect(store.userProfiles.isEmpty)
    }

    @Test func keepingGeometryMergesBothWays() {
        var look = ExposureSettings()
        look.exposureStops = 1.0
        var frame = ExposureSettings()
        frame.rotation = 180
        frame.cropRect = NormalizedRect(x: 0, y: 0, width: 0.5, height: 0.5)

        let composed = look.keepingGeometry(of: frame)
        #expect(composed.exposureStops == 1.0)
        #expect(composed.rotation == 180)
        #expect(composed.cropRect == frame.cropRect)
        // And the inverse: adjustmentsOnly drops the frame's geometry.
        #expect(composed.adjustmentsOnly.rotation == 0)
        #expect(composed.adjustmentsOnly.cropRect == nil)
        #expect(composed.adjustmentsOnly.exposureStops == 1.0)
    }
}
