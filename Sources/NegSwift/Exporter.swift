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

/// User-facing export options (the quality modal). Defaults are deliberately
/// high quality; persisted as the sticky last-used configuration.
struct ExportOptions: Codable, Equatable {
    var format: ExportFormat = .jpeg
    var jpegQuality: Double = 0.92
    var resize: Bool = false
    var maxLongEdge: Int = 3000

    static func loadSticky() -> ExportOptions {
        guard let data = UserDefaults.standard.data(forKey: "exportOptions"),
            let options = try? JSONDecoder().decode(ExportOptions.self, from: data)
        else { return ExportOptions() }
        return options
    }

    func saveSticky() {
        if let data = try? JSONEncoder().encode(self) {
            UserDefaults.standard.set(data, forKey: "exportOptions")
        }
    }
}

extension ExportFormat: Codable {}

enum Exporter {
    /// Write the encoded (ROMM TRC) buffer with the ROMM RGB profile embedded.
    static func write(_ encoded: RGBImage, to url: URL, options: ExportOptions) throws {
        var image = encoded
        if options.resize, options.maxLongEdge >= 16 {
            image = image.downsampled(maxLongEdge: options.maxLongEdge)
        }
        let cg: CGImage?
        let type: UTType
        var destOptions: [CFString: Any] = [:]
        switch options.format {
        case .jpeg:
            cg = ImageConversion.cgImage(fromEncoded: image, bitsPerComponent: 8)
            type = .jpeg
            destOptions[kCGImageDestinationLossyCompressionQuality] = options.jpegQuality
        case .tiff16:
            cg = ImageConversion.cgImage(fromEncoded: image, bitsPerComponent: 16)
            type = .tiff
        }
        guard let cg,
            let dest = CGImageDestinationCreateWithURL(url as CFURL, type.identifier as CFString, 1, nil)
        else { throw ExportError.encodeFailed }
        CGImageDestinationAddImage(dest, cg, destOptions as CFDictionary)
        guard CGImageDestinationFinalize(dest) else { throw ExportError.encodeFailed }
    }
}
