import Foundation
import Testing
import simd

@testable import NegativeKit

/// The canvas lays out from `CropGeometry.displayAspect`, not from the
/// rendered bitmap's pixel dimensions. The bitmap can't serve: `pixelROI`
/// truncates the crop to whole pixels and `fineRotated` floors the inscribed
/// rect, so the 1536px proxy and the full-resolution tier land on different
/// aspects for the SAME settings — which made the picture nudge when the HQ
/// preview swapped in.
@Suite struct DisplayAspectTests {

    /// Proxy and full-resolution dimensions for a 24MP frame: the preview is
    /// decoded half-size (3012×2010) then capped at 1536 on the long edge.
    private let proxy = SIMD2<Double>(1536, 1025)
    private let full = SIMD2<Double>(6024, 4020)

    /// What the OLD code laid out from — the cropped/rotated bitmap's real
    /// pixel ratio, rounded exactly as the pipeline rounds it.
    private func bitmapAspect(_ frame: SIMD2<Double>, radians: Double, crop: NormalizedRect?)
        -> Double
    {
        var w = Int(frame.x), h = Int(frame.y)
        if abs(radians) > 1e-9 {
            let inscribed = RGBImage.inscribedRectSize(
                width: Double(w), height: Double(h), radians: radians)
            w = max(Int(inscribed.width.rounded(.down)), 1)
            h = max(Int(inscribed.height.rounded(.down)), 1)
        }
        if let crop, let roi = crop.pixelROI(width: w, height: h) {
            w = roi.x1 - roi.x0
            h = roi.y1 - roi.y0
        }
        return Double(w) / Double(h)
    }

    private let crops: [NormalizedRect?] = [
        nil,
        NormalizedRect(x: 0, y: 0, width: 1, height: 1),
        NormalizedRect(x: 0.1, y: 0.08, width: 0.8, height: 0.84),
        NormalizedRect(x: 0.05, y: 0.05, width: 0.9, height: 0.9),
        NormalizedRect(x: 0.33, y: 0.25, width: 0.34, height: 0.5),
        NormalizedRect(x: 0.2013, y: 0.1077, width: 0.6431, height: 0.7752),
    ]
    private let angles: [Double] = [0, 0.4, -1.3, 2.7, -5.0, 12.0]

    /// THE invariant: both tiers must lay out identically. The proxy and the
    /// full-resolution render describe the same continuous rectangle, so the
    /// aspect must not depend on which one produced the pixels.
    @Test func aspectIsIdenticalForProxyAndFullResolution() {
        for crop in crops {
            for deg in angles {
                let radians = deg * .pi / 180
                let p = CropGeometry.displayAspect(frame: proxy, radians: radians, crop: crop)
                let f = CropGeometry.displayAspect(frame: full, radians: radians, crop: crop)
                // The two bases differ by the proxy's own 1.9e-5 rounding
                // (1536/1025 vs 6024/4020); everything downstream is continuous,
                // so nothing may amplify it.
                #expect(
                    abs(p - f) / f < 5e-5,
                    "crop \(String(describing: crop)) at \(deg)°: proxy \(p) vs full \(f)")
            }
        }
    }

    /// The bug, pinned: the old bitmap-derived layout really did disagree
    /// between tiers, by far more than the fix's tolerance. If this ever stops
    /// holding, the test above has become vacuous.
    @Test func bitmapDerivedAspectDisagreedBetweenTiers() {
        let crop = NormalizedRect(x: 0.33, y: 0.25, width: 0.34, height: 0.5)
        let p = bitmapAspect(proxy, radians: 0, crop: crop)
        let f = bitmapAspect(full, radians: 0, crop: crop)
        // ~2e-3 — at 1200pt fitted and 4x zoom that is several points of jump.
        #expect(abs(p - f) / f > 1e-3)
        // And the fix removes it for that same crop.
        let pf = CropGeometry.displayAspect(frame: proxy, radians: 0, crop: crop)
        let ff = CropGeometry.displayAspect(frame: full, radians: 0, crop: crop)
        #expect(abs(pf - ff) / ff < 5e-5)
    }

    /// The layout aspect must still describe the picture the render produced —
    /// close enough to each tier's true bitmap ratio that filling the rect is a
    /// sub-pixel stretch rather than a visible distortion.
    @Test func aspectStaysCloseToEachTiersRealBitmap() {
        for crop in crops {
            for deg in angles {
                let radians = deg * .pi / 180
                for frame in [proxy, full] {
                    let ideal = CropGeometry.displayAspect(frame: frame, radians: radians, crop: crop)
                    let real = bitmapAspect(frame, radians: radians, crop: crop)
                    #expect(
                        abs(ideal - real) / real < 3e-3,
                        "crop \(String(describing: crop)) at \(deg)° frame \(frame)")
                }
            }
        }
    }

    // MARK: - Plain correctness

    @Test func uncroppedUnrotatedIsTheFrameAspect() {
        #expect(CropGeometry.displayAspect(frame: proxy, radians: 0, crop: nil) == 1536.0 / 1025.0)
        let square = SIMD2<Double>(1000, 1000)
        #expect(CropGeometry.displayAspect(frame: square, radians: 0, crop: nil) == 1.0)
    }

    @Test func cropScalesTheAspectByItsOwnRatio() {
        let frame = SIMD2<Double>(1000, 1000)
        // A half-width, full-height crop of a square is 2:1 tall — aspect 0.5.
        let tall = NormalizedRect(x: 0.25, y: 0, width: 0.5, height: 1)
        #expect(abs(CropGeometry.displayAspect(frame: frame, radians: 0, crop: tall) - 0.5) < 1e-12)
        let wide = NormalizedRect(x: 0, y: 0.25, width: 1, height: 0.5)
        #expect(abs(CropGeometry.displayAspect(frame: frame, radians: 0, crop: wide) - 2.0) < 1e-12)
    }

    /// A crop is normalized over the INSCRIBED rect, so rotation and crop
    /// compose: the crop's ratio multiplies whatever straightening left.
    @Test func rotationAndCropCompose() {
        let radians = 5.0 * .pi / 180
        let inscribed = CropGeometry.inscribedSize(frame: proxy, radians: radians)
        let crop = NormalizedRect(x: 0.1, y: 0.1, width: 0.6, height: 0.7)
        let expected = (inscribed.x * 0.6) / (inscribed.y * 0.7)
        #expect(
            abs(CropGeometry.displayAspect(frame: proxy, radians: radians, crop: crop) - expected)
                < 1e-12)
    }

    /// Straightening a landscape frame narrows it toward square — the inscribed
    /// rect loses more width than height.
    @Test func rotationMovesAspectTowardSquare() {
        let flat = CropGeometry.displayAspect(frame: proxy, radians: 0, crop: nil)
        var previous = flat
        for deg in [1.0, 3.0, 6.0, 10.0] {
            let a = CropGeometry.displayAspect(frame: proxy, radians: deg * .pi / 180, crop: nil)
            #expect(a > previous, "\(deg)° should be narrower than the last")
            previous = a
        }
        // Sign of the angle can't matter: ±θ inscribe the same rectangle.
        for deg in [0.4, 2.7, 9.0] {
            let plus = CropGeometry.displayAspect(frame: proxy, radians: deg * .pi / 180, crop: nil)
            let minus = CropGeometry.displayAspect(frame: proxy, radians: -deg * .pi / 180, crop: nil)
            #expect(abs(plus - minus) < 1e-12, "\(deg)°")
        }
    }

    // MARK: - The crop WINDOW, not just its shape

    /// The bigger half of the HQ-swap shift: `pixelROI` truncates on each
    /// tier's own grid, so the same normalized rect selects a different part of
    /// the picture at 1536px than at 6024px. `hqSourceTexture` re-derives the
    /// full-resolution window from the proxy's instead; this pins that the
    /// re-derivation lands on the same normalized window.
    private func anchored(_ roi: (x0: Int, y0: Int, x1: Int, y1: Int), from p: SIMD2<Double>,
        to f: SIMD2<Double>) -> (x0: Int, y0: Int, x1: Int, y1: Int)
    {
        let sx = f.x / p.x, sy = f.y / p.y
        let x0 = min(max(Int((Double(roi.x0) * sx).rounded()), 0), Int(f.x) - 1)
        let y0 = min(max(Int((Double(roi.y0) * sy).rounded()), 0), Int(f.y) - 1)
        let x1 = min(max(Int((Double(roi.x1) * sx).rounded()), x0 + 1), Int(f.x))
        let y1 = min(max(Int((Double(roi.y1) * sy).rounded()), y0 + 1), Int(f.y))
        return (x0, y0, x1, y1)
    }

    @Test func anchoringPutsBothTiersOnTheSameNormalizedWindow() {
        for case let crop? in crops {
            guard let p = crop.pixelROI(width: Int(proxy.x), height: Int(proxy.y)),
                let independent = crop.pixelROI(width: Int(full.x), height: Int(full.y))
            else { continue }
            let a = anchored(p, from: proxy, to: full)

            // Independent truncation drifts by up to ~5e-4 of frame width —
            // several points of CONTENT movement at 4x zoom.
            let independentDrift = max(
                abs(Double(p.x0) / proxy.x - Double(independent.x0) / full.x),
                abs(Double(p.y0) / proxy.y - Double(independent.y0) / full.y))
            // Anchoring leaves only the full-res grid's own half pixel.
            let anchoredDrift = max(
                abs(Double(p.x0) / proxy.x - Double(a.x0) / full.x),
                abs(Double(p.y0) / proxy.y - Double(a.y0) / full.y))

            // Half a full-res pixel, on whichever axis is coarser.
            let halfPixel = max(0.5 / full.x, 0.5 / full.y)
            #expect(anchoredDrift <= halfPixel + 1e-12, "origin drift for \(crop)")
            #expect(
                anchoredDrift <= independentDrift + 1e-12,
                "anchoring must never be worse than truncating: \(crop)")

            // Same for the window's extent, which is what sets the aspect.
            let wDrift = abs(
                Double(p.x1 - p.x0) / proxy.x - Double(a.x1 - a.x0) / full.x)
            let hDrift = abs(
                Double(p.y1 - p.y0) / proxy.y - Double(a.y1 - a.y0) / full.y)
            #expect(wDrift <= 1.0 / full.x + 1e-12, "width drift for \(crop)")
            #expect(hDrift <= 1.0 / full.y + 1e-12, "height drift for \(crop)")
        }
    }

    /// `cropped(toPixels:)` is the primitive the anchoring rides on: it must
    /// select exactly the requested window and refuse degenerate ones.
    @Test func croppedToPixelsSelectsExactlyTheRequestedWindow() {
        var img = RGBImage(width: 20, height: 10)
        for y in 0..<10 {
            for x in 0..<20 {
                img[y, x, 0] = Float(y * 20 + x)
            }
        }
        let out = img.cropped(toPixels: (x0: 3, y0: 2, x1: 11, y1: 8))
        #expect(out.width == 8 && out.height == 6)
        #expect(out[0, 0, 0] == Float(2 * 20 + 3))
        #expect(out[5, 7, 0] == Float(7 * 20 + 10))

        // Out-of-range clamps rather than trapping.
        let clamped = img.cropped(toPixels: (x0: -5, y0: -5, x1: 999, y1: 999))
        #expect(clamped.width == 20 && clamped.height == 10)
        // Degenerate returns self untouched.
        let degenerate = img.cropped(toPixels: (x0: 5, y0: 5, x1: 6, y1: 6))
        #expect(degenerate.width == 20 && degenerate.height == 10)
    }

    @Test func degenerateInputsFallBackRatherThanProducingGarbage() {
        let zeroCrop = NormalizedRect(x: 0.5, y: 0.5, width: 0, height: 0)
        let a = CropGeometry.displayAspect(frame: proxy, radians: 0, crop: zeroCrop)
        #expect(a.isFinite && a > 0)
        let degenerateFrame = SIMD2<Double>(0, 0)
        let b = CropGeometry.displayAspect(frame: degenerateFrame, radians: 0, crop: nil)
        #expect(b.isFinite && b > 0)
    }
}
