import Foundation
import CryptoKit

struct MailArrival: Equatable, Sendable {
    let accountID: String
    let threadID: String
    let sender: String
    let subject: String
    let receivedAt: Int64
    var snippet: String = ""
}

/// Tracks arrivals, not newly downloaded history. Each account establishes its
/// own quiet baseline only after its first successful sync in this session.
struct MailArrivalTracker {
    let sessionStartedAt: Int64
    private var seen: [String: [String: Int64]] = [:]
    private var enabledSince: Int64 = 0

    init(sessionStartedAt: Int64) {
        self.sessionStartedAt = sessionStartedAt
    }

    func hasBaseline(accountID: String) -> Bool { seen[accountID] != nil }
    mutating func resume(at time: Int64) { enabledSince = max(enabledSince, time) }

    mutating func consume(accountID: String, rows: [[String: Any]]) -> [MailArrival] {
        let wasInitialized = seen[accountID] != nil
        var versions = seen[accountID] ?? [:]
        var arrivals: [MailArrival] = []
        for row in rows {
            guard let id = row["id"] as? String,
                  let received = (row["last_message_at"] as? NSNumber)?.int64Value else { continue }
            let previous = versions[id] ?? 0
            versions[id] = max(previous, received)
            guard wasInitialized, received >= max(sessionStartedAt, enabledSince), received > previous,
                  ((row["unread_count"] as? NSNumber)?.intValue ?? 0) > 0 else { continue }
            arrivals.append(MailArrival(accountID: accountID, threadID: id,
                                        sender: row["sender"] as? String ?? "",
                                        subject: row["subject"] as? String ?? "New email",
                                        receivedAt: received))
        }
        seen[accountID] = versions
        return arrivals.sorted { $0.receivedAt > $1.receivedAt }
    }
}

struct DueFollowup: Equatable, Sendable {
    let accountID: String
    let threadID: String
    let subject: String
    let createdAt: Int64
    let dueAt: Int64
    var id: String {
        let data = try! JSONSerialization.data(withJSONObject: [accountID, threadID, String(createdAt), String(dueAt)])
        return "followup-" + SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}

struct FollowupNotificationTracker {
    private(set) var seenIDs: Set<String>
    private var enabledSince: Int64?
    init(seenIDs: Set<String> = [], enabledSince: Int64? = nil) {
        self.seenIDs = seenIDs; self.enabledSince = enabledSince
    }
    mutating func resume(at time: Int64) { enabledSince = max(enabledSince ?? 0, time) }
    mutating func markDelivered(_ reminder: DueFollowup) { seenIDs.insert(reminder.id) }
    mutating func consume(rows: [[String: Any]], enabled: Bool, now: Int64) -> [DueFollowup] {
        var active = Set<String>(), candidates: [DueFollowup] = []
        for row in rows {
            guard let account = row["account_id"] as? String, let thread = row["thread_id"] as? String,
                  let created = (row["created_at_ms"] as? NSNumber)?.int64Value,
                  let due = (row["due_at_ms"] as? NSNumber)?.int64Value,
                  let status = row["status"] as? String, ["waiting", "due"].contains(status) else { continue }
            let reminder = DueFollowup(accountID: account, threadID: thread,
                                       subject: row["subject"] as? String ?? "Follow up",
                                       createdAt: created, dueAt: due)
            guard active.insert(reminder.id).inserted else { continue }
            guard status == "due", due <= now else { continue }
            if !enabled || enabledSince.map({ due < $0 }) == true { seenIDs.insert(reminder.id); continue }
            if !seenIDs.contains(reminder.id) { candidates.append(reminder) }
        }
        seenIDs.formIntersection(active)
        return candidates.sorted { $0.dueAt < $1.dueAt }
    }
}

enum NotificationForegroundPolicy {
    static func shouldPresent(kind: String, enabled: Bool, readingSameThread: Bool) -> Bool {
        guard enabled else { return false }
        return kind == "followup" || kind == "open" || kind == "test" || kind == "agent" || !readingSameThread
    }
}


/// Collapse mail whitespace so a banner spends its limited space on content.
enum MailNotificationPreview {
    static func body(subject: String, snippet: String) -> String {
        let subject = subject.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
        let snippet = snippet.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
        return String(([subject.isEmpty ? "(No subject)" : String(subject.prefix(140))] + (snippet.isEmpty ? [] : [String(snippet.prefix(180))])).joined(separator: "\n").prefix(320))
    }

    /// Display name without the address: `"Ada" <ada@x.test>` → `Ada`.
    static func senderName(_ sender: String) -> String {
        let name = (sender.components(separatedBy: "<").first ?? sender)
            .trimmingCharacters(in: .whitespacesAndNewlines.union(CharacterSet(charactersIn: "\"'")))
        if !name.isEmpty { return name }
        return sender.trimmingCharacters(in: CharacterSet(charactersIn: "<> \n\t"))
    }

    /// Up to three newest "Sender — Subject" lines for a batch alert.
    static func batch(_ items: [(sender: String, subject: String)]) -> String {
        var lines = items.prefix(3).map { item -> String in
            let subject = item.subject.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
            return String("\(senderName(item.sender)) — \(subject.isEmpty ? "(No subject)" : subject)".prefix(90))
        }
        if items.count > 3 { lines.append("and \(items.count - 3) more") }
        return lines.joined(separator: "\n")
    }
}
