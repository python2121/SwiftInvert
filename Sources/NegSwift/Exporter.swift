import CoreGraphics
import Foundation
import ImageIO
import NegativeKit
import UniformTypeIdentifiers

enum ExportFormat: String, CaseIterable, Identifiable {
    case jpeg
    case tiff16

    var id: String { rawValue }
    var label: String {
        switch self {
        case .jpeg: return "JPEG"
        case .tiff16: return "TIFF (16-bit)"
        }
    }
    var fileExtension: String {
        switch self {
        case .jpeg: return "jpg"
        case .tiff16: return "tiff"
        }
    }
}

enum ExportError: Error { case encodeFailed }

enum Exporter {
    /// Write the encoded (ROMM TRC) buffer with the ROMM RGB profile embedded.
    static func write(_ encoded: RGBImage, to url: URL, format: ExportFormat) throws {
        let cg: CGImage?
        let type: UTType
        var options: [CFString: Any] = [:]
        switch format {
        case .jpeg:
            cg = ImageConversion.cgImage(fromEncoded: encoded, bitsPerComponent: 8)
            type = .jpeg
            options[kCGImageDestinationLossyCompressionQuality] = 0.92
        case .tiff16:
            cg = ImageConversion.cgImage(fromEncoded: encoded, bitsPerComponent: 16)
            type = .tiff
        }
        guard let cg,
            let dest = CGImageDestinationCreateWithURL(url as CFURL, type.identifier as CFString, 1, nil)
        else { throw ExportError.encodeFailed }
        CGImageDestinationAddImage(dest, cg, options as CFDictionary)
        guard CGImageDestinationFinalize(dest) else { throw ExportError.encodeFailed }
    }
}
