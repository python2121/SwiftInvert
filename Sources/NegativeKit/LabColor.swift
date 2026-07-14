import Foundation
import simd

/// CIELAB in the working space (linear ProPhoto RGB / ROMM, D50) and the
/// saturation/vibrance chroma ops — ports of negpy/kernel/image/logic.py
/// (rgb_to_lab_working / lab_to_rgb_working) and negpy/features/lab/logic.py
/// (apply_saturation / apply_vibrance). Mirrored in NegPipeline.metal.
public enum LabColor {
    // ProPhoto (ROMM) primaries, D50 white.
    static let toXYZ: (SIMD3<Double>, SIMD3<Double>, SIMD3<Double>) = (
        SIMD3(0.7976749, 0.1351917, 0.0313534),
        SIMD3(0.2880402, 0.7118741, 0.0000857),
        SIMD3(0.0000000, 0.0000000, 0.8252100)
    )
    static let toRGB: (SIMD3<Double>, SIMD3<Double>, SIMD3<Double>) = (
        SIMD3(1.3459433, -0.2556075, -0.0511118),
        SIMD3(-0.5445989, 1.5081673, 0.0205351),
        SIMD3(0.0000000, 0.0000000, 1.2118128)
    )
    static let d50White = SIMD3<Double>(0.96422, 1.00000, 0.82521)
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
    // Band order everywhere: Red, Yellow, Green, Blue.
    public static let bandCentersDeg = [25.0, 90.0, 145.0, 280.0]
    public static let bandHalfWidthsDeg = [45.0, 45.0, 50.0, 60.0]
    public static let bandChromaGateLow = 8.0  // below: fully protected
    public static let bandChromaGateHigh = 25.0  // above: full membership
    /// Hue rotation at slider ±1 (degrees; + rotates toward the next band
    /// counterclockwise: red→orange, yellow→green, green→teal, blue→purple).
    public static let bandMaxHueShiftDeg = 30.0

    /// Linear ProPhoto RGB → CIELAB (D50), OpenCV float scale (L 0–100).

    public static func rgbToLab(_ rgb: SIMD3<Double>) -> SIMD3<Double> {
        let lin = simd_max(rgb, SIMD3<Double>())
        var xyz = SIMD3(
            simd_dot(toXYZ.0, lin), simd_dot(toXYZ.1, lin), simd_dot(toXYZ.2, lin))
        xyz /= d50White
        var f = SIMD3<Double>()
        for i in 0..<3 {
            f[i] = xyz[i] > labEps ? cbrt(xyz[i]) : labKappa * xyz[i] + 16.0 / 116.0
        }
        return SIMD3(116.0 * f.y - 16.0, 500.0 * (f.x - f.y), 200.0 * (f.y - f.z))
    }

    /// CIELAB (D50) → linear ProPhoto RGB (lower-clamped, no upper clip).

    public static func labToRgb(_ lab: SIMD3<Double>) -> SIMD3<Double> {
        let fy = (lab.x + 16.0) / 116.0
        let f = SIMD3(lab.y / 500.0 + fy, fy, fy - lab.z / 200.0)
        var xyz = SIMD3<Double>()
        for i in 0..<3 {
            let f3 = f[i] * f[i] * f[i]
            xyz[i] = f3 > labEps ? f3 : (f[i] - 16.0 / 116.0) / labKappa
        }
        xyz *= d50White
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

    public static func applyVibranceSaturation(
        _ rgb: SIMD3<Double>, vibrance: Double, saturation: Double
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
        if saturation != 1.0 {
            var lab = rgbToLab(color)
            lab.y *= saturation
            lab.z *= saturation
            color = simd_clamp(labToRgb(lab), SIMD3<Double>(), SIMD3(repeating: 1))
        }
        return color
    }

    /// Whole-buffer application (CPU reference path).
    public static func apply(_ img: RGBImage, vibrance: Double, saturation: Double) -> RGBImage {
        guard vibrance != 1.0 || saturation != 1.0 else { return img }
        var out = img
        out.pixels.withUnsafeMutableBufferPointer { buf in
            var i = 0
            while i < buf.count {
                let rgb = SIMD3(Double(buf[i]), Double(buf[i + 1]), Double(buf[i + 2]))
                let res = applyVibranceSaturation(rgb, vibrance: vibrance, saturation: saturation)
                buf[i] = Float(res.x)
                buf[i + 1] = Float(res.y)
                buf[i + 2] = Float(res.z)
                i += 3
            }
        }
        return out
    }
}
