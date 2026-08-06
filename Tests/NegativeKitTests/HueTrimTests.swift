import Foundation
import Testing

@testable import NegativeKit

/// Hue Trim (NegPy 7a07f5c): a rotation of the print's colours about the
/// neutral axis in the working CIELAB a*b* plane, for a scanning light that
/// ROTATES hues rather than casting them.
///
/// The invariants are what make it safe to sit in front of everything else:
/// it is a rotation, so it preserves L* and chroma exactly and fixes the
/// origin — which is why it can never fight the cast removal in analysis.
@Suite struct HueTrimTests {

    private func lab(_ rgb: SIMD3<Double>) -> SIMD3<Double> { LabColor.rgbToLab(rgb) }
    private func chroma(_ l: SIMD3<Double>) -> Double { (l.y * l.y + l.z * l.z).squareRoot() }
    private func hueDeg(_ l: SIMD3<Double>) -> Double { atan2(l.z, l.y) * 180 / .pi }

    /// Chromatic samples spanning the chroma range upstream measured over
    /// (CIELAB chroma 4 to 60+), plus deliberate near-neutrals.
    private let samples: [SIMD3<Double>] = [
        SIMD3(0.62, 0.31, 0.24),  // skin-ish
        SIMD3(0.18, 0.42, 0.22),  // foliage
        SIMD3(0.22, 0.35, 0.71),  // sky
        SIMD3(0.81, 0.66, 0.20),  // warm highlight
        SIMD3(0.50, 0.50, 0.50),  // neutral
        SIMD3(0.51, 0.50, 0.49),  // near-neutral
        SIMD3(0.05, 0.05, 0.05),  // deep shadow
    ]

    // MARK: - Identity

    @Test func zeroIsExactlyIdentity() {
        for rgb in samples {
            #expect(LabColor.applyHueTrim(rgb, radians: 0) == rgb, "\(rgb)")
        }
    }

    /// The default must be identity or every NegPy fixture would move.
    @Test func defaultSettingIsOff() {
        #expect(ExposureSettings().hueTrim == 0)
        let analysis = ExposureAnalysis(
            baseBounds: LogNegativeBounds(floors: SIMD3(-2, -2, -2), ceils: SIMD3(-0.5, -0.5, -0.5)),
            anchor: 0.46, texturalRange: 1.0, shadowRefs: SIMD3(-0.6, -0.6, -0.6))
        #expect(ExposureKernel.deriveRenderParams(ExposureSettings(), analysis).hueTrim == 0)
    }

    // MARK: - It is a rotation

    /// Neutrals are the fixed point — the property that lets Hue Trim sit in
    /// front of the cast removal without fighting it.
    ///
    /// Bounded in ABSOLUTE terms, not as "chroma is exactly preserved": the
    /// working matrix's rowsums differ from the D65 reference white in the 7th
    /// digit, so `rgbToLab` already reports a few times 1e-6 of chroma for a
    /// perfect grey — and the rgb→Lab→rgb round trip perturbs that by about as
    /// much again. Asking for preservation at 1e-6 would be demanding 50%
    /// relative precision on the float noise floor itself.
    ///
    /// Swept 994 greys × 121 angles: worst residual chroma 1.05e-5, worst
    /// channel spread 1.3e-7. A visible chroma difference is ~0.5 Lab units,
    /// so the bounds below still sit ~10,000× under it.
    @Test func neutralsNeverMove() {
        for grey in [0.02, 0.1, 0.25, 0.5, 0.75, 0.98] {
            let rgb = SIMD3(grey, grey, grey)
            #expect(chroma(lab(rgb)) < 1e-5, "precondition: grey \(grey) starts near-neutral")
            for deg in [-30.0, -7.0, 3.0, 12.0, 30.0] {
                let out = LabColor.applyHueTrim(rgb, radians: deg * .pi / 180)
                #expect(
                    chroma(lab(out)) < 5e-5,
                    "grey \(grey) at \(deg)°: chroma became \(chroma(lab(out)))")
                #expect(abs(out.x - out.y) < 1e-6 && abs(out.y - out.z) < 1e-6)
            }
        }
    }

    @Test func lightnessAndChromaAreUntouched() {
        for rgb in samples {
            let before = lab(rgb)
            // Skip samples the RGB cube clips on rotation — the clamp is real,
            // but it is not what this invariant is about.
            guard chroma(before) > 1, chroma(before) < 40 else { continue }
            for deg in [-20.0, -5.0, 9.0, 25.0] {
                let after = lab(LabColor.applyHueTrim(rgb, radians: deg * .pi / 180))
                #expect(abs(after.x - before.x) < 0.02, "L* moved at \(deg)° for \(rgb)")
                #expect(
                    abs(chroma(after) - chroma(before)) < 0.05,
                    "chroma moved at \(deg)° for \(rgb)")
            }
        }
    }

    /// The angle you dial is the angle you get.
    @Test func theMeasuredRotationIsTheAngleAsked() {
        for rgb in samples {
            let before = lab(rgb)
            guard chroma(before) > 2, chroma(before) < 40 else { continue }
            for deg in [-18.0, -6.0, 4.0, 21.0] {
                let after = lab(LabColor.applyHueTrim(rgb, radians: deg * .pi / 180))
                var delta = hueDeg(after) - hueDeg(before)
                delta = (delta + 540).truncatingRemainder(dividingBy: 360) - 180
                #expect(abs(delta - deg) < 0.1, "asked \(deg)°, measured \(delta)° for \(rgb)")
            }
        }
    }

    /// Opposite angles cancel — the operation is invertible where the cube
    /// doesn't clip.
    @Test func oppositeAnglesCancel() {
        for rgb in samples {
            guard chroma(lab(rgb)) > 1, chroma(lab(rgb)) < 40 else { continue }
            for deg in [-25.0, -8.0, 11.0, 28.0] {
                let there = LabColor.applyHueTrim(rgb, radians: deg * .pi / 180)
                let back = LabColor.applyHueTrim(there, radians: -deg * .pi / 180)
                let err = max(abs(back.x - rgb.x), max(abs(back.y - rgb.y), abs(back.z - rgb.z)))
                #expect(err < 2e-3, "round trip at ±\(deg)° for \(rgb) drifted \(err)")
            }
        }
    }

    /// A rotation is NOT a cast: its effect must GROW with chroma, where a
    /// fixed a*/b* offset would be constant. This is the distinction
    /// upstream's measurement rests on, so pin the direction.
    @Test func effectGrowsWithChromaUnlikeACast() {
        // Same hue, increasing chroma, well inside the cube.
        let low = SIMD3(0.52, 0.50, 0.48)
        let high = SIMD3(0.72, 0.45, 0.25)
        let deg = 15.0
        func shift(_ rgb: SIMD3<Double>) -> Double {
            let a = lab(rgb), b = lab(LabColor.applyHueTrim(rgb, radians: deg * .pi / 180))
            return ((b.y - a.y) * (b.y - a.y) + (b.z - a.z) * (b.z - a.z)).squareRoot()
        }
        #expect(chroma(lab(high)) > chroma(lab(low)))
        #expect(
            shift(high) > shift(low) * 2,
            "a rotation must move saturated colour much further than muted")
    }

    // MARK: - Wiring

    @Test func deriveConvertsDegreesToRadiansAndClamps() {
        let analysis = ExposureAnalysis(
            baseBounds: LogNegativeBounds(floors: SIMD3(-2, -2, -2), ceils: SIMD3(-0.5, -0.5, -0.5)),
            anchor: 0.46, texturalRange: 1.0, shadowRefs: SIMD3(-0.6, -0.6, -0.6))
        func trim(_ deg: Double) -> Double {
            var s = ExposureSettings()
            s.hueTrim = deg
            return ExposureKernel.deriveRenderParams(s, analysis).hueTrim
        }
        #expect(abs(trim(30) - 30 * .pi / 180) < 1e-12)
        #expect(abs(trim(-12.5) - (-12.5 * .pi / 180)) < 1e-12)
        // Out-of-range input clamps to the slider's ±30 rather than rotating wildly.
        #expect(abs(trim(400) - 30 * .pi / 180) < 1e-12)
        #expect(abs(trim(-400) - (-30 * .pi / 180)) < 1e-12)
    }
}
