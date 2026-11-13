// swift-tools-version: 6.0
import PackageDescription

// Sparkle is pinned exactly. Release tooling checks the same version and the
// SHA-256 of its command line tools (see scripts/macos-updates-env.sh).
let package = Package(
    name: "EmailDesktop",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "email-desktop", targets: ["EmailDesktop"]),
        .library(name: "EmailUI", type: .dynamic, targets: ["EmailUI"]),
        // Loaded at runtime by EmailUI only in signed release bundles, so the
        // development app and the Linux shell never link Sparkle.
        .library(name: "EmailUpdater", type: .dynamic, targets: ["EmailUpdater"]),
        .executable(name: "email-update-policy-checks", targets: ["EmailUpdatePolicyChecks"]),
    ],
    dependencies: [
        .package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.1"),
    ],
    targets: [
        .target(name: "EmailUI"),
        .executableTarget(name: "EmailDesktop"),
        .target(name: "EmailUpdatePolicy"),
        .target(
            name: "EmailUpdater",
            dependencies: ["EmailUpdatePolicy", .product(name: "Sparkle", package: "Sparkle")]
        ),
        .executableTarget(name: "EmailUpdatePolicyChecks", dependencies: ["EmailUpdatePolicy"], path: "Tests/EmailUpdatePolicyChecks"),
    ]
)
