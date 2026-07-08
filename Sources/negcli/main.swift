import Foundation
import ImageIO
import MetalRenderKit
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
          negcli render <raw-file> -o <out.tiff> [--full] [--density D] [--grade G]
                        [--cyan C] [--magenta M] [--yellow Y]
              Full C-41 conversion (analysis + Metal render), 16-bit ROMM TIFF.
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

/// Write an RGBImage as a 16-bit TIFF. `romm: true` tags ROMM RGB (ProPhoto,
/// gamma 1.8 — exactly the pipeline's encoded output space); false tags linear
/// sRGB (debug dumps of linear sensor data).
func writeTIFF16(_ img: RGBImage, to url: URL, romm: Bool = false) throws {
    let count = img.width * img.height * 3
    var u16 = [UInt16](repeating: 0, count: count)
    for i in 0..<count { u16[i] = UInt16(max(0, min(65535, img.pixels[i] * 65535 + 0.5))) }
    let data = u16.withUnsafeBufferPointer { Data(buffer: $0) }
    guard let provider = CGDataProvider(data: data as CFData),
        let cs = CGColorSpace(name: romm ? CGColorSpace.rommrgb : CGColorSpace.linearSRGB),
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
    case "render":
        guard let input = positional.first, let output = options["-o"] else { usage() }
        let full = flags.contains("--full")
        var settings = ExposureSettings()
        if let v = options["--density"].flatMap(Double.init) { settings.density = v }
        if let v = options["--grade"].flatMap(Double.init) { settings.grade = v }
        if let v = options["--cyan"].flatMap(Double.init) { settings.wbCyan = v }
        if let v = options["--magenta"].flatMap(Double.init) { settings.wbMagenta = v }
        if let v = options["--yellow"].flatMap(Double.init) { settings.wbYellow = v }

        let start = Date()
        let img = try RawDecoder().decode(
            url: URL(fileURLWithPath: input), quality: full ? .full : .preview,
            maxLongEdge: full ? nil : 1536)
        let tDecode = -start.timeIntervalSinceNow
        // Analysis on a working-size copy (NegPy meters at preview resolution).
        let analysisImage = full ? img.downsampled(maxLongEdge: 1536) : img
        let analysis = ExposureKernel.analyze(linearImage: analysisImage)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let tAnalyze = -start.timeIntervalSinceNow - tDecode
        let pipeline = try RenderPipeline()
        let (encoded, _) = try pipeline.render(image: img, params: params)
        let tRender = -start.timeIntervalSinceNow - tDecode - tAnalyze
        try writeTIFF16(encoded, to: URL(fileURLWithPath: output), romm: true)
        print(
            "rendered \(encoded.width)x\(encoded.height)  decode \(String(format: "%.2f", tDecode))s"
                + "  analyze \(String(format: "%.2f", tAnalyze))s  render \(String(format: "%.2f", tRender))s")
        print(
            "bounds floors \(params.finalBounds.floors)  anchor \(String(format: "%.3f", analysis.anchor))"
                + "  cast confidence \(analysis.neutralConfidence.map { String(format: "%.2f", $0) } ?? "n/a")")
    default:
        usage()
    }
} catch {
    FileHandle.standardError.write("error: \(error)\n".data(using: .utf8)!)
    exit(1)
}
