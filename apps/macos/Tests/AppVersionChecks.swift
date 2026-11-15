import Foundation

// Compiled with AppVersion.swift by scripts/check-release-swift.py.
@main enum AppVersionChecks {
    static func main() {
        let release: [String: Any] = ["EmailReleaseBuild": true, "CFBundleShortVersionString": "0.1.0-beta.2",
                                      "CFBundleVersion": "2"]
        precondition(AppVersion.display(info: release, developmentVersion: "9.9.9") == "0.1.0-beta.2 (2)",
                     "a release shows its version and build number")
        var final = release
        final["CFBundleShortVersionString"] = "0.1.0"
        final["CFBundleVersion"] = "3"
        precondition(AppVersion.display(info: final, developmentVersion: "9.9.9") == "0.1.0 (3)", "final release")
        var noBuild = release
        noBuild["CFBundleVersion"] = " "
        precondition(AppVersion.display(info: noBuild, developmentVersion: "9.9.9") == "0.1.0-beta.2",
                     "a blank build number is left out")
        // The development bundle's Info.plist is not a release and never names the version.
        let development: [String: Any] = ["CFBundleShortVersionString": "0.1", "CFBundleVersion": "1"]
        precondition(AppVersion.display(info: development, developmentVersion: "0.1.0-beta.2") == "0.1.0-beta.2",
                     "a development build shows the release version it was built from")
        precondition(AppVersion.display(info: nil, developmentVersion: "0.1.0-beta.2") == "0.1.0-beta.2", "no Info.plist")
        var emptyVersion = release
        emptyVersion["CFBundleShortVersionString"] = ""
        precondition(AppVersion.display(info: emptyVersion, developmentVersion: "0.1.0-beta.2") == "0.1.0-beta.2",
                     "an empty release version falls back")
        print("AppVersion checks passed")
    }
}
