import Foundation
import Darwin
import Security

// Distribution launcher: only sealed, same-signer libraries inside this app.
// Keep this separate from the development launcher and its Keychain identity.
@main
struct EmailReleaseLauncher {
    static func main() {
        guard let frameworks = Bundle.main.privateFrameworksURL,
              Bundle.main.object(forInfoDictionaryKey: "EmailReleaseBuild") as? Bool == true else {
            fputs("Email must run from its complete application bundle.\n", stderr)
            exit(2)
        }
        let ui = frameworks.appendingPathComponent("libEmailUI.dylib").path
        let service = frameworks.appendingPathComponent("libmail_service.dylib").path
        guard validLibrary(ui), validLibrary(service) else {
            fputs("Email's bundled runtime could not be verified. Reinstall the app.\n", stderr)
            exit(2)
        }
        // Never honor a caller-supplied service path in a distribution build.
        setenv("MAIL_SERVICE_LIBRARY", service, 1)
        guard let library = dlopen(ui, RTLD_NOW | RTLD_LOCAL),
              let symbol = dlsym(library, "email_ui_main") else {
            if let error = dlerror() { fputs("Email runtime loader: \(String(cString: error))\n", stderr) }
            fputs("Email could not load its bundled interface.\n", stderr)
            exit(2)
        }
        unsafeBitCast(symbol, to: (@convention(c) () -> Void).self)()
    }

    private static func validLibrary(_ path: String) -> Bool {
        var running: SecCode?
        var code: SecStaticCode?
        var requirement: SecRequirement?
        var library: SecStaticCode?
        guard SecCodeCopySelf([], &running) == errSecSuccess, let running,
              SecCodeCopyStaticCode(running, [], &code) == errSecSuccess, let code,
              SecCodeCopyDesignatedRequirement(code, [], &requirement) == errSecSuccess, let requirement,
              SecStaticCodeCreateWithPath(URL(fileURLWithPath: path) as CFURL, [], &library) == errSecSuccess,
              let library else { return false }
        return SecStaticCodeCheckValidity(library, SecCSFlags(rawValue: kSecCSStrictValidate), requirement) == errSecSuccess
    }
}
