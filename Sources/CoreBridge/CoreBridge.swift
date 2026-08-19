import Foundation
import NegativeKit
import RawDecodeKit
import VulkanRenderKit

// The C ABI consumed by the Qt frontend (qt/swiftinvert_core.h mirrors these
// signatures — keep both in sync). Design rules:
//  - settings travel as sidecar-format JSON (ExposureSettings' own codec, so
//    missing keys mean defaults and the Qt side never re-implements fields),
//  - every returned buffer is malloc'd and owned by the caller (si_free),
//  - sessions are opaque int64 handles; all functions are thread-safe (the
//    registry has its own lock, the Vulkan pipeline serializes internally).
//  - failures return 0/NULL and leave a message for si_last_error.

/// Per-image cache tower, a miniature of the Mac's ImageSession with the
/// same semantics: user orientation is baked into pixels so every
/// coordinate space agrees; analysis runs on the ORIENTATION-ONLY image
/// (fine rotation must not re-meter — the straighten invariant) scoped by
/// crop/analysis rects; the render source bakes fine rotation (inscribed)
/// and THEN the crop, so cropRect is normalized on the fine-rotated frame.
/// Every tier is keyed and rebuilt only when its inputs change — slider
/// drags hit only derive+render.
private final class Session {
    /// Display tiers, mirroring the Mac's RenderTier scheme. Analysis ALWAYS
    /// runs on the proxy, so switching tiers can never move the conversion.
    enum Tier: Int32 {
        case proxy = 0   // ≤1536px — the interactive default
        case medium = 1  // the native preview decode the proxy was cut from
        case full = 2    // full-resolution best demosaic, decoded on demand
    }

    let url: URL
    let proxy: RGBImage  // ≤1536, EXIF orientation baked; the analysis input
    /// The buffer the preview decode already produced and used to discard —
    /// 4× the proxy's pixels for free. Not retained when the preview decode
    /// was already full-size (X-Trans) or huge (the Mac's 20 MP budget).
    let medium: RGBImage?
    var full: RGBImage?  // lazy; retained for the session's lifetime

    struct OrientKey: Equatable {
        var rotation: Int
        var flip: Bool
    }
    struct AnalysisKey: Equatable {
        var orient: OrientKey
        var cropRect: NormalizedRect?
        var analysisRect: NormalizedRect?
    }
    struct SourceKey: Equatable {
        var orient: OrientKey
        var fineRotation: Double
        var cropRect: NormalizedRect?
    }

    var meterKey: OrientKey?
    var meterPreview: RGBImage?
    var analysisKey: AnalysisKey?
    var cachedAnalysis: ExposureAnalysis?
    /// Per-tier caches: the oriented bake (expensive at full res) and the
    /// uploaded GPU source, each keyed by the geometry that produced them.
    var orientedCache: [Int32: (key: SourceKey, image: RGBImage)] = [:]
    var sourceCache: [Int32: (key: SourceKey, buffer: VulkanRenderPipeline.SourceBuffer)] = [:]

    static let mediumTierPixelBudget = 20_000_000

    init(url: URL, previewDecode: RGBImage) {
        self.url = url
        let long = max(previewDecode.width, previewDecode.height)
        if long > 1536 {
            proxy = previewDecode.downsampled(maxLongEdge: 1536)
            medium = previewDecode.width * previewDecode.height <= Self.mediumTierPixelBudget
                ? previewDecode : nil
        } else {
            proxy = previewDecode
            medium = nil
        }
    }

    /// Orientation-only proxy (the analysis input).
    func meter(settings: ExposureSettings) -> RGBImage {
        let key = OrientKey(rotation: settings.rotation, flip: settings.flipHorizontal)
        if let meterPreview, meterKey == key { return meterPreview }
        let img = (key.rotation == 0 && !key.flip)
            ? proxy
            : proxy.oriented(rotationCW: key.rotation, flipHorizontal: key.flip)
        meterPreview = img
        meterKey = key
        return img
    }

    func analysis(settings: ExposureSettings) -> ExposureAnalysis {
        let key = AnalysisKey(
            orient: OrientKey(rotation: settings.rotation, flip: settings.flipHorizontal),
            cropRect: settings.cropRect, analysisRect: settings.analysisRect)
        if let cachedAnalysis, analysisKey == key { return cachedAnalysis }
        let result = ExposureKernel.analyze(
            linearImage: meter(settings: settings),
            cropRect: settings.cropRect, analysisRect: settings.analysisRect)
        cachedAnalysis = result
        analysisKey = key
        return result
    }

    /// The tier's base pixels; requesting an unavailable tier falls back
    /// (medium → full-decode would defeat "instant", so medium falls back to
    /// PROXY when not retained; full decodes lazily — the ~3–5 s export
    /// decode, paid once per session).
    private func tierImage(_ tier: Tier) throws -> RGBImage {
        switch tier {
        case .proxy:
            return proxy
        case .medium:
            return medium ?? proxy
        case .full:
            if let full { return full }
            let img = try RawDecoder().decode(url: url, quality: .full, maxLongEdge: nil)
            full = img
            return img
        }
    }

    /// GPU source for a tier: oriented (+fine rotation, inscribed) then
    /// cropped, both cached per tier so threshold crossings re-upload only
    /// when geometry actually changed.
    func source(
        settings: ExposureSettings, tier: Tier, pipeline: VulkanRenderPipeline
    ) throws -> VulkanRenderPipeline.SourceBuffer {
        let key = SourceKey(
            orient: OrientKey(rotation: settings.rotation, flip: settings.flipHorizontal),
            fineRotation: settings.fineRotation,
            cropRect: settings.cropRect)
        if let cached = sourceCache[tier.rawValue], cached.key == key { return cached.buffer }

        let orientKey = SourceKey(orient: key.orient, fineRotation: key.fineRotation, cropRect: nil)
        var image: RGBImage
        if let cached = orientedCache[tier.rawValue], cached.key == orientKey {
            image = cached.image
        } else {
            let base = try tierImage(tier)
            image = (key.orient.rotation == 0 && !key.orient.flip && key.fineRotation == 0)
                ? base
                : base.oriented(
                    rotationCW: key.orient.rotation, flipHorizontal: key.orient.flip,
                    fineRotation: key.fineRotation)
            orientedCache[tier.rawValue] = (orientKey, image)
        }
        if let crop = key.cropRect {
            image = image.cropped(to: crop)
        }
        let buffer = try pipeline.upload(image)
        sourceCache[tier.rawValue] = (key, buffer)
        return buffer
    }
}

private final class Bridge: @unchecked Sendable {
    static let shared = Bridge()
    let lock = NSLock()
    var sessions: [Int64: Session] = [:]
    var nextHandle: Int64 = 1
    var pipeline: VulkanRenderPipeline?
    var lastError = ""

    func setError(_ message: String) {
        lock.lock()
        lastError = message
        lock.unlock()
    }

    func getPipeline() throws -> VulkanRenderPipeline {
        lock.lock()
        defer { lock.unlock() }
        if let pipeline { return pipeline }
        let p = try VulkanRenderPipeline()
        pipeline = p
        return p
    }
}

private func mallocCopy(_ bytes: [UInt8]) -> UnsafeMutablePointer<UInt8> {
    let ptr = malloc(bytes.count)!.assumingMemoryBound(to: UInt8.self)
    bytes.withUnsafeBufferPointer { ptr.update(from: $0.baseAddress!, count: $0.count) }
    return ptr
}

private func mallocCString(_ s: String) -> UnsafeMutablePointer<CChar> {
    strdup(s)!
}

/// Message describing the most recent failure on any thread (best-effort,
/// process-global). Caller frees.
@_cdecl("si_last_error")
public func si_last_error() -> UnsafeMutablePointer<CChar> {
    Bridge.shared.lock.lock()
    defer { Bridge.shared.lock.unlock() }
    return mallocCString(Bridge.shared.lastError)
}

@_cdecl("si_free")
public func si_free(_ ptr: UnsafeMutableRawPointer?) {
    if let ptr { free(ptr) }
}

/// Decode the RAW's preview, analyze it, and upload to the GPU. Heavy
/// (~0.5–1 s) — call off the UI thread. Returns 0 on failure.
@_cdecl("si_open")
public func si_open(_ path: UnsafePointer<CChar>?) -> Int64 {
    guard let path else { return 0 }
    let url = URL(fileURLWithPath: String(cString: path))
    do {
        // Native preview size: the downsample to the 1536 proxy happens in
        // the Session, which keeps the intermediate as the medium tier.
        let img = try RawDecoder().decode(url: url, quality: .preview, maxLongEdge: nil)
        let session = Session(url: url, previewDecode: img)
        Bridge.shared.lock.lock()
        defer { Bridge.shared.lock.unlock() }
        let handle = Bridge.shared.nextHandle
        Bridge.shared.nextHandle += 1
        Bridge.shared.sessions[handle] = session
        return handle
    } catch {
        Bridge.shared.setError("open \(url.lastPathComponent): \(error)")
        return 0
    }
}

@_cdecl("si_close")
public func si_close(_ handle: Int64) {
    Bridge.shared.lock.lock()
    Bridge.shared.sessions[handle] = nil
    Bridge.shared.lock.unlock()
}

@_cdecl("si_size")
public func si_size(
    _ handle: Int64, _ outWidth: UnsafeMutablePointer<Int32>?,
    _ outHeight: UnsafeMutablePointer<Int32>?
) -> Int32 {
    Bridge.shared.lock.lock()
    let session = Bridge.shared.sessions[handle]
    Bridge.shared.lock.unlock()
    guard let session else { return 0 }
    outWidth?.pointee = Int32(session.proxy.width)
    outHeight?.pointee = Int32(session.proxy.height)
    return 1
}

/// The interactive path: settings JSON → derive → Vulkan renderDisplay.
/// Returns a malloc'd RGBA8 buffer (width×height×4, alpha 255); caller frees.
/// `srgbDisplay` != 0 converts the output for an unmanaged sRGB canvas;
/// `histogram`, when non-NULL, receives the 4×256 bins (R,G,B,luma — raw
/// counts in the Adobe-encoded display domain, levels included, matching
/// the Mac histogram's semantics).
/// `tier`: 0 = proxy (≤1536, the interactive default), 1 = medium (the
/// half-size decode, instant — falls back to proxy when not retained),
/// 2 = full resolution (first call pays the ~3–5 s decode, then cached).
/// Analysis always runs on the proxy regardless, so the conversion is
/// tier-invariant.
@_cdecl("si_render")
public func si_render(
    _ handle: Int64, _ settingsJSON: UnsafePointer<CChar>?, _ srgbDisplay: Int32,
    _ tier: Int32,
    _ outWidth: UnsafeMutablePointer<Int32>?, _ outHeight: UnsafeMutablePointer<Int32>?,
    _ histogram: UnsafeMutablePointer<UInt32>?
) -> UnsafeMutablePointer<UInt8>? {
    Bridge.shared.lock.lock()
    let session = Bridge.shared.sessions[handle]
    Bridge.shared.lock.unlock()
    guard let session else {
        Bridge.shared.setError("render: bad session handle \(handle)")
        return nil
    }
    var settings = ExposureSettings()
    if let settingsJSON {
        let data = Data(String(cString: settingsJSON).utf8)
        if !data.isEmpty {
            do {
                settings = try JSONDecoder().decode(ExposureSettings.self, from: data)
            } catch {
                Bridge.shared.setError("render: settings JSON: \(error)")
                return nil
            }
        }
    }
    do {
        let pipeline = try Bridge.shared.getPipeline()
        let analysis = session.analysis(settings: settings)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let source = try session.source(
            settings: settings, tier: Session.Tier(rawValue: tier) ?? .proxy,
            pipeline: pipeline)
        let display = try pipeline.renderDisplay(
            source: source, params: params, computeHistogram: histogram != nil,
            srgbDisplay: srgbDisplay != 0)
        outWidth?.pointee = Int32(display.width)
        outHeight?.pointee = Int32(display.height)
        if let histogram {
            display.histogram.withUnsafeBufferPointer {
                histogram.update(from: $0.baseAddress!, count: 1024)
            }
        }
        return mallocCopy(display.rgba)
    } catch {
        Bridge.shared.setError("render: \(error)")
        return nil
    }
}

/// The zero-allocation render: writes RGBA8 into caller-owned memory that
/// persists across frames (see si_render for the semantics of the other
/// parameters). Returns 1 on success; -1 when `dest` is NULL or
/// `destCapacity` is too small — outWidth/outHeight are then set to the
/// frame's dimensions so the caller can size its buffer and retry (the
/// GPU source this measured is cached, so the retry pays nothing extra);
/// 0 on failure (si_last_error explains). The destination must not be
/// read while a render is writing it.
@_cdecl("si_render_into")
public func si_render_into(
    _ handle: Int64, _ settingsJSON: UnsafePointer<CChar>?, _ srgbDisplay: Int32,
    _ tier: Int32, _ dest: UnsafeMutableRawPointer?, _ destCapacity: Int64,
    _ outWidth: UnsafeMutablePointer<Int32>?, _ outHeight: UnsafeMutablePointer<Int32>?,
    _ histogram: UnsafeMutablePointer<UInt32>?
) -> Int32 {
    Bridge.shared.lock.lock()
    let session = Bridge.shared.sessions[handle]
    Bridge.shared.lock.unlock()
    guard let session else {
        Bridge.shared.setError("render: bad session handle \(handle)")
        return 0
    }
    var settings = ExposureSettings()
    if let settingsJSON {
        let data = Data(String(cString: settingsJSON).utf8)
        if !data.isEmpty {
            do {
                settings = try JSONDecoder().decode(ExposureSettings.self, from: data)
            } catch {
                Bridge.shared.setError("render: settings JSON: \(error)")
                return 0
            }
        }
    }
    do {
        let pipeline = try Bridge.shared.getPipeline()
        let source = try session.source(
            settings: settings, tier: Session.Tier(rawValue: tier) ?? .proxy,
            pipeline: pipeline)
        outWidth?.pointee = Int32(source.width)
        outHeight?.pointee = Int32(source.height)
        let needed = Int64(source.width * source.height * 4)
        guard let dest, destCapacity >= needed else { return -1 }
        let analysis = session.analysis(settings: settings)
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let bins = try pipeline.renderDisplay(
            source: source, params: params, computeHistogram: histogram != nil,
            srgbDisplay: srgbDisplay != 0, into: dest)
        if let histogram {
            bins.withUnsafeBufferPointer { histogram.update(from: $0.baseAddress!, count: 1024) }
        }
        return 1
    } catch {
        Bridge.shared.setError("render: \(error)")
        return 0
    }
}

/// Frame facts for the UI, as JSON: dimensions, the darkroom "negative
/// character" row's inputs (measured density range vs the grade default and
/// its label), the metered anchor, and cast confidence (null when no
/// neutral axis was found).
@_cdecl("si_session_info")
public func si_session_info(_ handle: Int64) -> UnsafeMutablePointer<CChar>? {
    Bridge.shared.lock.lock()
    let session = Bridge.shared.sessions[handle]
    Bridge.shared.lock.unlock()
    guard let session else { return nil }
    let analysis = session.cachedAnalysis ?? session.analysis(settings: ExposureSettings())
    let range = analysis.baseBounds.luminanceDensityRange
    var info: [String: Any] = [
        "width": session.proxy.width,
        "height": session.proxy.height,
        "densityRange": range,
        "defaultGradeRange": CurveLogic.defaultGradeRange,
        "anchor": analysis.anchor,
    ]
    if let label = Densitometry.character(densityRange: range)?.label {
        info["character"] = label
    }
    if let confidence = analysis.neutralConfidence {
        info["castConfidence"] = confidence
    }
    let data = (try? JSONSerialization.data(withJSONObject: info, options: [.sortedKeys])) ?? Data("{}".utf8)
    return mallocCString(String(decoding: data, as: UTF8.self))
}

// Sidecar IO — same naming and payload shape as the Mac's SidecarStore
// (`<basename>.swiftinvert.json`, `{version: 1, settings: {...}}`, legacy
// `.negswift.json` read as fallback), so edits round-trip between the two
// frontends. The bridge owns the format; the JSON crossing the ABI is the
// bare settings object.
private struct SidecarPayload: Codable {
    var version = 1
    var settings: ExposureSettings
}

private func sidecarURL(source: URL, legacy: Bool = false) -> URL {
    source.deletingPathExtension()
        .appendingPathExtension(legacy ? "negswift.json" : "swiftinvert.json")
}

/// Settings JSON from the sidecar next to `path`, or NULL if none exists.
@_cdecl("si_sidecar_load")
public func si_sidecar_load(_ path: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    guard let path else { return nil }
    let source = URL(fileURLWithPath: String(cString: path))
    for url in [sidecarURL(source: source), sidecarURL(source: source, legacy: true)] {
        guard let data = try? Data(contentsOf: url),
            let payload = try? JSONDecoder().decode(SidecarPayload.self, from: data)
        else { continue }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        guard let json = try? encoder.encode(payload.settings) else { continue }
        return mallocCString(String(decoding: json, as: UTF8.self))
    }
    return nil
}

/// Write the sidecar next to `path`. Returns 1 on success. The settings pass
/// through ExposureSettings decode→encode, so the file is always canonical
/// regardless of what subset the frontend tracked.
@_cdecl("si_sidecar_save")
public func si_sidecar_save(
    _ path: UnsafePointer<CChar>?, _ settingsJSON: UnsafePointer<CChar>?
) -> Int32 {
    guard let path, let settingsJSON else { return 0 }
    let source = URL(fileURLWithPath: String(cString: path))
    do {
        let settings = try JSONDecoder().decode(
            ExposureSettings.self, from: Data(String(cString: settingsJSON).utf8))
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(SidecarPayload(settings: settings))
        try data.write(to: sidecarURL(source: source), options: .atomic)
        try? FileManager.default.removeItem(at: sidecarURL(source: source, legacy: true))
        return 1
    } catch {
        Bridge.shared.setError("sidecar save \(source.lastPathComponent): \(error)")
        return 0
    }
}

/// Embedded camera JPEG for library thumbnails (Qt decodes JPEG natively).
/// Caller frees. Returns NULL when the file has none.
@_cdecl("si_thumbnail")
public func si_thumbnail(
    _ path: UnsafePointer<CChar>?, _ outLength: UnsafeMutablePointer<Int32>?
) -> UnsafeMutablePointer<UInt8>? {
    guard let path else { return nil }
    let url = URL(fileURLWithPath: String(cString: path))
    do {
        let data = try RawDecoder().embeddedThumbnail(url: url)
        outLength?.pointee = Int32(data.count)
        return mallocCopy([UInt8](data))
    } catch {
        Bridge.shared.setError("thumbnail \(url.lastPathComponent): \(error)")
        return nil
    }
}

/// Default ExposureSettings encoded as JSON — how the frontend learns the
/// slider defaults without duplicating them. Caller frees.
@_cdecl("si_default_settings")
public func si_default_settings() -> UnsafeMutablePointer<CChar> {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    let data = (try? encoder.encode(ExposureSettings())) ?? Data("{}".utf8)
    return mallocCString(String(decoding: data, as: UTF8.self))
}

// ── Export ────────────────────────────────────────────────────────────────

private struct ExportOptions: Decodable {
    var colorspace: String? = nil  // "srgb" (default) | "adobe"
    var maxLongEdge: Int? = nil    // 0/absent = full resolution
}

/// The export pipeline, mirroring the Mac's Exporter + negcli --full:
/// analysis comes from the PREVIEW-SIZED decode (the same proxy the
/// interactive session meters, so the export matches the preview — the
/// what-you-see invariant), while pixels come from the full-resolution
/// best-demosaic decode with the same orientation → fine rotation → crop
/// chain. Resize happens on the encoded output (Mac order), colorspace
/// conversion in-kernel.
private func exportRender(
    path: String, settingsJSON: String, optionsJSON: String
) throws -> (image: RGBImage, srgb: Bool) {
    let url = URL(fileURLWithPath: path)
    var settings = ExposureSettings()
    if !settingsJSON.isEmpty, settingsJSON != "{}" {
        settings = try JSONDecoder().decode(ExposureSettings.self, from: Data(settingsJSON.utf8))
    }
    var options = ExportOptions()
    if !optionsJSON.isEmpty {
        options = try JSONDecoder().decode(ExportOptions.self, from: Data(optionsJSON.utf8))
    }

    let decoder = RawDecoder()
    let preview = try decoder.decode(url: url, quality: .preview, maxLongEdge: 1536)
    let meter = preview.oriented(rotationCW: settings.rotation, flipHorizontal: settings.flipHorizontal)
    let analysis = ExposureKernel.analyze(
        linearImage: meter, cropRect: settings.cropRect, analysisRect: settings.analysisRect)

    var full = try decoder.decode(url: url, quality: .full, maxLongEdge: nil)
    full = full.oriented(
        rotationCW: settings.rotation, flipHorizontal: settings.flipHorizontal,
        fineRotation: settings.fineRotation)
    if let crop = settings.cropRect {
        full = full.cropped(to: crop)
    }

    let pipeline = try Bridge.shared.getPipeline()
    let params = ExposureKernel.deriveRenderParams(settings, analysis)
    let source = try pipeline.upload(full)
    let srgb = (options.colorspace ?? "srgb") != "adobe"
    var encoded = try pipeline.render(
        source: source, params: params, computeHistogram: false, srgbEncode: srgb
    ).encoded
    if let maxEdge = options.maxLongEdge, maxEdge >= 16 {
        encoded = encoded.downsampled(maxLongEdge: maxEdge)
    }
    return (encoded, srgb)
}

/// Full-resolution export render as RGBA8 in the requested colorspace, for
/// the frontend to encode (JPEG). ~3–6 s per frame; call off the UI thread.
/// Caller frees.
@_cdecl("si_export_render")
public func si_export_render(
    _ path: UnsafePointer<CChar>?, _ settingsJSON: UnsafePointer<CChar>?,
    _ optionsJSON: UnsafePointer<CChar>?,
    _ outWidth: UnsafeMutablePointer<Int32>?, _ outHeight: UnsafeMutablePointer<Int32>?
) -> UnsafeMutablePointer<UInt8>? {
    guard let path else { return nil }
    do {
        let img = try exportRender(
            path: String(cString: path),
            settingsJSON: settingsJSON.map { String(cString: $0) } ?? "",
            optionsJSON: optionsJSON.map { String(cString: $0) } ?? "").image
        let n = img.width * img.height
        outWidth?.pointee = Int32(img.width)
        outHeight?.pointee = Int32(img.height)
        let ptr = malloc(n * 4)!.assumingMemoryBound(to: UInt8.self)
        img.pixels.withUnsafeBufferPointer { src in
            for i in 0..<n {
                ptr[i * 4] = UInt8(max(0, min(255, src[i * 3] * 255 + 0.5)))
                ptr[i * 4 + 1] = UInt8(max(0, min(255, src[i * 3 + 1] * 255 + 0.5)))
                ptr[i * 4 + 2] = UInt8(max(0, min(255, src[i * 3 + 2] * 255 + 0.5)))
                ptr[i * 4 + 3] = 255
            }
        }
        return ptr
    } catch {
        Bridge.shared.setError("export \(String(cString: path)): \(error)")
        return nil
    }
}

/// Same pipeline written by the core as a 16-bit baseline TIFF tagged with
/// the requested colorspace's ICC profile (sRGB by default, Adobe RGB on
/// `"colorspace":"adobe"` — matching the in-kernel encode, so wide-gamut
/// bytes are never read as sRGB). Returns 1 on success.
@_cdecl("si_export_tiff")
public func si_export_tiff(
    _ path: UnsafePointer<CChar>?, _ dest: UnsafePointer<CChar>?,
    _ settingsJSON: UnsafePointer<CChar>?, _ optionsJSON: UnsafePointer<CChar>?
) -> Int32 {
    guard let path, let dest else { return 0 }
    do {
        let (img, srgb) = try exportRender(
            path: String(cString: path),
            settingsJSON: settingsJSON.map { String(cString: $0) } ?? "",
            optionsJSON: optionsJSON.map { String(cString: $0) } ?? "")
        try TIFF16.write(
            img, to: URL(fileURLWithPath: String(cString: dest)),
            icc: srgb ? ICCProfiles.sRGB : ICCProfiles.adobeRGB1998)
        return 1
    } catch {
        Bridge.shared.setError("export tiff \(String(cString: path)): \(error)")
        return 0
    }
}

/// Vulkan device the renders run on (diagnostics; "" until first render).
@_cdecl("si_device_name")
public func si_device_name() -> UnsafeMutablePointer<CChar> {
    Bridge.shared.lock.lock()
    let name = Bridge.shared.pipeline?.deviceName ?? ""
    Bridge.shared.lock.unlock()
    return mallocCString(name)
}
