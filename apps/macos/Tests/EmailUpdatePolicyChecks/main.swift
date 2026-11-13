import EmailUpdatePolicy
import Foundation

// Release tooling: validate a packaged Info.plist with the shipping rules.
// Prints the result; exits 0 only when updates would be enabled, or when the
// feed is absent and --allow-unconfigured is passed.
if CommandLine.arguments.count >= 3, CommandLine.arguments[1] == "--validate-info" {
    guard let data = FileManager.default.contents(atPath: CommandLine.arguments[2]),
          let info = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any] else {
        print("unreadable"); exit(2)
    }
    switch UpdatePolicy.evaluate(info) {
    case .success(let configuration):
        print("enabled \(configuration.feedURL.absoluteString) build \(configuration.buildVersion) channel \(configuration.defaultChannel.rawValue)")
        exit(0)
    case .failure(let reason):
        print(reason.rawValue)
        exit(reason == .notConfigured && CommandLine.arguments.contains("--allow-unconfigured") ? 0 : 1)
    }
}

var failures = 0
@MainActor func check(_ condition: Bool, _ label: String) {
    if condition { print("ok   \(label)") } else { failures += 1; print("FAIL \(label)") }
}

let key = Data((1...32).map { UInt8($0) }).base64EncodedString()
func release(_ overrides: [String: Any?] = [:]) -> [String: Any] {
    var info: [String: Any] = [
        "EmailReleaseBuild": true,
        "CFBundleIdentifier": "com.example.mail",
        UpdatePolicy.allowedHostsKey: ["example.test", "updates.example.test"],
        "CFBundleVersion": "120",
        "SUFeedURL": "https://updates.example.test/email/macos/appcast.xml",
        "SUPublicEDKey": key,
        "SUVerifyUpdateBeforeExtraction": true,
        "SURequireSignedFeed": true,
        "SUSignedFeedFailureExpirationInterval": 0,
    ]
    for (name, value) in overrides { info[name] = value ?? nil }
    return info
}
func reason(_ info: [String: Any]) -> UpdateDisabledReason? {
    if case .failure(let reason) = UpdatePolicy.evaluate(info) { return reason }
    return nil
}

check(reason(release()) == nil, "production configuration is accepted")
check(reason(release(["SUFeedURL": "https://example.test/email/appcast.xml"])) == nil, "apex host is accepted")
check(reason(release(["SUFeedURL": nil])) == .notConfigured, "no feed disables updates")
check(reason(release([UpdatePolicy.allowedHostsKey: nil])) == .notConfigured, "no allowed hosts disables updates")
check(reason(release([UpdatePolicy.allowedHostsKey: [String]()])) == .notConfigured, "empty allowed hosts disables updates")
check(reason(release([UpdatePolicy.allowedHostsKey: ["other.example.test"]])) == .untrustedFeedHost, "feed outside allowed hosts is refused")
check(reason(release(["SUFeedURL": "  "])) == .notConfigured, "blank feed disables updates")
check(reason(release(["EmailReleaseBuild": nil])) == .notReleaseBuild, "development bundles never update")
check(reason(release(["SUFeedURL": "http://updates.example.test/appcast.xml"])) == .insecureFeed, "http feed is refused")
check(reason(release(["SUFeedURL": "https://updates.example.test:8443/appcast.xml"])) == .insecureFeed, "custom port is refused")
check(reason(release(["SUFeedURL": "https://user@updates.example.test/appcast.xml"])) == .insecureFeed, "credentials in URL are refused")
check(reason(release(["SUFeedURL": "https://updates.example.test.evil.test/appcast.xml"])) == .untrustedFeedHost, "lookalike host is refused")
check(reason(release(["SUFeedURL": "https://evil.test/example.test/appcast.xml"])) == .untrustedFeedHost, "other host is refused")
check(reason(release(["SUFeedURL": "https://cdn.updates.example.test/appcast.xml"])) == .untrustedFeedHost, "unlisted subdomain is refused")
check(reason(release(["SUFeedURL": "https://127.0.0.1/appcast.xml"])) == .untrustedFeedHost, "production loopback is refused")
check(reason(release(["SUPublicEDKey": nil])) == .invalidPublicKey, "missing key is refused")
check(reason(release(["SUPublicEDKey": "not base64"])) == .invalidPublicKey, "malformed key is refused")
check(reason(release(["SUPublicEDKey": Data(count: 32).base64EncodedString()])) == .invalidPublicKey, "zero key is refused")
check(reason(release(["SUPublicEDKey": Data(repeating: 7, count: 31).base64EncodedString()])) == .invalidPublicKey, "short key is refused")
check(reason(release(["CFBundleVersion": "1.2"])) == .invalidBuildVersion, "dotted build is refused")
check(reason(release(["CFBundleVersion": "0"])) == .invalidBuildVersion, "zero build is refused")
check(reason(release(["CFBundleVersion": "012"])) == .invalidBuildVersion, "leading zero build is refused")
check(reason(release(["SUVerifyUpdateBeforeExtraction": nil])) == .unsafeSparkleSettings, "pre-extraction verification is required")
check(reason(release(["SURequireSignedFeed": false])) == .unsafeSparkleSettings, "signed feed is required")
check(reason(release(["SUSignedFeedFailureExpirationInterval": nil])) == .unsafeSparkleSettings, "feed failure expiry must be disabled")
check(reason(release(["SUSignedFeedFailureExpirationInterval": 1728000])) == .unsafeSparkleSettings, "expiring feed failures are refused")
check(reason(release(["SUEnableAutomaticChecks": true])) == .unsafeSparkleSettings, "forced automatic checks are refused")
check(reason(release(["SUAutomaticallyUpdate": true])) == .unsafeSparkleSettings, "forced silent install is refused")
check(reason(release(["SUEnableInstallerLauncherService": true])) == .unsafeSparkleSettings, "sandbox installer service is refused")
check(reason(release(["SUEnableJavaScript": true])) == .unsafeSparkleSettings, "release-notes JavaScript is refused")
check(reason(release(["EmailUpdateDefaultChannel": "nightly"])) == .invalidChannel, "unknown channel is refused")
check(reason(release(["SUEnableAutomaticChecks": false])) == nil, "explicitly disabled checks are allowed")

let fixtureFeed = "http://127.0.0.1:8765/appcast.xml"
check(reason(release(["EmailUpdateFixture": true, "SUFeedURL": fixtureFeed])) == .fixtureNotAllowed, "fixture flag on production identifier is refused")
let fixture = release(["EmailUpdateFixture": true, "SUFeedURL": fixtureFeed, "CFBundleIdentifier": UpdatePolicy.fixtureBundleIdentifier])
check(reason(fixture) == nil, "fixture loopback feed is accepted")
check(reason(release(["EmailUpdateFixture": true, "SUFeedURL": "http://localhost:1/appcast.xml",
                      "CFBundleIdentifier": UpdatePolicy.fixtureBundleIdentifier])) == .untrustedFeedHost, "fixture non-IP loopback is refused")
check(reason(release(["SUFeedURL": fixtureFeed, "CFBundleIdentifier": UpdatePolicy.fixtureBundleIdentifier])) == .insecureFeed, "loopback without fixture flag is refused")

guard case .success(let configuration) = UpdatePolicy.evaluate(release()) else { fatalError("configuration") }
check(configuration.defaultChannel == .stable, "stable is the default channel")
check(UpdatePolicy.selectedChannel(stored: "testing", configuration: configuration) == .testing, "stored testing choice is used")
check(UpdatePolicy.selectedChannel(stored: "bogus", configuration: configuration) == .stable, "unknown stored choice falls back")
check(UpdateChannel.stable.sparkleChannels.isEmpty, "stable reads only the default channel")
check(UpdateChannel.testing.sparkleChannels == ["testing"], "testing adds the testing channel")
check(UpdatePolicy.itemRejection(itemVersion: "121", itemChannel: nil, feedSigned: true, configuration: configuration, channel: .stable) == nil, "newer signed stable item proceeds")
check(UpdatePolicy.itemRejection(itemVersion: "121", itemChannel: nil, feedSigned: false, configuration: configuration, channel: .stable) != nil, "unsigned feed item is refused")
check(UpdatePolicy.itemRejection(itemVersion: "120", itemChannel: nil, feedSigned: true, configuration: configuration, channel: .stable) != nil, "same build is refused")
check(UpdatePolicy.itemRejection(itemVersion: "119", itemChannel: nil, feedSigned: true, configuration: configuration, channel: .stable) != nil, "older build is refused")
check(UpdatePolicy.itemRejection(itemVersion: "1.2.1", itemChannel: nil, feedSigned: true, configuration: configuration, channel: .stable) != nil, "non-integer item build is refused")
check(UpdatePolicy.itemRejection(itemVersion: "121", itemChannel: "testing", feedSigned: true, configuration: configuration, channel: .stable) != nil, "testing item is refused on stable")
check(UpdatePolicy.itemRejection(itemVersion: "121", itemChannel: "testing", feedSigned: true, configuration: configuration, channel: .testing) == nil, "testing item proceeds on testing")
check(UpdatePolicy.shouldPostponeRelaunch(hasActiveDraft: true), "open draft postpones relaunch")
check(!UpdatePolicy.shouldPostponeRelaunch(hasActiveDraft: false), "no draft relaunches")

if failures > 0 { print("\(failures) update policy checks failed"); exit(1) }
print("All update policy checks passed")
