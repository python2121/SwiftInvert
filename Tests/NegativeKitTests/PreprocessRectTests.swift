import Foundation
import Testing

@testable import NegativeKit

/// Pre-process rects: the analysis region and output crop must scope the meters
/// exactly like running the kernel on a pre-cropped image (whose math is already
/// fixture-anchored), with NegPy's resolve_analysis_region priority rules.
@Suite struct PreprocessRectTests {
    static let rect = NormalizedRect(x: 0.25, y: 0.2, width: 0.5, height: 0.55)

    @Test func croppedImageMatchesManualSlice() {
        let img = SyntheticGrid.input
        let out = img.cropped(to: Self.rect)
        let roi = Self.rect.pixelROI(width: img.width, height: img.height)!
        #expect(out.width == roi.x1 - roi.x0 && out.height == roi.y1 - roi.y0)
        // Spot-check corners map to the source ROI.
        #expect(out[0, 0, 0] == img[roi.y0, roi.x0, 0])
        #expect(out[out.height - 1, out.width - 1, 2] == img[roi.y1 - 1, roi.x1 - 1, 2])
    }

    @Test func analysisRectEqualsPreCroppedWithZeroBuffer() {
        let img = SyntheticGrid.input
        let viaRect = ExposureKernel.analyze(linearImage: img, analysisRect: Self.rect)
        let viaCrop = ExposureKernel.analyze(
            linearImage: img.cropped(to: Self.rect), analysisBuffer: 0.0)
        #expect(viaRect == viaCrop)
        // And it actually changes the result vs the full frame.
        let full = ExposureKernel.analyze(linearImage: img)
        #expect(viaRect.baseBounds != full.baseBounds)
    }

    @Test func cropRectScopesAnalysisWithBufferInside() {
        let img = SyntheticGrid.input
        let viaCropSetting = ExposureKernel.analyze(linearImage: img, cropRect: Self.rect)
        let viaPreCrop = ExposureKernel.analyze(linearImage: img.cropped(to: Self.rect))
        #expect(viaCropSetting == viaPreCrop)
    }

    @Test func analysisRectWinsOverCrop() {
        let img = SyntheticGrid.input
        let other = NormalizedRect(x: 0.0, y: 0.0, width: 0.4, height: 0.4)
        let both = ExposureKernel.analyze(linearImage: img, cropRect: other, analysisRect: Self.rect)
        let rectOnly = ExposureKernel.analyze(linearImage: img, analysisRect: Self.rect)
        #expect(both == rectOnly)
    }

    @Test func degenerateRectIsIgnored() {
        let img = Synthetic64.input
        let tiny = NormalizedRect(x: 0.5, y: 0.5, width: 0.001, height: 0.001)
        let with = ExposureKernel.analyze(linearImage: img, analysisRect: tiny)
        let without = ExposureKernel.analyze(linearImage: img)
        #expect(with == without)
    }

    @Test func sidecarRoundTripsRects() throws {
        var s = ExposureSettings()
        s.analysisRect = Self.rect
        s.cropRect = NormalizedRect(x: 0.1, y: 0.1, width: 0.8, height: 0.8)
        let data = try JSONEncoder().encode(s)
        let back = try JSONDecoder().decode(ExposureSettings.self, from: data)
        #expect(back == s)
        // Pre-rect sidecars decode with nil rects.
        let legacy = try JSONDecoder().decode(ExposureSettings.self, from: Data("{}".utf8))
        #expect(legacy.analysisRect == nil && legacy.cropRect == nil)
    }
}
