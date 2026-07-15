import CoreGraphics
import Foundation
import NegativeKit

/// Hover read-out state for the canvas spot densitometer.
///
/// Isolated in its own `@Observable` box on purpose: only the read-out view
/// reads `reading`, so a pointer move invalidates that label alone. Holding it
/// as `DetailView` state instead would re-render the whole canvas — bitmap,
/// overlays and all — on every mouse event (the same reason
/// `HistogramHoverLayer` exists).
@MainActor
@Observable
final class DensitometerState {
    /// The probed pixel, or nil when the pointer is off the image.
    var reading: Densitometry.Reading?

    @ObservationIgnored private var pixels: CFData?
    @ObservationIgnored private var width = 0
    @ObservationIgnored private var height = 0
    @ObservationIgnored private var bytesPerRow = 0

    /// Adopt the newly rendered bitmap. The bytes are grabbed once per render
    /// because `CGDataProvider.data` can copy the whole buffer — several MB per
    /// pointer move would be absurd for reading three of them.
    func adopt(_ image: CGImage?) {
        reading = nil
        guard let image, let data = image.dataProvider?.data else {
            pixels = nil
            width = 0
            height = 0
            return
        }
        pixels = data
        width = image.width
        height = image.height
        bytesPerRow = image.bytesPerRow
    }

    /// Probe at `u`,`v` — 0…1 across the displayed bitmap.
    func probe(u: Double, v: Double) {
        guard let pixels, width > 0, height > 0,
            (0...1).contains(u), (0...1).contains(v),
            let base = CFDataGetBytePtr(pixels)
        else {
            reading = nil
            return
        }
        let x = min(Int(u * Double(width)), width - 1)
        let y = min(Int(v * Double(height)), height - 1)
        let i = y * bytesPerRow + x * 4
        guard i >= 0, i + 2 < CFDataGetLength(pixels) else {
            reading = nil
            return
        }
        // ImageConversion writes rgba8 (noneSkipLast) tagged ROMM, so these
        // bytes ARE the working-space encoded values the read-out is defined
        // on — no colour conversion, just the /255.
        let rgb = SIMD3<Double>(
            Double(base[i]) / 255.0, Double(base[i + 1]) / 255.0, Double(base[i + 2]) / 255.0)
        // Most pointer moves land in the pixel we're already showing; an
        // unconditional write would invalidate the read-out for nothing.
        let next = Densitometry.read(encodedRGB: rgb)
        if next != reading { reading = next }
    }

    func clear() {
        if reading != nil { reading = nil }
    }
}
