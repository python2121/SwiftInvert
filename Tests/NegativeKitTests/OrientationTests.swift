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

    @Test func fineRotationZeroIsIdentity() {
        let img = SyntheticGrid.input
        let out = img.oriented(rotationCW: 0, flipHorizontal: false, fineRotation: 0)
        #expect(out.pixels == img.pixels)
    }

    @Test func fineRotationCropsToInscribedRect() {
        let img = SyntheticGrid.input  // 1600×1066
        let degrees = 3.5
        let out = img.fineRotated(degrees: degrees)
        let expected = RGBImage.inscribedRectSize(
            width: Double(img.width), height: Double(img.height),
            radians: degrees * .pi / 180)
        #expect(abs(Double(out.width) - expected.width) <= 1.0, "width \(out.width) vs \(expected.width)")
        #expect(abs(Double(out.height) - expected.height) <= 1.0, "height \(out.height) vs \(expected.height)")
        // Smaller than the source in both dimensions (corners cropped away).
        #expect(out.width < img.width && out.height < img.height)
    }

    @Test func fineRotationOfUniformImageStaysUniform() {
        var img = RGBImage(width: 300, height: 200)
        for i in 0..<img.pixels.count { img.pixels[i] = 0.42 }
        let out = img.fineRotated(degrees: -7.3)
        // Bilinear resampling of a constant field is exact; no empty corners
        // may leak in (they'd show as 0).
        for v in out.pixels {
            #expect(abs(v - 0.42) < 1e-5)
        }
    }

    @Test func sidecarRoundTripsOrientation() throws {
        var s = ExposureSettings()
        s.rotation = 270
        s.flipHorizontal = true
        s.fineRotation = -2.4
        let back = try JSONDecoder().decode(ExposureSettings.self, from: JSONEncoder().encode(s))
        #expect(back == s)
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.rotation == 0 && legacy.flipHorizontal == false && legacy.fineRotation == 0)
    }
}
