# Connected calendar

## Product

Open Invitations → Open connected calendar… on Mac or Linux. Choose a mailbox, connect Google Calendar explicitly, then Refresh. Day/week views read the local cache; changing dates or reading mail does not wait for provider refresh. Left/right change the displayed range and Escape closes Calendar. A separate calendar worker keeps browser authorization and calendar network calls off the mail reader lane.

The agenda supports expanded recurring instances, timed events and all-day events, account selection, Yes/Maybe/No RSVP, and event creation with attendee invitations. The Create button explicitly names its external effect. Calendar responses and creation are never dispatched automatically. Creating after an uncertain provider error must be checked in Google Calendar before retrying to avoid duplicates.

Find times produces up to five 30-minute candidate slots in 09:00–17:00 on the selected day, in the device timezone. Copy suggested times includes the date, timezone and a review caveat. These are suggestions from readable connected calendars, not authoritative organization-wide free/busy. Coverage is incomplete when the requested window has no recent full snapshot. FreeBusy-only calendars, unconnected accounts and unsynced external changes may contain conflicts. No public booking URL is fabricated.

## Credentials and boundaries

Calendar requires an explicit OAuth flow requesting OpenID email identity, calendar.events and calendar.calendarlist.readonly. It verifies the Google account against the selected mailbox. Credentials are kept in the OS vault under a separate calendar key, leaving stored mail credentials unchanged. Google may include previously granted app scopes in incremental consent. No Calendar access is requested during ordinary mail sign-in or mail sync.

RSVP re-reads the event and requires its self attendee to match the verified Google identity, then sends a participant-only patch guarded by ETag/If-Match. The provider resolves recurrence and timezone offsets; all-day midnight is resolved with IANA timezone/DST data. Ambiguous times fail rather than being guessed. Remote descriptions are displayed as native text, never trusted executable HTML.

Snapshot limits: 93-day query, 32 calendars per service refresh, bounded provider pagination, 10,000 events/calendar, response/body/field limits. All calendar lists, windows and events commit in one SQLite transaction after complete fetch and validation. Refresh failures retain the previous snapshot. Coverage expires after five minutes; any attempted provider mutation invalidates it, and a successful created event is cached without certifying an entire window.

## API and MCP

Service operations: calendar_status, connect_calendar, list_calendars, list_agenda, sync_calendar, respond_calendar_invitation, create_calendar_event, availability. Time ranges use Unix milliseconds; duration_minutes is 5–1440. List/cache operations work without credentials.

MCP exposes list_calendars, list_agenda, sync_calendar, availability, respond_calendar_invitation and create_calendar_event. All require a specific account argument. Sync requires --allow-sync. RSVP and event creation require **--allow-calendar-write**, independently of --allow-write and --allow-send, and the account's in-app Agent access must include **Calendar changes** (off by default). These calendar writes are not approved per request; see [mcp-security.md](mcp-security.md). OAuth connection is deliberately unavailable as an MCP tool. Existing --account restrictions apply to every calendar operation.

## Remaining scope

Microsoft/CalDAV connections, editable work-hour preferences, provider free/busy, conflict-safe hosted booking links, event cancellation, and calendar-specific notifications. Public booking links require a deployed scheduling service with authorization, expiry and conflict rechecks; a desktop-only link would not work while the computer is offline.


## Day and Week grids

The old grouped agenda has been replaced by actual Day/Week time grids in
both native shells. Week is Monday–Sunday, includes empty days, uses local
hour rows and a separate all-day band. Intersecting events receive separate
lanes; multi-day timed events are clipped into each visible day. A narrow
window preserves Week via horizontal scrolling rather than shrinking event
columns until unreadable. Event blocks are buttons with full date/time
accessibility labels and tooltips. Clicking opens read-only details; existing
explicit, account-scoped RSVP controls remain gated by provider permissions.

The sidebar Calendar button now opens Calendar directly. Cached invitation
inspection remains available through the existing invitations command.
Back to mail is visible. Details, creation and suggested-times screens have
Back to calendar; Escape returns to the grid before it closes Calendar.
J/K scroll 80 pixels, Ctrl-D/U scroll half a viewport, and left/right change
the date range outside text editing. Shortcut help is visible in the footer.

Day headers currently scroll with the grid; pinned headers remain a follow-up
polish item.


## Unified mail and Calendar navigation

Calendar is embedded beside the main persistent mailbox sidebar on both
platforms. Mail columns and Calendar model remain retained
while switching, preserving selection, date/account, search and event details.
Calendar no longer requires a separate navigation window from the sidebar.

Shared contract: J/K or Up/Down select chronologically, Return opens details,
J/K scroll inside details, Escape returns to the grid, Ctrl-D/U scroll half a
viewport, and G C/G I switch Calendar/Inbox. Calendar C creates an event and
/ searches the cached events in the displayed date range; Shift is tolerated,
for keyboard layouts that type / with Shift (Norwegian Shift-7). Sidebar destinations
remain clickable. Contextual command palettes offer Calendar actions and block
mail mutations against hidden selections. Native editing takes precedence.


## Calendar navigation, overview modes and color

The interaction now distinguishes selecting an event (J/K, Enter) from changing
calendar periods (H/L or Left/Right). D/W/M/Y and 1/2/3/4 choose Day/Week/Month/Year;
T returns to today. B opens Go to date; Escape dismisses it without a write. View
controls and date arrows display their keys, the footer describes the active
period, and ? opens contextual help. These are application shortcuts, not global
system shortcuts. Typing fields retain normal keys. Calendar commands also appear
in the context-specific command palette; hidden mail mutations remain rejected.

Month is a real Monday-first 42-cell grid. Event chips adapt to cell height and
show overflow drill-down. Year is a responsive twelve-month overview with event
dots; month headings open Month and dates open Day. Selection outlines the matching
date even when its event chip is not visible. Local-day conversion respects date-only
all-day events. Mac normalizes event dates once per overview before building cells.

Calendar colors are stable across platforms: FNV-1a 32-bit over calendar ID selects
one of six subdued colors (#8ea6bf, #b4a3c8, #baaa88, #8fb5aa, #c39b9b, #aaaaba).
Events share their calendar's tint and a legend allows calendars to be hidden/shown.
Colors are a visual grouping aid, not event priority or sender verification.

New ranges render cached data first, then refresh connected Google Calendar in
the background. Requests are coalesced while busy; successful covering ranges are
fresh for five minutes and failed range attempts cool down for a minute. Refresh
never blocks mail navigation. Cached listing and provider sync allow up to 400 days
for leap-year views, while availability and event creation keep the 93-day bound.
Full-view overflow above 10,000 events is reported instead of silently truncated.
A refresh replaces overlapping events only, retaining cached other-month events.
Inbound parsing skips Google attendee addresses that cannot be represented,
keeping the event and its valid attendees; outbound validation and RSVP
identity checks remain strict.


## Compact toolbar and Linux surface

The calendar toolbar is compacted into date/account and
navigation/view/search rows, with a thin calendar-color legend. Linux now paints
an opaque calendar host, viewport and content surface while retaining sidebar
material. Event titles truncate rather than wrap vertically; weekday headings
have explicit height and padding. The narrow toolbar adapts below 780px.


## Calendar rail and header

A calendar rail replaces the horizontal source legend: mini month, account, source visibility, timezone and availability. The
header consolidates date navigation, view selection, search and creation. Event
colors are more distinct, grid lines quieter, and event details use a contextual
popover that leaves the grid visible. Plus/minus adjusts hour density; the viewed
time anchor is retained. Local reloads retain the previous grid until replacement
results arrive, with generation guards against stale-period responses.

Narrow Mac windows use a compact rail popover. Linux keeps a
minimum week-grid width with horizontal scrolling in very narrow tiles. Mac
mini-month dates are custom-drawn without individual accessibility elements. The
two native shells still differ in some month-cell and event-popover presentation.

Availability links, drag to reschedule and broader multi-select calendar actions
are not implemented.


## Unified accounts and contextual editing

Calendar now defaults to **All accounts**. The source rail groups calendars by
account; choosing an account narrows the view, while individual source controls
only affect visibility. Accounts without Calendar consent have a visible Connect
calendar entry. Mail consent alone does not grant Calendar access. Refresh reads
already connected accounts without unexpectedly starting OAuth.

The shared service filters hidden account/calendar pairs before removing repeated
copies of the same shared-calendar event. Matching requires calendar ID, event ID
and occurrence times across accounts; it never merges by title or holiday name,
and the account-relative `primary` alias is excluded. Hiding one copy leaves a
visible copy from another account. These are view operations, never deletions.

B opens a visual month picker and accepts today, tomorrow, next week or a date.
C / New event opens a contextual editor with title focus, labeled fields and
Tab navigation. Clicking an empty timed slot supplies its date/time. Return on
an event opens details; eligible events expose Edit / E. The destination account
is derived from the selected calendar, never whichever mail account happened
to be selected. Escape closes the active popover before leaving Calendar.

Updates use the cached ETag plus an If-Match request, preserve omitted fields and
existing guests, reject conflicting versions, and do not retry writes blindly.
All-day events and recurring series are still read-only; eligible recurring
instances use instance scope. Editing does not yet provide recurrence-rule or
attendee management.

MCP calendar updates require the separate calendar-write permission and explicit
account selection. General mail-write permission does not imply calendar writes.

The ignored `calendar_scale` test in `mail-core` measures warm week reads over
10,000 synthetic events in a disk-backed store
(`cargo test -p mail-core --release --test calendar_scale -- --ignored --nocapture`).
It excludes UI rendering, IPC and network time.

A connected Gmail mail account still needs separate Calendar consent; the account
selector exposes Connect calendar for accounts missing that grant.


### Natural date navigation

Press B in Calendar, type `20 October`, `next Friday`, `in 2 weeks`, or `+2w`,
and press Return after checking the full resolved date. Named dates without a
year use the current year. Tab opens visual day browsing and Escape closes.
The calendar command palette also accepts these date expressions. `?` opens
shortcut help; H/L changes the visible period and D/W/M/Y selects the view.


### Compact event editing

C opens a title-focused new-event editor using the selected day/time. Choose the
calendar and time; expand Add details for location, notes, or guests. Tab and
Shift-Tab move through controls. Cmd-Return on Mac or Ctrl-Return on Linux saves;
Escape cancels. Adding guests sends invitations only when you create the event.
Editing keeps existing guests and saving may notify them.


### Quick event entry

Press C and type `Lunch tomorrow at 12 for 1h` in the title field. The draft date and time update immediately as you type. The compact date/time
summary expands into editable controls when clicked. Manual time edits take
precedence; scheduling words are removed from the input so it stays consistent
with the edited time. Moving the start preserves the event duration. Create event and the save shortcut
always submit the current draft directly; there is no separate Apply step.
Return in the title moves focus to the date/time summary. An unsaved draft block
is shown in the day/week grid and removed when the editor closes.

Supported syntax is `<title> [date] [at] <time> [for duration]`, for example
`Design review next Friday at 2:30pm for 45m`, `lunch 12h`, `Lunch 12:30`,
or `Coffee at 9`. The `h` suffix denotes a 24-hour clock time; `for 1h` denotes
a duration. Without a date
it uses the selected calendar day; without a duration it uses 30 minutes.
Relative dates use today. Durations accept minutes or hours, from 5 minutes to
24 hours. Invalid numeric time expressions block saving with a correction hint.
This parser runs locally and does not call an AI service. A plain title still
uses the regular date and time fields.


### Ambiguous range input

Leading month/day ranges such as `jan 8-12 bbq w/ Sam` are currently detected
as ambiguous and block Create instead of silently retaining the selected date.
Date-first range creation and all-day creation are not implemented yet. Use the
explicit date/time controls for supported timed events.

### Subscribed calendar sidebar visibility

The Mac calendar list no longer caps its viewport at 264 points while a spacer
consumes the remaining rail height. Its scroll view fills available space;
overflow remains scrollable. Read-only/subscribed collections are fetched with
reader access and included in both shells. Linux uses a vertically expanding
scroller.
