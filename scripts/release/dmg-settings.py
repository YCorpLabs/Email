# dmgbuild settings for the macOS DMG (scripts/make-macos-dmg.sh).
# dmgbuild runs this file with `defines` holding the values the script passes
# with -D; the script owns the layout numbers so the background matches.
import os.path

app = defines["app"]
note = defines.get("note", "")
width, height = int(defines["width"]), int(defines["height"])
icon_y = int(defines["icon_y"])

format = "UDZO"
filesystem = "HFS+"
files = [app] + ([note] if note else [])
symlinks = {"Applications": "/Applications"}
icon = os.path.join(app, "Contents", "Resources", "Email.icns")
background = defines["background"]

# A fixed window: icon view only, no toolbar, sidebar, path or status bar.
default_view = "icon-view"
show_toolbar = False
show_sidebar = False
show_pathbar = False
show_status_bar = False
show_tab_view = False
# Finder measures the window origin from the bottom left of the screen.
window_rect = ((400, 340), (width, height))
arrange_by = None
icon_size = int(defines["icon_size"])
text_size = 13
icon_locations = {
    os.path.basename(app): (int(defines["left_x"]), icon_y),
    "Applications": (int(defines["right_x"]), icon_y),
}
if note:
    icon_locations[os.path.basename(note)] = (width // 2, int(defines["note_y"]))
