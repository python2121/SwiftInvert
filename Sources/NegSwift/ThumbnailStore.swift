import AppKit
import Foundation
import RawDecodeKit

/// Library-grid thumbnails: the embedded camera JPEG (unconverted, like NegPy's
/// contact thumbs), extracted concurrently with bounded parallelism and cached
/// in memory.
actor ThumbnailStore {
    private var cache: [URL: CGImage] = [:]
    private var inFlight: [URL: Task<CGImage?, Never>] = [:]

    func thumbnail(for url: URL) async -> CGImage? {
        if let hit = cache[url] { return hit }
        if let task = inFlight[url] { return await task.value }
        let task = Task<CGImage?, Never>.detached(priority: .utility) {
            guard let data = try? RawDecoder().embeddedThumbnail(url: url),
                let source = CGImageSourceCreateWithData(data as CFData, nil)
            else { return nil }
            let options: [CFString: Any] = [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceThumbnailMaxPixelSize: 384,
                kCGImageSourceCreateThumbnailWithTransform: true,
            ]
            return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
        }
        inFlight[url] = task
        let image = await task.value
        inFlight[url] = nil
        if let image { cache[url] = image }
        return image
    }

    func clear() {
        cache.removeAll()
        inFlight.values.forEach { $0.cancel() }
        inFlight.removeAll()
    }
}
