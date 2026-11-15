import Foundation

/// The version that About and the support information show
/// (docs/release-versions.md). A packaged release shows its signed Info.plist
/// values, `CFBundleShortVersionString (CFBundleVersion)`, as macOS's own About
/// panels do: "0.1.0-beta.2 (2)". A development build has no release
/// Info.plist and shows the release version it was built from.
enum AppVersion {
    static func display(info: [String: Any]?, developmentVersion: String) -> String {
        func text(_ key: String) -> String? {
            guard let value = (info?[key] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !value.isEmpty else { return nil }
            return value
        }
        guard info?["EmailReleaseBuild"] as? Bool == true, let version = text("CFBundleShortVersionString") else {
            return developmentVersion
        }
        guard let build = text("CFBundleVersion") else { return version }
        return "\(version) (\(build))"
    }
}
