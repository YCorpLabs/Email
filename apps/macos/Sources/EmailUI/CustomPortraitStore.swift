import AppKit
import CryptoKit
import ImageIO

/// User-selected portraits remain local and take priority over automatic sources.
@MainActor
final class CustomPortraitStore {
    static let shared = CustomPortraitStore()

    enum Scope { case sender, domain }

    private var imageCache: [String: NSImage] = [:]

    private func key(for address: String, scope: Scope) -> String? {
        let address = address.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let pieces = address.split(separator: "@")
        guard pieces.count == 2, !pieces[0].isEmpty, pieces[1].contains(".") else { return nil }
        return scope == .sender ? "sender:\(address)" : "domain:\(pieces[1])"
    }

    private func file(for key: String) -> URL? {
        // An isolated store keeps its own pictures beside it, as Linux does.
        let directory = IsolatedStore.path.map { URL(fileURLWithPath: $0).deletingLastPathComponent() }
            ?? DistributionProfile.current?.applicationSupportDirectory
        guard let base = directory?.appendingPathComponent("Portraits", isDirectory: true),
              (try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)) != nil else { return nil }
        try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: base.path)
        let digest = SHA256.hash(data: Data(key.utf8)).map { String(format: "%02x", $0) }.joined()
        return base.appendingPathComponent(digest + ".png")
    }

    func image(for address: String) -> NSImage? {
        for scope in [Scope.sender, .domain] {
            guard let key = key(for: address, scope: scope), let url = file(for: key) else { continue }
            if let cached = imageCache[key] { return cached }
            if let data = try? Data(contentsOf: url), data.count <= 262_144,
               let image = NSImage(data: data) {
                imageCache[key] = image
                return image
            }
        }
        return nil
    }

    func hasImage(for address: String, scope: Scope) -> Bool {
        guard let key = key(for: address, scope: scope), let url = file(for: key) else { return false }
        return FileManager.default.fileExists(atPath: url.path)
    }

    func save(source: URL, for address: String, scope: Scope) throws {
        guard let key = key(for: address, scope: scope), let destination = file(for: key),
              let sourceSize = try? source.resourceValues(forKeys: [.fileSizeKey]).fileSize,
              sourceSize <= 10_000_000,
              let imageSource = CGImageSourceCreateWithURL(source as CFURL, nil),
              let properties = CGImageSourceCopyPropertiesAtIndex(imageSource, 0, nil) as? [CFString: Any],
              let pixelWidth = properties[kCGImagePropertyPixelWidth] as? Int,
              let pixelHeight = properties[kCGImagePropertyPixelHeight] as? Int,
              pixelWidth > 0, pixelHeight > 0,
              pixelWidth <= 8192, pixelHeight <= 8192,
              let image = NSImage(contentsOf: source),
              image.size.width > 0, image.size.height > 0 else {
            throw PortraitError.invalidImage
        }
        let canvas = NSImage(size: NSSize(width: 128, height: 128), flipped: false) { rect in
            let side = min(image.size.width, image.size.height)
            let crop = NSRect(x: (image.size.width - side) / 2,
                              y: (image.size.height - side) / 2,
                              width: side, height: side)
            image.draw(in: rect, from: crop, operation: .copy, fraction: 1)
            return true
        }
        guard let tiff = canvas.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff),
              let png = bitmap.representation(using: .png, properties: [:]),
              png.count <= 262_144 else { throw PortraitError.invalidImage }
        try png.write(to: destination, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: destination.path)
        imageCache[key] = NSImage(data: png)
    }

    func remove(for address: String, scope: Scope) {
        guard let key = key(for: address, scope: scope), let url = file(for: key) else { return }
        try? FileManager.default.removeItem(at: url)
        imageCache.removeValue(forKey: key)
    }

    private enum PortraitError: LocalizedError {
        case invalidImage
        var errorDescription: String? { "Choose a PNG, JPEG, or HEIC image smaller than 10 MB." }
    }
}
