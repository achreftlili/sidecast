// Draws the Sidecast app icon into an .iconset directory (for `iconutil`).
// Usage: make-icon <output.iconset>
// A display with a landscape phone in front of it, on an indigo→teal tile.

import AppKit

func drawIcon(in ctx: CGContext, size: CGFloat) {
    ctx.saveGState()
    ctx.scaleBy(x: size / 1024, y: size / 1024)

    // Tile (macOS icon grid: 824 pt body, centred).
    let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
    let tilePath = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 24, color: NSColor.black.withAlphaComponent(0.35).cgColor)
    ctx.addPath(tilePath)
    ctx.setFillColor(NSColor.black.cgColor)
    ctx.fillPath()
    ctx.restoreGState()

    ctx.saveGState()
    ctx.addPath(tilePath)
    ctx.clip()
    let colors = [NSColor(red: 0.20, green: 0.22, blue: 0.62, alpha: 1).cgColor,
                  NSColor(red: 0.07, green: 0.62, blue: 0.70, alpha: 1).cgColor] as CFArray
    let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1])!
    ctx.drawLinearGradient(gradient, start: CGPoint(x: 100, y: 924), end: CGPoint(x: 924, y: 100), options: [])
    ctx.restoreGState()

    let white = NSColor.white.cgColor

    // Display (outline) with a stand, back left.
    let screen = CGRect(x: 205, y: 395, width: 470, height: 320)
    ctx.addPath(CGPath(roundedRect: screen, cornerWidth: 34, cornerHeight: 34, transform: nil))
    ctx.setStrokeColor(white)
    ctx.setLineWidth(30)
    ctx.strokePath()
    ctx.setFillColor(white)
    ctx.fill(CGRect(x: 420, y: 320, width: 40, height: 75))
    ctx.addPath(CGPath(roundedRect: CGRect(x: 345, y: 295, width: 190, height: 34),
                       cornerWidth: 17, cornerHeight: 17, transform: nil))
    ctx.fillPath()

    // "Cast" signal arcs above the phone's right end.
    ctx.setLineCap(.round)
    ctx.setLineWidth(24)
    ctx.setStrokeColor(NSColor.white.withAlphaComponent(0.85).cgColor)
    for r in [62.0, 112.0] {
        ctx.addArc(center: CGPoint(x: 790, y: 425), radius: r, startAngle: 0.35, endAngle: 1.22, clockwise: false)
        ctx.strokePath()
    }

    // Landscape phone, front right, with a tinted screen.
    let phone = CGRect(x: 520, y: 215, width: 330, height: 180)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -6), blur: 18, color: NSColor.black.withAlphaComponent(0.30).cgColor)
    ctx.addPath(CGPath(roundedRect: phone, cornerWidth: 38, cornerHeight: 38, transform: nil))
    ctx.setFillColor(white)
    ctx.fillPath()
    ctx.restoreGState()
    let phoneScreen = phone.insetBy(dx: 26, dy: 20)
    ctx.addPath(CGPath(roundedRect: phoneScreen, cornerWidth: 18, cornerHeight: 18, transform: nil))
    ctx.setFillColor(NSColor(red: 0.13, green: 0.40, blue: 0.68, alpha: 1).cgColor)
    ctx.fillPath()

    ctx.restoreGState()
}

func writePNG(pixels: Int, to url: URL) throws {
    guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
                                     bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                     colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
          let gc = NSGraphicsContext(bitmapImageRep: rep) else {
        throw NSError(domain: "make-icon", code: 1)
    }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = gc
    gc.cgContext.clear(CGRect(x: 0, y: 0, width: pixels, height: pixels))
    gc.cgContext.interpolationQuality = .high
    drawIcon(in: gc.cgContext, size: CGFloat(pixels))
    NSGraphicsContext.restoreGraphicsState()
    guard let png = rep.representation(using: .png, properties: [:]) else { throw NSError(domain: "make-icon", code: 2) }
    try png.write(to: url)
}

let args = CommandLine.arguments
guard args.count == 2 else {
    FileHandle.standardError.write(Data("usage: make-icon <output.iconset>\n".utf8))
    exit(2)
}
let out = URL(fileURLWithPath: args[1], isDirectory: true)
try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
for base in [16, 32, 128, 256, 512] {
    try writePNG(pixels: base, to: out.appendingPathComponent("icon_\(base)x\(base).png"))
    try writePNG(pixels: base * 2, to: out.appendingPathComponent("icon_\(base)x\(base)@2x.png"))
}
