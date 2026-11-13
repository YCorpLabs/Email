#!/usr/bin/env bash
# Sourced by Linux packagers. Installs the launcher, AppStream metadata and icon
# under the distribution profile's LINUX_APP_ID. GTK uses the same ID as its
# application ID, so compositors match windows to this launcher.

install_linux_desktop_entry() {
  local repo="$1" app_id="$2" root="$3"
  if [[ ! "$app_id" =~ ^[A-Za-z_][A-Za-z0-9_-]*(\.[A-Za-z_][A-Za-z0-9_-]*){2,4}$ ]]; then
    echo "Invalid Linux app ID from the distribution profile." >&2
    return 1
  fi
  install -d "$root/usr/share/applications" "$root/usr/share/metainfo"
  sed "s/@APP_ID@/$app_id/g" "$repo/apps/linux/email-linux.desktop" > "$root/usr/share/applications/$app_id.desktop"
  sed "s/@APP_ID@/$app_id/g" "$repo/apps/linux/packaging/metainfo.xml.in" > "$root/usr/share/metainfo/$app_id.metainfo.xml"
  chmod 644 "$root/usr/share/applications/$app_id.desktop" "$root/usr/share/metainfo/$app_id.metainfo.xml"
  install -Dm644 "$repo/assets/email-icon.png" "$root/usr/share/icons/hicolor/1024x1024/apps/$app_id.png"
}
