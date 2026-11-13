import AppKit
import EmailUpdatePolicy
import Foundation
import Security
@preconcurrency import Sparkle

// C ABI consumed by EmailUI through dlopen. EmailUI verifies this library and
// Sparkle.framework against the app's own signer before loading either.
public typealias EmailDraftProbe = @convention(c) () -> Bool

/// Reads the Info.plist bound into the host's code signature, not the file on
/// disk, and requires a certificate-backed signature that still validates.
enum HostSignature {
    static func signedInfo() -> Result<[String: Any], UpdateDisabledReason> {
        var code: SecStaticCode?
        var requirement: SecRequirement?
        var information: CFDictionary?
        guard SecStaticCodeCreateWithPath(Bundle.main.bundleURL as CFURL, [], &code) == errSecSuccess, let code,
              SecCodeCopyDesignatedRequirement(code, [], &requirement) == errSecSuccess, let requirement,
              // The resource seal is large; Gatekeeper and Sparkle check it.
              // Executable and Info.plist bindings are checked here.
              SecStaticCodeCheckValidity(code, SecCSFlags(rawValue: kSecCSDoNotValidateResources), requirement) == errSecSuccess,
              SecCodeCopySigningInformation(code, SecCSFlags(rawValue: kSecCSSigningInformation), &information) == errSecSuccess,
              let details = information as? [String: Any],
              let certificates = details[kSecCodeInfoCertificates as String] as? [SecCertificate], !certificates.isEmpty,
              let info = details[kSecCodeInfoPList as String] as? [String: Any] else {
            return .failure(.signatureInvalid)
        }
        return .success(info)
    }

    static func configuration() -> Result<UpdateConfiguration, UpdateDisabledReason> {
        signedInfo().flatMap(UpdatePolicy.evaluate)
    }
}

@MainActor
final class EmailUpdaterController: NSObject, SPUUpdaterDelegate, NSMenuItemValidation {
    static var shared: EmailUpdaterController?

    let configuration: UpdateConfiguration
    private let userDriver: SPUUserDriver
    private let draftProbe: EmailDraftProbe?
    private let relaunches: Bool
    private(set) var updater: SPUUpdater?
    private var postponedInstall: (() -> Void)?
    private var draftTimer: Timer?

    init(configuration: UpdateConfiguration, userDriver: SPUUserDriver, draftProbe: EmailDraftProbe?, relaunches: Bool) {
        self.configuration = configuration
        self.userDriver = userDriver
        self.draftProbe = draftProbe
        self.relaunches = relaunches
    }

    func start() throws {
        let updater = SPUUpdater(hostBundle: .main, applicationBundle: .main, userDriver: userDriver, delegate: self)
        // A feed URL left in user defaults by older Sparkle APIs is ignored.
        _ = updater.clearFeedURLFromUserDefaults()
        try updater.start()
        self.updater = updater
    }

    var channel: UpdateChannel {
        get { UpdatePolicy.selectedChannel(stored: UserDefaults.standard.string(forKey: UpdatePolicy.channelDefaultsKey), configuration: configuration) }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: UpdatePolicy.channelDefaultsKey) }
    }

    // MARK: Menu

    func installMenu(into menu: NSMenu, at index: Int) {
        let check = NSMenuItem(title: "Check for Updates…", action: #selector(checkForUpdates(_:)), keyEquivalent: "")
        check.target = self
        let settings = NSMenu(title: "Updates")
        for (title, action) in [("Check Automatically", #selector(toggleAutomaticChecks(_:))),
                                ("Install Automatically", #selector(toggleAutomaticInstall(_:))),
                                ("Testing Builds", #selector(toggleTestingChannel(_:)))] {
            settings.addItem(withTitle: title, action: action, keyEquivalent: "").target = self
        }
        let settingsItem = NSMenuItem(title: "Updates", action: nil, keyEquivalent: "")
        settingsItem.submenu = settings
        let position = min(max(index, 0), menu.numberOfItems)
        menu.insertItem(check, at: position)
        menu.insertItem(settingsItem, at: position + 1)
        menu.insertItem(.separator(), at: position + 2)
    }

    @objc func checkForUpdates(_ sender: Any?) { updater?.checkForUpdates() }
    @objc func toggleAutomaticChecks(_ sender: Any?) {
        guard let updater else { return }
        updater.automaticallyChecksForUpdates.toggle()
    }
    @objc func toggleAutomaticInstall(_ sender: Any?) {
        guard let updater, updater.allowsAutomaticUpdates else { return }
        updater.automaticallyDownloadsUpdates.toggle()
    }
    @objc func toggleTestingChannel(_ sender: Any?) {
        channel = channel == .testing ? .stable : .testing
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        guard let updater else { return false }
        switch item.action {
        case #selector(checkForUpdates(_:)):
            return updater.canCheckForUpdates
        case #selector(toggleAutomaticChecks(_:)):
            item.state = updater.automaticallyChecksForUpdates ? .on : .off
            return true
        case #selector(toggleAutomaticInstall(_:)):
            item.state = updater.automaticallyDownloadsUpdates ? .on : .off
            return updater.allowsAutomaticUpdates && updater.automaticallyChecksForUpdates
        case #selector(toggleTestingChannel(_:)):
            item.state = channel == .testing ? .on : .off
            return true
        default:
            return true
        }
    }

    // MARK: SPUUpdaterDelegate

    func feedURLString(for updater: SPUUpdater) -> String? { configuration.feedURL.absoluteString }

    func allowedChannels(for updater: SPUUpdater) -> Set<String> { channel.sparkleChannels }

    // Do not send Sparkle's hardware/system profile, including when a stored
    // preference from another build enables it. Update hosting still sees the
    // ordinary network request and user agent.
    func allowedSystemProfileKeys(for updater: SPUUpdater) -> [String]? { [] }

    func updater(_ updater: SPUUpdater, shouldProceedWithUpdate updateItem: SUAppcastItem, updateCheck: SPUUpdateCheck) throws {
        if let reason = UpdatePolicy.itemRejection(itemVersion: updateItem.versionString, itemChannel: updateItem.channel,
                                                   feedSigned: updateItem.signingValidationStatus == .succeeded,
                                                   configuration: configuration, channel: channel) {
            throw NSError(domain: "EmailUpdater", code: 1, userInfo: [NSLocalizedDescriptionKey: reason])
        }
    }

    func updaterShouldRelaunchApplication(_ updater: SPUUpdater) -> Bool { relaunches }

    func updater(_ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem,
                 untilInvokingBlock installHandler: @escaping () -> Void) -> Bool {
        guard UpdatePolicy.shouldPostponeRelaunch(hasActiveDraft: draftProbe?() ?? false) else { return false }
        postponedInstall = installHandler
        let alert = NSAlert()
        alert.messageText = "Update ready"
        alert.informativeText = "Email restarts when you close your draft."
        alert.runModal()
        draftTimer?.invalidate()
        draftTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
            MainActor.assumeIsolated { EmailUpdaterController.shared?.resumeIfDraftClosed() }
        }
        return true
    }

    private func resumeIfDraftClosed() {
        guard !(draftProbe?() ?? false), let install = postponedInstall else { return }
        draftTimer?.invalidate()
        draftTimer = nil
        postponedInstall = nil
        install()
    }
}

/// Answers Sparkle without UI. Only fixture bundles can reach this path.
@MainActor
final class FixtureUserDriver: NSObject, SPUUserDriver {
    private func finish(_ status: Int32, _ message: String) -> Never {
        FileHandle.standardError.write(Data("updater-fixture: \(message)\n".utf8))
        exit(status)
    }
    func show(_ request: SPUUpdatePermissionRequest, reply: @escaping (SUUpdatePermissionResponse) -> Void) {
        reply(SUUpdatePermissionResponse(automaticUpdateChecks: false, sendSystemProfile: false))
    }
    func showUserInitiatedUpdateCheck(cancellation: @escaping () -> Void) {}
    func showUpdateFound(with appcastItem: SUAppcastItem, state: SPUUserUpdateState, reply: @escaping (SPUUserUpdateChoice) -> Void) {
        FileHandle.standardError.write(Data("updater-fixture: found build \(appcastItem.versionString)\n".utf8))
        reply(.install)
    }
    func showUpdateReleaseNotes(with downloadData: SPUDownloadData) {}
    func showUpdateReleaseNotesFailedToDownloadWithError(_ error: Error) {}
    func showUpdateNotFoundWithError(_ error: Error, acknowledgement: @escaping () -> Void) {
        finish(11, "no update: \(error.localizedDescription)")
    }
    func showUpdaterError(_ error: Error, acknowledgement: @escaping () -> Void) {
        finish(10, "error: \(error.localizedDescription)")
    }
    func showDownloadInitiated(cancellation: @escaping () -> Void) {}
    func showDownloadDidReceiveExpectedContentLength(_ expectedContentLength: UInt64) {}
    func showDownloadDidReceiveData(ofLength length: UInt64) {}
    func showDownloadDidStartExtractingUpdate() {}
    func showExtractionReceivedProgress(_ progress: Double) {}
    func showReady(toInstallAndRelaunch reply: @escaping (SPUUserUpdateChoice) -> Void) { reply(.install) }
    func showInstallingUpdate(withApplicationTerminated applicationTerminated: Bool, retryTerminatingApplication: @escaping () -> Void) {
        // Fixtures intentionally do not relaunch after installation. Complete
        // their normal AppKit quit so install-on-quit can proceed, rather than
        // letting the test's timeout kill the host and accidentally pass.
        if !applicationTerminated { NSApplication.shared.terminate(nil) }
    }
    func showUpdateInstalledAndRelaunched(_ relaunched: Bool, acknowledgement: @escaping () -> Void) {
        acknowledgement()
        finish(0, "installed")
    }
    func showUpdateInFocus() {}
    func dismissUpdateInstallation() {}
}

private func statusDictionary() -> [String: Any] {
    let sparkleVersion = Bundle(for: SPUUpdater.self).object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown"
    switch HostSignature.configuration() {
    case .failure(let reason):
        return ["enabled": false, "reason": reason.rawValue, "sparkle": sparkleVersion]
    case .success(let configuration):
        let stored = UserDefaults.standard.string(forKey: UpdatePolicy.channelDefaultsKey)
        return ["enabled": true, "reason": "ok", "sparkle": sparkleVersion,
                "feed": configuration.feedURL.absoluteString, "build": configuration.buildVersion,
                "channel": UpdatePolicy.selectedChannel(stored: stored, configuration: configuration).rawValue,
                "fixture": configuration.isFixture]
    }
}

@_cdecl("email_updater_status_json")
public func emailUpdaterStatusJSON() -> UnsafeMutablePointer<CChar>? {
    let data = (try? JSONSerialization.data(withJSONObject: statusDictionary(), options: [.sortedKeys])) ?? Data("{}".utf8)
    return strdup(String(decoding: data, as: UTF8.self))
}

@_cdecl("email_updater_free_string")
public func emailUpdaterFreeString(_ value: UnsafeMutablePointer<CChar>?) { free(value) }

/// Starts Sparkle and adds update controls to `menu`. Returns 0 when running,
/// 1 when updates are disabled by configuration, 2 when Sparkle fails to start.
@_cdecl("email_updater_install")
public func emailUpdaterInstall(_ menuPointer: UnsafeMutableRawPointer?, _ index: Int32, _ probe: EmailDraftProbe?) -> Int32 {
    // Called on the main thread by EmailUI while it builds the main menu.
    nonisolated(unsafe) let menu = menuPointer
    return MainActor.assumeIsolated {
        guard EmailUpdaterController.shared == nil, let menu else { return 2 }
        guard case .success(let configuration) = HostSignature.configuration(), !configuration.isFixture else { return 1 }
        let driver = SPUStandardUserDriver(hostBundle: .main, delegate: nil)
        let controller = EmailUpdaterController(configuration: configuration, userDriver: driver, draftProbe: probe, relaunches: true)
        do { try controller.start() } catch {
            NSLog("Email updates unavailable: %@", error.localizedDescription)
            return 2
        }
        EmailUpdaterController.shared = controller
        controller.installMenu(into: Unmanaged<NSMenu>.fromOpaque(menu).takeUnretainedValue(), at: Int(index))
        return 0
    }
}

/// Non-interactive install used by acceptance tests. Fixture bundles only.
@_cdecl("email_updater_fixture_install")
public func emailUpdaterFixtureInstall(_ timeoutSeconds: Int32) -> Int32 {
    MainActor.assumeIsolated {
        guard case .success(let configuration) = HostSignature.configuration(), configuration.isFixture else {
            FileHandle.standardError.write(Data("updater-fixture: refused outside a fixture bundle\n".utf8))
            return 3
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.prohibited)
        let controller = EmailUpdaterController(configuration: configuration, userDriver: FixtureUserDriver(),
                                                draftProbe: nil, relaunches: true)
        EmailUpdaterController.shared = controller
        do { try controller.start() } catch {
            FileHandle.standardError.write(Data("updater-fixture: start failed: \(error.localizedDescription)\n".utf8))
            return 2
        }
        Timer.scheduledTimer(withTimeInterval: TimeInterval(max(timeoutSeconds, 5)), repeats: false) { _ in
            FileHandle.standardError.write(Data("updater-fixture: timed out\n".utf8))
            exit(20)
        }
        controller.updater?.checkForUpdates()
        app.run()
        return 0
    }
}
