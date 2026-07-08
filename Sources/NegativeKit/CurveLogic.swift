import Foundation

/// Print-curve parameter derivation — port of negpy/features/exposure/logic.py
/// (grade math, pivot solve, cast removal, WB filtration). Single source of truth
/// for the Metal uniforms, the CPU reference curve and (later) the curve chart.
public enum CurveLogic {
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

    /// Fallback density range: auto_grade_target * nominal ratio.
    public static var defaultGradeRange: Double { K.autoGradeTarget * K.autoGradeNominalRatio }

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

    /// effective_grade_range: Auto Grade damping of the floor-ceil/textural ratio.
    public static func effectiveGradeRange(
        autoNormalizeContrast: Bool, floorCeilRange: Double?, texturalRange: Double?
    ) -> Double? {
        if !autoNormalizeContrast { return floorCeilRange }
        guard let texturalRange, let floorCeilRange else { return defaultGradeRange }
        let measured = abs(texturalRange)
        if measured < 1e-6 { return 3.5 }
        let ratio = abs(floorCeilRange) / measured
        return K.autoGradeTarget * (K.autoGradeNominalRatio + K.autoGradeStrength * (ratio - K.autoGradeNominalRatio))
    }

    /// _reference_linear_value: straight-line density v* that the base toe/shoulder
    /// bounds map onto anchor_target_density (closed form via inverse softplus).
    public static func referenceLinearValue(dMin: Double = 0.0) -> Double {
        let t = K.anchorTargetDensity
        let aHl = K.shoulderSharpnessBase
        let aSh = K.toeSharpnessBase
        let v1 = K.dMax - invSoftplus(aSh * (K.dMax - t)) / aSh
        return dMin + invSoftplus(aHl * (v1 - dMin)) / aHl
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

    /// effective_cast_strength: auto biases the slider by neutral-set confidence.
    public static func effectiveCastStrength(_ strength: Double, auto: Bool, confidence: Double?) -> Double {
        if auto, let confidence { return confidence * strength }
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
        neutralAxisNorm: (mid: SIMD3<Double>, shadow: SIMD3<Double>, highlight: SIMD3<Double>?)? = nil
    ) -> (slopes: SIMD3<Double>, pivots: SIMD3<Double>, curvatures: SIMD3<Double>) {
        let rEff = effectiveGradeRange(
            autoNormalizeContrast: autoNormalizeContrast, floorCeilRange: lumRange, texturalRange: texturalRange)
        let baseSlope = gradeToSlope(grade, densityRange: rEff)
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
