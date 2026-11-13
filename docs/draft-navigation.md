# Draft navigation

Selecting a draft previews it without entering the composer. J/K move between
rows, including after closing an editor. Enter edits the selected draft;
macOS double-click and Linux row activation do the same. While editing, J/K
remain text input. macOS Escape saves changes through the existing close path
and returns focus to the list.

A first J with no selection starts at the first row rather than skipping it.
Draft bodies use the existing mail renderer with escaped plain text.

An AppKit fixture checks this behavior against an isolated demo database. Run
it after building the Rust service:

```sh
python3 apps/macos/Tests/run-draft-navigation-checks.py --library target/release/libmail_service.dylib
```

## Draft focus and recipient protection

Opening an addressed draft used to put focus in To with its whole value
selected, so the next key overwrote it, and Escape then saved the empty To
silently. Both shells now:

- open a composer that already has a recipient (an existing draft, a message
  to a contact) with focus in the message body; To is focused only when no
  recipient is set;
- focus To (and Cc/Bcc from Commands on macOS) with the caret at the end
  instead of selecting the value;
- ask before Escape or Close saves a draft whose To, Cc and Bcc were all
  emptied after opening. **Restore Recipients** is the cancel and default
  button and restores the values the draft opened with;
  **Save Without Recipients** saves as before. An explicit **Save draft**
  is not interrupted.

An editor that is already open does not reload a draft changed by an agent;
closing it saves the stale copy.
