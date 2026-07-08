import Foundation
import NegativeKit

/// Edits persist as `<basename>.negswift.json` next to the source (the same
/// pattern as NegPy's `.negpy` sidecars): human-inspectable, survives library
/// moves, zero infrastructure.
enum SidecarStore {
    struct Payload: Codable {
        var version = 1
        var settings: ExposureSettings
    }

    static func sidecarURL(for source: URL) -> URL {
        source.deletingPathExtension().appendingPathExtension("negswift.json")
    }

    static func load(for source: URL) -> ExposureSettings? {
        let url = sidecarURL(for: source)
        guard let data = try? Data(contentsOf: url),
            let payload = try? JSONDecoder().decode(Payload.self, from: data)
        else { return nil }
        return payload.settings
    }

    static func save(_ settings: ExposureSettings, for source: URL) {
        let url = sidecarURL(for: source)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(Payload(settings: settings)) else { return }
        try? data.write(to: url, options: .atomic)
    }
}
