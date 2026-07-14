import Foundation
import ImageIO
import MetalRenderKit
import NegativeKit
import RawDecodeKit
import UniformTypeIdentifiers

func usage() -> Never {
    print(
        """
        negcli — headless SwiftInvert pipeline driver

        Usage:
          negcli decode <raw-file> -o <out.tiff> [--preview] [--max-edge N]
              Linear sensor decode (16-bit TIFF dump, no conversion).
          negcli thumb <raw-file> -o <out.jpg>
              Extract the embedded camera JPEG thumbnail.
          negcli render <raw-file> -o <out.tiff> [--full] [--density D] [--grade G]
                        [--cyan C] [--magenta M] [--yellow Y] [--exposure STOPS]
                        [--shadows S] [--shadow-contrast S] [--highlights H]
                        [--highlight-contrast H]
              Full C-41 conversion (analysis + Metal render), 16-bit ROMM TIFF.
          negcli bench <raw-file> [--frames N]
              Slider-latency benchmark: decode+analyze once, re-render N times.
        """
    )
    exit(2)
}

func parseFlags(_ args: [String]) -> (positional: [String], options: [String: String], flags: Set<String>) {
    var positional: [String] = []
    var options: [String: String] = [:]
    var flags: Set<String> = []
    let booleanFlags: Set<String> = ["--preview", "--full"]
    var i = 0
    while i < args.count {
        let a = args[i]
        // A token is a value if it doesn't look like a flag — negative numbers
        // ("-1", "-0.5") count as values.
        func isValue(_ s: String) -> Bool { !s.hasPrefix("-") || Double(s) != nil }
        if a == "-o" || a.hasPrefix("--"), !booleanFlags.contains(a), i + 1 < args.count, isValue(args[i + 1]) {
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
        if let v = options["--exposure"].flatMap(Double.init) { settings.exposureStops = v }
        if let v = options["--shadows"].flatMap(Double.init) { settings.shadows = v }
        if let v = options["--shadow-contrast"].flatMap(Double.init) { settings.shadowContrast = v }
        if let v = options["--highlights"].flatMap(Double.init) { settings.highlights = v }
        if let v = options["--highlight-contrast"].flatMap(Double.init) { settings.highlightContrast = v }
        if let v = options["--pre-saturation"].flatMap(Double.init) { settings.preSaturation = v }
        if let v = options["--red-hue"].flatMap(Double.init) { settings.redHue = v }
        if let v = options["--red-saturation"].flatMap(Double.init) { settings.redSaturation = v }
        if let v = options["--yellow-hue"].flatMap(Double.init) { settings.yellowHue = v }
        if let v = options["--yellow-saturation"].flatMap(Double.init) { settings.yellowSaturation = v }
        if let v = options["--green-hue"].flatMap(Double.init) { settings.greenHue = v }
        if let v = options["--green-saturation"].flatMap(Double.init) { settings.greenSaturation = v }
        if let v = options["--blue-hue"].flatMap(Double.init) { settings.blueHue = v }
        if let v = options["--blue-saturation"].flatMap(Double.init) { settings.blueSaturation = v }

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
    case "bench":
        guard let input = positional.first else { usage() }
        let frames = options["--frames"].flatMap(Int.init) ?? 30
        let img = try RawDecoder().decode(url: URL(fileURLWithPath: input), quality: .preview, maxLongEdge: 1536)
        let tPrep = Date()
        let prep = ExposureKernel.prepare(linearImage: img)
        let prepMS = -tPrep.timeIntervalSinceNow * 1000
        let tFin = Date()
        for i in 0..<20 { _ = ExposureKernel.finalize(prep, blackPointOffset: Double(i) * 0.001) }
        print(String(
            format: "analysis: prepare %.1f ms, finalize (wp/bp tick) %.1f ms",
            prepMS, -tFin.timeIntervalSinceNow * 1000 / 20))
        let analysis = ExposureKernel.finalize(prep)
        let pipeline = try RenderPipeline()
        let source = try pipeline.upload(img)
        var settings = ExposureSettings()
        _ = try pipeline.render(source: source, params: ExposureKernel.deriveRenderParams(settings, analysis))  // warm-up
        let reupload = flags.contains("--reupload")  // simulate pre-cache behavior
        let start = Date()
        for i in 0..<frames {
            settings.density = 1.0 + Double(i % 10) * 0.05  // vary like a slider drag
            let params = ExposureKernel.deriveRenderParams(settings, analysis)
            let src = reupload ? try pipeline.upload(img) : source
            _ = try pipeline.renderDisplay(source: src, params: params)  // the app's interactive path
        }
        let total = -start.timeIntervalSinceNow
        print(String(
            format: "%d frames at %dx%d%@: %.1f ms/frame (%.1f fps), derive+render+readback",
            frames, img.width, img.height, reupload ? " (re-upload/frame)" : "",
            total / Double(frames) * 1000, Double(frames) / total))
    default:
        usage()
    }
} catch {
    FileHandle.standardError.write("error: \(error)\n".data(using: .utf8)!)
    exit(1)
}
