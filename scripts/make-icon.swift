import AppKit

guard CommandLine.arguments.count == 3 else {
    throw NSError(
        domain: "MacAi.Icon", code: 1,
        userInfo: [NSLocalizedDescriptionKey: "Usage: swift make-icon.swift <output.iconset> <shared-icon.png>"]
    )
}

let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let sharedIcon = URL(fileURLWithPath: CommandLine.arguments[2])
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = points * scale
        guard let bitmap = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
        ), let context = NSGraphicsContext(bitmapImageRep: bitmap) else {
            throw NSError(
                domain: "MacAi.Icon", code: 2,
                userInfo: [NSLocalizedDescriptionKey: "Cannot create the icon drawing context"]
            )
        }

        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        context.cgContext.scaleBy(x: CGFloat(pixels) / 1024, y: CGFloat(pixels) / 1024)

        NSGraphicsContext.saveGraphicsState()
        let shadow = NSShadow()
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.16)
        shadow.shadowBlurRadius = 24
        shadow.shadowOffset = NSSize(width: 0, height: -12)
        shadow.set()
        NSColor(srgbRed: 93.0 / 255, green: 145.0 / 255, blue: 181.0 / 255, alpha: 1).setFill()
        NSBezierPath(
            roundedRect: NSRect(x: 80, y: 80, width: 864, height: 864),
            xRadius: 194, yRadius: 194
        ).fill()
        NSGraphicsContext.restoreGraphicsState()

        let star = NSBezierPath()
        let vertices: [(CGFloat, CGFloat)] = [
            (0, 238), (67, 67), (238, 0), (67, -67),
            (0, -238), (-67, -67), (-238, 0), (-67, 67),
        ]
        for (index, vertex) in vertices.enumerated() {
            let point = NSPoint(x: 512 + vertex.0, y: 512 + vertex.1)
            if index == 0 {
                star.move(to: point)
            } else {
                star.line(to: point)
            }
        }
        star.close()
        NSColor(srgbRed: 237.0 / 255, green: 245.0 / 255, blue: 251.0 / 255, alpha: 1).setFill()
        star.fill()
        NSGraphicsContext.restoreGraphicsState()

        guard let png = bitmap.representation(using: .png, properties: [:]) else {
            throw NSError(
                domain: "MacAi.Icon", code: 3,
                userInfo: [NSLocalizedDescriptionKey: "Cannot encode the app icon"]
            )
        }
        let suffix = scale == 2 ? "@2x" : ""
        try png.write(to: output.appendingPathComponent("icon_\(points)x\(points)\(suffix).png"))
        if points == 512 && scale == 1 {
            try png.write(to: sharedIcon)
        }
    }
}
