import Foundation
import Testing

@testable import NegativeKit
@testable import SwiftInvert

/// The house default profile's contract (see DefaultProfile.swift): plain
/// settings, adjustments only, cancellable back to stock. When the profile
/// evolves, update ONLY the zeroing list here to match its documented
/// fields — the equality check then proves nothing else drifted (geometry
/// included, since any stray field breaks it).
@Suite struct DefaultProfileTests {
    @Test func profileTouchesOnlyItsDocumentedFields() {
        var s = DefaultProfile.settings
        s.exposureStops = 0
        s.highlights = 0
        s.shadows = 0
        s.blackPointOffset = 0
        s.overallContrast = 0
        #expect(s == ExposureSettings())
    }

    /// The profile must never carry geometry: a fresh frame's crop, rotation
    /// and analysis region are per-frame facts. (Subsumed by the equality
    /// test, but pinned separately so a violation names the actual problem.)
    @Test func profileCarriesNoGeometry() {
        let p = DefaultProfile.settings
        #expect(p.cropRect == nil)
        #expect(p.analysisRect == nil)
        #expect(p.rotation == 0)
        #expect(p.flipHorizontal == false)
        #expect(p.fineRotation == 0)
    }

    /// Start-from-scratch semantics: the profile is cancellable by plain
    /// stock settings, which stay NegPy-parity-neutral.
    @Test func profileDiffersFromStock() {
        #expect(DefaultProfile.settings != ExposureSettings())
    }
}
