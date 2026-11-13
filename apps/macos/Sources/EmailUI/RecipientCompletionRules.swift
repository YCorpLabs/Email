import Foundation

/// Ranges use UTF-16, matching the AppKit field editor. Quoted display-name
/// commas and angle-address delimiters never split an existing recipient.
enum RecipientCompletionRules {
    struct Context: Equatable { let range: NSRange; let query: String }
    static func ranges(_ text: String) -> [NSRange] {
        let units = Array(text.utf16)
        var quoted = false, escaped = false, angle = false, start = 0
        var result: [NSRange] = []
        for (i, unit) in units.enumerated() {
            if escaped { escaped = false; continue }
            if unit == 92 && quoted { escaped = true; continue }
            if unit == 34 { quoted.toggle(); continue }
            if !quoted {
                if unit == 60 { angle = true }; if unit == 62 { angle = false }
                if !angle && (unit == 44 || unit == 59) { result.append(NSRange(location: start, length: i-start)); start = i+1 }
            }
        }
        result.append(NSRange(location: start, length: units.count-start)); return result
    }
    static func context(_ text: String, caret: Int) -> Context? {
        guard caret >= 0, caret <= (text as NSString).length,
              let range = ranges(text).first(where: { caret >= $0.location && caret <= NSMaxRange($0) }) else { return nil }
        let part = (text as NSString).substring(with: range).trimmingCharacters(in: .whitespaces)
        let query = part.lastIndex(of: "<").map { String(part[part.index(after: $0)...]).trimmingCharacters(in: CharacterSet(charactersIn: "> ")) } ?? part.trimmingCharacters(in: CharacterSet(charactersIn: "\" "))
        guard !query.isEmpty, query.utf8.count <= 128 else { return nil }
        return Context(range: range, query: query)
    }
    static func existing(_ text: String, excluding: NSRange? = nil) -> Set<String> {
        Set(ranges(text).filter { $0 != excluding }.compactMap { range in
            let value = (text as NSString).substring(with: range).trimmingCharacters(in: .whitespaces)
            guard ComposerAddressRules.recipients(value)?.count == 1 else { return nil }
            return ComposerAddressRules.mailbox(value).lowercased()
        })
    }
    static func inserting(_ formatted: String, in text: String, context: Context) -> (text: String, caret: Int)? {
        guard ComposerAddressRules.recipients(formatted)?.count == 1, NSMaxRange(context.range) <= (text as NSString).length else { return nil }
        let insertion = (context.range.location == 0 ? "" : " ") + formatted
        return ((text as NSString).replacingCharacters(in: context.range, with: insertion), context.range.location + (insertion as NSString).length)
    }
}

