import AppKit
import Foundation

let destination = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
for (size, suffix) in [(16, "16x16"), (32, "16x16@2x"), (32, "32x32"),
    (64, "32x32@2x"), (128, "128x128"), (256, "128x128@2x"),
    (256, "256x256"), (512, "256x256@2x"), (512, "512x512"), (1024, "512x512@2x")] {
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    let scale = CGFloat(size) / 1024
    let transform = NSAffineTransform(); transform.scale(by: scale); transform.concat()
    let background = NSBezierPath(roundedRect: NSRect(x: 56, y: 56, width: 912, height: 912), xRadius: 208, yRadius: 208)
    NSColor(calibratedRed: 0.07, green: 0.13, blue: 0.22, alpha: 1).setFill(); background.fill()
    let fuse = NSBezierPath()
    fuse.move(to: NSPoint(x: 565, y: 630))
    fuse.curve(to: NSPoint(x: 740, y: 735), controlPoint1: NSPoint(x: 590, y: 820), controlPoint2: NSPoint(x: 690, y: 800))
    fuse.lineWidth = 38; fuse.lineCapStyle = .round
    NSColor(calibratedRed: 1, green: 0.75, blue: 0.2, alpha: 1).setStroke(); fuse.stroke()
    let bomb = NSBezierPath(ovalIn: NSRect(x: 230, y: 190, width: 530, height: 530))
    NSColor(calibratedRed: 0.15, green: 0.78, blue: 0.72, alpha: 1).setFill(); bomb.fill()
    NSColor(calibratedRed: 0.05, green: 0.1, blue: 0.17, alpha: 1).setFill()
    NSBezierPath(ovalIn: NSRect(x: 284, y: 242, width: 422, height: 422)).fill()
    NSColor(calibratedWhite: 1, alpha: 0.8).setFill()
    NSBezierPath(ovalIn: NSRect(x: 326, y: 494, width: 96, height: 96)).fill()
    NSColor(calibratedRed: 1, green: 0.48, blue: 0.18, alpha: 1).setFill()
    NSBezierPath(ovalIn: NSRect(x: 701, y: 700, width: 86, height: 86)).fill()
    NSGraphicsContext.restoreGraphicsState()
    try bitmap.representation(using: .png, properties: [:])!.write(to: destination.appendingPathComponent("icon_\(suffix).png"))
}
