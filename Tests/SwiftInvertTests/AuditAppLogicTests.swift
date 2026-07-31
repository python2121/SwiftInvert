import CoreGraphics
import Foundation
import ImageIO
import Testing

@testable import NegativeKit
@testable import SwiftInvert

/// DensitometerState's zone-grid lifecycle: the grid follows the toggle and
/// the adopted bitmap, and re-enabling rebuilds from the cached bytes with
/// no new adopt.
@MainActor @Suite struct AuditZoneGridLifecycleTests {

    /// 8×6 gradient bitmap via the real rgba8 display path.
    private func makeBitmap() throws -> CGImage {
        let w = 8, h = 6
        var bytes = [UInt8](repeating: 255, count: w * h * 4)
        for y in 0..<h {
            for x in 0..<w {
                let v = UInt8(x * 255 / (w - 1))
                let i = (y * w + x) * 4
                bytes[i] = v
                bytes[i + 1] = v
                bytes[i + 2] = v
            }
        }
        return try #require(ImageConversion.cgImage(rgba8: bytes, width: w, height: h))
    }

    @Test func zoneGridFollowsToggleAndAdopt() throws {
        let state = DensitometerState()

        // Enabled before any adopt: nothing to compute from.
        state.setZoneGridEnabled(true)
        #expect(state.zoneGrid == nil)

        // Adopt → grid appears.
        state.adopt(try makeBitmap())
        let grid = try #require(state.zoneGrid)
        #expect(grid.cols > 0 && grid.rows > 0)
        #expect(grid.zones.allSatisfy { (0...10).contains($0) })

        // Toggle off → nil.
        state.setZoneGridEnabled(false)
        #expect(state.zoneGrid == nil)

        // Re-enable → the grid returns from the cached bytes, no new adopt.
        state.setZoneGridEnabled(true)
        #expect(state.zoneGrid == grid)

        // Image switched away → grid dropped.
        state.adopt(nil)
        #expect(state.zoneGrid == nil)
    }

    @Test func disabledStateNeverComputesAGrid() throws {
        let state = DensitometerState()
        state.adopt(try makeBitmap())
        #expect(state.zoneGrid == nil)  // off by default — no consumer, no work
        // Redundant toggles are no-ops (the guard in setZoneGridEnabled).
        state.setZoneGridEnabled(false)
        #expect(state.zoneGrid == nil)
    }
}

/// ProfileStore recovery from a corrupt persistence blob.
@MainActor @Suite struct AuditProfileStoreRecoveryTests {

    @Test func garbageDefaultsYieldEmptyStoreAndNoneActive() {
        let suiteName = "test-audit-profiles-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defer { defaults.removePersistentDomain(forName: suiteName) }

        // Garbage JSON for the profile list, and an activeProfileID that is
        // a well-formed UUID but matches no profile.
        defaults.set(Data("not json at all".utf8), forKey: "settingsProfiles")
        defaults.set(UUID().uuidString, forKey: "activeProfileID")

        let store = ProfileStore(defaults: defaults)
        #expect(store.userProfiles.isEmpty)
        #expect(store.activeID == ProfileStore.noneID)
        #expect(store.active.settings == ExposureSettings())
    }
}

/// ExportOptions sticky persistence through an injected defaults suite.
@Suite struct AuditExportStickyTests {

    @Test func stickyRoundTrip() {
        let suiteName = "test-audit-export-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defer { defaults.removePersistentDomain(forName: suiteName) }

        var options = ExportOptions()
        options.format = .tiff16
        options.jpegQuality = 0.8
        options.resize = true
        options.maxLongEdge = 2048
        options.colorSpace = .rommRGB
        options.useCustomDestination = true
        options.customDestinationPath = "/exports"
        options.saveSticky(defaults: defaults)

        #expect(ExportOptions.loadSticky(defaults: defaults) == options)
    }

    @Test func corruptOrMissingStickyFallsBackToDefaults() {
        let suiteName = "test-audit-export-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defer { defaults.removePersistentDomain(forName: suiteName) }

        // Missing key → stock options.
        #expect(ExportOptions.loadSticky(defaults: defaults) == ExportOptions())
        // Corrupt blob → stock options.
        defaults.set(Data("{broken".utf8), forKey: "exportOptions")
        #expect(ExportOptions.loadSticky(defaults: defaults) == ExportOptions())
    }
}

/// Exporter.write, render-free: a hand-built encoded buffer through every
/// format/colour-space arm, re-opened and dimension-checked via ImageIO.
@Suite struct AuditExporterWriteTests {

    /// 32×24 encoded gradient (values already in the working TRC domain).
    private func encodedGradient() -> RGBImage {
        let w = 32, h = 24
        var img = RGBImage(width: w, height: h)
        for y in 0..<h {
            for x in 0..<w {
                img[y, x, 0] = Float(x) / Float(w - 1)
                img[y, x, 1] = Float(y) / Float(h - 1)
                img[y, x, 2] = 0.5
            }
        }
        return img
    }

    private func makeScratchDir() throws -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("swiftinvert-audit-export-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private func reopen(_ url: URL) throws -> (width: Int, height: Int, type: String) {
        let data = try #require(FileManager.default.contents(atPath: url.path))
        #expect(!data.isEmpty)
        let src = try #require(CGImageSourceCreateWithData(data as CFData, nil))
        let props = try #require(
            CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any])
        let w = try #require(props[kCGImagePropertyPixelWidth] as? Int)
        let h = try #require(props[kCGImagePropertyPixelHeight] as? Int)
        let type = CGImageSourceGetType(src) as String? ?? ""
        return (w, h, type)
    }

    @Test func writesEveryFormatAndColorSpaceArm() throws {
        let dir = try makeScratchDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let image = encodedGradient()

        // JPEG / sRGB (the default arm).
        var jpegSRGB = ExportOptions()
        jpegSRGB.format = .jpeg
        jpegSRGB.colorSpace = .sRGB
        let jpegURL = dir.appendingPathComponent("out-srgb.jpg")
        try Exporter.write(image, to: jpegURL, options: jpegSRGB)
        let jpeg = try reopen(jpegURL)
        #expect(jpeg.width == 32 && jpeg.height == 24)
        #expect(jpeg.type == "public.jpeg")

        // TIFF / sRGB (16-bit path).
        var tiffSRGB = ExportOptions()
        tiffSRGB.format = .tiff16
        tiffSRGB.colorSpace = .sRGB
        let tiffURL = dir.appendingPathComponent("out-srgb.tiff")
        try Exporter.write(image, to: tiffURL, options: tiffSRGB)
        let tiff = try reopen(tiffURL)
        #expect(tiff.width == 32 && tiff.height == 24)
        #expect(tiff.type == "public.tiff")

        // JPEG / wide gamut (the case still NAMED rommRGB — Adobe RGB since
        // b3490eb; the encoded buffer is written without conversion).
        var jpegWide = ExportOptions()
        jpegWide.format = .jpeg
        jpegWide.colorSpace = .rommRGB
        let wideURL = dir.appendingPathComponent("out-wide.jpg")
        try Exporter.write(image, to: wideURL, options: jpegWide)
        let wide = try reopen(wideURL)
        #expect(wide.width == 32 && wide.height == 24)
    }

    @Test func resizeCapsTheLongEdge() throws {
        let dir = try makeScratchDir()
        defer { try? FileManager.default.removeItem(at: dir) }

        var options = ExportOptions()
        options.format = .jpeg
        options.resize = true
        options.maxLongEdge = 16
        let url = dir.appendingPathComponent("out-small.jpg")
        try Exporter.write(encodedGradient(), to: url, options: options)
        let out = try reopen(url)
        // 32×24 at long edge 16 → scale 0.5 → 16×12 (RGBImage.downsampled).
        #expect(out.width == 16 && out.height == 12)
    }

    @Test func unwritableDestinationThrows() {
        var options = ExportOptions()
        options.format = .jpeg
        let bad = URL(fileURLWithPath: "/dev/null/sub/file.jpg")
        #expect(throws: (any Error).self) {
            try Exporter.write(encodedGradient(), to: bad, options: options)
        }
    }
}
