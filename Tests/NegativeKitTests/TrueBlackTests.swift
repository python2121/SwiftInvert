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
        #expect(base.last! > 0.04, "without BPC paper black floats (got \(base.last!))")
        #expect(bpc.last! < 0.02 && bpc.last! < base.last! * 0.4,
            "BPC should pull toward display black (base \(base.last!) → \(bpc.last!))")
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
