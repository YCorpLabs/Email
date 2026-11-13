//! Resolve the release channel once, at compile time.
//!
//! `EMAIL_RELEASE_CHANNEL=distribution` marks the public release. Its binaries
//! omit development-only Google scopes entirely (`cfg(email_distribution)`), so
//! no runtime setting or environment variable can re-enable them.
fn main() {
    println!("cargo:rerun-if-env-changed=EMAIL_RELEASE_CHANNEL");
    println!("cargo:rustc-check-cfg=cfg(email_distribution)");
    let requested = std::env::var("EMAIL_RELEASE_CHANNEL").unwrap_or_default();
    let channel = match requested.trim() {
        "" => "development",
        "development" => "development",
        "local-alpha" => "local-alpha",
        "fixture" => "fixture",
        "distribution" => "distribution",
        other => panic!(
            "Unknown EMAIL_RELEASE_CHANNEL `{other}`. Use development, local-alpha, fixture or distribution."
        ),
    };
    if channel == "distribution" {
        println!("cargo:rustc-cfg=email_distribution");
    }
    println!("cargo:rustc-env=EMAIL_RESOLVED_RELEASE_CHANNEL={channel}");

    // "Google OAuth publicly available" comes from the selected distribution
    // profile (`GOOGLE_OAUTH_PUBLIC` in config/profiles). Until Google approves
    // the restricted-scope review, the production project stays in Testing and
    // blocks every Google account that is not a listed test user, so the
    // official profile keeps Google sign-in off: Gmail goes straight to the
    // app-password path. Maintainers set it to true there after approval.
    // `EMAIL_GOOGLE_OAUTH_PUBLIC=true|false` overrides it for one build (for
    // example the recording for Google's review). A distribution-channel build
    // with a profile that is not for distribution keeps it off.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let profile = distribution_profile::emit(&root);
    println!("cargo:rerun-if-env-changed=EMAIL_GOOGLE_OAUTH_PUBLIC");
    let public = match std::env::var("EMAIL_GOOGLE_OAUTH_PUBLIC")
        .unwrap_or_default()
        .trim()
    {
        "" => {
            profile.flag("GOOGLE_OAUTH_PUBLIC")
                && (channel != "distribution" || profile.flag("DISTRIBUTION"))
        }
        "true" => true,
        "false" => false,
        other => panic!("EMAIL_GOOGLE_OAUTH_PUBLIC must be true or false, not `{other}`."),
    };
    println!("cargo:rustc-env=EMAIL_RESOLVED_GOOGLE_OAUTH_PUBLIC={public}");
}
