#!/usr/bin/env bash
# Builds the macOS DMG with its Finder window layout: a fixed icon-view window
# without toolbar or sidebar, Email on the left, Applications on the right and
# a background with an arrow between them. dmgbuild writes the layout
# (.DS_Store) directly, so no Finder session or AppleScript is needed.
# package-macos-release.sh calls this before it signs and notarizes the DMG.
#
#   scripts/make-macos-dmg.sh --app PATH/Email.app --volume-name Email --output OUT.dmg [--note FILE]
#
# --note adds a file (LOCAL-ALPHA.txt) centred below the icons.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
usage="Usage: $0 --app PATH/Email.app --volume-name NAME --output OUT.dmg [--note FILE]"
app="" volume_name="" output="" note=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) app="${2:?$usage}"; shift 2 ;;
    --volume-name) volume_name="${2:?$usage}"; shift 2 ;;
    --output) output="${2:?$usage}"; shift 2 ;;
    --note) note="${2:?$usage}"; shift 2 ;;
    *) echo "$usage" >&2; exit 2 ;;
  esac
done
if [[ -z "$app" || -z "$volume_name" || -z "$output" || "$output" != *.dmg ]]; then echo "$usage" >&2; exit 2; fi
if [[ ! -d "$app" || "$(basename "$app")" != Email.app ]]; then echo "No Email.app at $app" >&2; exit 2; fi
if [[ -n "$note" && ! -f "$note" ]]; then echo "No note file at $note" >&2; exit 2; fi

# Layout in points from the top left of the window content; icon positions
# are centres. The height includes the title bar, so the background, drawn at
# the full height, always covers the content below it.
width=640 height=380 icon_size=128 icon_y=150 left_x=170 right_x=470 caption_y=285
note_y=0
if [[ -n "$note" ]]; then note_y=$((caption_y + 100)); height=$((height + 160)); fi

# Pinned dmgbuild in its own virtual environment, reused while the pins match.
requirements="$repo_root/scripts/release/dmgbuild-requirements.txt"
python="${EMAIL_DMG_PYTHON:-python3}"
if ! "$python" -c 'import sys; sys.exit(sys.version_info < (3, 10))' 2>/dev/null; then
  echo "dmgbuild needs Python 3.10 or later; set EMAIL_DMG_PYTHON to one." >&2
  exit 2
fi
tools="$repo_root/target/tools/dmgbuild-$(shasum -a 256 "$requirements" | cut -c1-16)"
if [[ ! -x "$tools/bin/python" || ! -f "$tools/.installed" ]]; then
  rm -rf "$tools"
  mkdir -p "$(dirname "$tools")"
  "$python" -m venv "$tools"
  "$tools/bin/python" -I -m pip install --quiet --disable-pip-version-check --no-input \
    --require-hashes --only-binary :all: --no-deps -r "$requirements"
  touch "$tools/.installed"
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/email-dmg.XXXXXX")"
mount_point=""
cleanup() {
  if [[ -n "$mount_point" ]]; then hdiutil detach -quiet "$mount_point" 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT
swift "$repo_root/scripts/release/dmg-background.swift" "$work" \
  "$width" "$height" "$icon_size" "$icon_y" "$left_x" "$right_x" "$caption_y"
rm -f "$output"
TMPDIR="$work" "$tools/bin/python" -I -m dmgbuild -s "$repo_root/scripts/release/dmg-settings.py" \
  -D "app=$app" -D "note=$note" -D "background=$work/background.png" \
  -D "width=$width" -D "height=$height" -D "icon_size=$icon_size" -D "icon_y=$icon_y" \
  -D "left_x=$left_x" -D "right_x=$right_x" -D "note_y=$note_y" \
  "$volume_name" "$output"

# dmgbuild does not check its copies: confirm the image holds an intact app,
# the Applications link and the layout before anything signs it.
mount_point="$work/mount"
mkdir "$mount_point"
hdiutil attach -quiet -readonly -nobrowse -noautoopen -mountpoint "$mount_point" "$output"
for name in .DS_Store .background.tiff .VolumeIcon.icns Email.app/Contents/Info.plist ${note:+"$(basename "$note")"}; do
  if [[ ! -e "$mount_point/$name" ]]; then echo "The DMG is missing $name." >&2; exit 1; fi
done
if [[ "$(readlink "$mount_point/Applications")" != /Applications ]]; then
  echo "The DMG has no Applications link." >&2
  exit 1
fi
codesign --verify --deep --strict "$mount_point/Email.app"
hdiutil detach -quiet "$mount_point"
mount_point=""
