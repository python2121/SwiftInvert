import Foundation
import NegativeKit

#if !canImport(ImageIO)
/// Minimal baseline-TIFF writer for platforms without ImageIO: one
/// uncompressed little-endian strip of 16-bit RGB. `icc` embeds a profile
/// (tag 34675) naming the space the caller encoded — export callers pass
/// `ICCProfiles.sRGB`/`.adobeRGB1998` to match the in-kernel encode, so a
/// colour-managed viewer reads the file correctly even though the
/// lcms2-based CONVERSION path hasn't landed (tagging is separable from
/// converting; an untagged wide-gamut file silently reads as sRGB).
/// `icc: nil` stays untagged (debug dumps of linear sensor data, which have
/// no real colorimetry to declare).
enum TIFF16 {
    static func write(_ img: RGBImage, to url: URL, icc: [UInt8]? = nil) throws {
        let count = img.width * img.height * 3
        var data = Data(capacity: 8 + count * 2 + 512)

        func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) } }
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) } }

        // Header: "II" (little-endian), magic 42, offset of the first IFD —
        // we put pixel data first, IFD after it.
        let headerSize = 8
        let pixelBytes = count * 2
        let ifdOffset = UInt32(headerSize + pixelBytes)
        data.append(contentsOf: [0x49, 0x49])  // II
        u16(42)
        u32(ifdOffset)

        img.pixels.withUnsafeBufferPointer { buf in
            for i in 0..<count {
                let v = UInt16(max(0, min(65535, buf[i] * 65535 + 0.5)))
                withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) }
            }
        }

        // IFD: entry count, then 12-byte entries sorted by tag, then next-IFD 0.
        // Out-of-line values (BitsPerSample's [16,16,16], the ICC blob) live
        // right after the IFD terminator, in that order.
        struct Entry {
            var tag: UInt16
            var type: UInt16  // 3 = SHORT, 4 = LONG, 7 = UNDEFINED
            var count: UInt32
            var value: UInt32
        }
        var entries: [Entry] = [
            Entry(tag: 256, type: 4, count: 1, value: UInt32(img.width)),   // ImageWidth
            Entry(tag: 257, type: 4, count: 1, value: UInt32(img.height)),  // ImageLength
            Entry(tag: 258, type: 3, count: 3, value: 0),                   // BitsPerSample → patched below
            Entry(tag: 259, type: 3, count: 1, value: 1),                   // Compression: none
            Entry(tag: 262, type: 3, count: 1, value: 2),                   // Photometric: RGB
            Entry(tag: 273, type: 4, count: 1, value: UInt32(headerSize)),  // StripOffsets
            Entry(tag: 277, type: 3, count: 1, value: 3),                   // SamplesPerPixel
            Entry(tag: 278, type: 4, count: 1, value: UInt32(img.height)),  // RowsPerStrip
            Entry(tag: 279, type: 4, count: 1, value: UInt32(pixelBytes)),  // StripByteCounts
            Entry(tag: 284, type: 3, count: 1, value: 1),                   // PlanarConfig: chunky
        ]
        if let icc {
            entries.append(Entry(tag: 34675, type: 7, count: UInt32(icc.count), value: 0))  // ICC Profile → patched below
        }
        let bitsOffset = ifdOffset + 2 + UInt32(entries.count) * 12 + 4
        let iccOffset = bitsOffset + 6  // after the three BitsPerSample SHORTs (stays word-aligned)
        u16(UInt16(entries.count))
        for e in entries {
            u16(e.tag)
            u16(e.type)
            u32(e.count)
            u32(e.tag == 258 ? bitsOffset : (e.tag == 34675 ? iccOffset : e.value))
        }
        u32(0)  // no next IFD
        u16(16); u16(16); u16(16)  // BitsPerSample values
        if let icc { data.append(contentsOf: icc) }

        try data.write(to: url)
    }
}
#endif
