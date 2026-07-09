import CoreGraphics
import Foundation
import Testing

@testable import MetalRenderKit
@testable import NegativeKit

/// The export sRGB conversion, pinned against NegPy's littleCMS display
/// transform (ProPhoto-v4 → sRGB-v4, relative colorimetric + BPC) — oracle
/// values generated with NegPy's own ICC profiles.
@Suite struct ColorIOTests {
    static let cases: [(romm: [UInt8], srgb: [UInt8])] = [
        ([200, 60, 50], [255, 0, 60]),
        ([70, 190, 60], [0, 218, 0]),
        ([60, 80, 200], [0, 102, 222]),
        ([180, 140, 120], [226, 148, 135]),
        ([128, 128, 128], [146, 146, 146]),
        ([160, 170, 140], [174, 186, 153]),
    ]

    @Test func srgbConversionMatchesLittleCMS() throws {
        let n = Self.cases.count
        var img = RGBImage(width: n, height: 1)
        for (i, c) in Self.cases.enumerated() {
            for ch in 0..<3 { img[0, i, ch] = Float(c.romm[ch]) / 255.0 }
        }
        let romm16 = try #require(ColorIO.cgImage(fromEncoded: img, bitsPerComponent: 16))
        let srgbSpace = try #require(CGColorSpace(name: CGColorSpace.sRGB))
        let out = try #require(ColorIO.converted(romm16, to: srgbSpace, bitsPerComponent: 8))

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
}
