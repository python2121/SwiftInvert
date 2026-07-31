import Foundation
import Testing

@testable import NegativeKit

/// Off-frame / degenerate rect safety (a crash was recently fixed in
/// `NormalizedRect.pixelROI` — the clamp now runs BEFORE the degeneracy
/// check) and orientation composition pins.
@Suite struct AuditRectAndGeometryTests {

    /// 64×64 ramp image, values 0.1…0.9 (never 0/1, so log stays finite).
    private func rampImage(width: Int = 64, height: Int = 64) -> RGBImage {
        var img = RGBImage(width: width, height: height)
        let n = width * height
        for i in 0..<n {
            let t = Float(i) / Float(max(n - 1, 1))
            let v = 0.1 + 0.8 * t
            img.pixels[i * 3] = v
            img.pixels[i * 3 + 1] = v * 0.9
            img.pixels[i * 3 + 2] = v * 0.8
        }
        return img
    }

    /// The pathological rects the crash fix is about. Expected ROI results
    /// derived by hand from the truncating mapping on a 64×64 frame:
    ///   x0 = max(Int(x·w), 0), x1 = min(Int((x+width)·w), w), need x1−x0 ≥ 2.
    private static let pathological: [(rect: NormalizedRect, note: String)] = [
        // x0 = max(Int(−32), 0) = 0; x1 = min(Int(−6.4), 64) = −6 → span < 2 → nil.
        (NormalizedRect(x: -0.5, y: 0.2, width: 0.4, height: 0.6), "left off-frame"),
        // x0 = max(Int(76.8), 0) = 76; x1 = min(Int(102.4), 64) = 64 → span < 0 → nil.
        (NormalizedRect(x: 1.2, y: 0.2, width: 0.4, height: 0.6), "right off-frame"),
        // x0 = max(−64, 0) = 0; x1 = min(128, 64) = 64 → full frame, both axes.
        (NormalizedRect(x: -1, y: -1, width: 3, height: 3), "surrounds frame"),
        // 0.0005·64 = 0.032 px wide → span < 2 → nil.
        (NormalizedRect(x: 0.5, y: 0.2, width: 0.0005, height: 0.5), "degenerate width"),
    ]

    @Test func pixelROIClampsAndRejectsOffFrameRects() {
        let w = 64, h = 64

        // Hand-derived expectations (see the table above).
        #expect(Self.pathological[0].rect.pixelROI(width: w, height: h) == nil)
        #expect(Self.pathological[1].rect.pixelROI(width: w, height: h) == nil)
        let full = Self.pathological[2].rect.pixelROI(width: w, height: h)
        #expect(full != nil)
        if let full {
            #expect(full.x0 == 0 && full.y0 == 0 && full.x1 == w && full.y1 == h)
        }
        #expect(Self.pathological[3].rect.pixelROI(width: w, height: h) == nil)

        // Invariant sweep: every ROI ever returned must be a valid in-frame
        // box with both spans ≥ 2 (the contract `cropped` relies on).
        var probes = Self.pathological.map(\.rect)
        for x in stride(from: -1.5, through: 1.5, by: 0.25) {
            for width in [0.0, 0.001, 0.3, 1.0, 2.0] {
                probes.append(NormalizedRect(x: x, y: x / 2, width: width, height: width))
            }
        }
        for rect in probes {
            guard let roi = rect.pixelROI(width: w, height: h) else { continue }
            #expect(roi.x0 >= 0 && roi.x0 <= roi.x1 && roi.x1 <= w)
            #expect(roi.y0 >= 0 && roi.y0 <= roi.y1 && roi.y1 <= h)
            #expect(roi.x1 - roi.x0 >= 2 && roi.y1 - roi.y0 >= 2)
        }
    }

    @Test func croppedWithPathologicalRectsNeverTraps() {
        let img = rampImage()
        for (rect, note) in Self.pathological {
            let out = img.cropped(to: rect)
            // Either self (nil ROI) or an image whose dims match the ROI.
            if let roi = rect.pixelROI(width: img.width, height: img.height) {
                #expect(out.width == roi.x1 - roi.x0, "\(note)")
                #expect(out.height == roi.y1 - roi.y0, "\(note)")
            } else {
                #expect(out.width == img.width && out.height == img.height, "\(note)")
            }
            #expect(out.pixels.count == out.width * out.height * 3, "\(note)")
        }
    }

    @Test func prepareWithOffFrameCropRectDoesNotTrap() {
        let img = rampImage()
        for (rect, note) in Self.pathological {
            // An off-frame/degenerate crop rect must be ignored (nil ROI),
            // not crash prepare; the full frame is metered instead.
            let prepared = ExposureKernel.prepare(linearImage: img, cropRect: rect)
            #expect(prepared.grid.width > 0 && prepared.grid.height > 0, "\(note)")
            for ch in 0..<3 {
                #expect(prepared.baseBounds.floors[ch].isFinite, "\(note)")
                #expect(prepared.baseBounds.ceils[ch].isFinite, "\(note)")
            }
            #expect(prepared.anchor.isFinite, "\(note)")
        }
        // And the same rects as analysis regions.
        for (rect, note) in Self.pathological {
            let prepared = ExposureKernel.prepare(linearImage: img, analysisRect: rect)
            #expect(prepared.grid.width > 0 && prepared.grid.height > 0, "\(note)")
        }
    }

    @Test func fitScaleIsZeroWhenBoxCenterOutsideRotatedFrame() {
        let frame = SIMD2<Double>(100, 80)
        // θ = 0: the rotated frame is ±(50, 40); center (60, 0) is outside on
        // x. For the +x corners, t.x = +hx > 0 and limit = (50 − 60)/hx < 0,
        // so k = min(k, max(limit, 0)) = 0.
        #expect(
            CropGeometry.fitScale(
                center: SIMD2(60, 0), halfExtents: SIMD2(10, 10), radians: 0, frame: frame) == 0)
        // Rotated case: center far outside every possible bound.
        #expect(
            CropGeometry.fitScale(
                center: SIMD2(200, 200), halfExtents: SIMD2(5, 5), radians: 0.3, frame: frame) == 0)

        // A zero-extent box through normalizedRect produces a degenerate
        // NormalizedRect that pixelROI must reject (nil), not crash.
        let degenerate = CropGeometry.normalizedRect(
            center: SIMD2(0, 0), halfExtents: SIMD2(0, 0), frame: frame, radians: 0)
        #expect(degenerate.width == 0 && degenerate.height == 0)
        #expect(degenerate.pixelROI(width: 100, height: 80) == nil)
        // Scaled-to-zero box (from the fitScale == 0 corner) — same story.
        let collapsed = CropGeometry.normalizedRect(
            center: SIMD2(60, 0), halfExtents: SIMD2(10, 10) * 0, frame: frame, radians: 0)
        #expect(collapsed.pixelROI(width: 100, height: 80) == nil)
    }

    /// `oriented(rotationCW: 90, flipHorizontal: true)` — expected mapping
    /// derived by hand from PlanarImage.swift:
    ///   applyingFlip(6) (90° CW): out is h×w→w×h; out[y, x] = self[H−1−x, y]
    ///     (display: source (r, c) lands at (c, H−1−r)).
    ///   flippedHorizontally on the rotated image (width H):
    ///     (c, H−1−r) → (c, H−1−(H−1−r)) = (c, r).
    /// Composition: final[y, x] = original[x, y] — a pure transpose.
    @Test func orientedNinetyCWPlusFlipIsTranspose() {
        let w = 4, h = 3
        var img = RGBImage(width: w, height: h)
        for y in 0..<h {
            for x in 0..<w {
                for c in 0..<3 {
                    // Unique value per (y, x, c).
                    img[y, x, c] = Float((y * w + x) * 3 + c)
                }
            }
        }
        let out = img.oriented(rotationCW: 90, flipHorizontal: true, fineRotation: 0)
        #expect(out.width == h && out.height == w)
        for y in 0..<out.height {  // out rows: 0..<4
            for x in 0..<out.width {  // out cols: 0..<3
                for c in 0..<3 {
                    // Explicit index math, not the helpers: transpose.
                    #expect(out.pixels[(y * out.width + x) * 3 + c]
                        == Float((x * w + y) * 3 + c))
                }
            }
        }
    }
}
