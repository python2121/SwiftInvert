import Foundation
import NegativeKit

/// Edits persist as `<basename>.swiftinvert.json` next to the source (the same
/// pattern as NegPy's `.negpy` sidecars): human-inspectable, survives library
/// moves, zero infrastructure. Sidecars written before the SwiftInvert rename
/// (`.negswift.json`) are still read as a fallback.
enum SidecarStore {
    struct Payload: Codable {
        var version = 1
        var settings: ExposureSettings
    }

    static func sidecarURL(for source: URL) -> URL {
        source.deletingPathExtension().appendingPathExtension("swiftinvert.json")
    }

    static func legacySidecarURL(for source: URL) -> URL {
        source.deletingPathExtension().appendingPathExtension("negswift.json")
    }

    static func load(for source: URL) -> ExposureSettings? {
        for url in [sidecarURL(for: source), legacySidecarURL(for: source)] {
            if let data = try? Data(contentsOf: url),
                let payload = try? JSONDecoder().decode(Payload.self, from: data)
            {
                return payload.settings
            }
        }
        return nil
    }

    static func save(_ settings: ExposureSettings, for source: URL) {
        let url = sidecarURL(for: source)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(Payload(settings: settings)) else { return }
        try? data.write(to: url, options: .atomic)
        // A stale legacy sidecar would shadow nothing (new wins on load), but
        // remove it so edits don't fork across two files.
        try? FileManager.default.removeItem(at: legacySidecarURL(for: source))
    }
}
