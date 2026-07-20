import Foundation
import Testing

@testable import NegativeKit

/// True Black (black point compensation, NegPy 0.36): paper Dmax maps to
/// display black instead of floating at 10^−2.3 ≈ 0.5% gray.
@Suite struct TrueBlackTests {
    @Test func mapsPaperBlackToZero() {
        var p = ToneControlsTests.params()
        let base = ToneControlsTests.rampEncoded(p)
        p.trueBlack = true
        let bpc = ToneControlsTests.rampEncoded(p)

        // Without BPC paper black floats well above display black; BPC pulls it
        // down hard. Exact 0 is unreachable at neutral toe — the softplus bound
        // approaches d_max only asymptotically (NegPy documents the same).
        // Assert in LINEAR (decoded) domain so the contract is OETF-independent
        // (the old encoded thresholds baked in the ROMM toe; the Adobe RGB pure
        // gamma maps small linears to much larger code values).
        let baseLin = Double(WorkingOETF.decode(base.last!))
        let bpcLin = Double(WorkingOETF.decode(bpc.last!))
        #expect(baseLin > 0.003, "without BPC paper black floats (linear \(baseLin))")
        #expect(bpcLin < 0.0015 && bpcLin < baseLin * 0.4,
            "BPC should pull toward display black (linear \(baseLin) → \(bpcLin))")
        // Highlights are essentially untouched (normalization by 1−black ≈ 1.005).
        expectClose(Double(bpc[0]), Double(base[0]), accuracy: 0.01, "highlight end")
        // Still monotone.
        for i in 1..<bpc.count {
            #expect(bpc[i] <= bpc[i - 1] + 1e-4, "non-monotone at \(i)")
        }
    }

    @Test func sidecarRoundTrip() throws {
        var s = ExposureSettings()
        s.trueBlack = false
        let back = try JSONDecoder().decode(ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back == s)
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.trueBlack == true)  // sidecars without the key adopt the new default
    }
}
