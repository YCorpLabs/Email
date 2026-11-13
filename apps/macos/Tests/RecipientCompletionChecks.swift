import Foundation
@main enum RecipientCompletionChecks {
    static func main() {
        let text = "\"Ødegård, Torjus\" <torjus@example.com>, ma; Last <last@example.com>"
        let caret = (text as NSString).range(of: "ma;").location + 2
        let ctx = RecipientCompletionRules.context(text, caret: caret)!
        precondition(ctx.query == "ma")
        let inserted = RecipientCompletionRules.inserting("Maya <maya@example.com>", in: text, context: ctx)!
        precondition(inserted.text == "\"Ødegård, Torjus\" <torjus@example.com>, Maya <maya@example.com>; Last <last@example.com>")
        precondition(inserted.caret == (inserted.text as NSString).range(of: "; Last").location)
        precondition(RecipientCompletionRules.existing(text, excluding: ctx.range) == ["torjus@example.com", "last@example.com"])
        precondition(RecipientCompletionRules.context("Alice <al", caret: 9)?.query == "al")
        precondition(RecipientCompletionRules.context("a@example.com, ", caret: 15) == nil)
        precondition(RecipientCompletionRules.context("\"Smith, Al", caret: 10)?.query == "Smith, Al")
        precondition(RecipientCompletionRules.inserting("a@example.com\r\nBcc: b@example.com", in: text, context: ctx) == nil)
        precondition(RecipientCompletionRules.context("ø@example.com", caret: -1) == nil)
        precondition(RecipientCompletionRules.existing("One <ONE@example.com>, one@example.com") == ["one@example.com"])
        print("Recipient completion checks passed (10 scenarios)")
    }
}
