import Foundation
import ImageIO
import NegativeKit
import RawDecodeKit
import UniformTypeIdentifiers

func usage() -> Never {
    print(
        """
        negcli — headless NegSwift pipeline driver

        Usage:
          negcli decode <raw-file> -o <out.tiff> [--preview] [--max-edge N]
              Linear sensor decode (16-bit TIFF dump, no conversion).
          negcli thumb <raw-file> -o <out.jpg>
              Extract the embedded camera JPEG thumbnail.
        """
    )
    exit(2)
}

func parseFlags(_ args: [String]) -> (positional: [String], options: [String: String], flags: Set<String>) {
    var positional: [String] = []
    var options: [String: String] = [:]
    var flags: Set<String> = []
    var i = 0
    while i < args.count {
        let a = args[i]
        if a == "-o" || a.hasPrefix("--"), i + 1 < args.count, !args[i + 1].hasPrefix("-") {
            if a == "--preview" { flags.insert(a); i += 1; continue }
            options[a] = args[i + 1]
            i += 2
        } else if a.hasPrefix("-") {
            flags.insert(a)
            i += 1
        } else {
            positional.append(a)
            i += 1
        }
    }
    return (positional, options, flags)
}

/// Write an RGBImage as a 16-bit TIFF (debug dump; tagged linear sRGB so viewers
/// don't gamma-lift the linear data).
func writeTIFF16(_ img: RGBImage, to url: URL) throws {
    let count = img.width * img.height * 3
    var u16 = [UInt16](repeating: 0, count: count)
    for i in 0..<count { u16[i] = UInt16(max(0, min(65535, img.pixels[i] * 65535 + 0.5))) }
    let data = u16.withUnsafeBufferPointer { Data(buffer: $0) }
    guard let provider = CGDataProvider(data: data as CFData),
        let cs = CGColorSpace(name: CGColorSpace.linearSRGB),
        let cg = CGImage(
            width: img.width, height: img.height, bitsPerComponent: 16, bitsPerPixel: 48,
            bytesPerRow: img.width * 6, space: cs,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.none.rawValue | CGBitmapInfo.byteOrder16Little.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent
        ),
        let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.tiff.identifier as CFString, 1, nil)
    else {
        throw NSError(domain: "negcli", code: 1, userInfo: [NSLocalizedDescriptionKey: "TIFF encode failed"])
    }
    CGImageDestinationAddImage(dest, cg, nil)
    guard CGImageDestinationFinalize(dest) else {
        throw NSError(domain: "negcli", code: 2, userInfo: [NSLocalizedDescriptionKey: "TIFF write failed"])
    }
}

let args = Array(CommandLine.arguments.dropFirst())
guard let command = args.first else { usage() }
let (positional, options, flags) = parseFlags(Array(args.dropFirst()))

do {
    switch command {
    case "decode":
        guard let input = positional.first, let output = options["-o"] else { usage() }
        let quality: RawDecoder.Quality = flags.contains("--preview") ? .preview : .full
        let maxEdge = options["--max-edge"].flatMap { Int($0) }
        let start = Date()
        let img = try RawDecoder().decode(url: URL(fileURLWithPath: input), quality: quality, maxLongEdge: maxEdge)
        var sums = (0.0, 0.0, 0.0)
        img.pixels.withUnsafeBufferPointer { buf in
            var i = 0
            while i < buf.count {
                sums.0 += Double(buf[i]); sums.1 += Double(buf[i + 1]); sums.2 += Double(buf[i + 2])
                i += 3
            }
        }
        let n = Double(img.width * img.height)
        let mean = [sums.0 / n, sums.1 / n, sums.2 / n]
        try writeTIFF16(img, to: URL(fileURLWithPath: output))
        print("decoded \(img.width)x\(img.height) in \(String(format: "%.2f", -start.timeIntervalSinceNow))s")
        print("channel means: R=\(String(format: "%.5f", mean[0])) G=\(String(format: "%.5f", mean[1])) B=\(String(format: "%.5f", mean[2]))")
    case "thumb":
        guard let input = positional.first, let output = options["-o"] else { usage() }
        let data = try RawDecoder().embeddedThumbnail(url: URL(fileURLWithPath: input))
        try data.write(to: URL(fileURLWithPath: output))
        print("wrote \(data.count) bytes")
    default:
        usage()
    }
} catch {
    FileHandle.standardError.write("error: \(error)\n".data(using: .utf8)!)
    exit(1)
}
