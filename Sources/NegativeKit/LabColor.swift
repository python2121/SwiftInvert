import Foundation
import simd

/// CIELAB in the working space (linear Adobe RGB (1998), D65) and the
/// saturation/vibrance chroma ops — ports of negpy/kernel/image/logic.py
/// (rgb_to_lab_working / lab_to_rgb_working, Adobe RGB since b3490eb) and
/// negpy/features/lab/logic.py (apply_saturation / apply_vibrance).
/// Mirrored in NegPipeline.metal.
public enum LabColor {
    // Adobe RGB (1998) primaries, D65 white (upstream _WORKING_TO_XYZ).
    static let toXYZ: (SIMD3<Double>, SIMD3<Double>, SIMD3<Double>) = (
        SIMD3(0.5767309, 0.1855540, 0.1881852),
        SIMD3(0.2973769, 0.6273491, 0.0752741),
        SIMD3(0.0270343, 0.0706872, 0.9911085)
    )
    static let toRGB: (SIMD3<Double>, SIMD3<Double>, SIMD3<Double>) = (
        SIMD3(2.0413690, -0.5649464, -0.3446944),
        SIMD3(-0.9692660, 1.8760108, 0.0415560),
        SIMD3(0.0134474, -0.1183897, 1.0154096)
    )
    static let workingWhite = SIMD3<Double>(0.95047, 1.00000, 1.08883)
    static let labEps = 0.008856
    static let labKappa = 7.787
    /// Chroma below which vibrance considers a color "muted" (NegPy: /60).
    public static let vibranceChromaRange = 60.0

    // ── Color mixer bands (chroma-gated, hue-targeted) ────────────────────
    // Membership = raised-cosine hue window around each band center × a
    // chroma ramp that reaches zero at the neutral axis — whites/grays and
    // faint casts are untouched by construction (the complement of the WB
    // sliders, which move every pixel). SwiftInvert-only; NegPy has no
    // equivalent. Mirrored as literals in NegPipeline.metal colorPop.
    // Band order everywhere: Red, Yellow, Green, Blue. Tuned empirically on
    // real inverted scans, not colorimetric primaries: colorful blues land at
    // Lab hue ~200-250 in this pipeline (sky/cyan-blue), and typical colorful
    // content sits at chroma 12-30 (p90 of whole frames ~15-30), so the gate
    // must saturate well below the ~60-chroma of an sRGB primary.
    public static let bandCentersDeg = [25.0, 90.0, 135.0, 235.0]
    public static let bandHalfWidthsDeg = [45.0, 45.0, 50.0, 65.0]
    public static let bandChromaGateLow = 6.0  // below: fully protected
    public static let bandChromaGateHigh = 16.0  // above: full membership
    /// Hue rotation at slider ±1 (degrees; + rotates toward the next band
    /// counterclockwise: red→orange, yellow→green, green→teal, blue→purple).
    public static let bandMaxHueShiftDeg = 30.0

    /// Linear working RGB → CIELAB (D65), OpenCV float scale (L 0–100).

    public static func rgbToLab(_ rgb: SIMD3<Double>) -> SIMD3<Double> {
        let lin = simd_max(rgb, SIMD3<Double>())
        var xyz = SIMD3(
            simd_dot(toXYZ.0, lin), simd_dot(toXYZ.1, lin), simd_dot(toXYZ.2, lin))
        xyz /= workingWhite
        var f = SIMD3<Double>()
        for i in 0..<3 {
            f[i] = xyz[i] > labEps ? cbrt(xyz[i]) : labKappa * xyz[i] + 16.0 / 116.0
        }
        return SIMD3(116.0 * f.y - 16.0, 500.0 * (f.x - f.y), 200.0 * (f.y - f.z))
    }

    /// CIELAB (D65) → linear working RGB (lower-clamped, no upper clip).

    public static func labToRgb(_ lab: SIMD3<Double>) -> SIMD3<Double> {
        let fy = (lab.x + 16.0) / 116.0
        let f = SIMD3(lab.y / 500.0 + fy, fy, fy - lab.z / 200.0)
        var xyz = SIMD3<Double>()
        for i in 0..<3 {
            let f3 = f[i] * f[i] * f[i]
            xyz[i] = f3 > labEps ? f3 : (f[i] - 16.0 / 116.0) / labKappa
        }
        xyz *= workingWhite
        let lin = SIMD3(
            simd_dot(toRGB.0, xyz), simd_dot(toRGB.1, xyz), simd_dot(toRGB.2, xyz))
        return simd_max(lin, SIMD3<Double>())
    }

    static func smoothstep(_ e0: Double, _ e1: Double, _ x: Double) -> Double {
        let t = min(max((x - e0) / (e1 - e0), 0.0), 1.0)
        return t * t * (3.0 - 2.0 * t)
    }

    /// Per-band hue rotation + chroma scale on one linear pixel (band order
    /// R/Y/G/B). All band weights read the ORIGINAL hue and compose jointly,
    /// so overlapping feathers are order-independent; the shared chroma gate
    /// keeps neutrals and faint casts untouched.
    public static func applyColorMixer(
        _ rgb: SIMD3<Double>, hues: SIMD4<Double>, saturations: SIMD4<Double>
    ) -> SIMD3<Double> {
        guard hues != .zero || saturations != SIMD4(repeating: 1.0) else { return rgb }
        var lab = rgbToLab(rgb)
        let chroma = (lab.y * lab.y + lab.z * lab.z).squareRoot()
        let gate = smoothstep(bandChromaGateLow, bandChromaGateHigh, chroma)
        guard gate > 0 else { return rgb }
        let hueDeg = atan2(lab.z, lab.y) * 180.0 / .pi
        var deltaDeg = 0.0
        var chromaScale = 1.0
        for i in 0..<4 {
            let dh = (hueDeg - bandCentersDeg[i] + 540.0)
                .truncatingRemainder(dividingBy: 360.0) - 180.0
            let t = min(abs(dh) / bandHalfWidthsDeg[i], 1.0)
            let w = gate * 0.5 * (1.0 + cos(.pi * t))
            deltaDeg += hues[i] * bandMaxHueShiftDeg * w
            chromaScale *= 1.0 + (saturations[i] - 1.0) * w
        }
        if deltaDeg == 0 && chromaScale == 1.0 { return rgb }
        let newHue = (hueDeg + deltaDeg) * .pi / 180.0
        let newChroma = chroma * max(chromaScale, 0.0)
        lab.y = newChroma * cos(newHue)
        lab.z = newChroma * sin(newHue)
        return simd_clamp(labToRgb(lab), SIMD3<Double>(), SIMD3(repeating: 1))
    }

    /// Vibrance then saturation on one linear pixel (LabProcessor order):
    /// vibrance boosts muted chroma toward the strength, saturation scales all
    /// chroma; each returns clipped to [0, 1] like NegPy's per-op clip.

    // ── Gamut-aware chroma boost (NegPy 1b900ab, skin term removed in
    //    bfcd90a) ────────────────────────────────────────────────────────
    // A flat a*/b* scale preserves hue, but overshooting pixels get
    // hard-clamped per RGB channel afterward — clamping only the channel(s)
    // that overshot changes the R:G:B ratio, shifting the visible hue. The
    // boost path therefore soft-knees toward each pixel's true in-gamut
    // headroom (bisection against the real RGB cube) instead of clamping;
    // in-gamut pixels get the exact flat scale. Desaturation stays flat.
    // Constants mirrored as literals in NegPipeline.metal — keep in sync.
    public static let gamutIterations = 10
    public static let gamutTolerance = 1e-4

    // ── Skin Protection (NegPy bfcd90a + fb94aed) ────────────────────────
    // A one-directional soft chroma ceiling inside the measured CIELAB skin
    // locus, applied after the saturation scale and independent of it — it
    // also reins skin that arrived over-chromatic from the print curve.
    // Chroma is only ever reduced; a* and b* scale together, so hue and L*
    // never move. The mask: hue Gaussian around 52° (σ 20), a one-sided
    // chroma window (full ≤35, zero ≥60 — skin runs C*≈12–40; pure red
    // ≈104 drops out entirely), and a lightness rolloff (L* 15…95).
    public static let skinHueCenterDeg = 52.0
    public static let skinHueSigmaDeg = 20.0
    public static let skinChromaFull = 35.0
    public static let skinChromaZero = 60.0
    public static let skinLightLo = 15.0
    public static let skinLightHi = 95.0
    public static let skinChromaGate = 2.0
    /// Ceiling at full strength; the rein pulls toward ceil/strength.
    public static let skinCeilingAtFull = 22.0
    public static let skinKneeStartFraction = 0.6

    /// 0 (not skin) … 1 (dead centre of the skin locus). Near-neutral
    /// pixels gate to 0 (their hue angle is noise). Uses the mixer's
    /// smoothstep above.
    static func skinWeight(L: Double, a: Double, b: Double) -> Double {
        let chroma = (a * a + b * b).squareRoot()
        guard chroma >= skinChromaGate else { return 0 }
        let hueDeg = atan2(b, a) * 180.0 / .pi
        var dist = hueDeg - skinHueCenterDeg
        dist -= 360.0 * (dist / 360.0).rounded()
        let x = dist / skinHueSigmaDeg
        let wHue = exp(-0.5 * x * x)
        let wChroma = 1.0 - smoothstep(skinChromaFull, skinChromaZero, chroma)
        let wLight = smoothstep(0.0, skinLightLo, L) * (1.0 - smoothstep(skinLightHi, 100.0, L))
        return wHue * wChroma * wLight
    }

    /// The rein: soft chroma ceiling (22/strength, softplus knee from
    /// 0.6×ceiling) blended by the skin weight. Identity at strength ≤ 0
    /// and below the knee start.
    static func skinChromaRein(_ lab: SIMD3<Double>, strength: Double) -> SIMD3<Double> {
        guard strength > 0 else { return lab }
        let ceiling = skinCeilingAtFull / strength
        let start = skinKneeStartFraction * ceiling
        let span = ceiling - start
        let chroma = (lab.y * lab.y + lab.z * lab.z).squareRoot()
        guard chroma > start else { return lab }
        let w = skinWeight(L: lab.x, a: lab.y, b: lab.z)
        guard w > 0 else { return lab }
        let knee = start + span * (1.0 - exp(-(chroma - start) / span))
        let scale = (chroma + w * (knee - chroma)) / chroma
        return SIMD3(lab.x, lab.y * scale, lab.z * scale)
    }

    /// Whether (L, a, b) decodes to linear working RGB within [0, 1] on all
    /// channels (± tolerance). Unlike labToRgb, no lower clamp — the raw
    /// values ARE the gamut decision.
    static func inGamutLab(_ L: Double, _ a: Double, _ b: Double) -> Bool {
        let fy = (L + 16.0) / 116.0
        let f = SIMD3(a / 500.0 + fy, fy, fy - b / 200.0)
        var xyz = SIMD3<Double>()
        for i in 0..<3 {
            let f3 = f[i] * f[i] * f[i]
            xyz[i] = (f3 > labEps ? f3 : (f[i] - 16.0 / 116.0) / labKappa) * workingWhite[i]
        }
        let rgb = SIMD3(
            simd_dot(toRGB.0, xyz), simd_dot(toRGB.1, xyz), simd_dot(toRGB.2, xyz))
        let tol = gamutTolerance
        return rgb.min() >= -tol && rgb.max() <= 1.0 + tol
    }

    /// The effective boost for one pixel: the full push if it already lands
    /// in gamut, else a softplus-style knee toward the bisected in-gamut
    /// headroom. Pure gamut math — the skin term was removed upstream
    /// (bfcd90a); Skin Protection is its own operator now. Caller ensures
    /// saturation > 1.
    static func gamutAwareBoost(_ lab: SIMD3<Double>, saturation: Double) -> Double {
        if inGamutLab(lab.x, lab.y * saturation, lab.z * saturation) {
            // Full push already fits — use it directly (bisecting here would
            // misread the search's own upper bound as a constraint and
            // throttle pixels that were never going to clip).
            return saturation
        }
        var lo = 1.0, hi = saturation
        let stillOk = inGamutLab(lab.x, lab.y, lab.z)
        for _ in 0..<gamutIterations {
            let mid = (lo + hi) / 2.0
            if stillOk && inGamutLab(lab.x, lab.y * mid, lab.z * mid) {
                lo = mid
            } else {
                hi = mid
            }
        }
        let sMax = max(lo, 1.0 + gamutTolerance)
        let knee = sMax - 1.0
        return 1.0 + knee * (1.0 - exp(-(saturation - 1.0) / knee))
    }

    public static func applyVibranceSaturation(
        _ rgb: SIMD3<Double>, vibrance: Double, saturation: Double, skinProtection: Double = 0
    ) -> SIMD3<Double> {
        var color = rgb
        if vibrance != 1.0 {
            var lab = rgbToLab(color)
            let chroma = (lab.y * lab.y + lab.z * lab.z).squareRoot()
            let muted = min(max(1.0 - chroma / vibranceChromaRange, 0.0), 1.0)
            let boost = 1.0 + (vibrance - 1.0) * muted
            lab.y *= boost
            lab.z *= boost
            color = simd_clamp(labToRgb(lab), SIMD3<Double>(), SIMD3(repeating: 1))
        }
        if saturation != 1.0 || skinProtection > 0 {
            var lab = rgbToLab(color)
            if saturation != 1.0 {
                // Boosts are gamut-aware (1b900ab); desaturation never
                // overshoots, so it stays the exact flat scale.
                let eff = saturation > 1.0 ? gamutAwareBoost(lab, saturation: saturation) : saturation
                lab.y *= eff
                lab.z *= eff
            }
            // Skin Protection runs after the scale and independent of it —
            // reduce-only, so it can't overshoot the gamut the knee fitted.
            lab = skinChromaRein(lab, strength: skinProtection)
            color = simd_clamp(labToRgb(lab), SIMD3<Double>(), SIMD3(repeating: 1))
        }
        return color
    }

    /// Whole-buffer application (CPU reference path).
    public static func apply(
        _ img: RGBImage, vibrance: Double, saturation: Double, skinProtection: Double = 0
    ) -> RGBImage {
        guard vibrance != 1.0 || saturation != 1.0 || skinProtection > 0 else { return img }
        var out = img
        out.pixels.withUnsafeMutableBufferPointer { buf in
            var i = 0
            while i < buf.count {
                let rgb = SIMD3(Double(buf[i]), Double(buf[i + 1]), Double(buf[i + 2]))
                let res = applyVibranceSaturation(
                    rgb, vibrance: vibrance, saturation: saturation, skinProtection: skinProtection)
                buf[i] = Float(res.x)
                buf[i + 1] = Float(res.y)
                buf[i + 2] = Float(res.z)
                i += 3
            }
        }
        return out
    }
}
