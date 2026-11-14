import Foundation
import Darwin

struct MailAccount: Sendable {
    let id: String
    let provider: String
    let email: String
    let displayName: String
}

struct RenderedMessage: Sendable {
    let bodyPlain: String
    let bodyHTMLSafe: String?
}

/// The time in a list row, as the inbox shows it: today the time, older the date.
enum MailListTime {
    static func string(for date: Date) -> String {
        let today = Calendar.current.isDateInToday(date)
        return DateFormatter.localizedString(from: date, dateStyle: today ? .none : .short, timeStyle: today ? .short : .none)
    }
}

struct MailDraft: Sendable {
    let id: String
    let accountID: String
    let from: String
    let to: String
    let cc: String
    let bcc: String
    let subject: String
    let bodyPlain: String
}

struct MailSplit: Sendable {
    let id: String
    let name: String
    let query: String
}

struct OutboxItem: Sendable {
    let messageID: String
    let accountID: String
    let to: String
    let subject: String
    let bodyPreview: String
    let sendAt: Date
    let status: String
    let lastError: String?
}

/// What this build offers, from the Rust service (`build_features`). The
/// Linux shell reads the same request, so both hide the same features.
struct BuildFeatures: Sendable, Equatable {
    let channel: String
    let googleContactPhotos: Bool
    let microsoftAccounts: Bool
    /// "Google OAuth publicly available" (the profile's `GOOGLE_OAUTH_PUBLIC`).
    /// Off in official releases until Google's review passes: account setup then
    /// offers Gmail only through an app password. Connected accounts keep working.
    let googleOAuthPublic: Bool

    /// Used if the service cannot answer: withhold optional features.
    static let restricted = BuildFeatures(channel: "unknown", googleContactPhotos: false, microsoftAccounts: false, googleOAuthPublic: false)

    init(channel: String, googleContactPhotos: Bool, microsoftAccounts: Bool, googleOAuthPublic: Bool) {
        self.channel = channel
        self.googleContactPhotos = googleContactPhotos
        self.microsoftAccounts = microsoftAccounts
        self.googleOAuthPublic = googleOAuthPublic
    }

    init(response: Any?) {
        guard let value = response as? [String: Any], let channel = value["channel"] as? String else {
            self = .restricted
            return
        }
        self.init(channel: channel,
                  googleContactPhotos: value["google_contact_photos"] as? Bool ?? false,
                  microsoftAccounts: value["microsoft_accounts"] as? Bool ?? false,
                  googleOAuthPublic: value["google_oauth_public"] as? Bool ?? false)
    }
}

/// Features are fixed when the Rust library is compiled. Read them once, so a
/// later caller never waits behind a long request on the shared handle.
private final class BuildFeatureCache: @unchecked Sendable {
    let lock = NSLock()
    var value: BuildFeatures?
}

struct MailServiceError: LocalizedError, Sendable {
    let message: String
    /// Set when `add_imap_account` failed: the engine's one-line explanation,
    /// fix link and server details (`setup_error`, docs/service-api.md).
    var setupProblem: SetupProblem? = nil
    var errorDescription: String? { message }
}

/// Server settings the engine guessed for an address (`servers` in `setup_error`).
struct ServerGuess: Sendable, Equatable {
    let imapHost: String, imapPort: Int, imapImplicitTLS: Bool
    let smtpHost: String, smtpPort: Int, smtpImplicitTLS: Bool

    init?(_ value: Any?) {
        guard let value = value as? [String: Any],
              let imapHost = value["imap_host"] as? String, let imapPort = value["imap_port"] as? Int,
              let smtpHost = value["smtp_host"] as? String, let smtpPort = value["smtp_port"] as? Int else { return nil }
        self.imapHost = imapHost; self.imapPort = imapPort
        imapImplicitTLS = value["imap_security"] as? String != "start_tls"
        self.smtpHost = smtpHost; self.smtpPort = smtpPort
        smtpImplicitTLS = value["smtp_security"] as? String == "implicit_tls"
    }
}

/// An account-setup failure as the engine explains it. Linux reads the same
/// object (`apps/linux/src/account_setup.rs`).
struct SetupProblem: Sendable, Equatable {
    let code: String
    let message: String
    let actionLabel: String?
    let actionURL: URL?
    let detail: String
    let servers: ServerGuess?

    init?(_ value: Any?) {
        guard let value = value as? [String: Any], let code = value["code"] as? String,
              let message = value["message"] as? String else { return nil }
        self.code = code
        self.message = message
        let action = value["action"] as? [String: Any]
        let url = (action?["url"] as? String).flatMap(URL.init(string:))
        // Only web pages: a fix link never opens anything else.
        actionURL = url?.scheme == "https" ? url : nil
        actionLabel = actionURL == nil ? nil : action?["label"] as? String
        detail = value["detail"] as? String ?? ""
        servers = ServerGuess(value["servers"])
    }
}

/// One long-lived Rust service handle. Calls may perform network I/O, so UI callers
/// use callAsync. The lock also keeps FFI requests serial while the model evolves.
final class CoreBridge: @unchecked Sendable {
    private typealias Open = @convention(c) (UnsafePointer<CChar>?) -> UnsafeMutableRawPointer?
    private typealias Close = @convention(c) (UnsafeMutableRawPointer?) -> Void
    private typealias Request = @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?
    private typealias Free = @convention(c) (UnsafeMutablePointer<CChar>?) -> Void
    private let lock = NSLock()
    private let library: UnsafeMutableRawPointer
    private var store: UnsafeMutableRawPointer
    private let databasePath: String
    private let openFunction: Open
    private let closeFunction: Close
    private let requestFunction: Request
    private let freeFunction: Free
    // Network sync can take seconds. Keep it on a separate service connection so
    // cached inbox and message reads are never queued behind the network call.
    private lazy var syncBridge: CoreBridge? = CoreBridge()
    private lazy var calendarBridge: CoreBridge? = CoreBridge()
    private lazy var attachmentBridge: CoreBridge? = CoreBridge()
    private lazy var headersBridge: CoreBridge? = CoreBridge()
    private lazy var archiveBridge: CoreBridge? = CoreBridge()
    private lazy var photoBridge: CoreBridge? = CoreBridge()
    private lazy var inlineBridge: CoreBridge? = CoreBridge()
    private lazy var outgoingBridge: CoreBridge? = CoreBridge()
    private lazy var aliasesBridge: CoreBridge? = CoreBridge()
    private lazy var bookingBridge: CoreBridge? = CoreBridge()
    @MainActor private var syncFlights: [String: Task<Data, Error>] = [:]

    private static func libraryCandidates() -> [String] {
        let environment = ProcessInfo.processInfo.environment
        let bundledLibrary = Bundle.main.privateFrameworksPath.map { $0 + "/libmail_service.dylib" }
        // A packaged app must use the library shipped inside its signed bundle.
        // Development builds can still point at a local Rust build explicitly.
        if Bundle.main.object(forInfoDictionaryKey: "EmailReleaseBuild") as? Bool == true {
            // A missing distribution library is an installation error, never a
            // reason to search environment variables or the current directory.
            return bundledLibrary.map { [$0] } ?? []
        } else if let bundledLibrary, FileManager.default.fileExists(atPath: bundledLibrary) {
            return [bundledLibrary]
        }
        return [environment["MAIL_SERVICE_LIBRARY"],
                "../../target/debug/libmail_service.dylib",
                "target/debug/libmail_service.dylib"].compactMap { $0 }
    }

    /// The first loadable engine with the bridge entry points, as init uses.
    private static func openLibrary(_ path: String) -> UnsafeMutableRawPointer? {
        guard let handle = dlopen(path, RTLD_NOW | RTLD_LOCAL) else { return nil }
        for symbol in ["mail_service_open", "mail_service_request_json", "mail_service_free_string", "mail_service_close"]
            where dlsym(handle, symbol) == nil {
            dlclose(handle)
            return nil
        }
        return handle
    }

    /// Reads the profile compiled into the same engine the bridge would load.
    /// An engine without a profile yields nil rather than any default.
    static func loadDistributionProfile() -> DistributionProfile? {
        typealias ProfileJSON = @convention(c) () -> UnsafeMutablePointer<CChar>?
        // Keep this reference open: the bridge maps the same image for the
        // process lifetime, and unloading a Rust library is never needed.
        guard let handle = libraryCandidates().lazy.compactMap(openLibrary).first else { return nil }
        guard let profileSymbol = dlsym(handle, "mail_service_distribution_profile_json"),
              let freeSymbol = dlsym(handle, "mail_service_free_string"),
              let raw = unsafeBitCast(profileSymbol, to: ProfileJSON.self)() else { return nil }
        defer { unsafeBitCast(freeSymbol, to: Free.self)(raw) }
        return DistributionProfile(json: Data(String(cString: raw).utf8))
    }

    init?() {
        let environment = ProcessInfo.processInfo.environment
        var chosen: (UnsafeMutableRawPointer, Open, Close, Request, Free)?
        for path in Self.libraryCandidates() {
            guard let handle = Self.openLibrary(path),
                  let openSymbol = dlsym(handle, "mail_service_open"),
                  let requestSymbol = dlsym(handle, "mail_service_request_json"),
                  let freeSymbol = dlsym(handle, "mail_service_free_string"),
                  let closeSymbol = dlsym(handle, "mail_service_close") else { continue }
            chosen = (
                handle,
                unsafeBitCast(openSymbol, to: Open.self),
                unsafeBitCast(closeSymbol, to: Close.self),
                unsafeBitCast(requestSymbol, to: Request.self),
                unsafeBitCast(freeSymbol, to: Free.self)
            )
            break
        }
        guard let (library, open, close, request, free) = chosen else { return nil }
        let databasePath: String
        // An empty value is a mistake, never a request for the real store.
        guard !IsolatedStore.isEmpty else {
            fputs("\(IsolatedStore.emptyMessage)\n", stderr)
            dlclose(library)
            return nil
        }
        if let override = IsolatedStore.path {
            databasePath = override
        } else {
            guard let base = DistributionProfile.current?.applicationSupportDirectory,
                  (try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)) != nil,
                  (try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: base.path)) != nil else {
                dlclose(library)
                return nil
            }
            databasePath = base.appendingPathComponent("mail.sqlite").path
        }
        guard let store = databasePath.withCString({ open($0) }) else {
            dlclose(library)
            return nil
        }
        self.library = library
        self.store = store
        self.databasePath = databasePath
        self.openFunction = open
        self.closeFunction = close
        self.requestFunction = request
        self.freeFunction = free
    }

    deinit {
        closeFunction(store)
        dlclose(library)
    }

    private func reopenAfterExternalAccountChange() throws {
        lock.lock()
        defer { lock.unlock() }
        guard let fresh = databasePath.withCString({ openFunction($0) }) else {
            throw MailServiceError(message: "Account connected, but the mail engine could not reload it. Please relaunch the app.")
        }
        let old = store
        store = fresh
        closeFunction(old)
    }

    private func exchange(_ requestData: Data) throws -> Data {
        lock.lock()
        defer { lock.unlock() }
        guard let json = String(data: requestData, encoding: .utf8),
              let pointer = json.withCString({ requestFunction(store, $0) }) else {
            throw MailServiceError(message: "Could not call the mail engine.")
        }
        let response = String(cString: pointer)
        freeFunction(pointer)
        guard let data = response.data(using: .utf8) else {
            throw MailServiceError(message: "The mail engine returned an invalid response.")
        }
        return data
    }

    private func parse(_ data: Data) throws -> Any? {
        guard let envelope = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw MailServiceError(message: "The mail engine returned an invalid response.")
        }
        guard envelope["ok"] as? Bool == true else {
            throw MailServiceError(message: envelope["error"] as? String ?? "The mail action failed.",
                                   setupProblem: SetupProblem(envelope["setup_error"]))
        }
        return envelope["data"]
    }

    func request(_ payload: [String: Any]) throws -> Any? {
        let data = try JSONSerialization.data(withJSONObject: payload)
        return try parse(exchange(data))
    }

    @MainActor
    func callAsync(_ payload: [String: Any]) async throws -> Any? {
        let data = try JSONSerialization.data(withJSONObject: payload)
        let operation = payload["op"] as? String
        // Repeated local actions share one network sync instead of building an
        // unbounded queue on syncBridge's lock. Local writes never wait here.
        if operation == "sync_account" || operation == "sync_all" {
            let key = operation == "sync_all" ? "all" : (payload["account_id"] as? String ?? "all")
            if let existing = syncFlights["all"] ?? syncFlights[key] {
                return try parse(await existing.value)
            }
            guard let bridge = syncBridge else { throw MailServiceError(message: "Background sync unavailable; cached mail remains readable.") }
            let flight = Task.detached(priority: .utility) { try bridge.exchange(data) }
            syncFlights[key] = flight
            defer { syncFlights.removeValue(forKey: key) }
            return try parse(await flight.value)
        }
        let photoAccountID = payload["account_id"] as? String
        let isBooking = operation?.hasPrefix("booking_") == true || operation?.hasPrefix("tracking_") == true
        let dedicatedBooking = isBooking ? bookingBridge : nil
        if isBooking && dedicatedBooking == nil { throw MailServiceError(message:"Scheduling connection unavailable; cached mail remains readable.") }
        // Account setup can wait on a browser or a slow mail server. Run it on its
        // own handle so the window and cached reads never queue behind it.
        let isOAuth = operation == "start_oauth" || operation == "add_imap_account"
        // Connecting an agent runs the claude or codex CLI, which can take a few
        // seconds; give it its own handle so cached reads never wait behind it.
        let isAgentClient = operation == "agent_clients" || operation == "connect_agent_client"
        let isPhotoOAuth = operation == "enable_google_photos"
        let isCalendarNetwork = operation == "connect_calendar" || operation == "sync_calendar" || operation == "respond_calendar_invitation" || operation == "create_calendar_event" || operation == "delete_calendar_event"
        let dedicatedCalendar = isCalendarNetwork ? calendarBridge : nil
        let dedicatedSync = operation == "refresh_recipient_headers" ? headersBridge : operation == "sync_archives" ? archiveBridge : nil
        let dedicatedAttachment = (operation == "list_attachments" || operation == "fetch_attachment" || operation == "refresh_calendar_invitation") ? attachmentBridge : nil
        let isInline = operation == "render_message" && payload["include_inline_images"] as? Bool == true
        let dedicatedInline = isInline ? inlineBridge : nil
        let isOutgoing = operation == "dispatch_due" || operation == "send_draft"
        let dedicatedOutgoing = isOutgoing ? outgoingBridge : nil
        let isCleanup = operation?.hasSuffix("sender_cleanup") == true || operation == "list_sender_cleanups"
        let dedicatedAliases = (operation == "list_send_as" || isCleanup) ? aliasesBridge : nil
        let requiresNetwork = isCalendarNetwork || isCleanup || isInline || isOutgoing || operation == "list_send_as" || operation == "refresh_recipient_headers" || operation == "sync_archives" || operation == "list_attachments" || operation == "fetch_attachment" || operation == "refresh_calendar_invitation"
        guard !requiresNetwork || dedicatedCalendar != nil || dedicatedInline != nil || dedicatedOutgoing != nil || dedicatedAliases != nil || dedicatedSync != nil || dedicatedAttachment != nil else {
            throw MailServiceError(message: "Background mail connection unavailable; cached mail remains readable.")
        }
        let dedicatedPhoto = (operation == "google_photo_state" || operation == "google_contact_photo") ? photoBridge : nil
        let response = try await Task.detached(priority: .userInitiated) { [self] in
            if isPhotoOAuth {
                // Use the already-open photo service. Opening another SQLite handle
                // during active sync can fail, and the OAuth browser would never open.
                guard let dedicatedPhoto = photoBridge else {
                    throw MailServiceError(message: "Could not open the mail engine for Google photos.")
                }
                let result = try dedicatedPhoto.exchange(data)
                if let accountID = photoAccountID,
                   let reload = try? JSONSerialization.data(withJSONObject: ["op": "reload_account_credentials", "account_id": accountID]) {
                    _ = try exchange(reload)
                    if let syncBridge { _ = try syncBridge.exchange(reload) }
                    if let attachmentBridge { _ = try attachmentBridge.exchange(reload) }
                }
                return result
            }
            if isAgentClient {
                guard let separateHandle = CoreBridge() else {
                    throw MailServiceError(message: "Could not open the mail engine to check agents.")
                }
                return try separateHandle.exchange(data)
            }
            if isOAuth {
                guard let separateHandle = CoreBridge() else {
                    throw MailServiceError(message: operation == "add_imap_account"
                        ? "Could not open the mail engine to add the account. Try again in a moment."
                        : "Could not open the mail engine for browser sign-in.")
                }
                let result = try separateHandle.exchange(data)
                // Reload only when an account was actually added.
                if (try? JSONSerialization.jsonObject(with: result) as? [String: Any])?["ok"] as? Bool == true {
                    try reopenAfterExternalAccountChange()
                }
                return result
            }
            if let dedicatedBooking { return try dedicatedBooking.exchange(data) }
            if let dedicatedCalendar { return try dedicatedCalendar.exchange(data) }
            if let dedicatedSync { return try dedicatedSync.exchange(data) }
            if let dedicatedAttachment { return try dedicatedAttachment.exchange(data) }
            if let dedicatedPhoto { return try dedicatedPhoto.exchange(data) }
            if let dedicatedInline { return try dedicatedInline.exchange(data) }
            if let dedicatedOutgoing { return try dedicatedOutgoing.exchange(data) }
            if let dedicatedAliases { return try dedicatedAliases.exchange(data) }
            return try exchange(data)
        }.value
        return try parse(response)
    }

    func call(_ payload: [String: Any]) -> Any? { try? request(payload) }

    private static let featureCache = BuildFeatureCache()

    func buildFeatures() -> BuildFeatures {
        Self.featureCache.lock.lock()
        defer { Self.featureCache.lock.unlock() }
        if let cached = Self.featureCache.value { return cached }
        // The first answer sticks, so the offered choices never change while the
        // app runs. If the library could not answer, optional features stay off.
        let features = BuildFeatures(response: call(["op": "build_features"]))
        Self.featureCache.value = features
        return features
    }

    func accountRecords() -> [MailAccount] {
        (try? accountRecordsChecked()) ?? []
    }

    func accountRecordsChecked() throws -> [MailAccount] {
        guard let records = try request(["op": "list_accounts"]) as? [[String: Any]] else {
            throw MailServiceError(message: "Could not read accounts from the mail cache.")
        }
        return records.compactMap { item in
            guard let id = item["id"] as? String,
                  let provider = item["provider"] as? String,
                  let email = item["email"] as? String else { return nil }
            return MailAccount(id: id, provider: provider, email: email, displayName: item["display_name"] as? String ?? email)
        }
    }

    func drafts() -> [MailDraft] {
        (call(["op": "list_drafts"]) as? [[String: Any]] ?? []).compactMap { item in
            guard let id = item["id"] as? String,
                  let accountID = item["account_id"] as? String else { return nil }
            let to = (item["to"] as? String) ?? (item["to"] as? [String])?.joined(separator: ", ") ?? ""
            let headers = item["recipient_headers"] as? [String:Any]
            let cc = (headers?["cc"] as? [String] ?? []).joined(separator: ", ")
            let bcc = (headers?["bcc"] as? [String] ?? []).joined(separator: ", ")
            return MailDraft(id: id, accountID: accountID, from: item["sender"] as? String ?? "", to: to, cc: cc, bcc: bcc, subject: item["subject"] as? String ?? "", bodyPlain: item["body_plain"] as? String ?? "")
        }
    }

    func splits() -> [MailSplit] {
        (call(["op": "list_splits"]) as? [[String: Any]] ?? []).compactMap { item in
            guard let id = item["id"] as? String, let name = item["name"] as? String else { return nil }
            return MailSplit(id: id, name: name, query: item["query"] as? String ?? "")
        }
    }

    func outbox() -> [OutboxItem] {
        (call(["op": "list_outbox"]) as? [[String: Any]] ?? []).compactMap { item in
            guard let id = item["message_id"] as? String,
                  let accountID = item["account_id"] as? String else { return nil }
            let to = (item["to"] as? String) ?? (item["to"] as? [String])?.joined(separator: ", ") ?? ""
            let ms = (item["send_at_ms"] as? NSNumber)?.doubleValue ?? 0
            return OutboxItem(messageID: id, accountID: accountID, to: to, subject: item["subject"] as? String ?? "", bodyPreview: item["body_preview"] as? String ?? "", sendAt: Date(timeIntervalSince1970: ms / 1000), status: item["status"] as? String ?? "scheduled", lastError: item["last_error"] as? String)
        }
    }

    func threads(query: String? = nil, limit: Int = 100, accountID: String? = nil, splitID: String? = nil, unreadOnly: Bool = false, snoozed: Bool = false, pinned: Bool = false, followups: Bool = false, label: String? = nil, archived: Bool = false) -> [MailThread] {
        (try? threadsChecked(query: query, limit: limit, accountID: accountID, splitID: splitID, unreadOnly: unreadOnly, snoozed: snoozed, pinned: pinned, followups: followups, label: label, archived: archived)) ?? []
    }

    func threadsChecked(query: String? = nil, limit: Int = 100, accountID: String? = nil, splitID: String? = nil, unreadOnly: Bool = false, snoozed: Bool = false, pinned: Bool = false, followups: Bool = false, label: String? = nil, archived: Bool = false, sent: Bool = false) throws -> [MailThread] {
        var summaries: [[String: Any]] = []
        while summaries.count < limit {
            let pageSize = min(500, limit - summaries.count)
            var payload: [String: Any] = ["op":"search", "query":query ?? "", "limit":pageSize, "offset":summaries.count, "unread_only":unreadOnly, "starred_only":pinned, "snoozed_only":snoozed, "followups_only":followups]
            if let accountID { payload["account_id"] = accountID }
            if let splitID { payload["split_id"] = splitID }
            if let label { payload["label"] = label }
            if sent { payload["folder"] = "sent" }
            else if archived { payload["folder"] = "archive" }
            else if !snoozed && !pinned && !followups && label == nil { payload["is_inbox"] = true }
            guard let page = try request(payload) as? [[String: Any]] else { throw MailServiceError(message: "Could not search downloaded mail.") }
            summaries.append(contentsOf: page)
            if page.count < pageSize { break }
        }
        return mapThreads(summaries, accounts: try accountRecordsChecked())
    }

    private func mapThreads(_ summaries: [[String: Any]], accounts accountRecords: [MailAccount]) -> [MailThread] {
        let accounts = Dictionary(uniqueKeysWithValues: accountRecords.map { ($0.id, $0.email) })
        return summaries.compactMap { item in
            guard let id = item["id"] as? String,
                  let accountID = item["account_id"] as? String,
                  let sender = item["sender"] as? String,
                  let subject = item["subject"] as? String,
                  let preview = item["snippet"] as? String else { return nil }
            let milliseconds = (item["last_message_at"] as? NSNumber)?.doubleValue ?? 0
            let date = Date(timeIntervalSince1970: milliseconds / 1000)
            let time = MailListTime.string(for: date)
            return MailThread(id: id, accountID: accountID, account: accounts[accountID] ?? accountID, sender: sender, subject: subject, preview: preview, time: time, unread: ((item["unread_count"] as? NSNumber)?.intValue ?? 0) > 0, html: "", labels: item["labels"] as? [String] ?? [], pinned: item["is_pinned"] as? Bool ?? false, snoozedUntil: (item["snoozed_until_ms"] as? NSNumber).map { Date(timeIntervalSince1970: $0.doubleValue / 1000) })
        }
    }

    func renderedMessage(threadID: String) -> RenderedMessage? {
        guard let message = call(["op": "get_latest_message_for_thread", "thread_id": threadID]) as? [String: Any],
              let id = message["id"] as? String,
              let rendered = call(["op": "render_message", "message_id": id]) as? [String: Any] else { return nil }
        return RenderedMessage(bodyPlain: rendered["body_plain"] as? String ?? "", bodyHTMLSafe: rendered["body_html_safe"] as? String)
    }

    func plainBody(threadID: String) -> String? { renderedMessage(threadID: threadID)?.bodyPlain }

    func archive(threadID: String) -> Bool {
        call(["op": "apply_action", "operation_id": UUID().uuidString, "action": ["type": "archive", "thread_id": threadID]]) != nil
    }
}

/// The one rule for an isolated store: a non-empty EMAIL_STORE_PATH (an empty
/// one is refused). The mail cache, demo data and custom pictures follow it.
/// mail-service decides whether a path is the real store (its Scheduling
/// credential and demo-data refusal), so that rule lives once.
enum IsolatedStore {
    static let path: String? = {
        guard let path = ProcessInfo.processInfo.environment["EMAIL_STORE_PATH"], !path.isEmpty else { return nil }
        return path
    }()

    /// EMAIL_STORE_PATH is set but empty. The app stops at launch (as on Linux).
    static let isEmpty = ProcessInfo.processInfo.environment["EMAIL_STORE_PATH"] == ""
    static let emptyMessage = "EMAIL_STORE_PATH is empty. Set a database path or unset it."

    /// EMAIL_DEMO=1 (or small) adds three demo messages; EMAIL_DEMO=launch adds
    /// the launch demo mailbox. Only an isolated store is seeded; mail-service
    /// also refuses the real one however it is spelled, so demo messages never reach it.
    static let demoSet: String? = {
        guard let value = ProcessInfo.processInfo.environment["EMAIL_DEMO"], !value.isEmpty else { return nil }
        guard ["1", "small", "launch"].contains(value) else {
            fputs("EMAIL_DEMO=\(value) is not a demo set. Use 1 (or small) or launch.\n", stderr)
            return nil
        }
        guard IsolatedStore.path != nil else {
            fputs("EMAIL_DEMO=\(value) is ignored without EMAIL_STORE_PATH; demo messages only go into an isolated store.\n", stderr)
            return nil
        }
        return value == "launch" ? "launch" : "small"
    }()

    /// The mail-service request for the selected demo set. Launch dates follow
    /// this Mac's time zone, so the inbox reads today, yesterday and this week.
    static var demoRequest: [String: Any]? {
        switch demoSet {
        case "launch": ["op": "seed_demo", "set": "launch", "utc_offset_minutes": TimeZone.current.secondsFromGMT() / 60]
        case "small": ["op": "seed_demo"]
        default: nil
        }
    }
}

extension DistributionProfile {
    /// Loaded once from the engine library; nil when no engine can be loaded.
    static let current: DistributionProfile? = CoreBridge.loadDistributionProfile()

    /// `~/Library/Application Support/<profile directory>`.
    var applicationSupportDirectory: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent(dataDirectory, isDirectory: true)
    }
}
