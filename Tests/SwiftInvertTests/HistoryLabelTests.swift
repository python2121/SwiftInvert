import Foundation
import Testing

@testable import NegativeKit
@testable import SwiftInvert

/// `historyLabel` is a hand-maintained field-by-field diff — exactly the kind
/// of list that drifts when a settings field is added. The exhaustive test
/// makes the drift loud: every stored property, mutated alone, must produce a
/// real label (the "Edit" fallback means the diff missed the field).
@Suite struct HistoryLabelTests {

    /// One single-field mutation per stored property of ExposureSettings.
    static let mutations: [(field: String, mutate: (inout ExposureSettings) -> Void)] = [
        ("density", { $0.density = 0.8 }),
        ("grade", { $0.grade = 90 }),
        ("wbCyan", { $0.wbCyan = 0.1 }),
        ("wbMagenta", { $0.wbMagenta = -0.2 }),
        ("wbYellow", { $0.wbYellow = 0.3 }),
        ("autoExposure", { $0.autoExposure = false }),
        ("autoNormalizeContrast", { $0.autoNormalizeContrast = false }),
        ("castRemovalStrength", { $0.castRemovalStrength = 0.9 }),
        ("whitePointOffset", { $0.whitePointOffset = 0.05 }),
        ("blackPointOffset", { $0.blackPointOffset = -0.04 }),
        ("toe", { $0.toe = 0.4 }),
        ("toeWidth", { $0.toeWidth = 3.0 }),
        ("shoulder", { $0.shoulder = -0.3 }),
        ("shoulderWidth", { $0.shoulderWidth = 2.0 }),
        ("paperDmin", { $0.paperDmin = true }),
        ("trueBlack", { $0.trueBlack = false }),
        ("exposureStops", { $0.exposureStops = 0.5 }),
        ("shadows", { $0.shadows = 0.7 }),
        ("shadowContrast", { $0.shadowContrast = -0.6 }),
        ("darkShadows", { $0.darkShadows = 0.2 }),
        ("highlights", { $0.highlights = -0.8 }),
        ("highlightContrast", { $0.highlightContrast = 0.4 }),
        ("overallContrast", { $0.overallContrast = 0.3 }),
        ("vibrance", { $0.vibrance = 1.2 }),
        ("saturation", { $0.saturation = 0.9 }),
        ("redHue", { $0.redHue = -0.25 }),
        ("redSaturation", { $0.redSaturation = 1.1 }),
        ("yellowHue", { $0.yellowHue = 0.6 }),
        ("yellowSaturation", { $0.yellowSaturation = 0.8 }),
        ("greenHue", { $0.greenHue = -0.4 }),
        ("greenSaturation", { $0.greenSaturation = 1.3 }),
        ("blueHue", { $0.blueHue = 0.7 }),
        ("blueSaturation", { $0.blueSaturation = 0.6 }),
        ("preSaturation", { $0.preSaturation = 1.0 }),
        ("temp", { $0.temp = 0.35 }),
        ("tint", { $0.tint = -0.15 }),
        ("colorShadows", { $0.colorShadows = SIMD3(0.1, -0.2, 0.3) }),
        ("colorMids", { $0.colorMids = SIMD3(-0.1, 0.2, -0.3) }),
        ("colorHighs", { $0.colorHighs = SIMD3(0.05, 0.15, -0.25) }),
        ("rotation", { $0.rotation = 90 }),
        ("flipHorizontal", { $0.flipHorizontal = true }),
        ("fineRotation", { $0.fineRotation = 1.5 }),
        ("analysisRect", { $0.analysisRect = NormalizedRect(x: 0.1, y: 0.2, width: 0.5, height: 0.4) }),
        ("analysisRectFineRotation", { $0.analysisRectFineRotation = 2.5 }),
        ("cropRect", { $0.cropRect = NormalizedRect(x: 0.05, y: 0.05, width: 0.9, height: 0.85) }),
    ]

    /// The tripwire's other half (SidecarCodecTests pins the same count on the
    /// codec side): a new field must get a mutation here, which then forces a
    /// historyLabel line via `everyFieldChangeGetsARealLabel`.
    @Test func mutationListCoversEveryStoredProperty() {
        #expect(Self.mutations.count == Mirror(reflecting: ExposureSettings()).children.count)
    }

    @Test func everyFieldChangeGetsARealLabel() {
        for (field, mutate) in Self.mutations {
            let base = ExposureSettings()
            var changed = base
            mutate(&changed)
            #expect(changed != base, "\(field) mutation is a no-op")
            #expect(
                historyLabel(from: base, to: changed) != "Edit",
                "historyLabel misses \(field)")
        }
    }

    @Test func labelFormatting() {
        var s = ExposureSettings()
        s.density = 0.8  // Brightness shows the user-facing 2 − density.
        #expect(historyLabel(from: ExposureSettings(), to: s) == "Brightness 1.20")

        var two = ExposureSettings()
        two.grade = 90
        two.trueBlack = false
        #expect(historyLabel(from: ExposureSettings(), to: two) == "Grade 90, True black off")

        var many = two
        many.shadows = 1
        #expect(historyLabel(from: ExposureSettings(), to: many) == "Multiple changes")

        #expect(historyLabel(from: ExposureSettings(), to: ExposureSettings()) == "Edit")
    }

    @Test func clearingLabelsReadAsClears() {
        var withCrop = ExposureSettings()
        withCrop.cropRect = NormalizedRect(x: 0.1, y: 0.1, width: 0.8, height: 0.8)
        #expect(historyLabel(from: ExposureSettings(), to: withCrop) == "Crop")
        #expect(historyLabel(from: withCrop, to: ExposureSettings()) == "Crop cleared")

        var withRegion = ExposureSettings()
        withRegion.analysisRect = NormalizedRect(x: 0.2, y: 0.2, width: 0.5, height: 0.5)
        #expect(historyLabel(from: ExposureSettings(), to: withRegion) == "Analysis region")
        #expect(historyLabel(from: withRegion, to: ExposureSettings()) == "Analysis region cleared")
    }

    /// The audit-found bug (2026-07-15): a change to only the region's drawn
    /// angle used to fall through to "Edit".
    @Test func regionAngleChangeIsARegionEdit() {
        var a = ExposureSettings()
        a.analysisRect = NormalizedRect(x: 0.2, y: 0.2, width: 0.5, height: 0.5)
        var b = a
        b.analysisRectFineRotation = 1.5
        #expect(historyLabel(from: a, to: b) == "Analysis region")
    }
}
