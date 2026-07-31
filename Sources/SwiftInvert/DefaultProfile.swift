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
    /// The ACTIVE profile's settings — "None" (stock, no adjustments) out
    /// of the box; the built-in below or a user profile once picked via
    /// File → Choose Default Settings… (ProfileStore; survives restarts).
    @MainActor static var settings: ExposureSettings {
        ProfileStore.shared.active.settings
    }

    /// The code-derived house profile ("SwiftInvert Default", second in the
    /// picker after None, not user-editable — evolve it here).
    ///
    /// Originally derived 2026-07-25 from the aggregate of hand corrections
    /// across the two Negative Test rolls (7 thin, shadow-heavy frames);
    /// re-validated 2026-07-31 against the SAME frames after five days of
    /// re-editing on top of a live profile — the residual medians sat ON
    /// the profile values for highlights/shadows/black point/contrast
    /// (the profile is landing), with three corrections the residuals
    /// demanded:
    /// - exposureStops 0.7 → 0.6 (frames scattered around ~0.57–0.61;
    ///   the user's own profile had already corrected this way),
    /// - preSaturation joins the profile at 1.25 (5/7 frames pushed past
    ///   the stock 1.15, movers mean ~1.27, none pulled down),
    /// - shadowContrast 0.5 → 0.55 (repeated small up-nudges on 3–4
    ///   frames).
    /// Per-frame looks stayed out: red hue/saturation warmth, dark
    /// shadows, density trims, levels anchors, early printSaturation
    /// nudges (watch that one — 2/7 up after a day of existence).
    static var builtIn: ExposureSettings {
        var s = ExposureSettings()
        s.exposureStops = 0.6
        s.highlights = -0.2
        s.shadows = 0.3
        s.blackPointOffset = -0.09
        s.overallContrast = 0.2
        s.shadowContrast = 0.55
        s.preSaturation = 1.25
        return s
    }
}
