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
    /// The retained half-size buffer the preview decode already produced.
    private let medium = SIMD2<Double>(3012, 2010)
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

    // MARK: - The end-to-end invariant: same content, same screen position

    /// Simulate a tier all the way to the screen: floor the inscribed rect and
    /// the crop ROI exactly as the pipeline does, then ask where a given point
    /// of the FRAME lands in the fitted rect.
    ///
    /// `f` is in ideal-inscribed-rect coordinates (0…1 across the straightened
    /// frame). Returns the screen x/y in points.
    private func screenPosition(
        of f: SIMD2<Double>, frame: SIMD2<Double>, radians: Double, crop: NormalizedRect?,
        fittedWidth: Double = 1200
    ) -> SIMD2<Double> {
        let inscribed = CropGeometry.inscribedSize(frame: frame, radians: radians)
        // The oriented bitmap: inscribed rect floored to whole pixels.
        let ow = Double(max(Int(inscribed.x.rounded(.down)), 1))
        let oh = Double(max(Int(inscribed.y.rounded(.down)), 1))

        // Each tier crops independently on its own grid — no anchoring. The
        // placement must be exact anyway.
        var roi: SIMD4<Double>?
        if let crop {
            guard let p = crop.pixelROI(width: Int(ow), height: Int(oh)) else { return .zero }
            roi = SIMD4(Double(p.x0), Double(p.y0), Double(p.x1), Double(p.y1))
        }

        // Each tier measures its own bitmap against its OWN frame...
        let cw = CropGeometry.contentWindow(
            frame: frame, radians: radians, crop: crop,
            orientedPixels: SIMD2(ow, oh), roi: roi)
        // ...but the canvas is laid out from ONE aspect for both, which the app
        // takes from the proxy (`orientedFrameSize` always reads basePreview).
        let aspect = CropGeometry.displayAspect(frame: proxy, radians: radians, crop: crop)
        let fitted = SIMD2(fittedWidth, fittedWidth / aspect)

        // The bitmap's screen rect, placed by its real window.
        let originPt = SIMD2(fitted.x * cw.x, fitted.y * cw.y)
        let sizePt = SIMD2(fitted.x * cw.width, fitted.y * cw.height)

        // Where `f` sits inside the bitmap, as a fraction of it.
        let ideal = crop ?? NormalizedRect(x: 0, y: 0, width: 1, height: 1)
        // f in ideal-cropped units, then into bitmap units via the window.
        let inCropped = SIMD2(
            (f.x - ideal.x) / ideal.width, (f.y - ideal.y) / ideal.height)
        let p = SIMD2((inCropped.x - cw.x) / cw.width, (inCropped.y - cw.y) / cw.height)
        return SIMD2(originPt.x + sizePt.x * p.x, originPt.y + sizePt.y * p.y)
    }

    /// THE invariant the whole fix exists for: a given point of the picture must
    /// land on the same pixel of the screen whether the proxy or the
    /// full-resolution tier produced it. Before the fix this drifted by several
    /// points at zoom; placing each bitmap by its true window makes the two
    /// tiers resolve to the SAME affine frame→screen map, so it is exact.
    @Test func sameContentLandsAtTheSameScreenPositionInBothTiers() {
        for crop in crops {
            for deg in angles {
                let radians = deg * .pi / 180
                // Sample the frame, including the corners of the crop.
                for f in [
                    SIMD2(0.2, 0.2), SIMD2(0.5, 0.5), SIMD2(0.8, 0.75), SIMD2(0.35, 0.62),
                ] {
                    let p = screenPosition(of: f, frame: proxy, radians: radians, crop: crop)
                    let m = screenPosition(of: f, frame: medium, radians: radians, crop: crop)
                    let h = screenPosition(of: f, frame: full, radians: radians, crop: crop)
                    // All THREE tiers must agree — Auto swaps proxy↔medium and
                    // On swaps proxy↔full, so any pair can meet on screen.
                    let drift = max(
                        max(abs(p.x - h.x), abs(p.y - h.y)),
                        max(
                            max(abs(p.x - m.x), abs(p.y - m.y)),
                            max(abs(m.x - h.x), abs(m.y - h.y))))
                    // Float noise, not geometry: ~1e-4 pt on a 1200 pt canvas
                    // is a billionth of the frame. The user-visible figure this
                    // replaces was several POINTS.
                    #expect(
                        drift < 1e-3,
                        "\(String(describing: crop)) at \(deg)°, point \(f): drifted \(drift) pt")
                }
            }
        }
    }

    /// The fix must beat the previous behaviour by orders of magnitude, not
    /// marginally — otherwise the tolerance above is doing the work. Compares
    /// placing each bitmap by its window against stretching both to fill.
    @Test func placingBeatsFillingByOrdersOfMagnitude() {
        let crop = NormalizedRect(x: 0.33, y: 0.25, width: 0.34, height: 0.5)
        let f = SIMD2(0.5, 0.5)

        // Filling: f maps through each bitmap's own window onto the same rect.
        func filled(_ frame: SIMD2<Double>) -> Double {
            let ow = frame.x, oh = frame.y
            guard let roi = crop.pixelROI(width: Int(ow), height: Int(oh)) else { return 0 }
            let ax = Double(roi.x0) / ow, aw = Double(roi.x1 - roi.x0) / ow
            return 1200.0 * ((f.x - ax) / aw)
        }
        let fillDrift = abs(filled(proxy) - filled(full))

        let placeDrift = abs(
            screenPosition(of: f, frame: proxy, radians: 0, crop: crop).x
                - screenPosition(of: f, frame: full, radians: 0, crop: crop).x)

        #expect(fillDrift > 0.01, "filling should still drift; got \(fillDrift) pt")
        #expect(
            placeDrift < fillDrift / 100,
            "placing (\(placeDrift) pt) should be <1% of filling (\(fillDrift) pt)")
    }

    @Test func contentWindowIsTheUnitRectWhenNothingQuantizes() {
        // No rotation and no crop: the bitmap IS the ideal rect.
        let cw = CropGeometry.contentWindow(
            frame: proxy, radians: 0, crop: nil, orientedPixels: proxy, roi: nil)
        #expect(abs(cw.x) < 1e-12 && abs(cw.y) < 1e-12)
        #expect(abs(cw.width - 1) < 1e-12 && abs(cw.height - 1) < 1e-12)
    }

    @Test func contentWindowStaysNearTheUnitRect() {
        // Whatever the settings, the bitmap is at most a pixel off the ideal —
        // a window far from (0,0,1,1) would mean the algebra is wrong.
        for crop in crops {
            for deg in angles {
                let radians = deg * .pi / 180
                let inscribed = CropGeometry.inscribedSize(frame: proxy, radians: radians)
                let ow = Double(max(Int(inscribed.x.rounded(.down)), 1))
                let oh = Double(max(Int(inscribed.y.rounded(.down)), 1))
                var roi: SIMD4<Double>?
                if let crop, let p = crop.pixelROI(width: Int(ow), height: Int(oh)) {
                    roi = SIMD4(Double(p.x0), Double(p.y0), Double(p.x1), Double(p.y1))
                }
                let cw = CropGeometry.contentWindow(
                    frame: proxy, radians: radians, crop: crop,
                    orientedPixels: SIMD2(ow, oh), roi: roi)
                #expect(abs(cw.x) < 0.01 && abs(cw.y) < 0.01, "origin \(cw) for \(deg)°")
                #expect(
                    abs(cw.width - 1) < 0.01 && abs(cw.height - 1) < 0.01,
                    "size \(cw) for \(deg)°")
            }
        }
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
