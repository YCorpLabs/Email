# Interface direction

The product should feel calm, precise, and fast. The visual language is original, not a copy of another mail client.

## Layout

Desktop: compact account/navigation rail, virtualized conversation list, reading pane. All panes resize; the reading pane can fill the window. Preserve selected thread and keyboard focus across sync updates.

## Visual system

- Start with native text rendering and system fonts, with clear size/weight hierarchy.
- Light theme: warm off-white canvas and restrained ink. Dark theme: deep graphite rather than pure black.
- One vivid accent for focus and primary actions; color never carries meaning alone.
- 4/8-point spacing rhythm, consistent 6–10 point corner radii, minimal shadows.
- Motion should explain state change and finish quickly; respect reduced-motion settings.
- No heavy blur or constant animation in the inbox.

## Keyboard behavior

The inbox is usable without a pointer. `j`/`k` move selection, `Enter` opens, `e` archives, `r` replies, `c` composes, `/` searches, and platform command key + `K` opens the command palette. When a text field or composer has focus, single-letter shortcuts do not fire. Include shortcut discovery and eventual remapping.

## Email rendering

The shell is native. A single reusable isolated webview displays the selected HTML message. Plain text gets a native view. Preserve safe message styling and inline resources. Do not force dark styling on arbitrary sender HTML. Keep navigation, scripts, and remote resource loading under app control.

## Validation

Build a fixture corpus of real-world message shapes: plain text, multipart alternative, CID images, newsletters with tables, long threads, RTL text, dark and light branding, malformed HTML, and large attachments. Compare screenshots and memory on macOS and Linux.


## Possible improvement: expanded email reader

Not scheduled. Explore an explicit, optional reader expansion that hides the
conversation list for focused reading, with an obvious way back, preserving the
selected conversation, scroll position and keyboard focus. Default reading
should keep the split view.

