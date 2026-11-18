#!/usr/bin/env swift

// Draws the DMG window background at 1x and 2x: a plain light field, a thin
// arrow from the app icon to Applications and one line of instruction.
// Finder shows the picture unchanged in light and dark mode, so it stays light
// and keeps its own dark text. scripts/make-macos-dmg.sh passes the layout.
import CoreGraphics
import CoreText
import Foundation
import ImageIO
import UniformTypeIdentifiers

let usage = "Usage: dmg-background.swift OUTPUT_DIR WIDTH HEIGHT ICON_SIZE ICON_Y LEFT_X RIGHT_X CAPTION_Y"
let arguments = CommandLine.arguments
guard arguments.count == 9 else { fatalError(usage) }
let outputDirectory = URL(fileURLWithPath: arguments[1], isDirectory: true)
let numbers = arguments[2...].map { Double($0) }
guard numbers.allSatisfy({ $0 != nil }) else { fatalError(usage) }
let values = numbers.map { CGFloat($0!) }
let (width, height, iconSize, iconY, leftX, rightX, captionY) =
    (values[0], values[1], values[2], values[3], values[4], values[5], values[6])
let caption = "Drag Email to Applications"

let space = CGColorSpace(name: CGColorSpace.sRGB)!
func gray(_ white: CGFloat) -> CGColor { CGColor(colorSpace: space, components: [white, white, white, 1])! }
let fieldColor = gray(0.965)
let arrowColor = gray(0.62)
let captionColor = gray(0.45)

func render(scale: Int, to url: URL) {
    let pixelWidth = Int(width) * scale, pixelHeight = Int(height) * scale
    guard let context = CGContext(data: nil, width: pixelWidth, height: pixelHeight,
        bitsPerComponent: 8, bytesPerRow: 0, space: space,
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else {
        fatalError("Could not create background canvas")
    }
    context.scaleBy(x: CGFloat(scale), y: CGFloat(scale))
    context.setFillColor(fieldColor)
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))

    // Layout positions are measured from the top, as Finder places icons.
    let y = height - iconY
    let gap: CGFloat = 34
    let start = leftX + iconSize / 2 + gap, end = rightX - iconSize / 2 - gap
    let head: CGFloat = 7
    context.setStrokeColor(arrowColor)
    context.setLineWidth(1.5)
    context.setLineCap(.round)
    context.setLineJoin(.round)
    context.move(to: CGPoint(x: start, y: y))
    context.addLine(to: CGPoint(x: end, y: y))
    context.move(to: CGPoint(x: end - head, y: y + head))
    context.addLine(to: CGPoint(x: end, y: y))
    context.addLine(to: CGPoint(x: end - head, y: y - head))
    context.strokePath()

    let font = CTFontCreateUIFontForLanguage(.system, 13, nil)!
    let text = NSAttributedString(string: caption, attributes: [
        NSAttributedString.Key(kCTFontAttributeName as String): font,
        NSAttributedString.Key(kCTForegroundColorAttributeName as String): captionColor,
    ])
    let line = CTLineCreateWithAttributedString(text)
    let bounds = CTLineGetBoundsWithOptions(line, .useOpticalBounds)
    context.textPosition = CGPoint(x: (width - bounds.width) / 2 - bounds.minX,
                                   y: height - captionY - bounds.height / 2 - bounds.minY)
    CTLineDraw(line, context)

    guard let image = context.makeImage(),
          let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        fatalError("Could not create \(url.path)")
    }
    // The DPI tells Finder that the 2x image covers the same points as the 1x.
    let dpi = 72 * scale
    CGImageDestinationAddImage(destination, image, [
        kCGImagePropertyDPIWidth: dpi, kCGImagePropertyDPIHeight: dpi,
    ] as CFDictionary)
    guard CGImageDestinationFinalize(destination) else { fatalError("Could not save \(url.path)") }
}

render(scale: 1, to: outputDirectory.appendingPathComponent("background.png"))
render(scale: 2, to: outputDirectory.appendingPathComponent("background@2x.png"))
