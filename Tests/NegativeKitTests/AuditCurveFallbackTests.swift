import Foundation
import Testing

@testable import NegativeKit

/// perChannelCurveParams' shadow-ref fallback branch (no neutral axis):
/// the one-point slope tie at the P98 shadow refs, its ±castRemovalMaxOffset
/// clamp, and the branch's zero curvatures. Expectations are hand-derived
/// from the implementation formula:
///   cast     = clamp(strength · (rG − ref[ch]), ±K.castRemovalMaxOffset)
///   slope_ch = clamp(baseSlope · (anchor − rG) / (anchor − (rG − cast)), 2…10)
///   pivot_ch = anchor − v*/slope_ch + (1 − density) · K.densityMultiplier
@Suite struct AuditCurveFallbackTests {

    // Fixed inputs, chosen so no slope clamp engages except where intended.
    static let grade = 115.0
    static let lumRange = 1.2
    static let anchor = 0.5
    static let density = 1.0

    /// baseSlope by hand: autoNormalizeContrast = false → range = lumRange;
    /// er = 115/100, rng = 1.2 → k = 2.9 · 1.2 / 1.15 = 3.02608695…,
    /// inside [2, 10] so no clamp.
    static var baseSlope: Double { K.gradeContrastScale * lumRange / (grade / 100.0) }

    /// v* from the documented closed form (K.dMax 2.3, target 0.75,
    /// sharpness 4/3), written out with libm inverse-softplus — NOT by
    /// calling referenceLinearValue.
    static var vStar: Double {
        let v1 = K.dMax - log(expm1(K.toeSharpnessBase * (K.dMax - K.anchorTargetDensity)))
            / K.toeSharpnessBase
        return log(expm1(K.shoulderSharpnessBase * v1)) / K.shoulderSharpnessBase
    }

    private func params(refs: SIMD3<Double>, strength: Double)
        -> (slopes: SIMD3<Double>, pivots: SIMD3<Double>, curvatures: SIMD3<Double>)
    {
        CurveLogic.perChannelCurveParams(
            grade: Self.grade,
            density: Self.density,
            autoNormalizeContrast: false,
            strength: strength,
            lumRange: Self.lumRange,
            shadowRefsNorm: refs,
            texturalRange: nil,
            dMin: 0.0,
            anchor: Self.anchor,
            neutralAxisNorm: nil)
    }

    @Test func shadowRefTieMatchesClosedForm() {
        // refs = (0.95, 0.90, 0.85), strength 0.5 — casts stay inside ±0.1:
        //   cast_r = 0.5·(0.90 − 0.95) = −0.025
        //   cast_b = 0.5·(0.90 − 0.85) = +0.025
        let refs = SIMD3(0.95, 0.90, 0.85)
        let p = params(refs: refs, strength: 0.5)
        let base = Self.baseSlope
        let numer = Self.anchor - refs.y  // 0.5 − 0.9 = −0.4

        // Green is the reference: exactly the base slope, no tie.
        #expect(p.slopes.y == base)

        // Red: denom = 0.5 − (0.90 − (−0.025)) = −0.425 →
        //   slope_r = base · (−0.4 / −0.425) = base · 0.941176…
        let slopeR = base * numer / (Self.anchor - (refs.y + 0.025))
        #expect(abs(p.slopes.x - slopeR) < 1e-12)
        // Blue: denom = 0.5 − (0.90 − 0.025) = −0.375 →
        //   slope_b = base · (−0.4 / −0.375) = base · 1.0666…
        let slopeB = base * numer / (Self.anchor - (refs.y - 0.025))
        #expect(abs(p.slopes.z - slopeB) < 1e-12)
        // Both inside [2, 10], so the clamp changed nothing (≈2.85, ≈3.23).
        #expect((K.slopeMin...K.slopeMax).contains(p.slopes.x))
        #expect((K.slopeMin...K.slopeMax).contains(p.slopes.z))

        // Curvatures are identically zero in this branch (straight lines).
        #expect(p.curvatures == .zero)

        // Pivots from the closed form (density 1.0 drops the last term).
        for ch in 0..<3 {
            let expected = Self.anchor - Self.vStar / p.slopes[ch]
                + (1.0 - Self.density) * K.densityMultiplier
            #expect(abs(p.pivots[ch] - expected) < 1e-12, "ch \(ch)")
        }
    }

    /// Extreme refs engage the cast clamp: the clamp applies AFTER the
    /// strength scaling, so the effective offset is exactly
    /// ±K.castRemovalMaxOffset (not strength · offset). Back-solved from the
    /// returned slope: cast = base·numer/slope − anchor + rG.
    @Test func extremeRefsClampAtCastRemovalMaxOffset() {
        // Red ref 0.5, strength 1.0: raw cast = 1.0·(0.9 − 0.5) = +0.4 → +0.1.
        // Blue ref 1.4: raw cast = 1.0·(0.9 − 1.4) = −0.5 → −0.1.
        let refs = SIMD3(0.5, 0.9, 1.4)
        let p = params(refs: refs, strength: 1.0)
        let base = Self.baseSlope
        let numer = Self.anchor - refs.y

        // Forward check with the clamped literal:
        //   slope_r = base · (−0.4) / (0.5 − (0.9 − 0.1)) = base · 4/3 ≈ 4.03
        //   slope_b = base · (−0.4) / (0.5 − (0.9 + 0.1)) = base · 0.8 ≈ 2.42
        // (both inside [2, 10], no slope clamp).
        #expect(abs(p.slopes.x - base * numer / (Self.anchor - (refs.y - K.castRemovalMaxOffset))) < 1e-12)
        #expect(abs(p.slopes.z - base * numer / (Self.anchor - (refs.y + K.castRemovalMaxOffset))) < 1e-12)

        // Back-solve the effective cast from each returned slope; it must be
        // EXACTLY the clamp constant.
        let castR = base * numer / p.slopes.x - Self.anchor + refs.y
        let castB = base * numer / p.slopes.z - Self.anchor + refs.y
        #expect(abs(castR - K.castRemovalMaxOffset) < 1e-12)
        #expect(abs(castB + K.castRemovalMaxOffset) < 1e-12)

        #expect(p.slopes.y == base)
        #expect(p.curvatures == .zero)
    }

    /// The tie scales with strength below the clamp: half strength moves the
    /// red slope exactly to the half-cast tie.
    @Test func strengthScalesTheCastBeforeTheClamp() {
        let refs = SIMD3(0.98, 0.90, 0.90)
        let base = Self.baseSlope
        let numer = Self.anchor - refs.y
        for strength in [0.25, 0.5, 1.0] {
            let p = params(refs: refs, strength: strength)
            // cast = strength · (0.90 − 0.98) = −0.08·strength — never clamped.
            let cast = strength * (refs.y - refs.x)
            #expect(abs(cast) < K.castRemovalMaxOffset)
            let expected = base * numer / (Self.anchor - (refs.y - cast))
            #expect(abs(p.slopes.x - expected) < 1e-12, "strength \(strength)")
            // Untied channels stay at base (blue's cast is exactly 0, so its
            // tie degenerates to base·numer/numer — fp round-trip, not bitwise).
            #expect(p.slopes.y == base)
            #expect(abs(p.slopes.z - base) < 1e-12)
        }
    }
}
