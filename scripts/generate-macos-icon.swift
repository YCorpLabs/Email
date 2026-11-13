#!/usr/bin/env swift

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

// Share the approved artwork, but export macOS-specific transparent framing.
guard CommandLine.arguments.count == 2 || CommandLine.arguments.count == 3 else {
    fatalError("Usage: generate-macos-icon.swift output.png [source.png]")
}
let repoRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let sourceURL = CommandLine.arguments.count == 3
    ? URL(fileURLWithPath: CommandLine.arguments[2])
    : repoRoot.appendingPathComponent("assets/email-icon.png")
guard let source = CGImageSourceCreateWithURL(sourceURL as CFURL, nil),
      let artwork = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
    fatalError("Could not load approved icon at \(sourceURL.path)")
}
let size = 1024
guard let context = CGContext(data: nil, width: size, height: size,
    bitsPerComponent: 8, bytesPerRow: size * 4,
    space: CGColorSpaceCreateDeviceRGB(),
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
    fatalError("Could not create icon canvas")
}
context.interpolationQuality = .high
// Classic .icns images carry their own rounded mask and transparent margin.
// Importing the opaque full-bleed artwork directly produces a large black
// square in the app switcher. Preserve the artwork within the Mac icon grid.
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let mask = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)
context.addPath(mask)
context.clip()
context.draw(artwork, in: tile)
guard let image = context.makeImage(),
      let destination = CGImageDestinationCreateWithURL(
        URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL,
        UTType.png.identifier as CFString, 1, nil) else {
    fatalError("Could not create icon PNG")
}
CGImageDestinationAddImage(destination, image, nil)
guard CGImageDestinationFinalize(destination) else { fatalError("Could not save icon PNG") }
