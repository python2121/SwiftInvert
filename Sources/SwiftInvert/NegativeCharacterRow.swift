import NegativeKit
import SwiftUI

/// "Negative: 1.42 · contrasty (≈N+1)" — the measured density range and the
/// grade it implies (NegPy's `stats._negative_row`). A read-out, not a control:
/// it tells you which way to take the Grade slider above it.
struct NegativeCharacterRow: View {
    var densityRange: Double

    var body: some View {
        let character = Densitometry.character(densityRange: densityRange)
        HStack(spacing: 4) {
            Text("Negative")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
            Spacer()
            Text(value(character))
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(character == nil ? .tertiary : .secondary)
        }
        .help(
            character == nil
                ? "The negative's measured density range, once the frame is analysed."
                : "Measured density range vs what a normal grade expects: "
                    + "flat negatives want a harder grade (lower ISO R), contrasty ones a softer grade.")
    }

    private func value(_ character: Densitometry.NegativeCharacter?) -> String {
        guard let character else { return "—" }
        return String(format: "%.2f · %@", densityRange, character.label)
    }
}
