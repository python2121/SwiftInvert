import Foundation

/// Darkroom read-outs: the spot densitometer and the negative-character
/// diagnostic (NegPy `features/exposure/densitometer.py` and `stats.py`'s
/// `_negative_row`). Pure measurement — nothing here feeds the render, so it
/// has no parity surface and no kernel mirror.
///
/// Densities are relative to this scan's normalization, not absolute.
public enum Densitometry {

    // MARK: - Spot densitometer

    /// Reflection density is meaningless past paper black; NegPy clamps here.
    public static let printDensityMax = 4.0
    /// Zone V is 18% reflectance by definition — the ruler's hinge.
    public static let zoneMidReflectance = 0.18

    /// Display-encoded lightness of Zone V.
    public static var midGrayEncoded: Double {
        Double(WorkingOETF.encode(Float(zoneMidReflectance)))
    }

    /// Rec.709 luma of a display-encoded triplet. NegPy takes luma on the
    /// ENCODED values and decodes afterwards; the order matters (the OETF is
    /// non-linear, so decoding per channel first gives a different number).
    public static func luma(_ rgb: SIMD3<Double>) -> Double {
        K.lumaR * rgb.x + K.lumaG * rgb.y + K.lumaB * rgb.z
    }

    /// Print zone (0…10) of a display-encoded lightness. Piecewise-linear in
    /// ENCODED space with V pinned to 18% gray, because stops-per-zone can't
    /// reach the top zones on a print (NegPy `zone_of_encoded`).
    public static func zone(ofEncoded enc: Double) -> Double {
        let e = min(max(enc, 0), 1)
        let mid = midGrayEncoded
        return e <= mid ? 5.0 * e / mid : 5.0 + 5.0 * (e - mid) / (1.0 - mid)
    }

    /// Reflection density of a display-encoded lightness.
    public static func printDensity(ofEncoded enc: Double) -> Double {
        let e = min(max(enc, 0), 1)
        let t = max(Double(WorkingOETF.decode(Float(e))), pow(10.0, -printDensityMax))
        return min(printDensityMax, -log10(t))
    }

    /// One probed pixel, in darkroom units.
    public struct Reading: Equatable, Sendable {
        /// Display-encoded RGB, 0…1 — what the pixel actually is on screen.
        public var rgb: SIMD3<Double>
        /// Reflection density of the displayed tone.
        public var printDensity: Double
        /// Print zone 0…10 (V = 18% gray).
        public var zone: Double

        public init(rgb: SIMD3<Double>, printDensity: Double, zone: Double) {
            self.rgb = rgb
            self.printDensity = printDensity
            self.zone = zone
        }
    }

    public static func read(encodedRGB rgb: SIMD3<Double>) -> Reading {
        let lum = luma(rgb)
        return Reading(
            rgb: rgb, printDensity: printDensity(ofEncoded: lum), zone: zone(ofEncoded: lum))
    }

    private static let roman = ["0", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"]
    private static let thirds = ["", "⅓", "⅔"]

    /// Zone in roman numerals with ⅓-stop fractions (4.33 → "IV⅓").
    public static func zoneRoman(_ zone: Double) -> String {
        let t = Int((min(max(zone, 0), 10) * 3).rounded())
        let (base, frac) = (t / 3, t % 3)
        if base >= 10 { return roman[10] }
        return roman[base] + thirds[frac]
    }

    // MARK: - Negative character

    /// How the negative's own contrast compares to what the grade expects.
    public enum NegativeCharacter: String, Equatable, Sendable {
        case flat
        case normal
        case contrasty

        /// NegPy's wording — the grade hint is the actionable half.
        public var label: String {
            switch self {
            case .flat: return "flat (≈N−1)"
            case .normal: return "normal"
            case .contrasty: return "contrasty (≈N+1)"
            }
        }
    }

    /// Ratio thresholds (NegPy `stats._DIAG_FLAT` / `_DIAG_CONTRASTY`).
    public static let characterFlatRatio = 0.80
    public static let characterContrastyRatio = 1.25

    /// The negative's measured density range against `defaultGradeRange`.
    ///
    /// `densityRange` is the PRE-offset luminance density range — NegPy's
    /// `norm_density_range`, the same value `effectiveGradeRange` reads (see
    /// `deriveRenderParams`). Nil when there's nothing measured yet.
    public static func character(densityRange: Double) -> NegativeCharacter? {
        guard densityRange > 0, densityRange.isFinite else { return nil }
        let ratio = densityRange / CurveLogic.defaultGradeRange
        if ratio < characterFlatRatio { return .flat }
        if ratio > characterContrastyRatio { return .contrasty }
        return .normal
    }
}
