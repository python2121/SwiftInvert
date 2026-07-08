import CoreGraphics
import Foundation
import NegativeKit

enum ImageConversion {
    /// Encoded (ROMM TRC) buffer → color-managed CGImage. Tagging rommrgb makes
    /// ColorSync handle the display transform (NegPy needed a littleCMS LUT here).
    static func cgImage(fromEncoded img: RGBImage, bitsPerComponent: Int = 8) -> CGImage? {
        guard let cs = CGColorSpace(name: CGColorSpace.rommrgb) else { return nil }
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
}
