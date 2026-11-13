import AppKit
import CryptoKit
import Darwin
import Security

/// Loads the optional Sparkle module from a signed release bundle. Development
/// builds and bundles without the module never load Sparkle.
@MainActor
enum EmailUpdaterLoader {
    typealias Status = @convention(c) () -> UnsafeMutablePointer<CChar>?
    typealias FreeString = @convention(c) (UnsafeMutablePointer<CChar>?) -> Void
    typealias Install = @convention(c) (UnsafeMutableRawPointer?, Int32, (@convention(c) () -> Bool)?) -> Int32
    typealias FixtureInstall = @convention(c) (Int32) -> Int32

    /// Reports whether a restart would discard an open composer.
    static var hasActiveDraft: () -> Bool = { false }
    private static var handle: UnsafeMutableRawPointer?
    private static var failure: String?

    private static func module() -> UnsafeMutableRawPointer? {
        if let handle { return handle }
        if failure != nil { return nil }
        guard Bundle.main.object(forInfoDictionaryKey: "EmailReleaseBuild") as? Bool == true,
              let frameworks = Bundle.main.privateFrameworksURL else { failure = "not-release-build"; return nil }
        let library = frameworks.appendingPathComponent("libEmailUpdater.dylib")
        let sparkle = frameworks.appendingPathComponent("Sparkle.framework")
        guard FileManager.default.fileExists(atPath: library.path),
              FileManager.default.fileExists(atPath: sparkle.path) else { failure = "module-missing"; return nil }
        guard let signer = sameSignerRequirement(),
              valid(library, signer, nested: false), valid(sparkle, signer, nested: true) else {
            failure = "module-signature-invalid"; return nil
        }
        guard let loaded = dlopen(library.path, RTLD_NOW | RTLD_LOCAL) else {
            if let error = dlerror() { NSLog("Email updates: %@", String(cString: error)) }
            failure = "module-load-failed"; return nil
        }
        handle = loaded
        return loaded
    }

    /// Requires the exact leaf certificate that signed this app.
    private static func sameSignerRequirement() -> SecRequirement? {
        var code: SecStaticCode?
        var information: CFDictionary?
        var requirement: SecRequirement?
        guard SecStaticCodeCreateWithPath(Bundle.main.bundleURL as CFURL, [], &code) == errSecSuccess, let code,
              SecCodeCopySigningInformation(code, SecCSFlags(rawValue: kSecCSSigningInformation), &information) == errSecSuccess,
              let certificates = (information as? [String: Any])?[kSecCodeInfoCertificates as String] as? [SecCertificate],
              let leaf = certificates.first else { return nil }
        let digest = Insecure.SHA1.hash(data: SecCertificateCopyData(leaf) as Data).map { String(format: "%02x", $0) }.joined()
        guard SecRequirementCreateWithString("certificate leaf = H\"\(digest)\"" as CFString, [], &requirement) == errSecSuccess else { return nil }
        return requirement
    }

    private static func valid(_ url: URL, _ requirement: SecRequirement, nested: Bool) -> Bool {
        var code: SecStaticCode?
        guard SecStaticCodeCreateWithPath(url as CFURL, [], &code) == errSecSuccess, let code else { return false }
        var flags = kSecCSStrictValidate | kSecCSCheckAllArchitectures
        if nested { flags |= kSecCSCheckNestedCode }
        return SecStaticCodeCheckValidity(code, SecCSFlags(rawValue: flags), requirement) == errSecSuccess
    }

    private static func symbol<T>(_ name: String, as type: T.Type) -> T? {
        guard let handle = module(), let pointer = dlsym(handle, name) else { return nil }
        return unsafeBitCast(pointer, to: type)
    }

    static func statusJSON() -> String {
        guard let status = symbol("email_updater_status_json", as: Status.self),
              let release = symbol("email_updater_free_string", as: FreeString.self) else {
            return "{\"enabled\":false,\"reason\":\"\(failure ?? "module-load-failed")\"}"
        }
        let pointer = status()
        defer { release(pointer) }
        return pointer.map { String(cString: $0) } ?? "{}"
    }

    /// Adds update controls to the app menu when updates are configured.
    static func install(into menu: NSMenu, at index: Int) {
        guard let install = symbol("email_updater_install", as: Install.self) else { return }
        let probe: @convention(c) () -> Bool = { MainActor.assumeIsolated { EmailUpdaterLoader.hasActiveDraft() } }
        _ = install(Unmanaged.passUnretained(menu).toOpaque(), Int32(index), probe)
    }

    static func fixtureInstall(timeout: Int32) -> Int32 {
        guard let install = symbol("email_updater_fixture_install", as: FixtureInstall.self) else {
            fputs("updater module unavailable: \(failure ?? "unknown")\n", stderr)
            return 3
        }
        return install(timeout)
    }
}
