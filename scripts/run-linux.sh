#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/linux-build-config.sh"
email_refuse_distribution_profile
email_require_google_client_id "$repo_root"
cd "$repo_root"
cargo build --manifest-path apps/linux/Cargo.toml --release
exec "$repo_root/apps/linux/target/release/email-linux" "$@"
