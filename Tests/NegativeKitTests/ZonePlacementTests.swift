import Foundation
import Testing

@testable import NegativeKit

/// Zone placement solver (NegPy 5a095f3 + 9dff124). The contracts distilled
/// from upstream's test_zone_placement.py: the ruler inverse is exact, each
/// solve places its pins within tolerance, clamping is honest, autos come
/// back off, and the third pin engages a knee control only when the outer
/// solve leaves it off target AND a control has measurable purchase.
@Suite struct ZonePlacementTests {

    /// A plausible frame: bounds/anchor in the fixtures' working range.
    static let analysis = ExposureAnalysis(
        baseBounds: LogNegativeBounds(
            floors: SIMD3(-2.0, -2.0, -2.0), ceils: SIMD3(-0.7, -0.7, -0.7)),
        anchor: 0.46, texturalRange: 1.0, shadowRefs: SIMD3(-0.9, -0.9, -0.9),
        neutralMid: nil, neutralShadow: nil, neutralHighlight: nil, neutralConfidence: nil)

    static func pin(_ valLuma: Double, target: Double) -> ZonePlacement.Pin {
        ZonePlacement.Pin(
            nx: 0.5, ny: 0.5, valRGB: SIMD3(repeating: valLuma), valLuma: valLuma,
            targetZone: target)
    }

    // MARK: Ruler

    @Test func encodedOfZoneIsExactInverse() {
        for i in 0...100 {
            let z = Double(i) / 10.0
            let enc = Densitometry.encoded(ofZone: z)
            #expect(abs(Densitometry.zone(ofEncoded: enc) - z) < 1e-12)
        }
        #expect(Densitometry.encoded(ofZone: 5.0) == Densitometry.midGrayEncoded)
        #expect(Densitometry.encoded(ofZone: 0) == 0)
        #expect(Densitometry.encoded(ofZone: 10) == 1)
        // Out-of-range asks clamp to the paper's ends.
        #expect(Densitometry.encoded(ofZone: -1) == 0)
        #expect(Densitometry.encoded(ofZone: 12) == 1)
    }

    // MARK: Forward model

    @Test func predictedZoneIsDecreasingInDensityAndVal() {
        var s = ExposureSettings()
        s.autoExposure = false
        let mid = 0.5
        var last = Double.infinity
        for density in stride(from: -2.0, through: 5.0, by: 0.5) {
            s.density = density
            let z = ZonePlacement.predictedZone(settings: s, analysis: Self.analysis, valLuma: mid)
            #expect(z <= last + 1e-9, "zone must fall as the print darkens")
            last = z
        }
        s.density = 1.0
        // Higher normalized-log = denser negative = darker print = lower zone.
        let dark = ZonePlacement.predictedZone(settings: s, analysis: Self.analysis, valLuma: 0.9)
        let light = ZonePlacement.predictedZone(settings: s, analysis: Self.analysis, valLuma: 0.1)
        #expect(dark < light)
    }

    @Test func predictedZoneSeesExposureStopsAndLevels() {
        var s = ExposureSettings()
        s.autoExposure = false
        let base = ZonePlacement.predictedZone(settings: s, analysis: Self.analysis, valLuma: 0.5)
        s.exposureStops = 1.0
        let brighter = ZonePlacement.predictedZone(settings: s, analysis: Self.analysis, valLuma: 0.5)
        #expect(brighter > base + 0.2, "a +1 stop must lift the predicted zone")
        s.exposureStops = 0
        s.levelsGreen = [SIMD2(0.5, 0.7)]
        let remapped = ZonePlacement.predictedZone(settings: s, analysis: Self.analysis, valLuma: 0.5)
        #expect(remapped > base, "the green levels remap reshapes the displayed tone")
    }

    // MARK: 1 pin

    @Test func onePinPlacesTheToneAndDisablesAutoExposure() throws {
        let p = Self.pin(0.5, target: 6.5)
        let sol = try #require(
            ZonePlacement.solve(settings: ExposureSettings(), analysis: Self.analysis, pins: [p]))
        #expect(!sol.settings.autoExposure)
        #expect(sol.settings.autoNormalizeContrast)  // 1 pin leaves auto grade alone
        #expect(!sol.clamped)
        #expect(sol.kneeLabel == nil)
        #expect(abs(sol.achieved[0] - 6.5) <= ZonePlacement.onTargetTol)
        let live = ZonePlacement.predictedZone(
            settings: sol.settings, analysis: Self.analysis, valLuma: p.valLuma)
        #expect(abs(live - sol.achieved[0]) < 1e-9, "achieved must be the rounded settings' truth")
        // Rounded to the slider's precision.
        #expect(abs(sol.settings.density * 100 - (sol.settings.density * 100).rounded()) < 1e-9)
    }

    @Test func onePinUnreachableTargetClampsToTheSliderEnd() throws {
        // Zone 10 for a deep-shadow tone: brighter than the slider can print.
        let sol = try #require(
            ZonePlacement.solve(
                settings: ExposureSettings(), analysis: Self.analysis,
                pins: [Self.pin(0.95, target: 10.0)]))
        #expect(sol.clamped)
        #expect(sol.settings.density == ZonePlacement.densityRange.lo)
    }

    // MARK: 2 pins

    /// Feasible asks derived from the model itself: what the tones print at
    /// with autos off, nudged outward by half a zone (a slightly harder
    /// grade, well inside R50…R180).
    static func feasibleOuterPins() -> (dark: ZonePlacement.Pin, light: ZonePlacement.Pin) {
        var s = ExposureSettings()
        s.autoExposure = false
        s.autoNormalizeContrast = false
        let zDark = ZonePlacement.predictedZone(settings: s, analysis: analysis, valLuma: 0.8)
        let zLight = ZonePlacement.predictedZone(settings: s, analysis: analysis, valLuma: 0.2)
        return (pin(0.8, target: zDark - 0.5), pin(0.2, target: zLight + 0.5))
    }

    @Test func twoPinsPlaceBothTonesAndDisableBothAutos() throws {
        let (dark, light) = Self.feasibleOuterPins()
        let sol = try #require(
            ZonePlacement.solve(
                settings: ExposureSettings(), analysis: Self.analysis, pins: [dark, light]))
        #expect(!sol.settings.autoExposure && !sol.settings.autoNormalizeContrast)
        #expect(!sol.clamped)
        #expect(sol.kneeLabel == nil)
        #expect(abs(sol.achieved[0] - dark.targetZone) <= ZonePlacement.onTargetTol)
        #expect(abs(sol.achieved[1] - light.targetZone) <= ZonePlacement.onTargetTol)
        #expect(sol.settings.grade == sol.settings.grade.rounded())
        #expect(sol.settings.grade >= K.isoRMin && sol.settings.grade <= K.isoRMax)
    }

    @Test func twoPinsPinOrderDoesNotMatter() throws {
        let (dark, light) = Self.feasibleOuterPins()
        let a = try #require(
            ZonePlacement.solve(
                settings: ExposureSettings(), analysis: Self.analysis, pins: [dark, light]))
        let b = try #require(
            ZonePlacement.solve(
                settings: ExposureSettings(), analysis: Self.analysis, pins: [light, dark]))
        #expect(a.settings == b.settings)
        #expect(a.achieved == b.achieved.reversed())
    }

    @Test func twoPinsDegenerateTonesReturnNil() {
        let sol = ZonePlacement.solve(
            settings: ExposureSettings(), analysis: Self.analysis,
            pins: [Self.pin(0.5, target: 3), Self.pin(0.5 + 1e-4, target: 8)])
        #expect(sol == nil)
    }

    @Test func twoPinsImpossibleSpreadClampsHonestly() throws {
        // Nearly identical tones asked to sit 6 zones apart: no grade can.
        let sol = try #require(
            ZonePlacement.solve(
                settings: ExposureSettings(), analysis: Self.analysis,
                pins: [Self.pin(0.52, target: 2.0), Self.pin(0.48, target: 8.0)]))
        #expect(sol.clamped)
        #expect(sol.settings.grade == K.isoRMin, "maximum hardness is the closest answer")
    }

    // MARK: 3 pins

    @Test func thirdPinOnTargetNeedsNoKnee() throws {
        // First solve the outer pair alone, then ask the mid pin for exactly
        // what it already gets — the knee must not engage.
        let (dark, light) = Self.feasibleOuterPins()
        let outer = try #require(
            ZonePlacement.solve(
                settings: ExposureSettings(), analysis: Self.analysis, pins: [dark, light]))
        let midLanded = ZonePlacement.predictedZone(
            settings: outer.settings, analysis: Self.analysis, valLuma: 0.55)
        let sol = try #require(
            ZonePlacement.solve(
                settings: ExposureSettings(), analysis: Self.analysis,
                pins: [dark, light, Self.pin(0.55, target: midLanded)]))
        #expect(sol.kneeLabel == nil)
        #expect(sol.settings.shadowContrast == 0 && sol.settings.highlightContrast == 0)
    }

    @Test func thirdPinSolvesAKneeControlAndKeepsTheOuterTones() throws {
        // A shadow-side tone asked to sit off where the straight 2-pin solve
        // puts it (the outer asks themselves are feasible).
        let (dark, light) = Self.feasibleOuterPins()
        let outer = try #require(
            ZonePlacement.solve(
                settings: ExposureSettings(), analysis: Self.analysis, pins: [dark, light]))
        let midVal = 0.7
        let landed = ZonePlacement.predictedZone(
            settings: outer.settings, analysis: Self.analysis, valLuma: midVal)
        // Modest ask: within the knee's physical reach against the outer
        // re-solve's counteraction (a large ask clamps honestly instead).
        let mid = Self.pin(midVal, target: min(landed + 0.3, 9.0))
        let sol = try #require(
            ZonePlacement.solve(
                settings: ExposureSettings(), analysis: Self.analysis, pins: [dark, light, mid]))
        #expect(sol.kneeLabel != nil, "an off-target mid pin must engage a knee")
        #expect(!sol.clamped, "a modest mid ask must be met without clamping")
        for (a, p) in zip(sol.achieved, [dark, light, mid]) {
            #expect(abs(a - p.targetZone) <= ZonePlacement.onTargetTol,
                "solution must meet every ask (zone \(a) vs \(p.targetZone))")
        }
        // The solved knee is a real slider value inside its range.
        let cand = ZonePlacement.kneeCandidates.first { $0.label == sol.kneeLabel }!
        let value = sol.settings[keyPath: cand.keyPath]
        #expect(value >= cand.lo && value <= cand.hi)
        #expect(value != 0, "the engaged knee must actually move")
    }

    @Test func fourPinsReturnNil() {
        let pins = [0.2, 0.4, 0.6, 0.8].map { Self.pin($0, target: 5) }
        #expect(ZonePlacement.solve(settings: ExposureSettings(), analysis: Self.analysis, pins: pins) == nil)
        #expect(ZonePlacement.solve(settings: ExposureSettings(), analysis: Self.analysis, pins: []) == nil)
    }

    /// The solve must respect the frame's own metering the way the render
    /// will: settings that carry crop/color state pass through untouched.
    @Test func solvePreservesUnrelatedSettings() throws {
        var s = ExposureSettings()
        s.temp = 0.4
        s.vibrance = 1.3
        s.cropRect = NormalizedRect(x: 0.1, y: 0.1, width: 0.8, height: 0.8)
        let sol = try #require(
            ZonePlacement.solve(settings: s, analysis: Self.analysis, pins: [Self.pin(0.5, target: 6)]))
        #expect(sol.settings.temp == 0.4)
        #expect(sol.settings.vibrance == 1.3)
        #expect(sol.settings.cropRect == s.cropRect)
    }
}
