# Reader privacy defaults

Both desktop shells block remote images by default, because a remote image can
tell the sender when and where the email was opened. Per-draft open tracking is
left out of released builds.

## Remote images

- **Default:** blocked. The reader's content security policy allows only
  `data:` images until the user loads remote images, so same-message `cid:`
  images, which the service inlines as bounded `data:` URLs, still render.
- **Notice:** when the shown conversation has remote images and they are
  blocked, a slim row above the message reads "Remote images are blocked." with
  a **Load images** button. Its tooltip explains that remote images can tell the
  sender when and where the email was opened, and that `I` always loads them.
  The button does not keep keyboard focus: Linux sets focus-on-click off and,
  after keyboard activation, both shells return focus to an open reply, else
  the message list. `J`/`K` keep working after it.
- **This email:** **Load images** (or the image control's "Load images in this
  email") allows remote images for what is shown: on macOS the conversation
  (all its messages render together), on Linux the shown message (one renders
  at a time), so a reply that arrives later starts blocked. Moving elsewhere
  and returning blocks them again. The remembered default is unchanged.
- **Default choice:** `I`, the image control's "Load/Block remote images by
  default", the macOS View menu item and the palette change the remembered
  default. Only that explicit choice is stored.
- **Existing settings:** Linux stores `remote-images-enabled` only when the user
  chooses, so an existing file is kept and a missing one now means blocked.
  The macOS build of 7 November wrote `true` for every user once and set
  `EmailRemoteImagesDefaultV2`; that value cannot be told apart from a choice.
  A one-time migration (`EmailRemoteImagesDefaultV3`) resets a stored `true`
  only when that marker is set, and keeps a stored `false` and any value from
  earlier builds, which stored it only on the user's toggle. Every later choice
  is recorded (`EmailRemoteImagesUserChoice`) and survives future default
  changes. A user who had turned images off and on again since 7 November
  presses `I` once.
- **Tooltips:** the image control says whether images are blocked, loaded in
  this email or loaded automatically; it and the Load images button say what
  `I` would do to the default. Same text on both shells.
- **Detection:** both shells use the same rule on sanitized HTML: an `<img>`
  with an HTTPS source (attribute values may contain `>`), or an HTTPS URL anywhere in a `style` attribute, which
  covers `url()`, `image-set()` and escaped forms such as `u\72l(…)`. The
  sanitizer drops `<style>` elements, and only HTTPS can load once allowed (the
  service upgrades `<img>` http sources), so a CSS `url(http://…)` does not
  raise the notice. A URL whose scheme itself is CSS-escaped would stay
  blocked without a notice; that is not a leak.
  macOS: `RemoteImagePreference.containsRemoteImages`; Linux:
  `has_remote_images`. The rule scans text rather than parsing HTML, so
  `<img src=https://…>` written inside another attribute value or
  `style="https://…"` in body text can show the notice on mail with no remote
  image; Load images then has no effect. That is benign and the same on both
  shells. Leaving the rule in the shells is deliberate: macOS
  checks the composed thread document after inline images resolve, so a
  per-message service flag would not cover what it renders.
- **Empty reader:** clearing the reader (an empty search, folder or list) ends
  a per-email allowance on both shells. On Linux a new-message composer also
  stops selection changes from re-rendering the hidden reader.

The sender-logo (BIMI) lookup on macOS is independent of this setting. It asks
for the sender domain's published logo, at most once a day per domain, and
carries nothing that identifies a particular message.

## Open tracking out of released builds

Open tracking follows the build's distribution profile (see
[distribution profiles](distribution-profile.md)), through one key:
`OPEN_TRACKING`. It is `true` only in the `development` profile and `false` in
`official` and `unofficial`. Both profile parsers refuse `true` in a
distribution profile (`DISTRIBUTION=true`) or without a `SCHEDULING_ORIGIN`.

- **Mail service:** `build.rs` sets the `email_open_tracking` cfg only for a
  profile with `OPEN_TRACKING=true`. Without it, tracking registration and
  pixel insertion are not compiled: the service refuses to register a record
  and never adds a pixel, whichever shell, scheduler or MCP call sends the
  draft (`crates/mail-service/src/tracking.rs`). An official-profile library
  contains none of the registration code (checked by searching the built
  `libmail_service.dylib` for its strings).
- **Shells:** both read the engine's value (macOS through
  `DistributionProfile.openTracking`, Linux through `mail_service::profile`) and
  hide the per-draft **Track opens** option, its palette command and copy that
  points users at it. A shell can never disagree with the engine it runs.
- **Builds:** `scripts/run-macos.sh`, `scripts/run-linux.sh`, the MCP helper and
  packaging all build with the selected profile. `--distribute` requires
  `official`, so a released build compiles tracking out. Builds made with the
  `development` profile, including a local alpha packaged from it, keep it.

In a released build:

- Mail already sent with tracking keeps its counts: the reader's open-activity
  button, contact-pane activity, alerts, and **Recent opens** still work.
  **Recent opens** and the Linux activity button appear only when such mail
  exists in the local cache, checked at launch and when the palette opens until
  found (an open palette is not rebuilt; the next one lists it). The check is
  the service's `tracking_capability` op: one indexed `EXISTS` query that also
  reports whether this build has open tracking (`available`).
- A draft that a development build marked for tracking is sent without a pixel,
  including from the schedule or the MCP helper. Its tracking is turned off in
  the same transaction that records the provider's acceptance
  (`mark_draft_sent_untracked`), so a failed send changes nothing. A direct
  `send_draft` result (for example from the MCP helper) carries the warning
  "Sent without open tracking: this build does not include it."; scheduled
  dispatch and the apps' send queue do not show it yet.
  Released composers make no tracking calls, so until it is sent the draft
  keeps its development-build setting and a development build can still send it
  tracked. This changes only local state.
- A new-message composer that covers the reader also hides the notice on macOS,
  so its button is not reachable by keyboard behind the composer (Linux does
  not re-render the covered reader at all).
- The hosted Scheduling service, its device grant and tracking API are
  unchanged. The hosted service sets the device grant's scope
  (`booking:read email:tracking`) and rejects client-chosen scopes, so released
  builds still ask for optional email-activity access in the browser, and the
  Linux approval copy keeps saying so. Narrowing the grant for released builds
  needs a coordinated Scheduling change.

On Linux the **Recent opens** reader button is shared by all conversations
(shown when any tracked sent mail exists in a released build, always in
development builds), while macOS shows its open-count button only on tracked
sent messages.

## Tests

Fixture data: `scripts/seed-reader-privacy-fixture.py` adds a synthetic
newsletter (a `cid:` image, an `<img>` and a CSS background on reserved
`.example` hosts), a plain message, and with `--tracked` a sent message with
three recorded opens plus a tracked draft. It refuses the HOME-based default
stores.

macOS actual-source AppKit harness:

```sh
EMAIL_DISTRIBUTION_PROFILE=official cargo build -p mail-service --release --target-dir target/profile-official
EMAIL_DISTRIBUTION_PROFILE=development cargo build -p mail-service --release --target-dir target/profile-development
python3 apps/macos/Tests/run-reader-privacy-checks.py \
  --library target/profile-official/release/libmail_service.dylib \
  --dev-library target/profile-development/release/libmail_service.dylib
```

The harness compiles the real `EmailUI` sources once and drives `MailWindow`
against disposable stores, a disposable HOME and a separate preferences domain.
The shell reads open tracking from the engine's profile, so the library picks
the variant: `official` (Scheduling on, `OPEN_TRACKING=false`) is the public
case, `development` the development one. Each run checks that the library
carries the intended profile and that the engine's `available` flag matches
it.

The migration rules have an isolated check (`RemoteImagePreferenceChecks`, run
by `scripts/check-release-swift.py`), including that a new install records no
choice. The service gate has
`builds_without_open_tracking_never_register_or_add_a_pixel`, and both profile
parsers test the `OPEN_TRACKING` rules and catalog values.
