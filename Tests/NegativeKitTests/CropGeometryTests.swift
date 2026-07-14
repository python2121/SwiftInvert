import Foundation
import Testing
import simd

@testable import NegativeKit

/// Unified Crop & Straighten geometry: fit-scaling and content-preserving
/// crop remapping across angles.
@Suite struct CropGeometryTests {
    let frame = SIMD2(3000.0, 2000.0)

    @Test func zeroAngleIsIdentity() {
        // At θ=0 the inscribed rect is the frame; a normalized rect
        // round-trips exactly.
        let rect = NormalizedRect(x: 0.1, y: 0.2, width: 0.5, height: 0.6)
        let b = CropGeometry.box(from: rect, frame: frame, radians: 0)
        let back = CropGeometry.normalizedRect(
            center: b.center, halfExtents: b.halfExtents, frame: frame, radians: 0)
        #expect(abs(back.x - rect.x) < 1e-9 && abs(back.width - rect.width) < 1e-9)
        #expect(abs(back.y - rect.y) < 1e-9 && abs(back.height - rect.height) < 1e-9)
    }

    @Test func fullInscribedBoxAlwaysFits() {
        for degrees in [0.0, 3.0, -7.5, 20.0, -45.0] {
            let radians = degrees * .pi / 180
            let ins = CropGeometry.inscribedSize(frame: frame, radians: radians)
            #expect(
                CropGeometry.boxFits(
                    center: .zero, halfExtents: ins / 2, radians: radians, frame: frame,
                    tolerance: 1e-3),
                "inscribed rect must fit at \(degrees)°")
        }
    }

    @Test func fitScaleShrinksOnlyWhenNeeded() {
        let radians = 5.0 * .pi / 180
        // A tiny centered box is unaffected.
        let small = CropGeometry.fitScale(
            center: .zero, halfExtents: SIMD2(100, 80), radians: radians, frame: frame)
        #expect(small == 1.0)
        // The full frame at 5° must shrink.
        let big = CropGeometry.fitScale(
            center: .zero, halfExtents: frame / 2, radians: radians, frame: frame)
        #expect(big < 1.0 && big > 0.5)
        // And the shrunk result actually fits.
        #expect(
            CropGeometry.boxFits(
                center: .zero, halfExtents: frame / 2 * big, radians: radians, frame: frame))
    }

    @Test func constrainedBoxAlwaysFits() {
        // Off-center boxes at assorted angles: after constrain, always inside.
        for degrees in [2.0, -10.0, 25.0, -40.0] {
            let radians = degrees * .pi / 180
            let (c, e) = CropGeometry.constrain(
                center: SIMD2(1200, -700), halfExtents: SIMD2(600, 500),
                radians: radians, frame: frame)
            #expect(CropGeometry.boxFits(center: c, halfExtents: e, radians: radians, frame: frame))
            #expect(e.x > 0 && e.y > 0)
        }
    }

    @Test func remapIsIdentityAtSameAngle() {
        let rect = NormalizedRect(x: 0.2, y: 0.1, width: 0.5, height: 0.7)
        let radians = 8.0 * .pi / 180
        let out = CropGeometry.remapCrop(rect, from: radians, to: radians, frame: frame)
        #expect(abs(out.x - rect.x) < 1e-6 && abs(out.y - rect.y) < 1e-6)
        #expect(abs(out.width - rect.width) < 1e-6 && abs(out.height - rect.height) < 1e-6)
    }

    @Test func remapPreservesContentCenter() {
        // The frame point under the crop center must stay under it.
        let rect = NormalizedRect(x: 0.3, y: 0.25, width: 0.3, height: 0.4)
        let a = 4.0 * .pi / 180
        let b = -6.0 * .pi / 180
        let before = CropGeometry.box(from: rect, frame: frame, radians: a)
        let contentBefore = CropGeometry.rotate(before.center, by: -a)

        let remapped = CropGeometry.remapCrop(rect, from: a, to: b, frame: frame)
        let after = CropGeometry.box(from: remapped, frame: frame, radians: b)
        let contentAfter = CropGeometry.rotate(after.center, by: -b)
        #expect(simd_length(contentBefore - contentAfter) < 1.0)  // within a pixel

        // And the remapped crop is representable (fits its rotation).
        #expect(
            CropGeometry.boxFits(
                center: after.center, halfExtents: after.halfExtents, radians: b, frame: frame,
                tolerance: 1e-3))
    }

    @Test func remapRoundTripKeepsSize() {
        // a → b → a returns to the original size when nothing forced a shrink.
        let rect = NormalizedRect(x: 0.35, y: 0.3, width: 0.3, height: 0.35)
        let a = 3.0 * .pi / 180
        let b = -3.0 * .pi / 180
        let there = CropGeometry.remapCrop(rect, from: a, to: b, frame: frame)
        let back = CropGeometry.remapCrop(there, from: b, to: a, frame: frame)
        let origin = CropGeometry.box(from: rect, frame: frame, radians: a)
        let final = CropGeometry.box(from: back, frame: frame, radians: a)
        #expect(simd_length(origin.halfExtents - final.halfExtents) < 1.0)
        #expect(simd_length(origin.center - final.center) < 1.0)
    }
}
