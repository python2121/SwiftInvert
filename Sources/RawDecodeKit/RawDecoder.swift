import CLibRaw
import Foundation
import NegativeKit

public enum RawDecodeError: Error, CustomStringConvertible {
    case libraw(String, code: Int32)
    case unsupportedOutput(String)

    public var description: String {
        switch self {
        case .libraw(let stage, let code):
            let msg = String(cString: libraw_strerror(code))
            return "LibRaw \(stage) failed: \(msg) (\(code))"
        case .unsupportedOutput(let why):
            return "Unsupported LibRaw output: \(why)"
        }
    }
}

/// LibRaw-backed linear sensor decode, mirroring NegPy's rawpy parameters
/// (negpy/services/rendering/preview_manager.py):
/// gamma=(1,1), no_auto_bright, output_bps=16, output_color=RAW (sensor-native,
/// no camera matrix), unity white balance (linear_raw path), user_flip=0 with the
/// EXIF rotation baked afterwards in code.
public struct RawDecoder {
    public enum Quality {
        /// half_size + linear demosaic — NegPy's fast preview path.
        /// X-Trans sensors skip half_size (it aliases the 6×6 CFA).
        case preview
        /// Full-resolution, LibRaw's default (highest quality) demosaic.
        case full
    }

    public init() {}

    /// Supported RAW file extensions (lowercase), from NegPy's loaders/constants.py
    /// minus TIFF/JPEG (NegSwift is camera-RAW only).
    public static let rawExtensions: Set<String> = [
        "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf",
        "rw2", "orf", "pef", "srw", "erf", "kdc", "dcr", "mos", "mrw", "raw",
        "rwl", "iiq", "3fr", "fff", "x3f", "mef",
    ]

    public static func isRawFile(_ url: URL) -> Bool {
        rawExtensions.contains(url.pathExtension.lowercased())
    }

    /// Decode to a scene-linear RGB float buffer with EXIF orientation baked in.
    public func decode(url: URL, quality: Quality, maxLongEdge: Int? = nil) throws -> RGBImage {
        guard let lr = libraw_init(0) else { throw RawDecodeError.libraw("init", code: -1) }
        defer { libraw_close(lr) }

        var rc = libraw_open_file(lr, url.path)
        guard rc == LIBRAW_SUCCESS.rawValue else { throw RawDecodeError.libraw("open", code: rc) }
        rc = libraw_unpack(lr)
        guard rc == LIBRAW_SUCCESS.rawValue else { throw RawDecodeError.libraw("unpack", code: rc) }

        let isXTrans = lr.pointee.idata.filters == 9

        // NegPy decode contract (linear_raw path).
        lr.pointee.params.gamm.0 = 1.0
        lr.pointee.params.gamm.1 = 1.0
        lr.pointee.params.no_auto_bright = 1
        lr.pointee.params.output_bps = 16
        lr.pointee.params.output_color = 0  // sensor-native, no color matrix
        lr.pointee.params.use_camera_wb = 0
        lr.pointee.params.use_auto_wb = 0
        lr.pointee.params.user_mul = (1, 1, 1, 1)  // unity WB
        lr.pointee.params.user_flip = 0

        switch quality {
        case .preview:
            if !isXTrans {
                lr.pointee.params.half_size = 1
                lr.pointee.params.user_qual = 0  // linear demosaic
            }
        case .full:
            break  // LibRaw default demosaic (AHD for Bayer, X-Trans-aware for Fuji)
        }

        rc = libraw_dcraw_process(lr)
        guard rc == LIBRAW_SUCCESS.rawValue else { throw RawDecodeError.libraw("process", code: rc) }

        var err: Int32 = 0
        guard let processed = libraw_dcraw_make_mem_image(lr, &err) else {
            throw RawDecodeError.libraw("make_mem_image", code: err)
        }
        defer { libraw_dcraw_clear_mem(processed) }

        let p = processed.pointee
        guard p.type == LIBRAW_IMAGE_BITMAP, p.bits == 16, p.colors == 3 else {
            throw RawDecodeError.unsupportedOutput("type=\(p.type) bits=\(p.bits) colors=\(p.colors)")
        }

        let w = Int(p.width), h = Int(p.height)
        let count = w * h * 3
        var pixels = [Float](repeating: 0, count: count)
        // `data` is a C flexible array member; address it via its offset from the
        // struct base (withUnsafePointer(to: .pointee.data) would copy to a
        // misaligned temporary and trap).
        let dataOffset = MemoryLayout<libraw_processed_image_t>.offset(of: \.data)!
        let raw = UnsafeRawPointer(processed) + dataOffset
        let inv: Float = 1.0 / 65535.0
        for i in 0..<count {
            pixels[i] = Float(raw.loadUnaligned(fromByteOffset: i * 2, as: UInt16.self)) * inv
        }

        var img = RGBImage(pixels: pixels, width: w, height: h)
        // With user_flip=0 the output is sensor-oriented; bake the camera's
        // recorded orientation ourselves (NegPy: apply_exif_orientation).
        img = img.applyingFlip(Int32(lr.pointee.sizes.flip))
        if let maxLongEdge { img = img.downsampled(maxLongEdge: maxLongEdge) }
        return img
    }

    /// Extract the embedded camera JPEG thumbnail (no negative conversion —
    /// matches NegPy's library-grid thumbs). Returns encoded JPEG bytes.
    public func embeddedThumbnail(url: URL) throws -> Data {
        guard let lr = libraw_init(0) else { throw RawDecodeError.libraw("init", code: -1) }
        defer { libraw_close(lr) }

        var rc = libraw_open_file(lr, url.path)
        guard rc == LIBRAW_SUCCESS.rawValue else { throw RawDecodeError.libraw("open", code: rc) }
        rc = libraw_unpack_thumb(lr)
        guard rc == LIBRAW_SUCCESS.rawValue else { throw RawDecodeError.libraw("unpack_thumb", code: rc) }

        var err: Int32 = 0
        guard let thumb = libraw_dcraw_make_mem_thumb(lr, &err) else {
            throw RawDecodeError.libraw("make_mem_thumb", code: err)
        }
        defer { libraw_dcraw_clear_mem(thumb) }

        let t = thumb.pointee
        guard t.type == LIBRAW_IMAGE_JPEG else {
            throw RawDecodeError.unsupportedOutput("thumbnail type=\(t.type), expected JPEG")
        }
        let dataOffset = MemoryLayout<libraw_processed_image_t>.offset(of: \.data)!
        return Data(bytes: UnsafeRawPointer(thumb) + dataOffset, count: Int(t.data_size))
    }
}
