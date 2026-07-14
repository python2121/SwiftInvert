import NegativeKit
import SwiftUI
import simd

/// The user's desired crop box in rotated-space pixels (screen-aligned axes,
/// origin at the frame center — see CropGeometry).
struct CropBoxValue: Equatable {
    var center: SIMD2<Double>
    var halfExtents: SIMD2<Double>
}

/// Lightroom-style crop editor for the unified Crop & Straighten mode: an
/// axis-aligned box over the image rotating behind it. `box` holds the
/// user's desired box (nil = follow the committed crop); what's shown is the
/// desired box constrained to the current angle, so it auto-scales live as
/// the straighten slider turns the image underneath.
struct CropBoxOverlay: View {
    @Binding var box: CropBoxValue?
    /// Unrotated frame dims, px.
    let frame: SIMD2<Double>
    /// Current (live) straighten angle.
    let radians: Double
    /// Window points per frame pixel.
    let scale: CGFloat
    let committed: NormalizedRect?
    let committedRadians: Double

    @State private var dragStart: CropBoxValue?

    static func defaultBox(
        committed: NormalizedRect?, committedRadians: Double, frame: SIMD2<Double>,
        radians: Double
    ) -> CropBoxValue {
        if let committed {
            let b = CropGeometry.box(from: committed, frame: frame, radians: committedRadians)
            return CropBoxValue(center: b.center, halfExtents: b.halfExtents)
        }
        let ins = CropGeometry.inscribedSize(frame: frame, radians: radians)
        return CropBoxValue(center: .zero, halfExtents: ins / 2)
    }

    private var effective: CropBoxValue {
        let desired = box
            ?? Self.defaultBox(
                committed: committed, committedRadians: committedRadians, frame: frame,
                radians: radians)
        let (c, e) = CropGeometry.constrain(
            center: desired.center, halfExtents: desired.halfExtents, radians: radians,
            frame: frame)
        return CropBoxValue(center: c, halfExtents: e)
    }

    var body: some View {
        GeometryReader { geo in
            let winCenter = CGPoint(x: geo.size.width / 2, y: geo.size.height / 2)
            let eff = effective
            let boxRect = CGRect(
                x: winCenter.x + CGFloat(eff.center.x - eff.halfExtents.x) * scale,
                y: winCenter.y + CGFloat(eff.center.y - eff.halfExtents.y) * scale,
                width: CGFloat(eff.halfExtents.x * 2) * scale,
                height: CGFloat(eff.halfExtents.y * 2) * scale)

            ZStack(alignment: .topLeading) {
                // Dim outside the box (even-odd).
                Path { p in
                    p.addRect(CGRect(origin: .zero, size: geo.size))
                    p.addRect(boxRect)
                }
                .fill(Color.black.opacity(0.45), style: FillStyle(eoFill: true))
                .allowsHitTesting(false)

                // Border + thirds (axis-aligned with the box, like Lightroom).
                Path { p in
                    p.addRect(boxRect)
                    for i in 1..<3 {
                        let x = boxRect.minX + boxRect.width * CGFloat(i) / 3
                        p.move(to: CGPoint(x: x, y: boxRect.minY))
                        p.addLine(to: CGPoint(x: x, y: boxRect.maxY))
                        let y = boxRect.minY + boxRect.height * CGFloat(i) / 3
                        p.move(to: CGPoint(x: boxRect.minX, y: y))
                        p.addLine(to: CGPoint(x: boxRect.maxX, y: y))
                    }
                }
                .stroke(.white.opacity(0.9), lineWidth: 1)
                .allowsHitTesting(false)

                // Interior: move (clamped along the drag so the box never
                // leaves the rotated frame and never shrinks while moving).
                Rectangle()
                    .fill(.clear)
                    .frame(width: max(boxRect.width, 1), height: max(boxRect.height, 1))
                    .offset(x: boxRect.minX, y: boxRect.minY)
                    .contentShape(Rectangle())
                    .gesture(moveGesture)

                // Corner handles (resize about the opposite corner).
                ForEach(0..<4, id: \.self) { i in
                    let sx: Double = i % 2 == 0 ? -1 : 1
                    let sy: Double = i < 2 ? -1 : 1
                    handleView
                        .position(
                            x: winCenter.x + CGFloat(eff.center.x + sx * eff.halfExtents.x) * scale,
                            y: winCenter.y + CGFloat(eff.center.y + sy * eff.halfExtents.y) * scale)
                        .gesture(cornerGesture(sx: sx, sy: sy, winCenter: winCenter))
                }
            }
        }
    }

    private var handleView: some View {
        ZStack {
            Circle()
                .fill(.white)
                .frame(width: 11, height: 11)
                .shadow(radius: 1, y: 0.5)
        }
        .frame(width: 30, height: 30)
        .contentShape(Rectangle())
    }

    private var moveGesture: some Gesture {
        DragGesture(minimumDistance: 1)
            .onChanged { g in
                if dragStart == nil { dragStart = effective }
                guard let start = dragStart else { return }
                let d = SIMD2(Double(g.translation.width / scale), Double(g.translation.height / scale))
                var candidate = start.center + d
                if !CropGeometry.boxFits(
                    center: candidate, halfExtents: start.halfExtents, radians: radians,
                    frame: frame)
                {
                    // Largest fraction of the translation that still fits.
                    var lo = 0.0
                    var hi = 1.0
                    for _ in 0..<20 {
                        let mid = (lo + hi) / 2
                        if CropGeometry.boxFits(
                            center: start.center + d * mid, halfExtents: start.halfExtents,
                            radians: radians, frame: frame)
                        {
                            lo = mid
                        } else {
                            hi = mid
                        }
                    }
                    candidate = start.center + d * lo
                }
                box = CropBoxValue(center: candidate, halfExtents: start.halfExtents)
            }
            .onEnded { _ in dragStart = nil }
    }

    private func cornerGesture(sx: Double, sy: Double, winCenter: CGPoint) -> some Gesture {
        DragGesture(minimumDistance: 1)
            .onChanged { g in
                if dragStart == nil { dragStart = effective }
                guard let start = dragStart else { return }
                let anchor = start.center - SIMD2(sx * start.halfExtents.x, sy * start.halfExtents.y)
                var p = SIMD2(
                    Double((g.location.x - winCenter.x) / scale),
                    Double((g.location.y - winCenter.y) / scale))
                let minSize = 24.0 / Double(scale)
                if sx > 0 { p.x = max(p.x, anchor.x + minSize) } else { p.x = min(p.x, anchor.x - minSize) }
                if sy > 0 { p.y = max(p.y, anchor.y + minSize) } else { p.y = min(p.y, anchor.y - minSize) }
                box = CropBoxValue(
                    center: (anchor + p) / 2,
                    halfExtents: SIMD2(abs(p.x - anchor.x), abs(p.y - anchor.y)) / 2)
            }
            .onEnded { _ in
                dragStart = nil
                // Snap the stored box to what's actually representable.
                if let b = box {
                    let (c, e) = CropGeometry.constrain(
                        center: b.center, halfExtents: b.halfExtents, radians: radians,
                        frame: frame)
                    box = CropBoxValue(center: c, halfExtents: e)
                }
            }
    }
}
