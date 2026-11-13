fn main() {
    // Compile the selected distribution profile into the engine. The macOS
    // shell reads it through mail_service_distribution_profile_json; the Linux
    // shell and the MCP server read mail_service::profile directly.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let profile = distribution_profile::emit(&root);
    // Open tracking is compiled in only for a profile that enables it (the
    // parsers allow that only with a Scheduling origin and never with
    // DISTRIBUTION=true). Without the cfg, registration and pixel insertion
    // are not part of the build.
    println!("cargo::rustc-check-cfg=cfg(email_open_tracking)");
    if profile.flag("OPEN_TRACKING") && !profile.get("SCHEDULING_ORIGIN").is_empty() {
        println!("cargo:rustc-cfg=email_open_tracking");
    }
}
