import Foundation
import Testing

@testable import NegativeKit
@testable import SwiftInvert

/// SidecarStore's file behavior over a temp directory: path shapes, the
/// legacy `.negswift.json` fallback, and its removal on save.
@Suite struct SidecarStoreTests {

    /// Fresh directory per test — the suite runs concurrently.
    private func makeSourceURL() throws -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("SidecarStoreTests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("IMG_0001.CR3")
    }

    @Test func pathShapes() {
        let source = URL(fileURLWithPath: "/roll/IMG_0001.CR3")
        #expect(SidecarStore.sidecarURL(for: source).path == "/roll/IMG_0001.swiftinvert.json")
        #expect(SidecarStore.legacySidecarURL(for: source).path == "/roll/IMG_0001.negswift.json")
    }

    @Test func saveThenLoadRoundTrips() throws {
        let source = try makeSourceURL()
        var settings = ExposureSettings()
        settings.grade = 140
        settings.cropRect = NormalizedRect(x: 0.1, y: 0.1, width: 0.8, height: 0.8)
        SidecarStore.save(settings, for: source)
        #expect(SidecarStore.load(for: source) == settings)
    }

    @Test func missingSidecarLoadsNil() throws {
        #expect(SidecarStore.load(for: try makeSourceURL()) == nil)
    }

    @Test func corruptSidecarLoadsNil() throws {
        let source = try makeSourceURL()
        try Data("not json".utf8).write(to: SidecarStore.sidecarURL(for: source))
        #expect(SidecarStore.load(for: source) == nil)
    }

    @Test func legacySidecarIsReadAsFallback() throws {
        let source = try makeSourceURL()
        var settings = ExposureSettings()
        settings.density = 0.7
        let payload = try JSONEncoder().encode(SidecarStore.Payload(settings: settings))
        try payload.write(to: SidecarStore.legacySidecarURL(for: source))
        #expect(SidecarStore.load(for: source)?.density == 0.7)
    }

    @Test func newSidecarWinsOverLegacy() throws {
        let source = try makeSourceURL()
        var old = ExposureSettings()
        old.density = 0.7
        var new = ExposureSettings()
        new.density = 1.3
        try JSONEncoder().encode(SidecarStore.Payload(settings: old))
            .write(to: SidecarStore.legacySidecarURL(for: source))
        try JSONEncoder().encode(SidecarStore.Payload(settings: new))
            .write(to: SidecarStore.sidecarURL(for: source))
        #expect(SidecarStore.load(for: source)?.density == 1.3)
    }

    /// Saving migrates: the legacy file is deleted so edits can't fork across
    /// two sidecars.
    @Test func saveRemovesTheLegacySidecar() throws {
        let source = try makeSourceURL()
        try JSONEncoder().encode(SidecarStore.Payload(settings: ExposureSettings()))
            .write(to: SidecarStore.legacySidecarURL(for: source))
        SidecarStore.save(ExposureSettings(), for: source)
        #expect(!FileManager.default.fileExists(atPath: SidecarStore.legacySidecarURL(for: source).path))
        #expect(FileManager.default.fileExists(atPath: SidecarStore.sidecarURL(for: source).path))
    }
}
