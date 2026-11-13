import Foundation

@main enum ComposerAddressChecks {
    static func main() {
        precondition(ComposerAddressRules.recipients("alice@example.com, bob@example.com")?.count == 2)
        precondition(ComposerAddressRules.recipients("Alice Smith <alice@example.com>")?.count == 1)
        precondition(ComposerAddressRules.recipients("\"Smith, Alice\" <alice@example.com>; Bob <bob@example.com>")?.count == 2)
        precondition(ComposerAddressRules.recipients("alice@example.com\r\nBcc: other@example.com") == nil)
        precondition(ComposerAddressRules.recipients("not an address") == nil)
        precondition(ComposerAddressRules.recipients("alice@example.com,,bob@example.com") == nil)
        precondition(ComposerAddressRules.recipients("Alice <alice@example.com") == nil)
        precondition(ComposerAddressRules.replyTarget(sender: "bounce@example.com", replyTo: ["", "Support <help@example.com>"]) == "Support <help@example.com>")
        precondition(ComposerAddressRules.replyTarget(sender: "person@example.com", replyTo: []) == "person@example.com")
        let reply = ComposerAddressRules.replyRecipients(sender: "bounce@example.com", replyTo: ["Support <help@example.com>"], to: ["me@example.com", "Other <other@example.com>"], cc: ["HELP@example.com", "Copy <copy@example.com>", "alias@example.com"], own: ["me@example.com", "ALIAS@example.com"], replyAll: true)
        precondition(reply.to == ["Support <help@example.com>", "Other <other@example.com>"])
        precondition(reply.cc == ["Copy <copy@example.com>"])
        let single = ComposerAddressRules.replyRecipients(sender: "Sender <sender@example.com>", replyTo: [], to: ["me@example.com", "other@example.com"], cc: ["copy@example.com"], own: ["me@example.com"], replyAll: false)
        precondition(single.to == ["Sender <sender@example.com>"] && single.cc.isEmpty)
        let sent = ComposerAddressRules.replyRecipients(sender: "alias@example.com", replyTo: [], to: ["other@example.com"], cc: ["copy@example.com"], own: ["alias@example.com"], replyAll: false)
        precondition(sent.to == ["other@example.com"])
        assert(ComposerAddressRules.draftRecipientSummary(to: "", cc: "copy@example.com", bcc: "hidden@example.com") == "Cc: copy@example.com")
assert(ComposerAddressRules.draftRecipientSummary(to: "", cc: "", bcc: "hidden@example.com") == "Bcc: hidden@example.com")
assert(ComposerAddressRules.draftRecipientSummary(to: "to@example.com", cc: "copy@example.com", bcc: "") == "To: to@example.com")
print("Composer address checks passed (16 scenarios)")
    }
}
