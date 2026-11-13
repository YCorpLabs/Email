import Foundation

/// Update rules shared by the Sparkle module, its checks and release tooling.
/// Nothing here touches the network, Sparkle or AppKit.
public enum UpdateChannel: String, CaseIterable, Sendable {
    case stable
    case testing

    /// Sparkle always reads the default channel. Stable items carry no
    /// channel element; testing items carry `<sparkle:channel>testing`.
    public var sparkleChannels: Set<String> { self == .testing ? ["testing"] : [] }
}

public enum UpdateDisabledReason: String, Error, Sendable, Equatable {
    case notConfigured = "not-configured"
    case notReleaseBuild = "not-release-build"
    case insecureFeed = "insecure-feed"
    case untrustedFeedHost = "untrusted-feed-host"
    case invalidPublicKey = "invalid-public-key"
    case invalidBuildVersion = "invalid-build-version"
    case unsafeSparkleSettings = "unsafe-sparkle-settings"
    case invalidChannel = "invalid-channel"
    case fixtureNotAllowed = "fixture-not-allowed"
    case signatureInvalid = "signature-invalid"
}

public struct UpdateConfiguration: Sendable, Equatable {
    public let feedURL: URL
    public let publicKey: String
    public let buildVersion: Int
    public let defaultChannel: UpdateChannel
    public let isFixture: Bool
}

public enum UpdatePolicy {
    /// Info.plist array of HTTPS hosts the feed may use, written from the
    /// distribution profile's UPDATE_ALLOWED_HOSTS. Absent or empty disables
    /// updates, so a fork can never follow another publisher's feed.
    public static let allowedHostsKey = "EmailUpdateAllowedHosts"
    /// Disposable acceptance-test bundles only; the reserved .invalid domain
    /// can never be a published app.
    public static let fixtureBundleIdentifier = "invalid.email.update-fixture"
    public static let channelDefaultsKey = "EmailUpdateChannel"

    /// Validates the signed Info.plist. Any doubt disables updates.
    public static func evaluate(_ info: [String: Any]) -> Result<UpdateConfiguration, UpdateDisabledReason> {
        guard info["EmailReleaseBuild"] as? Bool == true else { return .failure(.notReleaseBuild) }
        guard let feed = (info["SUFeedURL"] as? String)?.trimmingCharacters(in: .whitespaces), !feed.isEmpty else {
            return .failure(.notConfigured)
        }
        let isFixture = info["EmailUpdateFixture"] as? Bool == true
        if !isFixture && allowedHosts(info).isEmpty { return .failure(.notConfigured) }
        if isFixture && info["CFBundleIdentifier"] as? String != fixtureBundleIdentifier {
            return .failure(.fixtureNotAllowed)
        }
        guard let url = URL(string: feed), let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let scheme = components.scheme?.lowercased(), let host = components.host?.lowercased(),
              components.user == nil, components.password == nil, components.fragment == nil,
              components.path.count > 1 else {
            return .failure(.insecureFeed)
        }
        if isFixture {
            // Fixture bundles may only reach a loopback server on this Mac.
            guard scheme == "http" || scheme == "https" else { return .failure(.insecureFeed) }
            guard host == "127.0.0.1" else { return .failure(.untrustedFeedHost) }
        } else {
            guard scheme == "https", components.port == nil || components.port == 443 else { return .failure(.insecureFeed) }
            guard allowedHosts(info).contains(host) else { return .failure(.untrustedFeedHost) }
        }
        guard let key = info["SUPublicEDKey"] as? String, isValidPublicKey(key) else { return .failure(.invalidPublicKey) }
        guard let versionText = info["CFBundleVersion"] as? String, let version = buildNumber(versionText) else {
            return .failure(.invalidBuildVersion)
        }
        guard hasSafeSparkleSettings(info) else { return .failure(.unsafeSparkleSettings) }
        let channelText = info["EmailUpdateDefaultChannel"] as? String ?? UpdateChannel.stable.rawValue
        guard let channel = UpdateChannel(rawValue: channelText) else { return .failure(.invalidChannel) }
        return .success(UpdateConfiguration(feedURL: url, publicKey: key, buildVersion: version,
                                            defaultChannel: channel, isFixture: isFixture))
    }

    public static func allowedHosts(_ info: [String: Any]) -> Set<String> {
        Set((info[allowedHostsKey] as? [String] ?? []).map { $0.lowercased() }.filter { !$0.isEmpty })
    }

    /// Sparkle keys must enforce verification before extraction, a signed feed
    /// whose failures never expire, and opt-in background behavior.
    static func hasSafeSparkleSettings(_ info: [String: Any]) -> Bool {
        guard info["SUVerifyUpdateBeforeExtraction"] as? Bool == true,
              info["SURequireSignedFeed"] as? Bool == true,
              (info["SUSignedFeedFailureExpirationInterval"] as? NSNumber)?.intValue == 0 else { return false }
        // Never force background checks or silent installs on users.
        if info["SUEnableAutomaticChecks"] as? Bool == true { return false }
        if info["SUAutomaticallyUpdate"] as? Bool == true { return false }
        // XPC services are for sandboxed apps. Email is not sandboxed.
        if info["SUEnableInstallerLauncherService"] as? Bool == true { return false }
        if info["SUEnableDownloaderService"] as? Bool == true { return false }
        if info["SUEnableJavaScript"] as? Bool == true { return false }
        if info["SUEnableSystemProfiling"] as? Bool == true { return false }
        return true
    }

    public static func isValidPublicKey(_ text: String) -> Bool {
        guard let data = Data(base64Encoded: text), data.count == 32 else { return false }
        return data.contains { $0 != 0 }
    }

    /// Build numbers are positive decimal integers so ordering is unambiguous.
    public static func buildNumber(_ text: String) -> Int? {
        guard !text.isEmpty, text.count <= 9, text.allSatisfy({ $0.isASCII && $0.isNumber }),
              !text.hasPrefix("0"), let value = Int(text), value > 0 else { return nil }
        return value
    }

    public static func selectedChannel(stored: String?, configuration: UpdateConfiguration) -> UpdateChannel {
        stored.flatMap(UpdateChannel.init(rawValue:)) ?? configuration.defaultChannel
    }

    /// Final gate before Sparkle shows, downloads or installs an item.
    public static func itemRejection(itemVersion: String, itemChannel: String?, feedSigned: Bool,
                                     configuration: UpdateConfiguration, channel: UpdateChannel) -> String? {
        guard feedSigned else { return "The update feed signature is not valid." }
        guard let version = buildNumber(itemVersion), version > configuration.buildVersion else {
            return "The update version is not newer than this build."
        }
        if let itemChannel, !channel.sparkleChannels.contains(itemChannel) {
            return "The update is not on the selected channel."
        }
        return nil
    }

    /// An open composer always defers a restart. The user finishes first.
    public static func shouldPostponeRelaunch(hasActiveDraft: Bool) -> Bool { hasActiveDraft }
}
