import CoreGraphics
import Foundation
import Testing

@testable import NegativeKit
@testable import SwiftInvert

/// DensitometerState's probe over a synthetic bitmap: coordinate mapping,
/// range clamping, and the adopt/clear lifecycle. The hover *gesture* can't be
/// tested headlessly (see UPSTREAM.md 2026-07-15) — this pins everything
/// beneath it, so a regression there can only be in the SwiftUI wiring.
@MainActor @Suite struct DensitometerStateTests {

    /// 2×2 quadrants: red, green / blue, white — via the same rgba8 path the
    /// render output takes, so the test exercises the real byte layout.
    private func makeQuadrants() throws -> CGImage {
        let rgba8: [UInt8] = [
            255, 0, 0, 255,   0, 255, 0, 255,
            0, 0, 255, 255,   255, 255, 255, 255,
        ]
        return try #require(ImageConversion.cgImage(rgba8: rgba8, width: 2, height: 2))
    }

    @Test func probeMapsQuadrantsCorrectly() throws {
        let state = DensitometerState()
        state.adopt(try makeQuadrants())
        state.probe(u: 0.1, v: 0.1)
        #expect(state.reading?.rgb == SIMD3(1, 0, 0))
        state.probe(u: 0.9, v: 0.1)
        #expect(state.reading?.rgb == SIMD3(0, 1, 0))
        state.probe(u: 0.1, v: 0.9)
        #expect(state.reading?.rgb == SIMD3(0, 0, 1))
        state.probe(u: 0.9, v: 0.9)
        #expect(state.reading?.rgb == SIMD3(1, 1, 1))
    }

    /// u = v = 1.0 is a valid hover position (the far edge) and must clamp to
    /// the last pixel, not index past the buffer.
    @Test func farEdgeClampsToLastPixel() throws {
        let state = DensitometerState()
        state.adopt(try makeQuadrants())
        state.probe(u: 1.0, v: 1.0)
        #expect(state.reading?.rgb == SIMD3(1, 1, 1))
        state.probe(u: 0, v: 0)
        #expect(state.reading?.rgb == SIMD3(1, 0, 0))
    }

    @Test func outOfRangeProbeClearsTheReading() throws {
        let state = DensitometerState()
        state.adopt(try makeQuadrants())
        state.probe(u: 0.5, v: 0.5)
        #expect(state.reading != nil)
        state.probe(u: 1.2, v: 0.5)
        #expect(state.reading == nil)
        state.probe(u: 0.5, v: -0.1)
        #expect(state.reading == nil)
    }

    @Test func readingMatchesDensitometryOnTheSameTriplet() throws {
        let state = DensitometerState()
        state.adopt(try makeQuadrants())
        state.probe(u: 0.9, v: 0.9)
        let reading = try #require(state.reading)
        #expect(reading == Densitometry.read(encodedRGB: SIMD3(1, 1, 1)))
        // Paper white: density ~0, zone X.
        #expect(reading.printDensity < 0.01)
        #expect(reading.zone > 9.99)
    }

    @Test func adoptNilAndClearBothDropTheReading() throws {
        let state = DensitometerState()
        state.adopt(try makeQuadrants())
        state.probe(u: 0.5, v: 0.5)
        #expect(state.reading != nil)
        state.clear()
        #expect(state.reading == nil)

        state.probe(u: 0.5, v: 0.5)
        #expect(state.reading != nil)
        state.adopt(nil)  // image switched away
        #expect(state.reading == nil)
        state.probe(u: 0.5, v: 0.5)
        #expect(state.reading == nil)  // no bitmap → no reading
    }

    /// A new render replaces the bytes the probe reads.
    @Test func adoptSwapsTheBitmap() throws {
        let state = DensitometerState()
        state.adopt(try makeQuadrants())
        state.probe(u: 0.1, v: 0.1)
        #expect(state.reading?.rgb == SIMD3(1, 0, 0))
        let gray: [UInt8] = [128, 128, 128, 255]
        state.adopt(ImageConversion.cgImage(rgba8: gray, width: 1, height: 1))
        #expect(state.reading == nil)  // stale reading dropped on adopt
        state.probe(u: 0.1, v: 0.1)
        #expect(state.reading?.rgb == SIMD3(repeating: 128.0 / 255.0))
    }
}

/// ImageConversion — the CGImage factories the display path and the probe
/// both depend on.
@Suite struct ImageConversionTests {

    @Test func rgba8ImageShape() throws {
        let bytes = [UInt8](repeating: 100, count: 3 * 2 * 4)
        let image = try #require(ImageConversion.cgImage(rgba8: bytes, width: 3, height: 2))
        #expect(image.width == 3 && image.height == 2)
        #expect(image.bitsPerComponent == 8 && image.bitsPerPixel == 32)
        #expect(image.bytesPerRow == 12)
        #expect(image.colorSpace?.name == CGColorSpace.rommrgb)
    }

    @Test func encodedImageShapes() throws {
        var img = RGBImage(width: 4, height: 3)
        for i in 0..<img.pixels.count { img.pixels[i] = 0.5 }
        let eight = try #require(ImageConversion.cgImage(fromEncoded: img))
        #expect(eight.width == 4 && eight.height == 3 && eight.bitsPerComponent == 8)
        let sixteen = try #require(ImageConversion.cgImage(fromEncoded: img, bitsPerComponent: 16))
        #expect(sixteen.bitsPerComponent == 16 && sixteen.bitsPerPixel == 48)
        #expect(sixteen.colorSpace?.name == CGColorSpace.rommrgb)
    }
}
