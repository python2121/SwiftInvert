// SwiftInvert app icon generator — run with: swift scripts/generate_icon.swift
// Draws the icon with CoreGraphics (reproducible, no image assets needed):
// a sepia film strip crossing the canvas, and a magnifying glass showing the
// SAME strip magnified — perforations and all — with the frame content in
// full color: the negative-to-positive conversion in one picture.
// v1 (color scene only inside the lens, no perforations) is preserved as
// Assets/icon_1024_v1.png / SwiftInvert_v1.icns.
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let size: CGFloat = 1024
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(
    data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0,
    space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
        green: CGFloat((hex >> 8) & 0xFF) / 255,
        blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}

func gradient(_ colors: [CGColor]) -> CGGradient {
    CGGradient(colorsSpace: cs, colors: colors as CFArray, locations: nil)!
}

// ── Abstract scene (sun over layered hills), sepia or full color ────────────
func drawScene(in rect: CGRect, sepia: Bool) {
    ctx.saveGState()
    ctx.clip(to: rect)
    let sky = sepia
        ? gradient([color(0xd9bb8a), color(0xb08a55)])
        : gradient([color(0x8fd3f4), color(0x3f8fc0)])
    ctx.drawLinearGradient(
        sky, start: CGPoint(x: rect.midX, y: rect.maxY),
        end: CGPoint(x: rect.midX, y: rect.minY), options: [])
    ctx.setFillColor(sepia ? color(0xefdcae) : color(0xffb03a))
    let sunR = rect.width * 0.16
    ctx.fillEllipse(in: CGRect(
        x: rect.minX + rect.width * 0.62 - sunR, y: rect.minY + rect.height * 0.58 - sunR,
        width: sunR * 2, height: sunR * 2))
    ctx.setFillColor(sepia ? color(0x8a6538) : color(0x7b5ea7))
    ctx.beginPath()
    ctx.move(to: CGPoint(x: rect.minX, y: rect.minY))
    ctx.addQuadCurve(
        to: CGPoint(x: rect.maxX, y: rect.minY + rect.height * 0.30),
        control: CGPoint(x: rect.minX + rect.width * 0.45, y: rect.minY + rect.height * 0.72))
    ctx.addLine(to: CGPoint(x: rect.maxX, y: rect.minY))
    ctx.closePath()
    ctx.fillPath()
    ctx.setFillColor(sepia ? color(0x5f4224) : color(0x2e8b57))
    ctx.beginPath()
    ctx.move(to: CGPoint(x: rect.minX, y: rect.minY))
    ctx.addQuadCurve(
        to: CGPoint(x: rect.maxX, y: rect.minY),
        control: CGPoint(x: rect.minX + rect.width * 0.68, y: rect.minY + rect.height * 0.52))
    ctx.closePath()
    ctx.fillPath()
    ctx.restoreGState()
}

// ── Film strip (band + frames + classic perforations), drawn in the current
//    CTM. Called twice: canvas pass (sepia) and lens pass (color frames). ────
func drawStrip(colorFrames: Bool) {
    let bandH: CGFloat = 430
    // The strip is drawn in its own transparency layer so the perforations can
    // be punched out with .clear — real holes showing the background through,
    // whatever the background is.
    ctx.beginTransparencyLayer(auxiliaryInfo: nil)
    ctx.setFillColor(color(0x211a12))
    ctx.fill(CGRect(x: -size * 1.6, y: -bandH / 2, width: size * 3.2, height: bandH))

    // Phase offset places one frame's centre in strip-space under the lens
    // centre (lens (610,470) → +106 along the strip axis), so the lens shows a
    // whole color frame instead of straddling a boundary.
    let frameW: CGFloat = 252, frameH: CGFloat = 226, gap: CGFloat = 30
    let phase: CGFloat = 106
    for i in -3...3 {
        let x = CGFloat(i) * (frameW + gap) + phase - frameW / 2
        let frame = CGRect(x: x, y: -frameH / 2, width: frameW, height: frameH)
        drawScene(in: frame, sepia: !colorFrames)
        ctx.setStrokeColor(color(0x150f09))
        ctx.setLineWidth(6)
        ctx.stroke(frame)
    }

    // Classic perforations: two rows of rounded holes punched clean through
    // the film (cleared to transparent → the background shows through), with a
    // subtle dark rim so the cut edge reads.
    let holeW: CGFloat = 46, holeH: CGFloat = 40
    for yCenter in [bandH / 2 - 55, -bandH / 2 + 55] {
        var hx: CGFloat = -size * 1.6
        while hx < size * 1.6 {
            let hole = CGRect(x: hx, y: yCenter - holeH / 2, width: holeW, height: holeH)
            let path = CGPath(roundedRect: hole, cornerWidth: 10, cornerHeight: 10, transform: nil)
            ctx.setBlendMode(.clear)
            ctx.addPath(path)
            ctx.fillPath()
            ctx.setBlendMode(.normal)
            ctx.setStrokeColor(color(0x0c0a08, 0.85))
            ctx.setLineWidth(4)
            ctx.addPath(path)
            ctx.strokePath()
            hx += 94
        }
    }
    ctx.endTransparencyLayer()
}

let stripAngle: CGFloat = -0.32  // ~ -18°

// ── Background: macOS rounded square, warm dark gradient ────────────────────
let inset: CGFloat = 88
let bgRect = CGRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset)
let bgPath = CGPath(roundedRect: bgRect, cornerWidth: 190, cornerHeight: 190, transform: nil)
ctx.saveGState()
ctx.addPath(bgPath)
ctx.clip()

func drawBackgroundGradient() {
    ctx.drawLinearGradient(
        gradient([color(0xe8e8ec), color(0xc2c2c8)]),
        start: CGPoint(x: size / 2, y: size), end: CGPoint(x: size / 2, y: 0), options: [])
}
drawBackgroundGradient()

// ── Pass 1: the sepia strip across the canvas ───────────────────────────────
ctx.saveGState()
ctx.translateBy(x: size / 2, y: size / 2)
ctx.rotate(by: stripAngle)
drawStrip(colorFrames: false)
ctx.restoreGState()

// ── Pass 2: the magnified strip inside the lens, frames in color ────────────
let lensCenter = CGPoint(x: 610, y: 470)
let lensR: CGFloat = 235
let magnification: CGFloat = 1.32

ctx.saveGState()
ctx.addEllipse(in: CGRect(
    x: lensCenter.x - lensR, y: lensCenter.y - lensR, width: lensR * 2, height: lensR * 2))
ctx.clip()
drawBackgroundGradient()
// Magnify about the lens center so the strip stays continuous through the glass.
ctx.translateBy(x: lensCenter.x, y: lensCenter.y)
ctx.scaleBy(x: magnification, y: magnification)
ctx.translateBy(x: -lensCenter.x, y: -lensCenter.y)
ctx.translateBy(x: size / 2, y: size / 2)
ctx.rotate(by: stripAngle)
drawStrip(colorFrames: true)
ctx.restoreGState()

// Glass glare (drawn over the magnified strip, clipped to the lens).
ctx.saveGState()
ctx.addEllipse(in: CGRect(
    x: lensCenter.x - lensR, y: lensCenter.y - lensR, width: lensR * 2, height: lensR * 2))
ctx.clip()
ctx.setFillColor(color(0xffffff, 0.15))
ctx.beginPath()
ctx.addEllipse(in: CGRect(
    x: lensCenter.x - lensR * 0.78, y: lensCenter.y + lensR * 0.10,
    width: lensR * 1.1, height: lensR * 0.72))
ctx.fillPath()
ctx.restoreGState()

// Handle (before ring so the ring overlaps its joint) — 45° to bottom-right.
ctx.saveGState()
ctx.translateBy(x: lensCenter.x, y: lensCenter.y)
ctx.rotate(by: -.pi / 4)
let handle = CGRect(x: lensR - 10, y: -46, width: 300, height: 92)
ctx.addPath(CGPath(roundedRect: handle, cornerWidth: 46, cornerHeight: 46, transform: nil))
ctx.clip()
ctx.drawLinearGradient(
    gradient([color(0x4a4a52), color(0x232328)]),
    start: CGPoint(x: lensR, y: 46), end: CGPoint(x: lensR, y: -46), options: [])
ctx.restoreGState()

// Ring (metallic)
for (radius, width, colors) in [
    (lensR + 20, CGFloat(40), [color(0xe4e4ea), color(0x87878f)]),
    (lensR + 1, CGFloat(10), [color(0x55555c), color(0x3a3a40)]),
] as [(CGFloat, CGFloat, [CGColor])] {
    ctx.saveGState()
    ctx.addEllipse(in: CGRect(
        x: lensCenter.x - radius, y: lensCenter.y - radius, width: radius * 2, height: radius * 2))
    ctx.addEllipse(in: CGRect(
        x: lensCenter.x - radius + width, y: lensCenter.y - radius + width,
        width: (radius - width) * 2, height: (radius - width) * 2))
    ctx.clip(using: .evenOdd)
    ctx.drawLinearGradient(
        gradient(colors),
        start: CGPoint(x: lensCenter.x, y: lensCenter.y + radius),
        end: CGPoint(x: lensCenter.x, y: lensCenter.y - radius), options: [])
    ctx.restoreGState()
}

ctx.restoreGState()  // background clip

// ── Write PNG ────────────────────────────────────────────────────────────────
let out = URL(fileURLWithPath: "Assets/icon_1024.png")
try? FileManager.default.createDirectory(
    at: out.deletingLastPathComponent(), withIntermediateDirectories: true)
let image = ctx.makeImage()!
let dest = CGImageDestinationCreateWithURL(out as CFURL, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(dest, image, nil)
CGImageDestinationFinalize(dest)
print("wrote \(out.path)")
