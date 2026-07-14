import Foundation
import NegativeKit

/// Human-readable label for a settings change (the history entry text).
/// Diffs the two states field by field; single/double changes name themselves,
/// bigger diffs collapse to "Multiple changes".
func historyLabel(from a: ExposureSettings, to b: ExposureSettings) -> String {
    var changes: [String] = []
    func add(_ name: String, _ changed: Bool, _ value: String = "") {
        if changed { changes.append(value.isEmpty ? name : "\(name) \(value)") }
    }
    func num(_ v: Double, _ format: String = "%.2f") -> String { String(format: format, v) }

    add("Brightness", a.density != b.density, num(2 - b.density))
    add("Grade", a.grade != b.grade, num(b.grade, "%.0f"))
    add("Exposure", a.exposureStops != b.exposureStops, num(b.exposureStops, "%+.2f"))
    add("Contrast", a.overallContrast != b.overallContrast, num(b.overallContrast))
    add("Shadows", a.shadows != b.shadows, num(b.shadows))
    add("Shadow contrast", a.shadowContrast != b.shadowContrast, num(b.shadowContrast))
    add("Dark shadows", a.darkShadows != b.darkShadows, num(b.darkShadows))
    add("Highlights", a.highlights != b.highlights, num(b.highlights))
    add("Highlight contrast", a.highlightContrast != b.highlightContrast, num(b.highlightContrast))
    add("Pre-saturation", a.preSaturation != b.preSaturation, num(b.preSaturation))
    add("Vibrance", a.vibrance != b.vibrance, num(b.vibrance))
    add("Saturation", a.saturation != b.saturation, num(b.saturation))
    add("Red hue", a.redHue != b.redHue, num(b.redHue, "%+.2f"))
    add("Red saturation", a.redSaturation != b.redSaturation, num(b.redSaturation))
    add("Cast strength", a.castRemovalStrength != b.castRemovalStrength, num(b.castRemovalStrength))
    add("Temp", a.temp != b.temp, num(b.temp))
    add("Tint", a.tint != b.tint, num(b.tint))
    add("Shadow color", a.colorShadows != b.colorShadows)
    add("Mid color", a.colorMids != b.colorMids)
    add("Highlight color", a.colorHighs != b.colorHighs)
    add("WB trim", a.wbCyan != b.wbCyan || a.wbMagenta != b.wbMagenta || a.wbYellow != b.wbYellow)
    add("Toe", a.toe != b.toe, num(b.toe))
    add("Toe width", a.toeWidth != b.toeWidth, num(b.toeWidth))
    add("Shoulder", a.shoulder != b.shoulder, num(b.shoulder))
    add("Shoulder width", a.shoulderWidth != b.shoulderWidth, num(b.shoulderWidth))
    add("White point", a.whitePointOffset != b.whitePointOffset, num(b.whitePointOffset, "%.3f"))
    add("Black point", a.blackPointOffset != b.blackPointOffset, num(b.blackPointOffset, "%.3f"))
    add("Auto exposure", a.autoExposure != b.autoExposure, b.autoExposure ? "on" : "off")
    add("Auto contrast", a.autoNormalizeContrast != b.autoNormalizeContrast, b.autoNormalizeContrast ? "on" : "off")
    add("Paper white", a.paperDmin != b.paperDmin, b.paperDmin ? "on" : "off")
    add("True black", a.trueBlack != b.trueBlack, b.trueBlack ? "on" : "off")
    add("Rotate", a.rotation != b.rotation)
    add("Fine rotation", a.fineRotation != b.fineRotation, num(b.fineRotation, "%.1f°"))
    add("Flip", a.flipHorizontal != b.flipHorizontal)
    add(b.cropRect == nil ? "Crop cleared" : "Crop", a.cropRect != b.cropRect)
    add(b.analysisRect == nil ? "Analysis region cleared" : "Analysis region", a.analysisRect != b.analysisRect)

    switch changes.count {
    case 0: return "Edit"
    case 1, 2: return changes.joined(separator: ", ")
    default: return "Multiple changes"
    }
}
