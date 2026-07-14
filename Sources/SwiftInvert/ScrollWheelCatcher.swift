import AppKit
import SwiftUI

/// Two-finger trackpad panning (Preview-style). SwiftUI has no scroll-wheel
/// gesture, so an invisible NSView installs a local scrollWheel monitor and
/// forwards deltas while the cursor is inside its bounds (momentum events
/// included, so panning keeps Preview's inertial glide). Events over the
/// canvas are consumed; everywhere else they pass through untouched, so the
/// sidebar and library scroll views behave normally.
struct ScrollWheelCatcher: NSViewRepresentable {
    /// (deltaX, deltaY) in points, natural-scrolling direction from AppKit —
    /// content follows the fingers.
    let onScroll: (CGFloat, CGFloat) -> Void

    func makeNSView(context: Context) -> CatcherView {
        let view = CatcherView()
        view.onScroll = onScroll
        return view
    }

    func updateNSView(_ view: CatcherView, context: Context) {
        view.onScroll = onScroll
    }

    final class CatcherView: NSView {
        var onScroll: ((CGFloat, CGFloat) -> Void)?
        private var monitor: Any?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if window == nil {
                if let monitor { NSEvent.removeMonitor(monitor) }
                monitor = nil
            } else if monitor == nil {
                monitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
                    // Monitors fire on the main thread; only Sendable values
                    // cross the isolation boundary (NSEvent/NSWindow are not).
                    let dx = event.scrollingDeltaX
                    let dy = event.scrollingDeltaY
                    let location = event.locationInWindow
                    let eventWindowNumber = event.window?.windowNumber ?? -1
                    let consumed = MainActor.assumeIsolated { () -> Bool in
                        guard let self, let window = self.window,
                            window.windowNumber == eventWindowNumber
                        else { return false }
                        guard self.bounds.contains(self.convert(location, from: nil)) else {
                            return false
                        }
                        self.onScroll?(dx, dy)
                        return true
                    }
                    return consumed ? nil : event
                }
            }
        }
    }
}
