import Foundation
import Testing

@testable import SwiftInvert

/// ExportOptions.destinationURL — the pure URL math behind every export.
@Suite struct ExportOptionsTests {

    @Test func nextToSourceByDefault() {
        var options = ExportOptions()
        options.format = .jpeg
        let dest = options.destinationURL(for: URL(fileURLWithPath: "/roll/IMG_0001.CR3"))
        #expect(dest.path == "/roll/IMG_0001.jpg")
    }

    @Test func tiffExtension() {
        var options = ExportOptions()
        options.format = .tiff16
        let dest = options.destinationURL(for: URL(fileURLWithPath: "/roll/IMG_0001.CR3"))
        #expect(dest.path == "/roll/IMG_0001.tiff")
    }

    @Test func customDestinationRedirectsTheDirectoryOnly() {
        var options = ExportOptions()
        options.useCustomDestination = true
        options.customDestinationPath = "/exports"
        let dest = options.destinationURL(for: URL(fileURLWithPath: "/roll/IMG_0001.CR3"))
        #expect(dest.path == "/exports/IMG_0001.jpg")
    }

    /// The custom flag without a path must not crash — fall back to
    /// next-to-source (the persisted options blob can hold that combination).
    @Test func customFlagWithoutPathFallsBack() {
        var options = ExportOptions()
        options.useCustomDestination = true
        options.customDestinationPath = nil
        let dest = options.destinationURL(for: URL(fileURLWithPath: "/roll/IMG_0001.CR3"))
        #expect(dest.path == "/roll/IMG_0001.jpg")
    }

    /// Re-exporting the same frame must overwrite, not accumulate: the output
    /// name derives from the source basename alone.
    @Test func nameIsStableAcrossOptions() {
        var a = ExportOptions()
        a.jpegQuality = 0.5
        var b = ExportOptions()
        b.jpegQuality = 0.99
        let source = URL(fileURLWithPath: "/roll/IMG_0001.CR3")
        #expect(a.destinationURL(for: source) == b.destinationURL(for: source))
    }
}
