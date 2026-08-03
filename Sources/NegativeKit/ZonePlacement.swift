import Foundation

/// Zone placement (NegPy 5a095f3 + 9dff124, `features/exposure/placement.py`):
/// solve Brightness (and Grade, and one knee control) so pinned tones print on
/// their target zones. Inverts the render's own forward model by bisection —
/// the composite is monotone in each solved control but not analytically
/// invertible.
///
/// The scalar forward model is the ACHROMATIC green-reference curve, exactly as
/// upstream's `predicted_zone` (their chart's own curve): the pin freezes its
/// normalized-log luma once, and candidate settings re-derive the curve around
/// it. Ours runs the pixel through the real kernel (`ReferenceCurve` with the
/// params collapsed to the green channel) rather than a re-derived scalar copy,
/// so the model can never fork from the render. Divergence from upstream, on
/// purpose: the green cmyOffset stays in (it carries `exposureStops`, a
/// SwiftInvert-only global brightness control the model must see; upstream has
/// no such control and omits filtration entirely), and so does the green levels
/// remap (same reasoning — it reshapes the displayed tone the probe reads).
public enum ZonePlacement {

    // MARK: - Pins

    /// Density, grade, one knee control — a fourth pin has nothing left to solve.
    public static let maxPins = 3

    /// One probed spot: content-normalized position, frozen normalized-log
    /// sample, and the zone the user wants it to print on.
    public struct Pin: Equatable, Sendable {
        public var nx: Double
        public var ny: Double
        /// Normalized-log RGB at the spot (mean of the sampling patch).
        public var valRGB: SIMD3<Double>
        /// Rec.709 luma of `valRGB` — what the solve actually places.
        public var valLuma: Double
        public var targetZone: Double
        /// Set once a zone is asked for: a dragged pin keeps it instead of
        /// re-reading (upstream `retargeted`).
        public var retargeted: Bool

        public init(
            nx: Double, ny: Double, valRGB: SIMD3<Double>, valLuma: Double,
            targetZone: Double, retargeted: Bool = false
        ) {
            self.nx = nx
            self.ny = ny
            self.valRGB = valRGB
            self.valLuma = valLuma
            self.targetZone = targetZone
            self.retargeted = retargeted
        }
    }

    // MARK: - Solve domain (mirrors the sliders)

    /// Brightness slider −3…4 ↔ density = 2 − b (right = brighter).
    public static let densityRange = (lo: -2.0, hi: 5.0)
    // Half the sliders' precision: bisecting finer than the 0.01 / 1 R
    // rounding is wasted work.
    static let densityResolution = 0.005
    static let gradeResolution = 0.5
    static let clampTol = 1e-6
    static let degenerateValGap = 1e-3

    /// One control a third pin can be solved on: slider bounds, bisection
    /// resolution and commit rounding (mirrors ControlsSidebar).
    /// @unchecked: key paths into a value type are immutable; the compiler
    /// just can't prove it (Swift 6 lacks Sendable key-path literals here).
    public struct KneeCandidate: @unchecked Sendable {
        public let keyPath: WritableKeyPath<ExposureSettings, Double>
        /// History/readout name — matches the slider label.
        public let label: String
        public let lo: Double
        public let hi: Double
        let resolution: Double
        let digits: Int
    }

    /// Upstream solves Shadows Grade / Highlights Grade / Snap. We don't ship
    /// those controls (recorded Split Grade / Zone Density skip) — the
    /// measured-purchase picker is control-agnostic, so the candidates are our
    /// convergent tone-control knees instead. No midtone candidate: our only
    /// midtone contrast is global (`overallContrast`), which the 2-pin grade
    /// solve already owns.
    public static let kneeCandidates: [KneeCandidate] = [
        KneeCandidate(
            keyPath: \.shadowContrast, label: "Shadow contrast",
            lo: -3.0, hi: 6.0, resolution: 0.005, digits: 2),
        KneeCandidate(
            keyPath: \.highlightContrast, label: "Highlight contrast",
            lo: -1.0, hi: 1.0, resolution: 0.005, digits: 2),
    ]

    /// Fraction of a candidate's range perturbed to measure its purchase.
    static let kneeProbe = 0.2
    /// Zones; below this the control cannot place the pin.
    static let kneeSensitivityFloor = 0.05
    /// A third of a ⅓-zone step: on target, and off-target honestly.
    public static let onTargetTol = 1.0 / 6.0

    // MARK: - Forward model

    /// Zone the achromatic print curve puts `valLuma` on under `settings`.
    ///
    /// The green channel's exact kernel trajectory: derive, collapse every
    /// per-channel param to green, run the 1-px pixel through the real print
    /// curve + encode, read the zone ruler.
    public static func predictedZone(
        settings: ExposureSettings, analysis: ExposureAnalysis, valLuma: Double
    ) -> Double {
        var p = ExposureKernel.deriveRenderParams(settings, analysis)
        p.slopes = SIMD3(repeating: p.slopes.y)
        p.pivots = SIMD3(repeating: p.pivots.y)
        p.curvatures = SIMD3(repeating: p.curvatures.y)
        p.cmyOffsets = SIMD3(repeating: p.cmyOffsets.y)
        p.shadowCMY = SIMD3(repeating: p.shadowCMY.y)
        p.midCMY = SIMD3(repeating: p.midCMY.y)
        p.highlightCMY = SIMD3(repeating: p.highlightCMY.y)
        p.levelsPoints = [p.levelsPoints[1], p.levelsPoints[1], p.levelsPoints[1]]
        let v = Float(valLuma)
        var px = RGBImage(pixels: [v, v, v], width: 1, height: 1)
        px = ReferenceCurve.applyPrintCurve(px, params: p)
        px = ReferenceCurve.encodeOutput(px, levels: p.levelsPoints)
        return Densitometry.zone(ofEncoded: Double(px.pixels[1]))
    }

    // MARK: - Solution

    public struct Solution: Equatable, Sendable {
        /// The post-Apply settings: solved fields rounded to slider precision,
        /// autos off. Preview renders these; Apply commits them.
        public var settings: ExposureSettings
        /// Zone per pin at the rounded settings, pin order.
        public var achieved: [Double]
        /// A target sat outside a slider's reach, or the knee iteration
        /// settled short of every ask.
        public var clamped: Bool
        /// Label of the knee control a third pin was solved on; nil when none
        /// was needed (or none had purchase).
        public var kneeLabel: String?
    }

    /// 1 pin: Brightness. 2 pins: Brightness + Grade from the outer tones.
    /// 3 pins: those two plus one knee control for the middle one. Solved
    /// against the post-Apply settings (autos off), clamped to the slider
    /// ranges, rounded to the sliders' precision with achieved zones
    /// recomputed at the rounded values.
    public static func solve(
        settings: ExposureSettings, analysis: ExposureAnalysis, pins: [Pin]
    ) -> Solution? {
        var kneeLabel: String?
        var solved: ExposureSettings
        var clamped: Bool
        if pins.count == 1 {
            var candidate = settings
            candidate.autoExposure = false
            let (density, c) = solveDensity(candidate, analysis, pin: pins[0])
            candidate.density = round2(density)
            (solved, clamped) = (candidate, c)
        } else if pins.count == 2 || pins.count == 3 {
            // Dark pin = highest normalized-log luma (dense side of the print).
            let ordered = pins.sorted { $0.valLuma > $1.valLuma }
            let (dark, light) = (ordered[0], ordered[ordered.count - 1])
            guard abs(dark.valLuma - light.valLuma) >= degenerateValGap else { return nil }
            var candidate = settings
            candidate.autoExposure = false
            candidate.autoNormalizeContrast = false
            let grade: Double
            let density: Double
            if pins.count == 3 {
                let r = solveWithKnee(candidate, analysis, dark: dark, light: light, mid: ordered[1])
                (candidate, grade, density, clamped) = (r.candidate, r.grade, r.density, r.clamped)
                if let knee = r.knee {
                    kneeLabel = knee.label
                    candidate[keyPath: knee.keyPath] =
                        rounded(candidate[keyPath: knee.keyPath], digits: knee.digits)
                }
            } else {
                (grade, density, clamped) = solveGradeAndDensity(
                    candidate, analysis, dark: dark, light: light)
            }
            candidate.density = round2(density)
            candidate.grade = grade.rounded()
            solved = candidate
        } else {
            return nil
        }
        let achieved = pins.map { predictedZone(settings: solved, analysis: analysis, valLuma: $0.valLuma) }
        // The knee iteration can settle short of every ask: say so rather than
        // report the ask.
        if kneeLabel != nil,
            zip(achieved, pins).contains(where: { abs($0 - $1.targetZone) > onTargetTol }) {
            clamped = true
        }
        return Solution(settings: solved, achieved: achieved, clamped: clamped, kneeLabel: kneeLabel)
    }

    // MARK: - Inner solves

    private static func solveDensity(
        _ candidate: ExposureSettings, _ analysis: ExposureAnalysis, pin: Pin
    ) -> (Double, Bool) {
        func residual(_ density: Double) -> Double {
            var placed = candidate
            placed.density = density
            return predictedZone(settings: placed, analysis: analysis, valLuma: pin.valLuma)
                - pin.targetZone
        }
        return bisectDecreasing(residual, lo: densityRange.lo, hi: densityRange.hi, resolution: densityResolution)
    }

    /// Outer bisection on grade against the light pin, inner density solve
    /// pinning the dark pin at every candidate grade. Softer (higher R) lowers
    /// the light pin's zone with the dark pin held, so the residual is
    /// decreasing in R.
    private static func solveGradeAndDensity(
        _ candidate: ExposureSettings, _ analysis: ExposureAnalysis, dark: Pin, light: Pin
    ) -> (grade: Double, density: Double, clamped: Bool) {
        func lightResidual(_ grade: Double) -> Double {
            var graded = candidate
            graded.grade = grade
            let (density, _) = solveDensity(graded, analysis, pin: dark)
            graded.density = density
            return predictedZone(settings: graded, analysis: analysis, valLuma: light.valLuma)
                - light.targetZone
        }
        let (grade, gradeClamped) = bisectDecreasing(
            lightResidual, lo: K.isoRMin, hi: K.isoRMax, resolution: gradeResolution)
        var graded = candidate
        graded.grade = grade
        let (density, densityClamped) = solveDensity(graded, analysis, pin: dark)
        return (grade, density, gradeClamped || densityClamped)
    }

    /// Place the outer tones, then the middle one on the knee control it can
    /// actually move — by bisecting the knee against the COMPOSITE residual
    /// (the 2-pin grade+density solve nested inside every evaluation).
    ///
    /// Deliberate divergence from upstream, who alternate (place the
    /// extremes, place the middle, repeat) to avoid nesting cost: their knee
    /// controls pivot at zone centres OUTSIDE the span between typical pins,
    /// so their alternation converges. Ours pivot at the tone anchors, which
    /// sit BETWEEN typical pin positions — the knee's raw effect on the mid
    /// pin and its net effect after the outer re-solve can have opposite
    /// signs, and the alternation diverges to the slider corners. Nested
    /// bisection converges unconditionally, and our µs-scale forward model
    /// makes the multiplied cost single-digit milliseconds. Monotonicity also
    /// buys a no-harm guarantee: when the ask is out of reach, the returned
    /// endpoint is the argmax — never worse than leaving the knee alone.
    private static func solveWithKnee(
        _ start: ExposureSettings, _ analysis: ExposureAnalysis, dark: Pin, light: Pin, mid: Pin
    ) -> (candidate: ExposureSettings, grade: Double, density: Double, knee: KneeCandidate?, clamped: Bool) {
        var candidate = start
        var (grade, density, clamped) = solveGradeAndDensity(candidate, analysis, dark: dark, light: light)
        var placed = candidate
        placed.grade = grade
        placed.density = density
        let landed = predictedZone(settings: placed, analysis: analysis, valLuma: mid.valLuma)
        if abs(landed - mid.targetZone) <= onTargetTol {
            return (candidate, grade, density, nil, clamped)
        }

        /// Mid pin's zone at a knee value, with the outer tones re-placed.
        func compositeZone(_ knee: KneeCandidate, _ value: Double) -> Double {
            var c = candidate
            c[keyPath: knee.keyPath] = value
            let (g, d, _) = solveGradeAndDensity(c, analysis, dark: dark, light: light)
            c.grade = g
            c.density = d
            return predictedZone(settings: c, analysis: analysis, valLuma: mid.valLuma)
        }

        // The control that moves this pin most WITH the outers held, or none
        // when none of them can. Measured, not inferred from the pin's zone
        // (upstream's rule): each control's weight is a sigmoid window on
        // print density, and the contrasts also lose purchase at their own
        // anchor — a zone-number rule would have to mirror those constants
        // and the working OETF.
        var best: KneeCandidate?
        var bestDelta = kneeSensitivityFloor
        for cand in kneeCandidates {
            let current = candidate[keyPath: cand.keyPath]
            let step = kneeProbe * (cand.hi - cand.lo)
            let probe = current + step <= cand.hi ? current + step : current - step
            let delta = abs(compositeZone(cand, probe) - landed)
            if delta > bestDelta {
                best = cand
                bestDelta = delta
            }
        }
        guard let knee = best else {
            return (candidate, grade, density, nil, clamped)
        }

        let (value, kneeClamped) = bisectMonotone(
            { compositeZone(knee, $0) - mid.targetZone },
            lo: knee.lo, hi: knee.hi, resolution: knee.resolution)
        candidate[keyPath: knee.keyPath] = value
        (grade, density, clamped) = solveGradeAndDensity(candidate, analysis, dark: dark, light: light)
        return (candidate, grade, density, knee, clamped || kneeClamped)
    }

    // MARK: - Bisection

    /// Root of a monotone f, direction read off the endpoints. The knee
    /// controls pivot at their own anchor, so the sign of their effect flips
    /// with the pin's side of it — neither direction can be assumed the way
    /// density and grade can.
    private static func bisectMonotone(
        _ f: (Double) -> Double, lo: Double, hi: Double, resolution: Double
    ) -> (Double, Bool) {
        if f(lo) >= f(hi) {
            return bisectDecreasing(f, lo: lo, hi: hi, resolution: resolution)
        }
        return bisectDecreasing({ -f($0) }, lo: lo, hi: hi, resolution: resolution)
    }

    /// Root of a decreasing f on [lo, hi] to within `resolution`; (closest
    /// endpoint, true) when the target sits outside the bracket.
    private static func bisectDecreasing(
        _ f: (Double) -> Double, lo: Double, hi: Double, resolution: Double
    ) -> (Double, Bool) {
        let fLo = f(lo)
        if fLo <= 0.0 { return (lo, fLo < -clampTol) }
        let fHi = f(hi)
        if fHi >= 0.0 { return (hi, fHi > clampTol) }
        var (lo, hi) = (lo, hi)
        while hi - lo > resolution {
            let mid = 0.5 * (lo + hi)
            if f(mid) > 0.0 { lo = mid } else { hi = mid }
        }
        return (0.5 * (lo + hi), false)
    }

    private static func round2(_ v: Double) -> Double { (v * 100).rounded() / 100 }
    private static func rounded(_ v: Double, digits: Int) -> Double {
        let s = pow(10.0, Double(digits))
        return (v * s).rounded() / s
    }
}
