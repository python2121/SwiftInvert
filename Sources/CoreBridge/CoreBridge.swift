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

private final class Session {
    let source: VulkanRenderPipeline.SourceBuffer
    let analysis: ExposureAnalysis
    init(source: VulkanRenderPipeline.SourceBuffer, analysis: ExposureAnalysis) {
        self.source = source
        self.analysis = analysis
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
        let img = try RawDecoder().decode(url: url, quality: .preview, maxLongEdge: 1536)
        let analysis = ExposureKernel.analyze(linearImage: img)
        let pipeline = try Bridge.shared.getPipeline()
        let source = try pipeline.upload(img)
        let session = Session(source: source, analysis: analysis)
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
    outWidth?.pointee = Int32(session.source.width)
    outHeight?.pointee = Int32(session.source.height)
    return 1
}

/// The interactive path: settings JSON → derive → Vulkan renderDisplay.
/// Returns a malloc'd RGBA8 buffer (width×height×4, alpha 255); caller frees.
/// `srgbDisplay` != 0 converts the output for an unmanaged sRGB canvas;
/// `histogram`, when non-NULL, receives the 4×256 bins (R,G,B,luma — raw
/// counts in the Adobe-encoded display domain, levels included, matching
/// the Mac histogram's semantics).
@_cdecl("si_render")
public func si_render(
    _ handle: Int64, _ settingsJSON: UnsafePointer<CChar>?, _ srgbDisplay: Int32,
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
        let params = ExposureKernel.deriveRenderParams(settings, session.analysis)
        let display = try pipeline.renderDisplay(
            source: session.source, params: params, computeHistogram: histogram != nil,
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
    let range = session.analysis.baseBounds.luminanceDensityRange
    var info: [String: Any] = [
        "width": session.source.width,
        "height": session.source.height,
        "densityRange": range,
        "defaultGradeRange": CurveLogic.defaultGradeRange,
        "anchor": session.analysis.anchor,
    ]
    if let label = Densitometry.character(densityRange: range)?.label {
        info["character"] = label
    }
    if let confidence = session.analysis.neutralConfidence {
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

/// Vulkan device the renders run on (diagnostics; "" until first render).
@_cdecl("si_device_name")
public func si_device_name() -> UnsafeMutablePointer<CChar> {
    Bridge.shared.lock.lock()
    let name = Bridge.shared.pipeline?.deviceName ?? ""
    Bridge.shared.lock.unlock()
    return mallocCString(name)
}
