import Foundation
import Testing

@testable import NegativeKit

/// The sidecar codec's standing contracts, exhaustively. Individual feature
/// suites already round-trip their own slices; this suite is the drift-catcher
/// for the struct as a whole, so "added a field, forgot a list" fails loudly.
@Suite struct SidecarCodecTests {

    /// Every stored property set to a non-default value. When a field is added
    /// to ExposureSettings, `storedFieldCountIsPinned` fails first; extending
    /// this helper is step two, and `everyFieldSurvivesTheRoundTrip` then
    /// verifies the new decoder line actually decodes it.
    static func fullyMutated() -> ExposureSettings {
        var s = ExposureSettings()
        s.density = 0.8
        s.grade = 90
        s.wbCyan = 0.1
        s.wbMagenta = -0.2
        s.wbYellow = 0.3
        s.autoExposure = false
        s.autoNormalizeContrast = false
        s.castRemovalStrength = 0.9
        s.whitePointOffset = 0.05
        s.blackPointOffset = -0.04
        s.toe = 0.4
        s.toeWidth = 3.0
        s.shoulder = -0.3
        s.shoulderWidth = 2.0
        s.paperDmin = true
        s.trueBlack = false
        s.exposureStops = 0.5
        s.shadows = 0.7
        s.shadowContrast = -0.6
        s.darkShadows = 0.2
        s.highlights = -0.8
        s.highlightContrast = 0.4
        s.overallContrast = 0.3
        s.vibrance = 1.2
        s.saturation = 0.9
        s.redHue = -0.25
        s.redSaturation = 1.1
        s.yellowHue = 0.6
        s.yellowSaturation = 0.8
        s.greenHue = -0.4
        s.greenSaturation = 1.3
        s.blueHue = 0.7
        s.blueSaturation = 0.6
        s.preSaturation = 1.0
        s.temp = 0.35
        s.tint = -0.15
        s.colorShadows = SIMD3(0.1, -0.2, 0.3)
        s.colorMids = SIMD3(-0.1, 0.2, -0.3)
        s.colorHighs = SIMD3(0.05, 0.15, -0.25)
        s.rotation = 90
        s.flipHorizontal = true
        s.fineRotation = 1.5
        s.analysisRect = NormalizedRect(x: 0.1, y: 0.2, width: 0.5, height: 0.4)
        s.analysisRectFineRotation = 2.5
        s.cropRect = NormalizedRect(x: 0.05, y: 0.05, width: 0.9, height: 0.85)
        return s
    }

    /// The tripwire. A new stored property means THREE hand-maintained lists
    /// need a line: the custom `init(from:)` (sidecar back-compat), the
    /// `historyLabel` diff (HistoryLabels.swift), and `fullyMutated()` above.
    /// Update all three, then this count.
    @Test func storedFieldCountIsPinned() {
        #expect(Mirror(reflecting: ExposureSettings()).children.count == 45)
    }

    @Test func fullyMutatedActuallyMutatesEveryField() throws {
        // Compare the two encodings key by key: any key whose value matches the
        // default's means fullyMutated() missed that field and the round-trip
        // below would vacuously pass for it. Optionals (rects) are absent from
        // the default encoding, so presence alone proves mutation there.
        func json(_ s: ExposureSettings) throws -> [String: NSObject] {
            let obj = try JSONSerialization.jsonObject(with: JSONEncoder().encode(s))
            return try #require(obj as? [String: NSObject])
        }
        let defaults = try json(ExposureSettings())
        let mutated = try json(Self.fullyMutated())
        #expect(mutated.count == 45)
        for (key, value) in mutated {
            #expect(defaults[key] != value, "fullyMutated() left \(key) at its default")
        }
    }

    @Test func everyFieldSurvivesTheRoundTrip() throws {
        let original = Self.fullyMutated()
        let back = try JSONDecoder().decode(
            ExposureSettings.self, from: JSONEncoder().encode(original))
        #expect(back == original)
    }

    /// The back-compat contract: sidecars written before a control existed
    /// omit its key, and every missing key must decode to the field default —
    /// for ALL fields at once, not just the one a feature suite remembers.
    @Test func emptySidecarDecodesToAllDefaults() throws {
        let decoded = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(decoded == ExposureSettings())
    }

    @Test func partialSidecarKeepsUnmentionedDefaults() throws {
        let legacy = Data(#"{"density": 0.7, "grade": 140, "rotation": 180}"#.utf8)
        let decoded = try JSONDecoder().decode(ExposureSettings.self, from: legacy)
        var expected = ExposureSettings()
        expected.density = 0.7
        expected.grade = 140
        expected.rotation = 180
        #expect(decoded == expected)
    }

    /// A future field's key must not break today's decoder (forward compat:
    /// opening a library with sidecars written by a newer build).
    @Test func unknownKeysAreIgnored() throws {
        let future = Data(#"{"density": 0.9, "someFutureControl": 42}"#.utf8)
        let decoded = try JSONDecoder().decode(ExposureSettings.self, from: future)
        #expect(decoded.density == 0.9)
    }

    /// Never asserted anywhere before this suite (audit 2026-07-15): the angle
    /// an analysis region was drawn at must survive the sidecar.
    @Test func analysisRectFineRotationRoundTrips() throws {
        var s = ExposureSettings()
        s.analysisRect = NormalizedRect(x: 0.2, y: 0.2, width: 0.4, height: 0.4)
        s.analysisRectFineRotation = -1.75
        let back = try JSONDecoder().decode(ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back.analysisRectFineRotation == -1.75)
        // And pre-straighten sidecars default it to 0.
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.analysisRectFineRotation == 0)
    }
}
