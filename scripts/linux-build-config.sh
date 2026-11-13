#!/usr/bin/env bash
# Desktop OAuth application values only; never account access/refresh tokens.
# Never enable shell tracing around this file.
linux_build_repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$linux_build_repo/scripts/distribution-profile.sh"
# Identity and service values come from the selected distribution profile; the
# Cargo build scripts resolve the same file through EMAIL_DISTRIBUTION_PROFILE.
# OAuth application values come from the environment, then the ignored
# config/oauth-desktop.local.env.
email_load_distribution_profile "$linux_build_repo" || return 1
export EMAIL_GOOGLE_CLIENT_ID EMAIL_GOOGLE_CLIENT_SECRET EMAIL_MICROSOFT_CLIENT_ID

require_linux_oauth_config() {
  # Compile-time feature switches (crates/mail-cloud/build.rs); see
  # package-macos-release.sh. `distribution` omits development-only Google
  # scopes and Outlook; Google sign-in follows the profile's GOOGLE_OAUTH_PUBLIC.
  export EMAIL_RELEASE_CHANNEL="${EMAIL_LINUX_RELEASE_CHANNEL:-local-alpha}"
  email_resolve_google_oauth_public || return 1
  case "${EMAIL_LINUX_RELEASE_CHANNEL:-local-alpha}" in
    local-alpha)
      email_refuse_distribution_profile || return 1
      email_require_google_client_id "$linux_build_repo" || return 1
      ;;
    distribution)
      if [[ "$EMAIL_PROFILE_DISTRIBUTION" != true ]]; then
        echo "The $EMAIL_PROFILE_PROFILE distribution profile is not for distribution. Select official, or your fork's own profile." >&2
        return 1
      fi
      if [[ -z "${EMAIL_RELEASE_GOOGLE_CLIENT_ID:-}" || -z "${EMAIL_RELEASE_GOOGLE_CLIENT_SECRET:-}" ]]; then
        echo "Distribution requires EMAIL_RELEASE_GOOGLE_CLIENT_ID and EMAIL_RELEASE_GOOGLE_CLIENT_SECRET. Development OAuth configuration is not sufficient." >&2
        return 1
      fi
      if ! EMAIL_RELEASE_GOOGLE_CLIENT_ID="$EMAIL_RELEASE_GOOGLE_CLIENT_ID" \
        python3 "$linux_build_repo/scripts/distribution-profile.py" --root "$linux_build_repo" check-release-google-client; then
        echo "Distribution requires a production Google desktop client ID." >&2
        return 1
      fi
      export EMAIL_GOOGLE_CLIENT_ID="$EMAIL_RELEASE_GOOGLE_CLIENT_ID"
      export EMAIL_GOOGLE_CLIENT_SECRET="$EMAIL_RELEASE_GOOGLE_CLIENT_SECRET"
      ;;
    *) echo "Unknown Linux release channel. Use local-alpha or distribution." >&2; return 1 ;;
  esac
  # A configured Google desktop client needs its token-exchange value. A
  # profile without Gmail sign-in (the neutral default) needs neither.
  if [[ -n "${EMAIL_GOOGLE_CLIENT_ID:-}" && -z "${EMAIL_GOOGLE_CLIENT_SECRET:-}" ]]; then
    echo "Gmail desktop OAuth application configuration is missing. Configure EMAIL_GOOGLE_CLIENT_SECRET in config/oauth-desktop.local.env before packaging. Never use an account password or token." >&2
    return 1
  fi
}

export EMAIL_BUILD_ID="$(python3 "$linux_build_repo/scripts/build-identity.py")"
