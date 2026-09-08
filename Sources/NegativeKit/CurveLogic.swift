import Foundation

/// Print-curve parameter derivation — port of negpy/features/exposure/logic.py
/// (grade math, pivot solve, cast removal, WB filtration). Single source of truth
/// for the Metal uniforms, the CPU reference curve and (later) the curve chart.
public enum CurveLogic {
    /// Separation Damping's per-pixel effective k (NegPy
    /// separation_damping_gain): the frame-wide printSaturation k tapered by
    /// the pixel's own RMS dye-density spread. h = (ref − c)/(ref + c) runs
    /// from 1 at grey to −1 at extreme separation, so at damping 1 muted
    /// colour takes the full k, the reference spread stays at exactly 1.0
    /// and vivid colour gets 1/k. Clamped at 3 (bounds the k → 0 corner).
    /// Mirrored as MSL in NegPipeline.metal — keep in sync.
    public static func separationDampingGain(k: Double, damping: Double, chroma: Double) -> Double {
        guard k > 0 else { return 0 }
        let h = (K.separationDampingRefSpread - chroma) / (K.separationDampingRefSpread + chroma)
        return min(pow(k, (1.0 - damping) + damping * h), 3.0)
    }

    /// Numerically stable softplus log(1 + exp(x)).
    @inlinable public static func softplus(_ x: Double) -> Double {
        x > 0 ? x + log1p(exp(-x)) : log1p(exp(x))
    }

    /// Inverse softplus log(exp(y) − 1), stable for y > 20.
    @inlinable public static func invSoftplus(_ y: Double) -> Double {
        y > 20.0 ? y : log(expm1(max(y, 1e-12)))
    }

    /// Logistic sigmoid.
    @inlinable public static func sigmoid(_ x: Double) -> Double {
        if x >= 0 { return 1.0 / (1.0 + exp(-x)) }
        let z = exp(x)
        return z / (1.0 + z)
    }

    /// cmy_to_density / density_to_cmy.
    public static func cmyToDensity(_ val: Double, logRange: Double = 1.0) -> Double {
        val * K.cmyMaxDensity / max(logRange, 1e-6)
    }

    public static func densityToCmy(_ density: Double, logRange: Double = 1.0) -> Double {
        density * logRange / K.cmyMaxDensity
    }

    /// Fallback density range when none is measured: a normal negative's,
    /// scaled by K (default_grade_range, 8532dd92 form).
    public static var defaultGradeRange: Double {
        K.autoGradeTarget * K.autoGradeNominalRatio * K.autoGradeNominalRange
    }

    /// grade_to_slope: k = grade_contrast_scale * range / (ISO R / 100), clamped.
    public static func gradeToSlope(_ grade: Double, densityRange: Double?) -> Double {
        let rngIn = densityRange ?? defaultGradeRange
        let er = min(max(grade, K.isoRMin), K.isoRMax) / 100.0
        let rng = min(max(abs(rngIn), 0.3), 3.5)
        let k = K.gradeContrastScale * rng / er
        return min(max(k, K.slopeMin), K.slopeMax)
    }

    public static func slopeToGrade(_ slope: Double, densityRange: Double?) -> Double {
        let rngIn = densityRange ?? defaultGradeRange
        let rng = min(max(abs(rngIn), 0.3), 3.5)
        if slope <= 0 { return K.isoRMax }
        let er = K.gradeContrastScale * rng / slope
        return min(max(er * 100.0, K.isoRMin), K.isoRMax)
    }

    /// grade_coupled_shape: hard grades get snappier knees.
    public static func gradeCoupledShape(slopeG: Double, toe: Double, shoulder: Double) -> (toe: Double, shoulder: Double) {
        var slopeNorm = (slopeG - K.slopeMin) / (K.slopeMax - K.slopeMin)
        slopeNorm = min(max(slopeNorm, 0.0), 1.0)
        return (toe + K.toeGradeStrength * slopeNorm, shoulder + K.shoulderGradeStrength * slopeNorm)
    }

    /// effective_grade_range (8532dd92, Alkofer US 4,731,671). Auto Grade
    /// off: the measured floor-to-ceil range, a fixed paper. On: the paper
    /// gamma follows the negative's textural density scale, shrunk toward a
    /// normal negative's — effective = K · floor_ceil · ((1−s) + s·n/t) —
    /// so the printed textural range is (1−s) of the frame's own plus s of
    /// the norm, capped at maxOverfill of the norm's print span (a grade
    /// whose paper range is far shorter than the negative's clips both ends).
    public static func effectiveGradeRange(
        autoNormalizeContrast: Bool, floorCeilRange: Double?, texturalRange: Double?
    ) -> Double? {
        if !autoNormalizeContrast { return floorCeilRange }
        guard let texturalRange, let floorCeilRange else { return defaultGradeRange }
        let measured = abs(texturalRange)
        if measured < 1e-6 { return 3.5 }
        let q = K.autoGradeNominalRange / measured
        let s = K.autoGradeStrength
        let factor = min((1.0 - s) + s * q, q * K.autoGradeMaxOverfill)
        return K.autoGradeTarget * abs(floorCeilRange) * factor
    }

    /// _reference_linear_value: straight-line density v* that the base toe/shoulder
    /// bounds map onto `target` (default anchor_target_density; Shadow Reach and
    /// Highlight Hold pass their own — closed form via inverse softplus).
    public static func referenceLinearValue(dMin: Double = 0.0, target: Double? = nil) -> Double {
        let t = target ?? K.anchorTargetDensity
        let aHl = K.shoulderSharpnessBase
        let aSh = K.toeSharpnessBase
        let v1 = K.dMax - invSoftplus(aSh * (K.dMax - t)) / aSh
        return dMin + invSoftplus(aHl * (v1 - dMin)) / aHl
    }

    /// shadow_reach_slope (8532dd92; Gindele US 7,113,649, Ajewole
    /// US 5,046,118): Auto Grade's floor on the slope — the textured dark
    /// tail at `shadowPoint` must print at least shadowReachDensity while the
    /// anchor stays at its target, so the slope is raised to the straight
    /// line through both when the grade alone falls short. Never lowered.
    public static func shadowReachSlope(
        _ slope: Double, anchor: Double, shadowPoint: Double, dMin: Double = 0.0
    ) -> Double {
        let span = shadowPoint - anchor
        if span <= 1e-6 { return slope }
        let vBlack = referenceLinearValue(dMin: dMin, target: K.shadowReachDensity)
        let needed = (vBlack - referenceLinearValue(dMin: dMin)) / span
        return min(max(slope, needed), K.slopeMax)
    }

    /// highlight_hold_offset (8532dd92): Auto Grade's soft exposure — the
    /// highlight-zone burn that lands the textured bright tail at
    /// `highlightPoint` on highlightHoldDensity when the straight line would
    /// print it brighter. Solved against the zone term's own weight at that
    /// tone, so the burn lands exactly and stays under the shoulder. Never
    /// lifts; 0 when the tone already holds (or the target is 0 = off).
    public static func highlightHoldOffset(
        slope: Double, pivot: Double, highlightPoint: Double, dMin: Double = 0.0
    ) -> Double {
        let target = K.highlightHoldDensity
        if target <= 0.0 { return 0.0 }
        let v = slope * (highlightPoint - pivot)
        let vHold = referenceLinearValue(dMin: dMin, target: target)
        if v >= vHold { return 0.0 }
        let zHi = K.anchorTargetDensity + K.zoneDensityHighlightOffset
        let w = 1.0 - sigmoid(K.zoneDensitySharpness * (v - zHi))
        return min((vHold - v) / max(w, 1e-6), K.highlightHoldMax)
    }

    /// compute_pivot: solve so the reference tone prints at anchor_target_density.
    public static func computePivot(slope: Double, density: Double, dMin: Double = 0.0, anchor: Double? = nil) -> Double {
        let ref = anchor ?? K.assumedAnchor
        let vStar = referenceLinearValue(dMin: dMin)
        let base = ref - vStar / slope
        return base + (1.0 - density) * K.densityMultiplier
    }

    /// normalize_refs: raw log refs → normalized position in the floor→ceil stretch.
    public static func normalizeRefs(_ refs: SIMD3<Double>, bounds: LogNegativeBounds) -> SIMD3<Double> {
        let eps = 1e-6
        var out = SIMD3<Double>()
        for ch in 0..<3 {
            var denom = bounds.ceils[ch] - bounds.floors[ch]
            if abs(denom) < eps { denom = denom >= 0 ? eps : -eps }
            out[ch] = (refs[ch] - bounds.floors[ch]) / denom
        }
        return out
    }

    /// effective_cast_strength: the neutral-set confidence always biases the
    /// slider (NegPy 0.36 dropped the auto toggle — clean greys get full
    /// strength, ambiguous frames get gentler correction).
    public static func effectiveCastStrength(_ strength: Double, confidence: Double?) -> Double {
        if let confidence { return confidence * strength }
        return strength
    }

    /// filtration_offsets: WB sliders → normalized-space per-channel offsets.
    public static func filtrationOffsets(wbCMY: SIMD3<Double>, bounds: LogNegativeBounds?) -> SIMD3<Double> {
        var out = SIMD3<Double>()
        for ch in 0..<3 {
            var d = wbCMY[ch] * K.cmyMaxDensity
            if let bounds {
                d /= max(abs(bounds.ceils[ch] - bounds.floors[ch]), 1e-6)
            }
            out[ch] = d
        }
        return out
    }

    /// per_channel_curve_params: (slopes, pivots, curvatures) — the heart of
    /// C-41 gray balance. Green is the reference; R/B fit its neutral axis
    /// (quadratic through 3 green-matched points, else 2-point line, else the
    /// one-point shadow-ref tie, else a shared linear curve).
    public static func perChannelCurveParams(
        grade: Double,
        density: Double,
        autoNormalizeContrast: Bool,
        strength: Double,
        lumRange: Double?,
        shadowRefsNorm: SIMD3<Double>?,
        texturalRange: Double?,
        dMin: Double = 0.0,
        anchor: Double? = nil,
        neutralAxisNorm: (mid: SIMD3<Double>, shadow: SIMD3<Double>, highlight: SIMD3<Double>?)? = nil,
        shadowPoint: Double? = nil
    ) -> (slopes: SIMD3<Double>, pivots: SIMD3<Double>, curvatures: SIMD3<Double>) {
        let rEff = effectiveGradeRange(
            autoNormalizeContrast: autoNormalizeContrast, floorCeilRange: lumRange, texturalRange: texturalRange)
        var baseSlope = gradeToSlope(grade, densityRange: rEff)
        // Shadow Reach rides Auto Grade only: the dark tail's floor raises
        // the shared base slope before any per-channel cast solve.
        if autoNormalizeContrast, let shadowPoint {
            baseSlope = shadowReachSlope(
                baseSlope, anchor: anchor ?? K.assumedAnchor, shadowPoint: shadowPoint, dMin: dMin)
        }
        let eps = 1e-6

        if strength > 0, let na = neutralAxisNorm {
            let limit = K.midtoneCastMaxOffset
            let curvLim = K.neutralAxisCurvMaxRatio
            let mG = na.mid.y, sG = na.shadow.y
            let slopeG = min(max(baseSlope, K.slopeMin), K.slopeMax)
            let pivotG = computePivot(slope: slopeG, density: density, dMin: dMin, anchor: anchor)
            func target(_ g: Double) -> Double { slopeG * (g - pivotG) }
            let tM = target(mG), tS = target(sG)
            let hG: Double? = na.highlight?.y
            func clampDev(_ g: Double, _ v: Double) -> Double {
                g + min(max(strength * (v - g), -limit), limit)
            }

            var slopes = SIMD3<Double>(), pivots = SIMD3<Double>(), curvs = SIMD3<Double>()
            for ch in 0..<3 {
                if ch == 1 {
                    slopes[ch] = slopeG
                    pivots[ch] = pivotG
                    curvs[ch] = 0
                    continue
                }
                let uM = clampDev(mG, na.mid[ch])
                let uS = clampDev(sG, na.shadow[ch])

                var curv = 0.0
                if let hG, let hl = na.highlight {
                    let uH = clampDev(hG, hl[ch])
                    // Leading coefficient of the quadratic through the three
                    // green-matched points (divided differences form of the
                    // 3×3 Vandermonde solve in the Python original; a singular
                    // system there yields curv = 0, mirrored by the guard here).
                    if (uH - uM).magnitude > eps, (uM - uS).magnitude > eps, (uH - uS).magnitude > eps {
                        let d1 = (target(hG) - tM) / (uH - uM)
                        let d2 = (tM - tS) / (uM - uS)
                        curv = (d1 - d2) / (uH - uS)
                    }
                    curv = min(max(curv, -curvLim * slopeG), curvLim * slopeG)
                }

                let du = uM - uS
                var slopeCh = abs(du) < eps ? slopeG : ((tM - tS) - curv * (uM * uM - uS * uS)) / du
                slopeCh = min(max(slopeCh, K.slopeMin), K.slopeMax)
                let curvCh = curv
                let pivotCh = abs(slopeCh) > eps ? uM - (tM - curvCh * uM * uM) / slopeCh : pivotG
                slopes[ch] = slopeCh
                pivots[ch] = pivotCh
                curvs[ch] = curvCh
            }
            return (slopes, pivots, curvs)
        }

        if strength > 0, let refs = shadowRefsNorm {
            let anchorVal = anchor ?? K.assumedAnchor
            let limit = K.castRemovalMaxOffset
            let rGreen = refs.y
            let numer = anchorVal - rGreen

            var slopes = SIMD3<Double>(), pivots = SIMD3<Double>()
            for ch in 0..<3 {
                let cast = min(max(strength * (rGreen - refs[ch]), -limit), limit)
                let denom = anchorVal - (rGreen - cast)
                var slopeCh: Double
                if ch == 1 || abs(denom) < eps {
                    slopeCh = baseSlope
                } else {
                    slopeCh = baseSlope * numer / denom
                    slopeCh = min(max(slopeCh, K.slopeMin), K.slopeMax)
                }
                slopeCh = min(max(slopeCh, K.slopeMin), K.slopeMax)
                slopes[ch] = slopeCh
                pivots[ch] = computePivot(slope: slopeCh, density: density, dMin: dMin, anchor: anchor)
            }
            return (slopes, pivots, SIMD3(repeating: 0))
        }

        let s = min(max(baseSlope, K.slopeMin), K.slopeMax)
        let p = computePivot(slope: s, density: density, dMin: dMin, anchor: anchor)
        return (SIMD3(repeating: s), SIMD3(repeating: p), SIMD3(repeating: 0))
    }
}
