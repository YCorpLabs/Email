import Foundation
import Darwin
import Security

@main
struct EmailLauncher {
    static func main() {
        let executable = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath()
        let isPackaged = executable.path.contains(".app/Contents/MacOS/")
        let runtime = isPackaged
            ? executable.deletingLastPathComponent().deletingLastPathComponent()
                .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Runtime")
            : executable.deletingLastPathComponent()
        let uiPath = runtime.appendingPathComponent("libEmailUI.dylib").path
        let servicePath = runtime.appendingPathComponent("libmail_service.dylib").path
        if isPackaged {
            guard validRuntimeLibrary(uiPath), validRuntimeLibrary(servicePath) else {
                fputs("Email runtime signature is invalid. Rebuild the app.\n", stderr)
                exit(2)
            }
        }
        setenv("MAIL_SERVICE_LIBRARY", servicePath, 0)
        guard let library = dlopen(uiPath, RTLD_NOW | RTLD_LOCAL),
              let symbol = dlsym(library, "email_ui_main") else {
            fputs("Email UI could not load: \(String(cString: dlerror()))\n", stderr)
            exit(2)
        }
        let entry = unsafeBitCast(symbol, to: (@convention(c) () -> Void).self)
        entry()
    }

    private static func validRuntimeLibrary(_ path: String) -> Bool {
        var selfCode: SecCode?
        var selfStaticCode: SecStaticCode?
        var requirement: SecRequirement?
        var library: SecStaticCode?
        let selfStatus = SecCodeCopySelf([], &selfCode)
        guard selfStatus == errSecSuccess, let selfCode else { fputs("self code: \(selfStatus)\n", stderr); return false }
        let staticStatus = SecCodeCopyStaticCode(selfCode, [], &selfStaticCode)
        guard staticStatus == errSecSuccess, let selfStaticCode else { fputs("self static code: \(staticStatus)\n", stderr); return false }
        let requirementStatus = SecCodeCopyDesignatedRequirement(selfStaticCode, [], &requirement)
        guard requirementStatus == errSecSuccess, let requirement else { fputs("self requirement: \(requirementStatus)\n", stderr); return false }
        let libraryStatus = SecStaticCodeCreateWithPath(URL(fileURLWithPath: path) as CFURL, [], &library)
        guard libraryStatus == errSecSuccess, let library else { fputs("runtime code: \(libraryStatus)\n", stderr); return false }
        let validityStatus = SecStaticCodeCheckValidity(library, [], requirement)
        if validityStatus != errSecSuccess { fputs("runtime signature: \(validityStatus)\n", stderr) }
        return validityStatus == errSecSuccess
    }
}
