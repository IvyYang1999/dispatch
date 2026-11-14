// Renders the Dispatch mark and icons with Core Graphics, from one geometry (svg_mark.py
// writes the same geometry as SVG; change both together). make-icons.py runs this and puts
// every file in its place.
// usage: swift render.swift <out-dir>
// Writes: mark-1024.png (black on transparent), icon-mac-<n>.png (Apple grid: 824 tile in
// 1024, transparent margin, soft shadow), icon-full-<n>.png (tile to the edges, transparent
// corners), tray.png (the menu bar's template image, 44 x 36).
import AppKit
import CoreGraphics
import Foundation

let out = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ".")

// ---- geometry, in a 1024 box, y down ----
// The original mark (2026-11-04), kept as drawn: the D's bars run through the bowl, cross at
// its rightmost point and sweep out to the right; the heads are plain triangles whose tips sit
// on the D's top and bottom lines. A redrawing with rounded heads pointing nearly right was tried
// in 0.1.17 and taken back: the original reads better.
let W: CGFloat = 100            // stroke
let shiftX: CGFloat = -50       // centres the mark (the D is narrower on the left than the arrows on the right)

func lowerStem() -> CGMutablePath {
    let p = CGMutablePath()
    p.move(to: CGPoint(x: 270, y: 734))
    p.addLine(to: CGPoint(x: 270, y: 290))
    p.addLine(to: CGPoint(x: 380, y: 290))
    p.addCurve(to: CGPoint(x: 610, y: 512), control1: CGPoint(x: 530, y: 290), control2: CGPoint(x: 575, y: 420))
    p.addCurve(to: CGPoint(x: 780, y: 664), control1: CGPoint(x: 625, y: 549), control2: CGPoint(x: 728, y: 634))
    return p
}

func lowerHead() -> CGMutablePath {
    let p = CGMutablePath()
    p.move(to: CGPoint(x: 840, y: 560))
    p.addLine(to: CGPoint(x: 901, y: 734))
    p.addLine(to: CGPoint(x: 720, y: 768))
    p.closeSubpath()
    return p
}

func mirrored(_ p: CGPath) -> CGPath {
    var t = CGAffineTransform(a: 1, b: 0, c: 0, d: -1, tx: 0, ty: 1024)
    return p.copy(using: &t)!
}

/// The mark in a 1024 box (y down) drawn into `ctx` with the current fill/stroke colour.
func drawMark(_ ctx: CGContext, color: CGColor) {
    ctx.saveGState()
    ctx.translateBy(x: shiftX, y: 0)
    ctx.setStrokeColor(color)
    ctx.setFillColor(color)
    ctx.setLineWidth(W)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    for stem in [lowerStem() as CGPath, mirrored(lowerStem())] {
        ctx.addPath(stem)
        ctx.strokePath()
    }
    for head in [lowerHead() as CGPath, mirrored(lowerHead())] {
        ctx.addPath(head)
        ctx.fillPath()
    }
    ctx.restoreGState()
}

/// A rounded square. macOS 11 to 15 show the icon as drawn, so this is the shape they see;
/// macOS 26 and later put every icon in their own mask anyway.
func tile(_ r: CGRect, radius: CGFloat) -> CGPath {
    CGPath(roundedRect: r, cornerWidth: radius, cornerHeight: radius, transform: nil)
}

func context(_ size: Int) -> CGContext {
    let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high
    // y down, in 1024 units
    ctx.translateBy(x: 0, y: CGFloat(size))
    ctx.scaleBy(x: CGFloat(size) / 1024, y: -CGFloat(size) / 1024)
    return ctx
}

func save(_ ctx: CGContext, _ name: String) {
    let img = ctx.makeImage()!
    let rep = NSBitmapImageRep(cgImage: img)
    try! rep.representation(using: .png, properties: [:])!.write(to: out.appendingPathComponent(name))
}

let black = CGColor(srgbRed: 0x11 / 255.0, green: 0x11 / 255.0, blue: 0x11 / 255.0, alpha: 1)
let white = CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 1)

/// The icon on Apple's grid: an 824 tile centred in 1024, a soft shadow below it.
func macIcon(_ size: Int) -> CGContext {
    let ctx = context(size)
    let r = CGRect(x: 100, y: 100, width: 824, height: 824)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10 * CGFloat(size) / 1024), blur: 20 * CGFloat(size) / 1024, color: CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 0.30))
    ctx.addPath(tile(r, radius: 185.4))
    ctx.setFillColor(black)
    ctx.fillPath()
    ctx.restoreGState()
    ctx.saveGState()
    ctx.translateBy(x: 100, y: 100)
    ctx.scaleBy(x: 824 / 1024, y: 824 / 1024)
    drawMark(ctx, color: white)
    ctx.restoreGState()
    return ctx
}

/// The tile to the edges, corners transparent: Windows, Linux, the web page.
func fullIcon(_ size: Int) -> CGContext {
    let ctx = context(size)
    ctx.addPath(tile(CGRect(x: 0, y: 0, width: 1024, height: 1024), radius: 230))
    ctx.setFillColor(black)
    ctx.fillPath()
    drawMark(ctx, color: white)
    return ctx
}

let fm = FileManager.default
try? fm.createDirectory(at: out, withIntermediateDirectories: true)

let mark = context(1024)
drawMark(mark, color: black)
save(mark, "mark-1024.png")

for n in [16, 32, 64, 128, 256, 512, 1024] {
    save(macIcon(n), "icon-mac-\(n).png")
}
for n in [16, 24, 30, 32, 44, 48, 50, 64, 71, 89, 107, 128, 142, 150, 256, 284, 310, 512, 1024] {
    save(fullIcon(n), "icon-full-\(n).png")
}
/// Everything the mark covers, in the 1024 box.
func markBounds() -> CGRect {
    var r = CGRect.null
    for stem in [lowerStem() as CGPath, mirrored(lowerStem())] {
        r = r.union(stem.copy(strokingWithWidth: W, lineCap: .round, lineJoin: .round, miterLimit: 10).boundingBoxOfPath)
    }
    for head in [lowerHead() as CGPath, mirrored(lowerHead())] {
        r = r.union(head.boundingBoxOfPath)
    }
    return r.offsetBy(dx: shiftX, dy: 0)
}

// the menu bar's template image: 44 x 36 px (22 x 18 pt at 2x), the mark filling its height
do {
    let (w, h) = (44, 36)
    let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    let b = markBounds()
    let scale = min((CGFloat(w) - 2) / b.width, (CGFloat(h) - 2) / b.height)
    ctx.translateBy(x: 0, y: CGFloat(h))
    ctx.scaleBy(x: 1, y: -1)
    ctx.translateBy(x: (CGFloat(w) - b.width * scale) / 2, y: (CGFloat(h) - b.height * scale) / 2)
    ctx.scaleBy(x: scale, y: scale)
    ctx.translateBy(x: -b.minX, y: -b.minY)
    drawMark(ctx, color: CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 1))
    save(ctx, "tray.png")
}
let b = markBounds()
print("mark bounds", Int(b.minX), Int(b.minY), Int(b.maxX), Int(b.maxY))
print("rendered into \(out.path)")
