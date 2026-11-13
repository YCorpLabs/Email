import AppKit

/// Loads published BIMI artwork for sender portraits. A logo is decorative;
/// this is not a claim that the current message or mark certificate is verified.
@MainActor
final class BrandLogoStore {
    static let shared = BrandLogoStore()

    private var cache: [String: (data: Data?, expires: Date)] = [:]
    private var waiting: [String: [(NSImage?) -> Void]] = [:]

    func image(for address: String, displayName: String, completion: @escaping (NSImage?) -> Void) {
        guard Self.isOrganizationSender(address: address) else {
            completion(nil)
            return
        }
        guard let domain = address.split(separator: "@").last.map(String.init)?.lowercased(),
              domain.contains("."), domain.count < 254,
              domain.unicodeScalars.allSatisfy({ CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyz0123456789.-").contains($0) }) else {
            completion(nil)
            return
        }
        if let cached = cache[domain], cached.expires > Date() {
            completion(cached.data.flatMap(NSImage.init(data:)))
            return
        }
        if waiting[domain] != nil {
            waiting[domain]?.append(completion)
            return
        }
        waiting[domain] = [completion]
        Task.detached(priority: .utility) {
            let data = await Self.resolve(domain: domain)
            await MainActor.run {
                let store = BrandLogoStore.shared
                store.cache[domain] = (data, Date().addingTimeInterval(data == nil ? 1800 : 86400))
                let callbacks = store.waiting.removeValue(forKey: domain) ?? []
                let image = data.flatMap(NSImage.init(data:))
                callbacks.forEach { $0(image) }
            }
        }
    }

    /// BIMI is looked up only for role mailboxes such as support@ or news@,
    /// never for a person's own address.
    private static func isOrganizationSender(address: String) -> Bool {
        let localPart = address.lowercased().split(separator: "@", omittingEmptySubsequences: false).first.map(String.init) ?? ""
        let mailbox = localPart.split(whereSeparator: { $0 == "." || $0 == "-" || $0 == "_" || $0 == "+" }).first.map(String.init) ?? ""
        return ["admin", "alerts", "billing", "contact", "email", "hello", "help", "hey", "info",
                "marketing", "news", "newsletter", "notification", "notifications", "noreply",
                "no", "privacy", "sales", "security", "service", "support", "team", "updates"].contains(mailbox)
    }

    nonisolated private static func resolve(domain: String) async -> Data? {
        let parts = domain.split(separator: ".")
        guard parts.count >= 2 else { return nil }
        // A sender such as email.shopify.com can inherit shopify.com's BIMI record.
        for index in 0..<(parts.count - 1) {
            let candidate = parts[index...].joined(separator: ".")
            guard let url = bimiURL(for: candidate) else { continue }
            guard let data = download(url),
                  let svg = String(data: data, encoding: .utf8),
                  safeSVG(svg) else { continue }
            return data
        }
        return nil
    }

    nonisolated private static func bimiURL(for domain: String) -> URL? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/dig")
        process.arguments = ["+time=2", "+tries=1", "+short", "TXT", "default._bimi.\(domain)"]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = Pipe()
        guard (try? process.run()) != nil else { return nil }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0, data.count < 16_384,
              let text = String(data: data, encoding: .utf8) else { return nil }
        for line in text.split(separator: "\n") {
            let record = line.replacingOccurrences(of: "\"", with: "")
            let fields = record.split(separator: ";").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            guard fields.first?.lowercased() == "v=bimi1" else { continue }
            let values = Dictionary(fields.dropFirst().compactMap { field -> (String, String)? in
                guard let equals = field.firstIndex(of: "=") else { return nil }
                return (String(field[..<equals]).lowercased(), String(field[field.index(after: equals)...]))
            }, uniquingKeysWith: { first, _ in first })
            // The certificate field narrows candidates, but is not verified here.
            guard let logo = values["l"], let authority = values["a"], !authority.isEmpty,
                  let url = URL(string: logo), let certificate = URL(string: authority),
                  url.scheme == "https", certificate.scheme == "https",
                  let host = url.host, host == certificate.host,
                  url.user == nil, url.password == nil, url.fragment == nil,
                  validPublicHost(host) else { continue }
            return url
        }
        return nil
    }

    nonisolated private static func validPublicHost(_ host: String) -> Bool {
        let labels = host.lowercased().split(separator: ".")
        guard labels.count >= 2, host.count <= 253,
              !["local", "localhost", "internal"].contains(String(labels.last ?? "")),
              labels.last?.contains(where: { $0.isLetter }) == true else { return false }
        return labels.allSatisfy { label in
            !label.isEmpty && label.count <= 63 &&
            label.first != "-" && label.last != "-" &&
            label.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-") }
        }
    }

    nonisolated private static func download(_ url: URL) -> Data? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/curl")
        // No redirects, cookies, credentials, or unbounded download. This runs
        // off the UI thread and only once per sender domain until cache expiry.
        process.arguments = ["--silent", "--fail", "--proto", "=https",
                             "--max-time", "5", "--connect-timeout", "2",
                             "--max-filesize", "65536", "--url", url.absoluteString]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = Pipe()
        guard (try? process.run()) != nil else { return nil }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0, data.count <= 65_536 else { return nil }
        return data
    }

    nonisolated private static func safeSVG(_ svg: String) -> Bool {
        let lower = svg.lowercased()
        guard lower.contains("<svg"), lower.contains("</svg>") else { return false }
        for forbidden in ["<script", "<foreignobject", "<!doctype", "<!entity", "@import", "javascript:", "data:", "onload=", "onerror=", "href=", "url("] {
            if lower.contains(forbidden) { return false }
        }
        return true
    }
}
