import Foundation

@main enum SearchFilterChecks {
    static func main() throws {
        let raw = #"invoice "from:literal@example.com" from:alice@example.com is:unread has:attachment"#
        let one = SearchFilterRules.decompose(base: raw)!
        precondition(one.remainder == #"invoice "from:literal@example.com""# && one.fields["from"] == "alice@example.com" && one.read == 1 && one.attachments)
        let escaped = SearchFilterRules.decompose(base: #"subject:"A \"quoted\" \\ path" to:"Casey Copy""#)!
        precondition(escaped.fields["subject"] == #"A "quoted" \ path"# && escaped.fields["to"] == "Casey Copy")
        let rebuilt = try SearchFilterRules.query(base: one.remainder, fields: one.fields, read: one.read, attachments: one.attachments)
        let round = SearchFilterRules.decompose(base: rebuilt)!
        precondition(round.remainder == one.remainder && round.fields == one.fields && round.read == one.read && round.attachments == one.attachments)
        let duplicate = SearchFilterRules.decompose(base: #"FROM:alice from:"bob smith" subject:invoice"#)!
        precondition(duplicate.remainder == #"FROM:alice from:"bob smith""# && duplicate.fields["from"] == nil && duplicate.fields["subject"] == "invoice")
        let mixed = SearchFilterRules.decompose(base: "is:read is:unread")!
        precondition(mixed.read == 0 && mixed.remainder == "is:read is:unread")
        precondition((try? SearchFilterRules.query(base: mixed.remainder, fields: [:], read: 0, attachments: false)) == nil)
        precondition((try? SearchFilterRules.query(base: "is:unread", fields: [:], read: 2, attachments: false)) == nil)
        let starred = SearchFilterRules.decompose(base: "is:starred is:unread")!
        precondition(starred.remainder == "is:starred" && starred.read == 1)
        let attachments = SearchFilterRules.decompose(base: "has:attachment HAS:attachment")!
        precondition(!attachments.attachments && attachments.remainder == "has:attachment HAS:attachment")
        let spaces = SearchFilterRules.decompose(base: #"from:" alice " "from":literal future:operator"#)!
        precondition(spaces.fields.isEmpty && spaces.remainder == #"from:" alice " "from":literal future:operator"#)
        for malformed in [#"subject:"not closed"#, #"subject:"trailing\"#, #"from:"""#, "subject:x\u{0}"] { precondition(SearchFilterRules.decompose(base: malformed) == nil) }
        precondition(SearchFilterRules.decompose(base: "after:2024-02-29 before:2024-03-01")?.fields["after"] == "2024-02-29")
        for invalidDate in ["2025-02-29", "2024-13-01", "0000-01-01", "2024-2-01", "1500-02-29"] {
            precondition(SearchFilterRules.decompose(base: "after:\(invalidDate)") == nil)
            precondition((try? SearchFilterRules.query(base: "", fields: ["after": invalidDate], read: 0, attachments: false)) == nil)
        }
        let injection = try SearchFilterRules.query(base: "invoice", fields: ["subject": #"x" is:read from:"other"#], read: 1, attachments: false)
        let safe = SearchFilterRules.decompose(base: injection)!
        precondition(safe.remainder == "invoice" && safe.fields["subject"] == #"x" is:read from:"other"# && safe.read == 1 && safe.fields["from"] == nil)
        let unicode = SearchFilterRules.decompose(base: #"Ærlig\tekst cc:"Øyvind Andersen""#)!
        precondition(unicode.remainder == #"Ærlig\tekst"# && unicode.fields["cc"] == "Øyvind Andersen")
        precondition(SearchFilterRules.decompose(base: String(repeating: "x ", count: 65)) == nil)
        precondition(SearchFilterRules.decompose(base: String(repeating: "æ", count: 2049)) == nil)
        precondition((try? SearchFilterRules.query(base: duplicate.remainder, fields: ["from": "charlie"], read: 0, attachments: false)) == nil)
        precondition((try? SearchFilterRules.query(base: "after:2024-03-01", fields: ["before": "2024-03-01"], read: 0, attachments: false)) == nil)
        precondition((try? SearchFilterRules.query(base: "in:unknown", fields: [:], read: 0, attachments: false)) == nil)
        precondition((try? SearchFilterRules.query(base: "has:unknown", fields: [:], read: 0, attachments: false)) == nil)
        precondition((try? SearchFilterRules.query(base: "is:unknown", fields: [:], read: 0, attachments: false)) == nil)
        precondition((try? SearchFilterRules.query(base: "", fields: ["unknown": "x"], read: 0, attachments: false)) == nil)
        precondition((try? SearchFilterRules.query(base: "", fields: ["subject": "x\ny"], read: 0, attachments: false)) == nil)
        print("Search filter checks passed (17 scenario groups)")
    }
}
