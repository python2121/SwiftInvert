import CoreGraphics
import Foundation
import NegativeKit

/// CGImage construction and color-space conversion for the pipeline's
/// ROMM-encoded output. Conversion goes through a ColorSync-managed CGContext
/// draw — verified byte-identical to NegPy's littleCMS display transform
/// (relative colorimetric) on reference colors.
public enum ColorIO {
    public static func rommColorSpace() -> CGColorSpace? { CGColorSpace(name: CGColorSpace.rommrgb) }

    /// Encoded (ROMM TRC) float buffer → ROMM-tagged CGImage.
    public static func cgImage(fromEncoded img: RGBImage, bitsPerComponent: Int = 8) -> CGImage? {
        guard let cs = rommColorSpace() else { return nil }
        if bitsPerComponent == 16 {
            var u16 = [UInt16](repeating: 0, count: img.pixels.count)
            for i in 0..<img.pixels.count {
                u16[i] = UInt16(max(0, min(65535, img.pixels[i] * 65535 + 0.5)))
            }
            let data = u16.withUnsafeBufferPointer { Data(buffer: $0) }
            guard let provider = CGDataProvider(data: data as CFData) else { return nil }
            return CGImage(
                width: img.width, height: img.height, bitsPerComponent: 16, bitsPerPixel: 48,
                bytesPerRow: img.width * 6, space: cs,
                bitmapInfo: CGBitmapInfo(
                    rawValue: CGImageAlphaInfo.none.rawValue | CGBitmapInfo.byteOrder16Little.rawValue),
                provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
        }
        var u8 = [UInt8](repeating: 0, count: img.pixels.count)
        for i in 0..<img.pixels.count {
            u8[i] = UInt8(max(0, min(255, img.pixels[i] * 255 + 0.5)))
        }
        let data = u8.withUnsafeBufferPointer { Data(buffer: $0) }
        guard let provider = CGDataProvider(data: data as CFData) else { return nil }
        return CGImage(
            width: img.width, height: img.height, bitsPerComponent: 8, bitsPerPixel: 24,
            bytesPerRow: img.width * 3, space: cs,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.none.rawValue),
            provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }

    /// Color-managed conversion into another space (ColorSync does the math).
    public static func converted(
        _ image: CGImage, to space: CGColorSpace, bitsPerComponent: Int
    ) -> CGImage? {
        let bitmapInfo: UInt32 =
            bitsPerComponent == 16
            ? CGImageAlphaInfo.noneSkipLast.rawValue | CGBitmapInfo.byteOrder16Little.rawValue
            : CGImageAlphaInfo.noneSkipLast.rawValue
        guard
            let ctx = CGContext(
                data: nil, width: image.width, height: image.height,
                bitsPerComponent: bitsPerComponent, bytesPerRow: 0, space: space,
                bitmapInfo: bitmapInfo)
        else { return nil }
        ctx.interpolationQuality = .none
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        return ctx.makeImage()
    }
}
