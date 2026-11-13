#!/usr/bin/env bash
# Sourced by build scripts. Loads the selected distribution profile as
# EMAIL_PROFILE_* shell variables, plus the OAuth application build values
# (environment first, then the ignored config/oauth-desktop.local.env).
# Never enable shell tracing around this file.

email_load_distribution_profile() {
  local root="$1" assignments
  assignments="$(python3 "$root/scripts/distribution-profile.py" --root "$root" shell)" || return 1
  eval "$assignments"
  # Cargo build scripts resolve exactly the same profile file.
  export EMAIL_DISTRIBUTION_PROFILE
}

# Prints one value of the selected profile without loading OAuth values.
email_profile_value() {
  python3 "$1/scripts/distribution-profile.py" --root "$1" get "$2"
}

# Local development builds never use a distribution profile: they would share
# an installed release's data directory and Keychain namespace.
email_refuse_distribution_profile() {
  if [[ "${EMAIL_PROFILE_DISTRIBUTION:-}" == true ]]; then
    echo "The $EMAIL_PROFILE_PROFILE profile is for signed releases and shares an installed release's data and Keychain items." >&2
    echo "Use a non-distribution profile (such as development) for local builds." >&2
    return 1
  fi
}

# Fails with a message naming the variable and the local file when the
# profile requires a Google client ID that is missing or malformed.
email_require_google_client_id() {
  local root="$1"
  EMAIL_GOOGLE_CLIENT_ID="${EMAIL_GOOGLE_CLIENT_ID:-}" \
    python3 "$root/scripts/distribution-profile.py" --root "$root" check-google-client
}

# "Google OAuth publicly available" for this build, exported for
# crates/mail-cloud/build.rs, which applies the same rule: EMAIL_GOOGLE_OAUTH_PUBLIC
# when set (true or false, for one build such as the recording for Google's
# review), otherwise the profile's GOOGLE_OAUTH_PUBLIC. A distribution-channel
# build with a profile that is not for distribution keeps it off. Call after
# loading the profile and setting EMAIL_RELEASE_CHANNEL.
email_resolve_google_oauth_public() {
  if [[ -z "${EMAIL_GOOGLE_OAUTH_PUBLIC:-}" ]]; then
    if [[ "${EMAIL_RELEASE_CHANNEL:-}" == distribution && "${EMAIL_PROFILE_DISTRIBUTION:-}" != true ]]; then
      EMAIL_GOOGLE_OAUTH_PUBLIC=false
    else
      EMAIL_GOOGLE_OAUTH_PUBLIC="${EMAIL_PROFILE_GOOGLE_OAUTH_PUBLIC:-false}"
    fi
  fi
  case "$EMAIL_GOOGLE_OAUTH_PUBLIC" in
    true|false) export EMAIL_GOOGLE_OAUTH_PUBLIC ;;
    *) echo "EMAIL_GOOGLE_OAUTH_PUBLIC must be true or false." >&2; return 1 ;;
  esac
}
