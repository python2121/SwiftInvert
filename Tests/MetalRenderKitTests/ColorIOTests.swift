import CoreGraphics
import Foundation
import Testing

@testable import MetalRenderKit
@testable import NegativeKit

/// The export sRGB conversion, pinned against NegPy's littleCMS display
/// transform (AdobeCompat-v4 → sRGB-v4, relative colorimetric + BPC, since
/// the b3490eb working-space port) — oracle values regenerated 2026-07-20
/// with NegPy's own ICC profiles at 96adfde.
@Suite struct ColorIOTests {
    static let cases: [(working: [UInt8], srgb: [UInt8])] = [
        ([200, 60, 50], [231, 57, 46]),
        ([70, 190, 60], [0, 191, 40]),
        ([60, 80, 200], [46, 79, 205]),
        ([180, 140, 120], [195, 141, 120]),
        ([128, 128, 128], [129, 129, 129]),
        ([160, 170, 140], [157, 171, 140]),
    ]

    @Test func srgbConversionMatchesLittleCMS() throws {
        let n = Self.cases.count
        var img = RGBImage(width: n, height: 1)
        for (i, c) in Self.cases.enumerated() {
            for ch in 0..<3 { img[0, i, ch] = Float(c.working[ch]) / 255.0 }
        }
        let working16 = try #require(ColorIO.cgImage(fromEncoded: img, bitsPerComponent: 16))
        let srgbSpace = try #require(CGColorSpace(name: CGColorSpace.sRGB))
        let out = try #require(ColorIO.converted(working16, to: srgbSpace, bitsPerComponent: 8))

        let data = try #require(out.dataProvider?.data as Data?)
        let bpr = out.bytesPerRow
        let comps = out.bitsPerPixel / 8
        for (i, c) in Self.cases.enumerated() {
            for ch in 0..<3 {
                let got = Int(data[i * comps + ch])
                let want = Int(c.srgb[ch])
                #expect(abs(got - want) <= 1, "pixel \(i) ch\(ch): \(got) vs littleCMS \(want)")
            }
        }
        _ = bpr
    }

    /// The 8-bit branch (JPEG exports take it) was previously untested: pin
    /// the shape and that the bytes are the straight 0.5-rounded quantization.
    @Test func eightBitEncodedImage() throws {
        var img = RGBImage(width: 2, height: 1)
        for ch in 0..<3 { img[0, 0, ch] = 0.5 }
        for ch in 0..<3 { img[0, 1, ch] = 1.0 }
        let out = try #require(ColorIO.cgImage(fromEncoded: img, bitsPerComponent: 8))
        #expect(out.bitsPerComponent == 8 && out.bitsPerPixel == 24)
        #expect(out.colorSpace?.name == CGColorSpace.adobeRGB1998)
        let data = try #require(out.dataProvider?.data as Data?)
        #expect(data[0] == 128 && data[3] == 255)  // 0.5*255+0.5 → 128; 1.0 → 255
    }
}
