import NegativeKit

/// The house default look: adjustments applied on top of the stock
/// (NegPy-neutral) conversion whenever a frame is seen fresh — first open
/// with no sidecar, Reset All, and sidecar-less frames in batch export and
/// Copy Adjustments. "Start from Scratch" (ControlsSidebar) cancels it.
///
/// Ground rules for evolving this profile:
/// - Single source of truth: every "what does a fresh frame get?" seam
///   reads `DefaultProfile.settings`. Stock `ExposureSettings()` stays
///   NegPy-parity-neutral — the fixtures, negcli and the parity suite
///   never see the profile.
/// - Adjustments only, never geometry: crop/rotation/analysis region are
///   per-frame facts. `DefaultProfileTests` pins that every deviation
///   from stock is one of the documented fields below.
/// - The profile is plain settings — nothing hidden in derivation — so
///   cancelling it is always just `ExposureSettings()`.
enum DefaultProfile {
    /// Derived 2026-07-25 from the aggregate of hand corrections across
    /// the two Negative Test rolls (7 frames; thin, shadow-heavy scans):
    /// every frame was brightened (mean +0.73 EV counting both brightness
    /// controls), highlights recovered on 4/7 (mean −0.20, never raised),
    /// shadows lifted on 4/7 (mean +0.37), black point pulled on 6/7
    /// (mean −0.09) to keep blacks anchored after the brightening.
    /// Mixed-sign and single-frame moves (shadow contrast, dark shadows,
    /// mixer bands, toe) were left out — those are per-frame looks.
    /// 2026-07-25 (same day): overall contrast +0.2 and shadow contrast
    /// +0.5 added by request — beyond the measured aggregate (contrast:
    /// 2/7 frames, mean +0.03; shadow contrast: 3/7, mixed sign, median 0).
    static var settings: ExposureSettings {
        var s = ExposureSettings()
        s.exposureStops = 0.7
        s.highlights = -0.2
        s.shadows = 0.3
        s.blackPointOffset = -0.09
        s.overallContrast = 0.2
        s.shadowContrast = 0.5
        return s
    }
}
