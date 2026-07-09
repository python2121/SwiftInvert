import CoreGraphics
import Foundation
import ImageIO
import MetalRenderKit
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

enum ExportColorSpace: String, Codable, CaseIterable, Identifiable {
    case sRGB
    case rommRGB

    var id: String { rawValue }
    var label: String {
        switch self {
        case .sRGB: return "sRGB"
        case .rommRGB: return "ProPhoto (wide gamut)"
        }
    }
}

/// User-facing export options (the quality modal). Defaults are deliberately
/// high quality; persisted as the sticky last-used configuration.
struct ExportOptions: Codable, Equatable {
    var format: ExportFormat = .jpeg
    var jpegQuality: Double = 0.92
    var resize: Bool = false
    var maxLongEdge: Int = 3000
    /// sRGB default (NegPy's default): correct everywhere, including viewers
    /// that mishandle wide-gamut profiles.
    var colorSpace: ExportColorSpace = .sRGB
    /// Destination: next to each source (default) or a chosen folder.
    var useCustomDestination: Bool = false
    var customDestinationPath: String?

    /// Output URL for one source file (extension per format; overwrites a
    /// previous export of the same image, matching next-to-source behavior).
    func destinationURL(for source: URL) -> URL {
        let name = source.deletingPathExtension().lastPathComponent
        let dir: URL
        if useCustomDestination, let path = customDestinationPath {
            dir = URL(fileURLWithPath: path, isDirectory: true)
        } else {
            dir = source.deletingLastPathComponent()
        }
        return dir.appendingPathComponent(name).appendingPathExtension(format.fileExtension)
    }

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
        let outputBits = options.format == .jpeg ? 8 : 16
        var cg: CGImage?
        switch options.colorSpace {
        case .sRGB:
            // Build 16-bit ROMM first so the ColorSync conversion quantizes once.
            if let romm16 = ColorIO.cgImage(fromEncoded: image, bitsPerComponent: 16),
                let srgb = CGColorSpace(name: CGColorSpace.sRGB)
            {
                cg = ColorIO.converted(romm16, to: srgb, bitsPerComponent: outputBits)
            }
        case .rommRGB:
            cg = ColorIO.cgImage(fromEncoded: image, bitsPerComponent: outputBits)
        }
        let type: UTType
        var destOptions: [CFString: Any] = [:]
        switch options.format {
        case .jpeg:
            type = .jpeg
            destOptions[kCGImageDestinationLossyCompressionQuality] = options.jpegQuality
        case .tiff16:
            type = .tiff
        }
        guard let cg,
            let dest = CGImageDestinationCreateWithURL(url as CFURL, type.identifier as CFString, 1, nil)
        else { throw ExportError.encodeFailed }
        CGImageDestinationAddImage(dest, cg, destOptions as CFDictionary)
        guard CGImageDestinationFinalize(dest) else { throw ExportError.encodeFailed }
    }
}
