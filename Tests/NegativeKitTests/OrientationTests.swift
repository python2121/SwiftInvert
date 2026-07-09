import Foundation
import Testing

@testable import NegativeKit

@Suite struct OrientationTests {
    /// 2×3 test image with unique per-pixel values.
    static func probe() -> RGBImage {
        var img = RGBImage(width: 3, height: 2)
        for y in 0..<2 {
            for x in 0..<3 {
                for c in 0..<3 { img[y, x, c] = Float(y * 100 + x * 10 + c) }
            }
        }
        return img
    }

    @Test func rotate90CW() {
        let out = Self.probe().oriented(rotationCW: 90, flipHorizontal: false)
        #expect(out.width == 2 && out.height == 3)
        // Top-left of a 90° CW rotation is the source's bottom-left.
        #expect(out[0, 0, 0] == Self.probe()[1, 0, 0])
        #expect(out[0, 1, 0] == Self.probe()[0, 0, 0])
        #expect(out[2, 0, 0] == Self.probe()[1, 2, 0])
    }

    @Test func rotate270IsInverseOf90() {
        let img = Self.probe()
        let roundTrip = img.oriented(rotationCW: 90, flipHorizontal: false)
            .oriented(rotationCW: 270, flipHorizontal: false)
        #expect(roundTrip.pixels == img.pixels)
    }

    @Test func rotate180() {
        let img = Self.probe()
        let out = img.oriented(rotationCW: 180, flipHorizontal: false)
        #expect(out.width == 3 && out.height == 2)
        #expect(out[0, 0, 1] == img[1, 2, 1])
    }

    @Test func flipHorizontal() {
        let img = Self.probe()
        let out = img.oriented(rotationCW: 0, flipHorizontal: true)
        #expect(out[0, 0, 0] == img[0, 2, 0])
        #expect(out[1, 2, 2] == img[1, 0, 2])
        // Involution.
        #expect(out.flippedHorizontally().pixels == img.pixels)
    }

    @Test func negativeAndWrappedRotations() {
        let img = Self.probe()
        #expect(img.oriented(rotationCW: -90, flipHorizontal: false).pixels
            == img.oriented(rotationCW: 270, flipHorizontal: false).pixels)
        #expect(img.oriented(rotationCW: 360, flipHorizontal: false).pixels == img.pixels)
    }

    @Test func sidecarRoundTripsOrientation() throws {
        var s = ExposureSettings()
        s.rotation = 270
        s.flipHorizontal = true
        let back = try JSONDecoder().decode(ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back == s)
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.rotation == 0 && legacy.flipHorizontal == false)
    }
}
