import CoreGraphics
import Foundation
import Testing

@testable import SwiftInvert

/// The HQ preview control is three-state: Off (always the 1536px proxy), Auto
/// (proxy when fitted, full resolution once magnified past the threshold, swapped
/// in behind the scenes) and On (always full resolution). `HQMode.resolve` is the
/// pure decision behind it — everything else about the feature is timing and
/// pixels, but this is the part that can be wrong silently.
@Suite struct HQModeTests {

    private let threshold = AppModel.hqAutoZoomThreshold

    private func resolve(
        _ mode: AppModel.HQMode,
        zoom: CGFloat = 1,
        hasSelection: Bool = true,
        proxyTool: Bool = false,
        cropTool: Bool = false
    ) -> Bool {
        mode.resolve(
            zoom: zoom, threshold: threshold, hasSelection: hasSelection,
            canvasOwnedByProxyTool: proxyTool, cropToolActive: cropTool)
    }

    // MARK: - The three modes

    @Test func offNeverUsesFullResolution() {
        for zoom: CGFloat in [0.5, 1, 1.99, 2, 4, 8] {
            #expect(resolve(.off, zoom: zoom) == false, "zoom \(zoom)")
        }
    }

    @Test func onAlwaysUsesFullResolutionRegardlessOfZoom() {
        for zoom: CGFloat in [0.5, 1, 1.99, 2, 4, 8] {
            #expect(resolve(.on, zoom: zoom) == true, "zoom \(zoom)")
        }
        // Even pulled back past fit — On means on.
        #expect(resolve(.on, zoom: 0.5) == true)
    }

    @Test func autoFollowsZoomAcrossTheThreshold() {
        #expect(resolve(.auto, zoom: 1) == false)
        #expect(resolve(.auto, zoom: threshold - 0.01) == false)
        // "2x AND GREATER" — the threshold itself must engage, not just exceed.
        #expect(resolve(.auto, zoom: threshold) == true)
        #expect(resolve(.auto, zoom: threshold + 0.01) == true)
        #expect(resolve(.auto, zoom: 8) == true)
    }

    @Test func modeOrderAndThresholdAreSane() {
        // Order matters: the menu picker and the badge both cycle in this order.
        #expect(AppModel.HQMode.allCases == [.off, .auto, .on])
        // Must sit above fit (or Auto is just On) and within the gesture's range.
        #expect(threshold > 1 && threshold <= 8)
    }

    @MainActor
    @Test func autoIsTheLaunchDefault() {
        // `.on` would make every launch pay full-res costs; `.off` would make
        // the feature invisible to anyone who never finds the badge.
        #expect(AppModel().hqMode == .auto)
    }

    // MARK: - Cycling

    @Test func clickingCyclesOffAutoOnAndWrapsAround() {
        #expect(AppModel.HQMode.off.next == .auto)
        #expect(AppModel.HQMode.auto.next == .on)
        #expect(AppModel.HQMode.on.next == .off)
        // Three clicks return to the start, from any state.
        for mode in AppModel.HQMode.allCases {
            #expect(mode.next.next.next == mode)
        }
    }

    // MARK: - Contexts that suppress HQ

    /// Test strip and zone placement render from the proxy tower by
    /// construction; showing an HQ badge over them would lie about what is on
    /// screen. This replaces the old boolean's explicit force-off.
    @Test func proxyOwnedCanvasSuppressesEveryMode() {
        for mode in AppModel.HQMode.allCases {
            #expect(resolve(mode, zoom: 8, proxyTool: true) == false, "\(mode)")
        }
    }

    /// Crop & Straighten pins the canvas to fit whatever the zoom state holds,
    /// so a stale magnification must not trigger a decode there — but an
    /// explicit On is the user's call and still applies.
    @Test func cropToolSuppressesAutoButNotOn() {
        #expect(resolve(.auto, zoom: 8, cropTool: true) == false)
        #expect(resolve(.on, zoom: 8, cropTool: true) == true)
        #expect(resolve(.off, zoom: 8, cropTool: true) == false)
    }

    @Test func noSelectionNeverRenders() {
        for mode in AppModel.HQMode.allCases {
            #expect(resolve(mode, zoom: 8, hasSelection: false) == false, "\(mode)")
        }
    }

    // MARK: - Model wiring

    /// `hqActive` is what the render path reads. With no image open it must be
    /// false in every mode, so cycling the badge on an empty canvas can't queue
    /// a full-resolution decode.
    @MainActor
    @Test func modelStaysInactiveWithoutAnImage() {
        let model = AppModel()
        for mode in AppModel.HQMode.allCases {
            model.hqMode = mode
            #expect(model.hqActive == false, "\(mode)")
        }
    }

    /// Zoom is reported from the view; the model should accept it without an
    /// image and still resolve to inactive.
    @MainActor
    @Test func reportingZoomWithoutAnImageIsInert() {
        let model = AppModel()
        model.hqMode = .auto
        model.canvasZoom = 8
        #expect(model.canvasZoom == 8)
        #expect(model.hqActive == false)
    }
}
