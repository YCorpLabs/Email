# Support and feedback

Both desktop shells expose **Support & feedback** in the sidebar, plus **Ask for
support…** and **Give feedback…** in Commands. The destinations come only from
the build's [distribution profile](distribution-profile.md) (`SUPPORT_ADDRESS`
and `FEEDBACK_ADDRESS`), compiled into the mail engine. The official and
development profiles use `support@ycorplabs.com` and `feedback@ycorplabs.com`.

## Builds without support addresses

A fork or self-build may configure both addresses or neither; the profile
parser refuses one alone. A build without them never falls back to Y Corp AS's
addresses, because a fork's users could otherwise send logs and screenshots to
a team that cannot help them. In both shells, the sidebar button and both
commands remain and open one notice instead of the chooser:

> This build has no support address configured, so Email can't start a support
> or feedback message.
>
> If someone else gave you this build, ask them for help. If you built it
> yourself, see FORKING.md to add your own support channels.

It has a single **OK** button, which has keyboard focus. Return or Escape
dismisses it. On macOS a single-button alert ignores Escape by default, so the
notice adds an Escape handler. No draft is created, and the feedback thank-you can never
trigger. The notice is chosen over links to the project's GitHub pages: those
would still route a fork's users to Y Corp AS.

The chooser explains the destination and opens a normal, editable draft with a
subject and short writing prompts. It never sends a message. The message body
receives keyboard focus. An existing composer must be saved and closed first;
opening support cannot replace an unsaved draft. Escape/Cancel dismisses the
chooser without creating a draft; closing the composer uses ordinary draft saving.

**Include basic app information** starts unchecked every time. The exact
information is displayed before opting in: app version, build identifier and OS
(macOS version, or Linux/CPU architecture). Opting in inserts that text into the
editable body. Users can delete it before sending. No account details, mail,
credentials, database contents or raw errors are collected by this flow.

Screenshots, existing log files and other files are optional. Use the composer's
**Attach file** action and remove unwanted attachment chips before sending.
There is no automatic screenshot capture, log harvesting or background upload.
The existing composer attachment limits apply. This flow does not generate a
log bundle; the separate [local diagnostic export](diagnostics.md) remains an
explicit, offline command and should be reviewed before manually attaching it.

Without a connected account (or on macOS without an available engine), the flow
asks the system mail handler to open a prefilled `mailto:` draft. External mail
apps control their own attachment UI. A launch failure displays the destination
address so users can still contact the team.

## Feedback thank-you easter egg

After a successful Send queue or Schedule operation, a message addressed in To
to the profile's feedback address (`feedback@ycorplabs.com` in official builds)
shows a compact, one-line **Thanks for the feedback!** card
with a purple heart. Matching uses parsed
mailboxes, ignores case/display names, and checks the final recipient, including
reopened drafts. Changing the destination away from feedback disables the effect.
Saving, cancelling, opening an external mail app, failed saves and failed queue
requests do not trigger it. The thank-you does not claim provider delivery.

Both shells position the card at the bottom center, 40 points/pixels above the
content edge, away from the send/undo controls in the header. The card has a
roughly 320-wide footprint, a subtle border, 13-point semibold text and native
theme colors. A small burst of 36 confetti pieces arcs upward around it and fades
within 2.1 seconds. The card eases in over 220 ms with an 8-point rise, holds, and
fades out over 350 ms; the whole effect clears after about 3.2 seconds. Showing
it again replaces the previous celebration instead of stacking cards.

The effect is click-through and never takes keyboard focus. macOS Reduce Motion
and GTK’s disabled-animation preference retain the static card without particles,
sliding or fading. No assets, network calls or new dependencies are needed.

## Distribution profile

Each shell has one profile source. On macOS, `DistributionProfile.current` is
loaded from the same engine library `CoreBridge` opens. On Linux, the shell
reads `mail_service::profile`. Neither shell contains an address literal. The
macOS harness (`DistributionProfileChecks`) and Linux unit tests cover
configured, partial, malformed and absent addresses. They also check the
feedback match.
