import Foundation
import Testing

@testable import NegativeKit
@testable import SwiftInvert

/// `ImageSession.prepare` hands the renderer the METER image directly when
/// nothing is rotated — a copy-on-write that makes the overwhelmingly common
/// case free. `aliasesMeterPreview` is the pure decision behind that shortcut,
/// and the only part of it that can be wrong silently: the alias is a bitmap
/// swap, so taking it in the wrong state doesn't fail, it just renders a
/// different picture than every control describes.
///
/// The trap it guards is that the meter image is NOT built at the straighten
/// angle. An analysis region pins the angle it was drawn at (`meterAngle`) so
/// the meters keep reading the content the user pointed at, and that angle
/// survives the straighten slider going back to zero — including via the one
/// click of Clear Crop & Straighten, which zeroes `fineRotation` and leaves the
/// region alone.
@Suite struct MeterPreviewAliasTests {

    private func alias(fineRotation: Double, meterAngle: Double) -> Bool {
        ImageSession.aliasesMeterPreview(fineRotation: fineRotation, meterAngle: meterAngle)
    }

    // MARK: - The fast path must survive

    /// The ordinary frame: no straighten, no pinned region. This is the case
    /// the COW exists for, and a fix that lost it would put a full orientation
    /// copy on every frame.
    @Test func flatFrameStillAliases() {
        #expect(alias(fineRotation: 0, meterAngle: 0))
    }

    /// A region drawn on an unstraightened frame pins 0, which is still flat —
    /// so merely HAVING an analysis region must not cost the copy.
    @Test func regionDrawnUnrotatedStillAliases() {
        var settings = ExposureSettings()
        settings.analysisRect = NormalizedRect(x: 0.2, y: 0.2, width: 0.5, height: 0.5)
        settings.analysisRectFineRotation = 0
        #expect(alias(fineRotation: settings.fineRotation, meterAngle: settings.analysisRectFineRotation))
    }

    // MARK: - The bug

    /// Straighten → draw a region → return the slider to zero. The straighten
    /// angle reads flat, but the meter image is still tilted at the angle the
    /// region was drawn at, so the alias would render THAT angle.
    @Test func pinnedRegionAngleRefusesTheAliasAtZeroStraighten() {
        for pinned in [0.5, 3.0, 10.0, -7.5, 45.0] {
            #expect(alias(fineRotation: 0, meterAngle: pinned) == false, "pinned \(pinned)")
        }
    }

    /// The pre-existing half of the condition: a live straighten angle has
    /// always needed its own orientation.
    @Test func straightenAngleRefusesTheAlias() {
        for angle in [0.5, 3.0, -10.0, 45.0] {
            #expect(alias(fineRotation: angle, meterAngle: 0) == false, "angle \(angle)")
            // Both tilted (region drawn at the angle currently applied) is also
            // its own orientation — the two buffers are inscribed separately.
            #expect(alias(fineRotation: angle, meterAngle: angle) == false, "both \(angle)")
        }
    }

    /// The threshold has to mirror `RGBImage.oriented`, which is what actually
    /// decides whether a fine rotation is applied — a stricter test here would
    /// refuse the alias for angles that produce a byte-identical image, and a
    /// looser one would alias across a real rotation.
    @Test func thresholdMatchesTheOneOrientedUses() {
        #expect(alias(fineRotation: 0.004, meterAngle: 0.004))
        #expect(alias(fineRotation: 0.005, meterAngle: 0.005))
        #expect(alias(fineRotation: 0.006, meterAngle: 0) == false)
        #expect(alias(fineRotation: 0, meterAngle: 0.006) == false)
    }

    // MARK: - Why it matters (keeps the tests above from going vacuous)

    /// The alias is only a bug because the two buffers genuinely differ: a fine
    /// rotation auto-crops to the inscribed rectangle, so a pinned angle
    /// changes both the pixels and the SHAPE of the render input — while
    /// `displayAspect`/`contentWindow` read `fineRotation` and lay out for the
    /// untilted frame. This pins that the wrong branch really would have put a
    /// differently-shaped bitmap on the canvas.
    @Test func aliasingAPinnedAngleWouldChangeTheRenderedShape() {
        let base = RGBImage(width: 1536, height: 1024, fill: 0.5)
        let flat = base.oriented(rotationCW: 0, flipHorizontal: false, fineRotation: 0)
        let pinned = base.oriented(rotationCW: 0, flipHorizontal: false, fineRotation: 10)

        #expect(flat.width == 1536 && flat.height == 1024)
        // Inscribed: strictly smaller in both axes.
        #expect(pinned.width < flat.width)
        #expect(pinned.height < flat.height)

        // And a different aspect — which is the half the canvas layout gets
        // wrong, since it would still be sizing for the untilted frame.
        let flatAspect = Double(flat.width) / Double(flat.height)
        let pinnedAspect = Double(pinned.width) / Double(pinned.height)
        #expect(abs(flatAspect - pinnedAspect) > 0.1, "\(flatAspect) vs \(pinnedAspect)")
    }
}
