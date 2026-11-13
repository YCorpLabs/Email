#!/usr/bin/env bash
# Shared packaging checks; source this file without side effects.
require_package_notices() {
  local source_root="$1" notice
  for notice in LICENSE THIRD_PARTY_NOTICES.md; do
    if [[ ! -s "$source_root/$notice" ]]; then
      echo "Required package notice is missing or empty: $notice" >&2
      return 1
    fi
  done
}

install_package_notices() {
  local source_root="$1" destination="$2"
  require_package_notices "$source_root" || return 1
  install -d "$destination" || return 1
  install -m644 "$source_root/LICENSE" "$source_root/THIRD_PARTY_NOTICES.md" "$destination/"
}
