import Foundation

/// UI validation; provider address parsing remains authoritative at delivery.
enum ComposerAddressRules {
    static func mailbox(_ value: String) -> String {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if let start = trimmed.lastIndex(of: "<"), let end = trimmed[start...].firstIndex(of: ">") {
            return String(trimmed[trimmed.index(after: start)..<end]).trimmingCharacters(in: .whitespaces)
        }
        return trimmed
    }

    /// Preserve display names and quoted commas while validating individual mailboxes.
    static func recipients(_ value: String) -> [String]? {
        var parts: [String] = [], token = "", quoted = false, escaped = false, angle = false
        for character in value {
            if character.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains) { return nil }
            if escaped { token.append(character); escaped = false; continue }
            if character == "\\", quoted { token.append(character); escaped = true; continue }
            if character == "\"" { quoted.toggle() }
            if !quoted {
                if character == "<" { if angle { return nil }; angle = true }
                if character == ">" { if !angle { return nil }; angle = false }
                if (character == "," || character == ";"), !angle {
                    let part = token.trimmingCharacters(in: .whitespaces)
                    guard !part.isEmpty else { return nil }
                    parts.append(part); token = ""; continue
                }
            }
            token.append(character)
        }
        guard !quoted, !escaped, !angle else { return nil }
        let final = token.trimmingCharacters(in: .whitespaces)
        guard !final.isEmpty else { return nil }
        parts.append(final)
        for part in parts {
            let address = mailbox(part)
            let pieces = address.split(separator: "@", omittingEmptySubsequences: false)
            guard pieces.count == 2, !pieces[0].isEmpty, !pieces[1].isEmpty,
                  !address.contains(where: { $0.isWhitespace || "<>;,\"".contains($0) }) else { return nil }
        }
        return parts
    }

    static func draftRecipientSummary(to: String, cc: String, bcc: String) -> String {
        for (label, value) in [("To", to), ("Cc", cc), ("Bcc", bcc)] {
            if !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return "\(label): \(value)" }
        }
        return "No recipient"
    }

    static func replyTarget(sender: String, replyTo: [String]) -> String {
        replyTo.first(where: { !mailbox($0).isEmpty }) ?? sender
    }
    static func replyRecipients(sender: String, replyTo: [String], to: [String], cc: [String], own: [String], replyAll: Bool) -> (to: [String], cc: [String]) {
        let own = Set(own.map { mailbox($0).lowercased() })
        var seen = own
        func unique(_ values: [String]) -> [String] {
            values.filter { value in
                let address = mailbox(value).lowercased()
                return !address.isEmpty && seen.insert(address).inserted
            }
        }
        let targets = replyTo.filter { !mailbox($0).isEmpty }
        var primary = unique(targets.isEmpty ? [sender] : targets)
        // Replying to a sent message should address its other participants.
        if primary.isEmpty { primary = unique(to) }
        if replyAll { primary += unique(to) }
        return (primary, replyAll ? unique(cc) : [])
    }

}
