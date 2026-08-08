import Foundation
import NegativeKit

#if !canImport(ImageIO)
/// Minimal baseline-TIFF writer for platforms without ImageIO: one
/// uncompressed little-endian strip of 16-bit RGB. No ICC tag — the bytes
/// are whatever space the caller encoded (the pipeline's Adobe RGB (1998)),
/// so colour-managed viewers will assume sRGB until the lcms2-based export
/// path lands. Good enough for parity dumps and headless inspection.
enum TIFF16 {
    static func write(_ img: RGBImage, to url: URL) throws {
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
        // BitsPerSample's [16,16,16] doesn't fit in 4 bytes; it lives right
        // after the IFD terminator.
        struct Entry {
            var tag: UInt16
            var type: UInt16  // 3 = SHORT, 4 = LONG
            var count: UInt32
            var value: UInt32
        }
        let entries: [Entry] = [
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
        let bitsOffset = ifdOffset + 2 + UInt32(entries.count) * 12 + 4
        u16(UInt16(entries.count))
        for e in entries {
            u16(e.tag)
            u16(e.type)
            u32(e.count)
            u32(e.tag == 258 ? bitsOffset : e.value)
        }
        u32(0)  // no next IFD
        u16(16); u16(16); u16(16)  // BitsPerSample values

        try data.write(to: url)
    }
}
#endif
