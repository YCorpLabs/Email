//! What this build offers. Both native shells read these values through the
//! service's `build_features` request so their account setup and contact
//! photos stay identical.
//!
//! Only the `distribution` channel (the public release) withholds features.
//! Development, local-alpha and fixture builds keep everything so maintainers
//! can keep testing features that are not yet ready for Google review or release.

/// `development`, `local-alpha`, `fixture` or `distribution`.
pub const RELEASE_CHANNEL: &str = env!("EMAIL_RESOLVED_RELEASE_CHANNEL");

/// Public builds. Decided at compile time from `EMAIL_RELEASE_CHANNEL`.
pub const DISTRIBUTION: bool = cfg!(email_distribution);

/// Google contact photos need `contacts.readonly` and `contacts.other.readonly`.
/// They are not part of the first Google review, so distribution builds neither
/// offer the feature nor contain the scope request.
pub const GOOGLE_CONTACT_PHOTOS: bool = !DISTRIBUTION;

/// Outlook/Hotmail has not been tested with real accounts yet.
pub const MICROSOFT_ACCOUNTS: bool = !DISTRIBUTION;

/// "Google OAuth publicly available": the profile's `GOOGLE_OAUTH_PUBLIC`,
/// unless `EMAIL_GOOGLE_OAUTH_PUBLIC` overrides it for one build (see
/// `build.rs`). When false, account setup offers no Google sign-in and the
/// service refuses to start one; Gmail connects with an app password.
/// Accounts already connected through Google sign-in keep working. The
/// official profile keeps it false until Google's review passes.
pub const GOOGLE_OAUTH_PUBLIC: bool = matches!(
    env!("EMAIL_RESOLVED_GOOGLE_OAUTH_PUBLIC").as_bytes(),
    b"true"
);

#[cfg(test)]
mod tests {
    use super::*;

    /// The build script's rule, checked against the profile compiled in.
    #[test]
    fn google_sign_in_follows_the_profile_unless_overridden() {
        let profile = env!("EMAIL_PROFILE_GOOGLE_OAUTH_PUBLIC") == "true";
        let for_distribution = env!("EMAIL_PROFILE_DISTRIBUTION") == "true";
        let expected = match option_env!("EMAIL_GOOGLE_OAUTH_PUBLIC").map(str::trim) {
            Some("true") => true,
            Some("false") => false,
            _ => profile && (!DISTRIBUTION || for_distribution),
        };
        println!(
            "profile={} channel={RELEASE_CHANNEL} google_oauth_public={GOOGLE_OAUTH_PUBLIC}",
            env!("EMAIL_PROFILE_PROFILE")
        );
        assert_eq!(GOOGLE_OAUTH_PUBLIC, expected);
    }
}
