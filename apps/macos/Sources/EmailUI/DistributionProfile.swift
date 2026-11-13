import Foundation

/// The build's distribution profile, compiled into the mail engine from
/// `config/profiles` (see FORKING.md). Swift keeps no defaults: without a
/// profile there is no support channel, Scheduling service or data directory,
/// so an unofficial build can never reach Y Corp AS's services or data.
struct DistributionProfile: Sendable, Equatable {
    let id: String
    let supportAddress: String?
    let feedbackAddress: String?
    let schedulingOrigin: URL?
    /// Per-draft open tracking (Track opens), compiled in from the profile's
    /// `OPEN_TRACKING`. Never true without a Scheduling service.
    let openTracking: Bool
    let dataDirectory: String
    /// The website's step-by-step setup guide; only for builds whose support
    /// channel is Y Corp's own (the engine decides, `profile::setup_guide_url`).
    let setupGuide: URL?

    /// Shown instead of the support chooser when the build has no channels.
    static let supportUnavailableMessage = "This build has no support address configured, so Email can't start a support or feedback message.\n\nIf someone else gave you this build, ask them for help. If you built it yourself, see FORKING.md to add your own support channels."

    init?(json data: Data) {
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let id = object["profile"] as? String, !id.isEmpty,
              let directory = object["macos_data_directory"] as? String, Self.isDirectoryName(directory) else { return nil }
        self.id = id
        dataDirectory = directory
        let support = Self.mailbox(object["support_address"])
        let feedback = Self.mailbox(object["feedback_address"])
        // Both channels or neither. A partial profile has no support channel.
        if let support, let feedback, support.caseInsensitiveCompare(feedback) != .orderedSame {
            supportAddress = support
            feedbackAddress = feedback
        } else {
            supportAddress = nil
            feedbackAddress = nil
        }
        schedulingOrigin = Self.origin(object["scheduling_origin"])
        openTracking = object["open_tracking"] as? Bool == true && schedulingOrigin != nil
        let guide = (object["setup_guide_url"] as? String).flatMap(URLComponents.init(string:))
        setupGuide = guide?.scheme == "https" && guide?.host != nil && supportAddress != nil ? guide?.url : nil
    }

    var hasScheduling: Bool { schedulingOrigin != nil }

    /// A section of the setup guide, such as `gmail`.
    func setupGuideURL(_ section: String) -> URL? {
        guard let setupGuide, var components = URLComponents(url: setupGuide, resolvingAgainstBaseURL: false) else { return nil }
        components.fragment = section
        return components.url
    }

    /// True when the To field addresses this build's feedback mailbox.
    func isFeedbackRecipient(_ to: String) -> Bool {
        guard let feedbackAddress else { return false }
        return (ComposerAddressRules.recipients(to) ?? []).contains {
            ComposerAddressRules.mailbox($0).caseInsensitiveCompare(feedbackAddress) == .orderedSame
        }
    }

    /// A page on the Scheduling service, such as `/studio#analytics`.
    func schedulingURL(path: String, fragment: String? = nil) -> URL? {
        guard let schedulingOrigin, var components = URLComponents(url: schedulingOrigin, resolvingAgainstBaseURL: false) else { return nil }
        components.path = path
        components.percentEncodedFragment = fragment
        return components.url
    }

    /// Only HTTPS pages on the configured Scheduling host are trusted.
    func isSchedulingURL(_ url: URL) -> Bool {
        guard let schedulingOrigin else { return false }
        // An explicit :443 is the default port; the Linux URL parser treats it the same.
        return url.scheme == "https" && (url.port == nil || url.port == 443) && url.user == nil && url.password == nil
            && url.host?.lowercased() == schedulingOrigin.host?.lowercased()
    }

    private static func isDirectoryName(_ value: String) -> Bool {
        !value.isEmpty && value.count <= 64 && !value.hasPrefix(".") && !value.contains("/") && !value.contains("..")
            && value.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || " ._-".contains($0)) }
    }

    private static func mailbox(_ value: Any?) -> String? {
        guard let value = value as? String, let recipients = ComposerAddressRules.recipients(value),
              recipients.count == 1, ComposerAddressRules.mailbox(recipients[0]) == value else { return nil }
        return value
    }

    private static func origin(_ value: Any?) -> URL? {
        guard let value = value as? String, let components = URLComponents(string: value),
              components.scheme == "https", let host = components.host, !host.isEmpty,
              components.port == nil, components.user == nil, components.password == nil,
              components.path.isEmpty, components.query == nil, components.fragment == nil else { return nil }
        return components.url
    }
}
