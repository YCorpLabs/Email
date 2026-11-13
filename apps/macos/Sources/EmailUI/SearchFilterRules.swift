import Foundation

/// Mirrors the Rust cache-search token grammar. Unsupported or ambiguous terms
/// stay verbatim in the remainder, so opening a filter editor cannot broaden it.
enum SearchFilterRules {
    private struct Token {
        let raw: String
        let value: String
        let literal: Bool
        var pair: (key: String, value: String)? {
            guard !literal, let colon = value.firstIndex(of: ":") else { return nil }
            return (String(value[..<colon]).lowercased(), String(value[value.index(after: colon)...]))
        }
        var controlKey: String? {
            guard let pair else { return nil }
            if ["from", "to", "cc", "subject", "after", "before"].contains(pair.key) { return pair.key }
            if pair.key == "is", ["read", "unread"].contains(pair.value.lowercased()) { return "read" }
            if pair.key == "has", pair.value.lowercased() == "attachment" { return "attachments" }
            return nil
        }
    }

    static func decompose(base: String) -> (remainder: String, fields: [String: String], read: Int, attachments: Bool)? {
        guard let terms = tokens(base) else { return nil }
        var counts: [String: Int] = [:]
        for token in terms { if let key = token.controlKey { counts[key, default: 0] += 1 } }
        var remainder: [String] = [], fields: [String: String] = [:]
        var read = 0, attachments = false
        for token in terms {
            if let pair = token.pair, ["from", "to", "cc", "subject", "after", "before", "is", "has", "in", "label"].contains(pair.key), pair.value.isEmpty { return nil }
            if let pair = token.pair, ["after", "before"].contains(pair.key), !validDate(pair.value) { return nil }
            guard let key = token.controlKey, counts[key] == 1, let pair = token.pair,
                  pair.value == pair.value.trimmingCharacters(in: .whitespacesAndNewlines) else {
                remainder.append(token.raw); continue
            }
            if key == "read" { read = pair.value.lowercased() == "unread" ? 1 : 2 }
            else if key == "attachments" { attachments = true }
            else {
                if ["after", "before"].contains(key), !validDate(pair.value) { return nil }
                fields[key] = pair.value
            }
        }
        return (remainder.joined(separator: " "), fields, read, attachments)
    }

    static func query(base: String, fields: [String: String], read: Int, attachments: Bool) throws -> String {
        guard let existing = tokens(base) else { throw invalid("Close quoted values and use at most 64 search terms (4096 bytes).") }
        guard (0...2).contains(read) else { throw invalid("Choose Any read status, Unread, or Read.") }
        for (key, value) in fields where !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            guard ["from", "to", "cc", "subject", "after", "before"].contains(key) else { throw invalid("Unsupported visual filter: \(key)") }
        }
        var statuses = Set(existing.compactMap { token -> String? in
            guard let pair = token.pair, pair.key == "is", ["read", "unread"].contains(pair.value.lowercased()) else { return nil }
            return pair.value.lowercased()
        })
        if read == 1 { statuses.insert("unread") }
        if read == 2 { statuses.insert("read") }
        guard statuses.count <= 1 else { throw invalid("The existing query and filters ask for both Read and Unread. Remove one read-status filter.") }
        var existingFields: [String: [String]] = [:]
        for token in existing {
            guard let pair = token.pair else { continue }
            if ["from", "to", "cc", "subject", "after", "before", "is", "has", "in", "label"].contains(pair.key), pair.value.isEmpty { throw invalid("\(pair.key): needs a value.") }
            if ["from", "to", "cc", "subject", "after", "before"].contains(pair.key) { existingFields[pair.key, default: []].append(pair.value) }
            if ["after", "before"].contains(pair.key), !validDate(pair.value) { throw invalid("Use a real date in YYYY-MM-DD format.") }
            if pair.key == "is", !["read", "unread", "starred"].contains(pair.value.lowercased()) { throw invalid("Use is:read, is:unread, or is:starred.") }
            if pair.key == "has", pair.value.lowercased() != "attachment" { throw invalid("Use has:attachment.") }
            if pair.key == "in", !["inbox", "archive", "spam", "junk", "trash"].contains(pair.value.lowercased()) { throw invalid("Use in:inbox, in:archive, in:spam, or in:trash.") }
        }
        for key in ["from", "to", "cc", "subject", "after", "before"] {
            let value = (fields[key] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if !value.isEmpty, let prior = existingFields[key], prior.contains(where: { $0 != value }) { throw invalid("The existing query already contains \(key): filters. Edit them in search before adding a different value.") }
            if !value.isEmpty { existingFields[key, default: []].append(value) }
        }
        if let after = existingFields["after"]?.max(), let before = existingFields["before"]?.min(), after >= before { throw invalid("On or after must be earlier than Before.") }
        var parts = [base.trimmingCharacters(in: .whitespacesAndNewlines)].filter { !$0.isEmpty }
        for key in ["from", "to", "cc", "subject", "after", "before"] {
            let value = (fields[key] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !value.isEmpty else { continue }
            guard !value.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else { throw invalid("\(key): cannot contain control characters.") }
            if ["after", "before"].contains(key), !validDate(value) { throw invalid("Use a real date in YYYY-MM-DD format.") }
            let escaped = value.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
            parts.append("\(key):\"\(escaped)\"")
        }
        if read == 1 { parts.append("is:unread") }
        if read == 2 { parts.append("is:read") }
        if attachments { parts.append("has:attachment") }
        let query = parts.joined(separator: " ")
        guard tokens(query) != nil else { throw invalid("Search is limited to 64 terms and 4096 bytes; check quotes and control characters.") }
        return query
    }

    private static func invalid(_ message: String) -> NSError {
        NSError(domain: "Search", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
    private static func validDate(_ text: String) -> Bool {
        let b = Array(text.utf8)
        guard b.count == 10, b[4] == 45, b[7] == 45,
              b.enumerated().allSatisfy({ $0.offset == 4 || $0.offset == 7 || (48...57).contains($0.element) }),
              let year = Int(text.prefix(4)), (1...9999).contains(year) else { return false }
        guard let month = Int(text.dropFirst(5).prefix(2)), (1...12).contains(month), let day = Int(text.suffix(2)) else { return false }
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
        let days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        return (1...days[month - 1]).contains(day)
    }
    private static func tokens(_ query: String) -> [Token]? {
        guard query.utf8.count <= 4096 else { return nil }
        var result: [Token] = [], raw = "", value = ""
        var quoted = false, escaped = false, literal = false, active = false
        func finish() {
            if active { result.append(Token(raw: raw, value: value, literal: literal)); raw = ""; value = ""; literal = false; active = false }
        }
        for char in query {
            if char.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) && !CharacterSet.whitespacesAndNewlines.contains($0) }) { return nil }
            if char.isWhitespace && !quoted && !escaped { finish(); continue }
            if !active { literal = char == "\"" }; active = true; raw.append(char)
            if escaped { value.append(char); escaped = false; continue }
            if quoted && char == "\\" { escaped = true; continue }
            if char == "\"" { quoted.toggle(); continue }
            value.append(char)
        }
        guard !quoted, !escaped else { return nil }
        finish()
        guard result.count <= 64, result.allSatisfy({ !$0.value.isEmpty }) else { return nil }
        return result
    }
}
