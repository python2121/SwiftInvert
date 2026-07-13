// SwiftInvert app icon generator — run with: swift scripts/generate_icon.swift
// Draws the icon with CoreGraphics (reproducible, no image assets needed):
// a sepia film strip crossing the canvas, with a magnifying glass whose lens
// reveals the same abstract scene in full color — the negative-to-positive
// conversion in one picture.
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

// ── Background: macOS rounded square, warm dark gradient ────────────────────
let inset: CGFloat = 88
let bgRect = CGRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset)
let bgPath = CGPath(roundedRect: bgRect, cornerWidth: 190, cornerHeight: 190, transform: nil)
ctx.saveGState()
ctx.addPath(bgPath)
ctx.clip()
ctx.drawLinearGradient(
    gradient([color(0x3a332a), color(0x171412)]),
    start: CGPoint(x: size / 2, y: size), end: CGPoint(x: size / 2, y: 0), options: [])

// ── Abstract scene (sun over layered hills), sepia or full color ────────────
func drawScene(in rect: CGRect, sepia: Bool) {
    ctx.saveGState()
    ctx.clip(to: rect)
    // Sky
    let sky = sepia
        ? gradient([color(0xd9bb8a), color(0xb08a55)])
        : gradient([color(0x8fd3f4), color(0x3f8fc0)])
    ctx.drawLinearGradient(
        sky, start: CGPoint(x: rect.midX, y: rect.maxY),
        end: CGPoint(x: rect.midX, y: rect.minY), options: [])
    // Sun
    ctx.setFillColor(sepia ? color(0xefdcae) : color(0xffb03a))
    let sunR = rect.width * 0.16
    ctx.fillEllipse(in: CGRect(
        x: rect.minX + rect.width * 0.62 - sunR, y: rect.minY + rect.height * 0.58 - sunR,
        width: sunR * 2, height: sunR * 2))
    // Back hill
    ctx.setFillColor(sepia ? color(0x8a6538) : color(0x7b5ea7))
    ctx.beginPath()
    ctx.move(to: CGPoint(x: rect.minX, y: rect.minY))
    ctx.addQuadCurve(
        to: CGPoint(x: rect.maxX, y: rect.minY + rect.height * 0.30),
        control: CGPoint(x: rect.minX + rect.width * 0.45, y: rect.minY + rect.height * 0.72))
    ctx.addLine(to: CGPoint(x: rect.maxX, y: rect.minY))
    ctx.closePath()
    ctx.fillPath()
    // Front hill
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

// ── Film strip: rotated band with sprocket holes and three sepia frames ─────
ctx.saveGState()
ctx.translateBy(x: size / 2, y: size / 2)
ctx.rotate(by: -0.32)  // ~ -18°
let bandH: CGFloat = 430
let bandRect = CGRect(x: -size, y: -bandH / 2, width: size * 2, height: bandH)
ctx.setFillColor(color(0x211a12))
ctx.fill(bandRect)
// Frames
let frameW: CGFloat = 252, frameH: CGFloat = 262, gap: CGFloat = 30
for i in -2...2 {
    let x = CGFloat(i) * (frameW + gap) - frameW / 2
    let frame = CGRect(x: x, y: -frameH / 2, width: frameW, height: frameH)
    drawScene(in: frame, sepia: true)
    // subtle frame line
    ctx.setStrokeColor(color(0x150f09))
    ctx.setLineWidth(6)
    ctx.stroke(frame)
}
// Sprocket holes
ctx.setFillColor(color(0x0c0a08))
let holeW: CGFloat = 46, holeH: CGFloat = 34
var hx: CGFloat = -size
while hx < size {
    for y in [bandH / 2 - 62, -bandH / 2 + 28] {
        let hole = CGRect(x: hx, y: y, width: holeW, height: holeH)
        ctx.addPath(CGPath(roundedRect: hole, cornerWidth: 9, cornerHeight: 9, transform: nil))
    }
    hx += 94
}
ctx.fillPath()
ctx.restoreGState()

// ── Magnifying glass: lens reveals the scene in color ───────────────────────
let lensCenter = CGPoint(x: 610, y: 470)
let lensR: CGFloat = 235
// Lens interior: the same scene, in color, slightly "magnified"
ctx.saveGState()
ctx.addEllipse(in: CGRect(
    x: lensCenter.x - lensR, y: lensCenter.y - lensR, width: lensR * 2, height: lensR * 2))
ctx.clip()
let magnified = CGRect(
    x: lensCenter.x - lensR * 1.35, y: lensCenter.y - lensR * 1.28,
    width: lensR * 2.7, height: lensR * 2.56)
drawScene(in: magnified, sepia: false)
// glass glare
ctx.setFillColor(color(0xffffff, 0.18))
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
