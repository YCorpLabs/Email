import Foundation

@main enum NotificationTrackerChecks {
    static func row(_ id: String, _ timestamp: Int64, unread: Int = 1) -> [String: Any] {
        ["id": id, "last_message_at": timestamp, "unread_count": unread,
         "sender": "Example <hello@example.com>", "subject": "Hello"]
    }
    static func main() {
        precondition(MailNotificationPreview.body(subject: "  Update\n ", snippet: "Hello\n\tworld") == "Update\nHello world")
        precondition(MailNotificationPreview.body(subject: " ", snippet: "") == "(No subject)")
        precondition(MailNotificationPreview.body(subject: "Hi", snippet: String(repeating: "é", count: 300)).count == 183)
        precondition(MailNotificationPreview.senderName("\"Ada Lovelace\" <ada@example.test>") == "Ada Lovelace")
        precondition(MailNotificationPreview.senderName("<ada@example.test>") == "ada@example.test")
        precondition(MailNotificationPreview.batch([("A <a@x>", "One"), ("B", ""), ("C", "Three"), ("D", "Four")]) == "A — One\nB — (No subject)\nC — Three\nand 1 more")
        var tracker = MailArrivalTracker(sessionStartedAt: 1000)
        precondition(tracker.consume(accountID: "a", rows: [row("thread", 1100)]).isEmpty, "Initial sync must be quiet")
        precondition(tracker.consume(accountID: "b", rows: [row("other", 1200)]).isEmpty, "Each account needs its own baseline")
        precondition(tracker.consume(accountID: "a", rows: [row("new", 1200)]).count == 1, "New unread mail should notify")
        precondition(tracker.consume(accountID: "a", rows: [row("new", 1200)]).isEmpty, "Repeat sync must not notify twice")
        precondition(tracker.consume(accountID: "a", rows: [row("old", 999), row("read", 1300, unread: 0)]).isEmpty, "History and read mail must be quiet")
        precondition(tracker.consume(accountID: "a", rows: [row("read", 1300)]).isEmpty, "Marking unread must not create a new alert")
        precondition(tracker.consume(accountID: "a", rows: [row("thread", 1400)]).first?.threadID == "thread", "New reply should notify")
        _ = tracker.consume(accountID: "a", rows: [])
        precondition(tracker.consume(accountID: "a", rows: [row("thread", 1400)]).isEmpty, "Reappearing threads must not notify twice")
        precondition(tracker.consume(accountID: "a", rows: [["id": "bad"], ["last_message_at": 1500]]).isEmpty, "Malformed metadata must be ignored")
        var primed = MailArrivalTracker(sessionStartedAt: 1000)
        _ = primed.consume(accountID: "a", rows: [row("cached", 900)])
        precondition(primed.hasBaseline(accountID: "a"))
        precondition(primed.consume(accountID: "a", rows: [row("first-sync-new", 1100)]).count == 1, "Priming cached mail must preserve first sync arrivals")
        primed.resume(at: 2000)
        precondition(primed.consume(accountID: "a", rows: [row("disabled-backlog", 1900)]).isEmpty)
        precondition(primed.consume(accountID: "a", rows: [row("after-enable", 2100)]).count == 1)

        func reminder(_ due: Int64, status: String = "due", account: String = "a") -> [String: Any] {
            ["account_id": account, "thread_id": "thread", "created_at_ms": Int64(1000), "due_at_ms": due, "subject": "Follow up", "status": status]
        }
        var followups = FollowupNotificationTracker()
        let due = followups.consume(rows: [reminder(2000), reminder(2000)], enabled: true, now: 2000)
        precondition(due.count == 1, "Duplicate reminder rows must dispatch once")
        precondition(followups.consume(rows: [reminder(2000)], enabled: true, now: 2000).count == 1, "Failed delivery must remain retryable")
        followups.markDelivered(due[0])
        precondition(followups.consume(rows: [reminder(2000)], enabled: true, now: 2100).isEmpty)
        var relaunched = FollowupNotificationTracker(seenIDs: followups.seenIDs)
        precondition(relaunched.consume(rows: [reminder(2000)], enabled: true, now: 2100).isEmpty, "Delivered IDs must survive relaunch")
        precondition(relaunched.consume(rows: [reminder(2200)], enabled: true, now: 2200).count == 1, "Rescheduled reminder has a new stable ID")
        precondition(relaunched.consume(rows: [reminder(2200, account: "b")], enabled: true, now: 2200).first?.id != due[0].id, "Reminder identity must include account")
        var disabled = FollowupNotificationTracker()
        precondition(disabled.consume(rows: [reminder(2000)], enabled: false, now: 2100).isEmpty)
        precondition(disabled.consume(rows: [reminder(2000)], enabled: true, now: 2100).isEmpty, "Disabled reminders must not replay")
        disabled.resume(at: 3000)
        precondition(disabled.consume(rows: [reminder(2500)], enabled: true, now: 3100).isEmpty, "Offline disabled backlog must not replay on enable")
        precondition(disabled.consume(rows: [reminder(3200)], enabled: true, now: 3100).isEmpty, "Future reminders must wait")
        precondition(disabled.consume(rows: [reminder(3200)], enabled: true, now: 3200).count == 1)
        precondition(disabled.consume(rows: [reminder(3200, status: "cancelled"), reminder(3200, status: "replied")], enabled: true, now: 3300).isEmpty)
        precondition(NotificationForegroundPolicy.shouldPresent(kind: "followup", enabled: true, readingSameThread: true))
        precondition(NotificationForegroundPolicy.shouldPresent(kind: "open", enabled: true, readingSameThread: true))
        precondition(NotificationForegroundPolicy.shouldPresent(kind: "test", enabled: true, readingSameThread: true))
        precondition(NotificationForegroundPolicy.shouldPresent(kind: "agent", enabled: true, readingSameThread: true))
        precondition(!NotificationForegroundPolicy.shouldPresent(kind: "agent", enabled: false, readingSameThread: false))
        precondition(!NotificationForegroundPolicy.shouldPresent(kind: "mail", enabled: true, readingSameThread: true))
        precondition(NotificationForegroundPolicy.shouldPresent(kind: "mail", enabled: true, readingSameThread: false))
        precondition(!NotificationForegroundPolicy.shouldPresent(kind: "followup", enabled: false, readingSameThread: false))
        print("Notification arrival, follow-up and foreground checks passed")
    }
}
