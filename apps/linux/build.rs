fn main() {
    // option_env! embeds the desktop application's OAuth configuration only.
    // Account credentials remain in Secret Service and are never read here.
    for name in [
        "EMAIL_BUILD_ID",
        "EMAIL_GOOGLE_CLIENT_ID",
        "EMAIL_GOOGLE_CLIENT_SECRET",
        "EMAIL_MICROSOFT_CLIENT_ID",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    // Identity and service values come from mail_service::profile.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let profile = distribution_profile::emit(&root);
    // The Linux shell compiles its Google client in. Build scripts load it from
    // config/oauth-desktop.local.env; a direct Cargo build must export it.
    let client = std::env::var("EMAIL_GOOGLE_CLIENT_ID").unwrap_or_default();
    if profile.flag("GOOGLE_CLIENT_ID_REQUIRED") && client.trim().is_empty() {
        panic!(
            "The {} profile needs EMAIL_GOOGLE_CLIENT_ID. Build with scripts/run-linux.sh or a package script, which read config/oauth-desktop.local.env, or export it.",
            profile.get("PROFILE")
        );
    }
}
