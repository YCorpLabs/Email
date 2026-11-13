import AppKit
import UserNotifications

@MainActor
final class MailNotifications: NSObject, UNUserNotificationCenterDelegate {
    static let shared = MailNotifications()
    static let enabledKey = "email.notifications.enabled"
    static let previewsKey = "email.notifications.previews"
    private static let sinceKey = "email.notifications.enabledSince"
    private static let followupSeenKey = "email.notifications.followupSeen"
    private static let agentSeenKey = "email.notifications.agentSeen"
    // Builds before 11 Nov 2026 stored hidden previews as their default; reset once.
    private static let previewsDefaultMigrationKey = "email.notifications.previewsDefaultV2"
    nonisolated static let threadCategory = "email.mail-thread"
    nonisolated static let markReadAction = "email.mark-read"
    nonisolated static let archiveAction = "email.archive"
    var onOpen: ((String, String) -> Void)?
    var onOpenTracked: ((String, String) -> Void)?
    var onOpenActivity: (() -> Void)?
    var onOpenFollowup: ((String, String) -> Void)?
    var onOpenAgent: (() -> Void)?
    var onTriage: ((String, String, String) -> Void)?
    var isReadingThread: ((String, String) -> Bool)?
    var onSettingsChanged: (() -> Void)?
    private var tracker = MailArrivalTracker(sessionStartedAt: Int64(Date().timeIntervalSince1970 * 1000))
    private var followups = FollowupNotificationTracker(
        seenIDs: Set(UserDefaults.standard.stringArray(forKey: "email.notifications.followupSeen") ?? []),
        enabledSince: (UserDefaults.standard.object(forKey: "email.notifications.enabledSince") as? NSNumber)?.int64Value)
    private let sessionSince = Int64(Date().timeIntervalSince1970 * 1000)
    private var lastOpenCheck: Date?
    private var openCheckInFlight = false
    private var authorizationTask: Task<Bool, Never>?
    private var settingsGeneration = 0
    private var lastFollowupCheck: Date?
    private var followupCheckInFlight = false
    var enabled: Bool { Self.available && UserDefaults.standard.bool(forKey: Self.enabledKey) }
    var previews: Bool { (UserDefaults.standard.object(forKey: Self.previewsKey) as? Bool) ?? true }

    static let authorizationOptions: UNAuthorizationOptions = [.alert, .badge, .sound]
    /// UserNotifications requires an app bundle. The unbundled SwiftPM executable
    /// (scripts/dev.sh, or run-macos.sh with EMAIL_STORE_PATH) would otherwise
    /// crash with "bundleProxyForCurrentProcess is nil", so it runs without
    /// notifications. Every notification call goes through `center`.
    nonisolated static let available = Bundle.main.bundleURL.pathExtension == "app"
    private static var center: UNUserNotificationCenter? { available ? .current() : nil }

    func start() {
        guard let center = Self.center else { return }
        center.delegate = self
        if !UserDefaults.standard.bool(forKey: Self.previewsDefaultMigrationKey) {
            UserDefaults.standard.removeObject(forKey: Self.previewsKey)
            UserDefaults.standard.set(true, forKey: Self.previewsDefaultMigrationKey)
        }
        let actions = [
            UNNotificationAction(identifier: Self.markReadAction, title: "Mark as Read", options: []),
            UNNotificationAction(identifier: Self.archiveAction, title: "Archive", options: []),
        ]
        // macOS "Show previews: When Unlocked/Never" still hides the content.
        center.setNotificationCategories([
            UNNotificationCategory(identifier: Self.threadCategory, actions: actions, intentIdentifiers: [],
                                   hiddenPreviewsBodyPlaceholder: "New email", options: [])
        ])
        Task {
            // Mail notifications are on unless the user turned them off.
            guard UserDefaults.standard.object(forKey: Self.enabledKey) != nil else { _ = await enable(); return }
            guard enabled else { return }
            guard let center = Self.center else { return }
            let settings = await center.notificationSettings()
            switch settings.authorizationStatus {
            case .denied: setEnabled(false)
            case .notDetermined:
                // A rebuilt or re-signed app can lose its grant while the
                // preference still says on; delivery then failed silently.
                _ = try? await center.requestAuthorization(options: Self.authorizationOptions)
            default:
                // Older grants lacked badge and sound; asking again adds them
                // without a prompt when the user already allowed alerts.
                _ = try? await center.requestAuthorization(options: Self.authorizationOptions)
            }
        }
    }

    private func setEnabled(_ value: Bool) {
        let wasEnabled = enabled
        UserDefaults.standard.set(value, forKey: Self.enabledKey)
        settingsGeneration += 1
        if value && !wasEnabled {
            let time = Int64(Date().timeIntervalSince1970 * 1000)
            UserDefaults.standard.set(time, forKey: Self.sinceKey)
            tracker.resume(at: time); followups.resume(at: time)
        }
        if !value {
            Self.center?.removeAllPendingNotificationRequests()
            Self.center?.removeAllDeliveredNotifications()
        }
        onSettingsChanged?()
    }

    private func enable() async -> Bool {
        guard Self.available else { showUnavailableNotice(); return false }
        if enabled { return true }
        if let task = authorizationTask { return await task.value }
        let task = Task { @MainActor in
            guard let center = Self.center else { return false }
            do {
                let allowed = try await center.requestAuthorization(options: Self.authorizationOptions)
                setEnabled(allowed)
                if !allowed { showPermissionNotice() }
                return allowed
            } catch { showPermissionNotice(); return false }
        }
        authorizationTask = task
        let allowed = await task.value
        authorizationTask = nil
        return allowed
    }

    func toggleEnabled() {
        if enabled { setEnabled(false) }
        else { Task { _ = await enable() } }
    }

    func togglePreviews() {
        let value = !previews
        UserDefaults.standard.set(value, forKey: Self.previewsKey)
        settingsGeneration += 1
        if !value {
            Self.center?.removeAllPendingNotificationRequests()
            Self.center?.removeAllDeliveredNotifications()
        }
        onSettingsChanged?()
    }

    func primeMailBaseline(core: CoreBridge, accountIDs: [String]) async {
        for accountID in accountIDs where !tracker.hasBaseline(accountID: accountID) {
            guard let rows = try? await core.callAsync(["op": "list_threads", "account_id": accountID,
                                                       "unread_only": false, "limit": 100, "offset": 0]) as? [[String: Any]] else { continue }
            _ = tracker.consume(accountID: accountID, rows: rows)
        }
    }

    func recordSync(core: CoreBridge, successfulAccountIDs: [String]) async {
        for accountID in successfulAccountIDs {
            guard let rows = try? await core.callAsync(["op": "list_threads", "account_id": accountID,
                                                       "unread_only": false, "limit": 100, "offset": 0]) as? [[String: Any]] else { continue }
            let candidates = tracker.consume(accountID: accountID, rows: rows)
            guard enabled else { continue }
            var arrivals: [MailArrival] = []
            for arrival in candidates {
                // Thread unread count can belong to an older incoming message
                // while its newest timestamp belongs to our sent reply/draft.
                guard let latest = try? await core.callAsync(["op":"get_latest_message_for_thread", "thread_id":arrival.threadID]) as? [String: Any],
                      latest["account_id"] as? String == arrival.accountID, latest["thread_id"] as? String == arrival.threadID,
                      latest["is_sent"] as? Bool == false, latest["is_draft"] as? Bool == false else { continue }
                let preview = MailArrival(accountID: arrival.accountID, threadID: arrival.threadID,
                                      sender: latest["sender"] as? String ?? arrival.sender,
                                      subject: latest["subject"] as? String ?? arrival.subject,
                                      receivedAt: arrival.receivedAt,
                                      snippet: latest["notification_snippet"] as? String ?? "")
                arrivals.append(preview)
            }
            guard enabled, !arrivals.isEmpty else { continue }
            if arrivals.count > 3 {
                let latest = arrivals[0]
                _ = await deliver(id: "mail-batch-\(accountID)-\(latest.receivedAt)", title: "\(arrivals.count) new emails",
                                  body: previews ? MailNotificationPreview.batch(arrivals.map { ($0.sender, $0.subject) }) : "Click to open your inbox.", arrival: latest)
            } else {
                for arrival in arrivals {
                    let subject = arrival.subject.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
                    _ = await deliver(id: "mail-\(arrival.accountID)-\(arrival.threadID)-\(arrival.receivedAt)",
                                      title: previews ? String(MailNotificationPreview.senderName(arrival.sender).prefix(120)) : "New email",
                                      subtitle: previews ? String((subject.isEmpty ? "(No subject)" : subject).prefix(140)) : "",
                                      body: previews ? arrival.snippet : "Click to read, or mark it read or archive it from here.",
                                      arrival: arrival, category: Self.threadCategory)
                }
            }
        }
    }

    // Called by the local timer, independently of network sync success.
    func recordFollowups(core: CoreBridge) async {
        let time = Date()
        if let last = lastFollowupCheck, time.timeIntervalSince(last) >= 0 && time.timeIntervalSince(last) < 30 { return }
        guard !followupCheckInFlight else { return }
        followupCheckInFlight = true; lastFollowupCheck = time
        defer { followupCheckInFlight = false }
        guard let rows = try? await core.callAsync(["op":"list_followups"]) as? [[String: Any]] else { return }
        let due = followups.consume(rows: rows, enabled: enabled, now: Int64(time.timeIntervalSince1970 * 1000))
        UserDefaults.standard.set(Array(followups.seenIDs).sorted(), forKey: Self.followupSeenKey)
        if due.count > 3, let first = due.first {
            let arrival = MailArrival(accountID: first.accountID, threadID: first.threadID, sender: "", subject: first.subject, receivedAt: first.dueAt)
            if await deliver(id: "followup-batch-" + first.id, title: "\(due.count) follow-ups are due", body: "Open Email to review your follow-ups.", arrival: arrival, kind: "followup") {
                for reminder in due { followups.markDelivered(reminder) }
                UserDefaults.standard.set(Array(followups.seenIDs).sorted(), forKey: Self.followupSeenKey)
            }
            return
        }
        for reminder in due {
            let arrival = MailArrival(accountID: reminder.accountID, threadID: reminder.threadID, sender: "", subject: reminder.subject, receivedAt: reminder.dueAt)
            if await deliver(id: reminder.id, title: "Follow-up due", body: previews ? String(reminder.subject.prefix(240)) : "Open Email to review your follow-up.", arrival: arrival, kind: "followup") {
                followups.markDelivered(reminder)
                UserDefaults.standard.set(Array(followups.seenIDs).sorted(), forKey: Self.followupSeenKey)
            }
        }
    }

    func recordOpens(core: CoreBridge) async {
        let now = Date()
        if let lastOpenCheck, now.timeIntervalSince(lastOpenCheck) < 60 { return }
        guard !openCheckInFlight else { return }
        openCheckInFlight = true; lastOpenCheck = now
        defer { openCheckInFlight = false }
        let generation = settingsGeneration
        guard let data = try? await core.callAsync(["op":"tracking_notifications", "enabled":enabled,
                                                    "session_since":max(sessionSince, (UserDefaults.standard.object(forKey: Self.sinceKey) as? NSNumber)?.int64Value ?? 0)]) as? [String:Any],
              enabled, generation == settingsGeneration,
              let rows = data["items"] as? [[String:Any]] else { return }
        // One alert per message per poll, even if a proxy loaded its image repeatedly.
        if !rows.isEmpty { onOpenActivity?() }
        for row in rows.prefix(3) {
            guard let id = row["id"] as? String, let account = row["account_id"] as? String,
                  let thread = row["thread_id"] as? String else { continue }
            let subject = (row["subject"] as? String ?? "").split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
            let count = row["pixel_requests"] as? Int ?? 0
            let summary = count > 1 ? "Opened \(count) times" : "Opened"
            let arrival = MailArrival(accountID: account, threadID: thread, sender: "", subject: subject, receivedAt: 0)
            _ = await deliver(id: id, title: previews ? "Opened: \(subject.isEmpty ? "(No subject)" : String(subject.prefix(120)))" : "Email opened",
                              body: summary,
                              arrival: arrival, kind: "open")
        }
        if rows.count > 3 {
            _ = await deliver(id: "open-batch-\(rows.count)-\(Int(now.timeIntervalSince1970))", title: "\(rows.count - 3) more emails opened", body: "Open Email to see recent opens.", arrival: nil, kind: "open")
        }
    }

    /// One alert per new agent request or new blocked-access entry, remembered
    /// across launches. Previews off keeps subjects and recipients out of it.
    func recordAgentRequests(_ summary: [String: Any]) async {
        // Oldest first, so trimming keeps the most recent entries.
        var seen = UserDefaults.standard.stringArray(forKey: Self.agentSeenKey) ?? []
        var fresh: [(id: String, title: String, body: String)] = []
        for item in summary["items"] as? [[String: Any]] ?? [] {
            guard let id = item["id"] as? String, !seen.contains(id) else { continue }
            seen.append(id)
            let text = item["summary"] as? String ?? ""
            fresh.append(("agent-\(id)", previews ? "Agent request: \(String(text.prefix(120)))" : "An agent is waiting for your approval", "Review it in Agent access."))
        }
        if let blocked = summary["blocked"] as? [String: Any], let id = blocked["id"] as? String, !seen.contains(id) {
            seen.append(id)
            let text = blocked["summary"] as? String ?? ""
            fresh.append(("agent-blocked-\(id)", "Agent access needed", previews ? "\(String(text.prefix(140))). Open Agent access to turn it on." : "An agent tried to use your mail. Open Agent access to review."))
        }
        guard !fresh.isEmpty else { return }
        UserDefaults.standard.set(Array(seen.suffix(400)), forKey: Self.agentSeenKey)
        for item in fresh.prefix(3) { _ = await deliver(id: item.id, title: item.title, body: item.body, arrival: nil, kind: "agent") }
        if fresh.count > 3 {
            _ = await deliver(id: "agent-batch-\(fresh.count)-\(Int(Date().timeIntervalSince1970))", title: "\(fresh.count - 3) more agent requests", body: "Review them in Agent access.", arrival: nil, kind: "agent")
        }
    }

    func sendTest() {
        Task {
            guard await enable() else { return }
            _ = await deliver(id: "email-notification-test", title: "Email notifications are ready",
                              body: "New mail and due follow-ups appear while Email is running.", arrival: nil, kind: "test")
        }
    }

    private func deliver(id: String, title: String, subtitle: String = "", body: String, arrival: MailArrival?,
                         kind: String = "mail", category: String? = nil) async -> Bool {
        guard enabled, let center = Self.center else { return false }
        let generation = settingsGeneration
        let settings = await center.notificationSettings()
        guard enabled, generation == settingsGeneration else { return false }
        guard settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional else {
            if settings.authorizationStatus == .denied { setEnabled(false) }
            return false
        }
        let content = UNMutableNotificationContent()
        content.title = title; content.subtitle = subtitle; content.body = body
        if let category { content.categoryIdentifier = category }
        content.sound = .default
        content.userInfo = ["kind":kind]
        if let arrival {
            content.threadIdentifier = arrival.accountID
            content.userInfo["account_id"] = arrival.accountID
            content.userInfo["thread_id"] = arrival.threadID
        }
        do {
            try await center.add(UNNotificationRequest(identifier: id, content: content, trigger: nil))
            if !enabled || generation != settingsGeneration {
                center.removePendingNotificationRequests(withIdentifiers: [id])
                center.removeDeliveredNotifications(withIdentifiers: [id])
                return false
            }
            return true
        } catch { return false }
    }

    private func showUnavailableNotice() {
        let alert = NSAlert()
        alert.messageText = "Notifications need the Email app"
        alert.informativeText = "This development build runs without an app bundle, and macOS only delivers notifications to apps. Build the app with scripts/run-macos.sh to use notifications."
        alert.runModal()
    }

    private func showPermissionNotice() {
        let alert = NSAlert()
        alert.messageText = "Notifications are off"
        alert.informativeText = "Allow Email in System Settings → Notifications, then enable notifications here again. Email will not ask on every launch."
        alert.runModal()
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                            willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        let data = notification.request.content.userInfo
        let account = data["account_id"] as? String ?? "", thread = data["thread_id"] as? String ?? ""
        let kind = data["kind"] as? String ?? (notification.request.identifier == "email-notification-test" ? "test" : "mail")
        return await MainActor.run {
            let present = NotificationForegroundPolicy.shouldPresent(kind: kind, enabled: self.enabled, readingSameThread: self.isReadingThread?(account, thread) ?? false)
            return present ? [.banner, .list, .sound] : []
        }
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                            didReceive response: UNNotificationResponse,
                                            withCompletionHandler completionHandler: @escaping () -> Void) {
        let data = response.notification.request.content.userInfo
        let account = data["account_id"] as? String ?? "", thread = data["thread_id"] as? String ?? ""
        let followup = data["kind"] as? String == "followup"
        let tracked = data["kind"] as? String == "open"
        let agent = data["kind"] as? String == "agent"
        let action = response.actionIdentifier
        if action == Self.markReadAction || action == Self.archiveAction {
            let id = response.notification.request.identifier
            center.removeDeliveredNotifications(withIdentifiers: [id])
            Task { @MainActor [weak self] in self?.onTriage?(action == Self.archiveAction ? "archive" : "read", account, thread) }
        } else if action == UNNotificationDefaultActionIdentifier {
            Task { @MainActor [weak self] in
                if agent { self?.onOpenAgent?() }
                else if tracked { self?.onOpenTracked?(account, thread) }
                else if followup { self?.onOpenFollowup?(account, thread) }
                else { self?.onOpen?(account, thread) }
            }
        }
        completionHandler()
    }
}
