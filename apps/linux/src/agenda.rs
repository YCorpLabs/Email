use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CalendarMode {
    Day,
    Week,
    Month,
    Year,
}
impl CalendarMode {
    fn index(self) -> usize {
        match self {
            Self::Day => 0,
            Self::Week => 1,
            Self::Month => 2,
            Self::Year => 3,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }
}
const CALENDAR_COLORS: [&str; 6] = [
    "#7da8dd", "#a98ad5", "#d0a565", "#75b5a1", "#cf8e9f", "#929ec6",
];
fn calendar_tone(id: &str) -> usize {
    id.as_bytes().iter().fold(2166136261u32, |hash, byte| {
        (hash ^ *byte as u32).wrapping_mul(16777619)
    }) as usize
        % CALENDAR_COLORS.len()
}
const CALENDAR_KEYS: &str =
    "D / W / M / Y  View    H / L  Previous / next period    T  Today    B  Go to date    ?  Help";
struct OverviewEntry<'a> {
    event: &'a Value,
    start: i64,
    end: i64,
    all_day: bool,
}
pub(super) struct Agenda {
    ui: Rc<Ui>,
    window: gtk::ApplicationWindow,
    accounts: Vec<Account>,
    account: gtk::DropDown,
    date: RefCell<glib::DateTime>,
    date_label: gtk::Label,
    status: gtk::Label,
    rows: gtk::Box,
    connect: gtk::Button,
    refresh: gtk::Button,
    calendars: RefCell<Vec<Value>>,
    generation: Cell<u64>,
    busy: Cell<bool>,
    connected_accounts: RefCell<Vec<String>>,
    mode: Cell<CalendarMode>,
    mode_buttons: Vec<gtk::ToggleButton>,
    mode_menu: gtk::DropDown,
    mini_calendar: gtk::Box,
    updating_mini: Cell<bool>,
    hour_height: Cell<i32>,
    detail_popup: RefCell<Option<gtk::Popover>>,
    editor_popup: RefCell<Option<gtk::Popover>>,
    create_button: gtk::Button,
    legend: gtk::Box,
    legend_scroll: gtk::ScrolledWindow,
    hidden_calendars: RefCell<std::collections::HashSet<String>>,
    all_events: RefCell<Vec<Value>>,
    live_draft: Rc<RefCell<Option<(String, i64, i64)>>>,
    draft_canvases: Rc<RefCell<Vec<gtk::DrawingArea>>>,
    date_popup: gtk::Popover,
    date_entry: gtk::Entry,
    help_popup: gtk::Popover,
    source_popup: gtk::Popover,
    back: gtk::Button,
    scroll: gtk::ScrolledWindow,
    footer: gtk::Label,
    events: RefCell<Vec<Value>>,
    event_controls: RefCell<Vec<(String, gtk::Button)>>,
    selected: Cell<usize>,
    sync_successes: RefCell<Vec<(String, i64, i64, std::time::Instant)>>,
    sync_attempts: RefCell<Vec<(String, i64, i64, std::time::Instant)>>,
}
fn text(value: &str, strong: bool) -> gtk::Label {
    let label = gtk::Label::new(Some(value));
    label.set_xalign(0.);
    label.set_wrap(true);
    label.set_selectable(true);
    if strong {
        label.add_css_class("heading")
    } else {
        label.add_css_class("dim-label")
    };
    label
}
fn date(ms: i64) -> String {
    glib::DateTime::from_unix_local(ms / 1000)
        .ok()
        .and_then(|v| v.format("%H:%M").ok())
        .map(|v| v.to_string())
        .unwrap_or_else(|| "Time unavailable".into())
}
fn parse_calendar_date(value: &str, today: &glib::DateTime) -> Option<glib::DateTime> {
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .replace(',', "");
    let today =
        glib::DateTime::from_local(today.year(), today.month(), today.day_of_month(), 0, 0, 0.)
            .ok()?;
    let mut value = normalized.as_str();
    for prefix in ["go to ", "goto ", "jump to ", "jump ", "date ", "on "] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = rest.trim();
            break;
        }
    }
    match value {
        "today" | "now" => return Some(today.clone()),
        "tomorrow" => return today.add_days(1).ok(),
        "yesterday" => return today.add_days(-1).ok(),
        _ => {}
    }
    let units = |number: i32, unit: &str| -> Option<glib::DateTime> {
        if !(-100000..=100000).contains(&number) {
            return None;
        }
        match unit {
            "d" | "day" | "days" => today.add_days(number).ok(),
            "w" | "week" | "weeks" => today.add_days(number.checked_mul(7)?).ok(),
            "m" | "mo" | "month" | "months" => today.add_months(number).ok(),
            "y" | "year" | "years" => today.add_years(number).ok(),
            _ => None,
        }
    };
    let relative = value.strip_prefix("in ").unwrap_or(value);
    let relative = relative.strip_suffix(" from now").unwrap_or(relative);
    let (relative, sign) = if let Some(v) = relative.strip_suffix(" ago") {
        (v, -1)
    } else {
        (relative, 1)
    };
    let words: Vec<_> = relative.split_whitespace().collect();
    if words.len() == 2 {
        if let Ok(n) = words[0].parse::<i32>() {
            if let Some(day) = units(n.checked_mul(sign)?, words[1]) {
                return Some(day);
            }
        }
        if words[0] == "next" || words[0] == "last" {
            if let Some(date) = units(if words[0] == "next" { 1 } else { -1 }, words[1]) {
                return Some(date);
            }
        }
    }
    if let Some(index) = relative.find(|c: char| c.is_ascii_alphabetic()) {
        if let Ok(n) = relative[..index].trim().parse::<i32>() {
            if let Some(date) = units(n.checked_mul(sign)?, relative[index..].trim()) {
                return Some(date);
            }
        }
    }
    let (weekday, direction) = if let Some(v) = value.strip_prefix("next ") {
        (v, 1)
    } else if let Some(v) = value.strip_prefix("last ") {
        (v, -1)
    } else {
        (value, 0)
    };
    let weekdays = [
        ("mon", "monday"),
        ("tue", "tuesday"),
        ("wed", "wednesday"),
        ("thu", "thursday"),
        ("fri", "friday"),
        ("sat", "saturday"),
        ("sun", "sunday"),
    ];
    if let Some(index) = weekdays
        .iter()
        .position(|(short, full)| weekday == *short || weekday == *full)
    {
        let mut offset = (index as i32 + 1 - today.day_of_week()).rem_euclid(7);
        if direction < 0 {
            offset = if offset == 0 { -7 } else { offset - 7 }
        } else if direction > 0 && offset == 0 {
            offset = 7
        }
        return today.add_days(offset).ok();
    }
    let ordinal = |v: &str| {
        v.strip_suffix("st")
            .or_else(|| v.strip_suffix("nd"))
            .or_else(|| v.strip_suffix("rd"))
            .or_else(|| v.strip_suffix("th"))
            .unwrap_or(v)
            .parse::<i32>()
    };
    let tokens: Vec<_> = value.split_whitespace().collect();
    if tokens.len() == 2 || tokens.len() == 3 {
        let months = [
            "january",
            "february",
            "march",
            "april",
            "may",
            "june",
            "july",
            "august",
            "september",
            "october",
            "november",
            "december",
        ];
        let parse_month = |v: &str| {
            months
                .iter()
                .position(|m| *m == v || (v.len() == 3 && m.starts_with(v)))
                .map(|i| i as i32 + 1)
        };
        let pair = if let (Ok(day), Some(month)) = (ordinal(tokens[0]), parse_month(tokens[1])) {
            Some((day, month))
        } else if let (Some(month), Ok(day)) = (parse_month(tokens[0]), ordinal(tokens[1])) {
            Some((day, month))
        } else {
            None
        };
        if let Some((day, month)) = pair {
            let year = if tokens.len() == 3 {
                tokens[2].parse().ok()?
            } else {
                today.year()
            };
            let date = glib::DateTime::from_local(year, month, day, 0, 0, 0.).ok()?;
            if date.day_of_month() == day && date.month() == month {
                return Some(date);
            }
            return None;
        }
    }
    for separator in ['.', '/'] {
        let parts: Vec<_> = value.split(separator).collect();
        if parts.len() == 3 {
            let day = parts[0].parse::<i32>().ok()?;
            let month = parts[1].parse::<i32>().ok()?;
            let year = parts[2].parse::<i32>().ok()?;
            let date = glib::DateTime::from_local(year, month, day, 0, 0, 0.).ok()?;
            return (date.day_of_month() == day && date.month() == month).then_some(date);
        }
    }
    if value.len() != 10 {
        return None;
    }
    glib::DateTime::from_iso8601(&format!("{value}T00:00:00"), Some(&glib::TimeZone::local())).ok()
}
#[derive(Clone)]
struct QuickCalendarEvent {
    title: String,
    start: glib::DateTime,
    end: glib::DateTime,
    minutes: i32,
}
fn parse_quick_calendar_event(
    raw: &str,
    selected: &glib::DateTime,
    today: &glib::DateTime,
) -> Result<Option<QuickCalendarEvent>, String> {
    let value = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = value.to_ascii_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    if let (Some(month), Some(range)) = (words.first(), words.get(1)) {
        if let Some(name) = months
            .iter()
            .find(|name| month.eq_ignore_ascii_case(name) || month.eq_ignore_ascii_case(&name[..3]))
        {
            if let Some((first, last)) = range.split_once('-').or_else(|| range.split_once('–')) {
                if !first.is_empty()
                    && !last.is_empty()
                    && first.chars().all(|c| c.is_ascii_digit())
                    && last.chars().all(|c| c.is_ascii_digit())
                {
                    return Err(format!("Is this {name} {first}–{last}, or {name} {first} at {first:0>2}:00–{last:0>2}:00? Use an explicit date and time."));
                }
            }
        }
    }
    let (scheduled, duration) = if let Some(index) = lower.rfind(" for") {
        let tail = lower[index + 4..].trim();
        if lower[index + 4..].starts_with(' ')
            || tail.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-')
        {
            (&lower[..index], Some(tail))
        } else {
            (lower.as_str(), None)
        }
    } else {
        (lower.as_str(), None)
    };
    let (prefix, clock, explicit) = if let Some(at) = scheduled.rfind(" at ") {
        (value[..at].trim(), &scheduled[at + 4..], true)
    } else {
        let Some((before, last)) = scheduled.rsplit_once(' ') else {
            return Ok(None);
        };
        let clock_start = if ["am", "pm", "h"].contains(&last) {
            let Some((prefix, _)) = before.rsplit_once(' ') else {
                return Ok(None);
            };
            prefix.len() + 1
        } else {
            before.len() + 1
        };
        (
            value[..clock_start].trim(),
            &scheduled[clock_start..],
            false,
        )
    };
    if !clock.starts_with(|c: char| c.is_ascii_digit()) {
        return Ok(None);
    }
    if !explicit
        && (clock
            .chars()
            .any(|c| !c.is_ascii_digit() && ![':', 'a', 'p', 'm', 'h', ' '].contains(&c))
            || clock.chars().take_while(|c| c.is_ascii_digit()).count() > 2)
    {
        return Ok(None);
    }
    let minutes = if let Some(duration) = duration {
        let compact = duration.replace(' ', "");
        let boundary = compact
            .find(|c: char| c.is_ascii_alphabetic())
            .ok_or("Use a duration like 30m or 1.5h.")?;
        let amount = compact[..boundary]
            .parse::<f64>()
            .map_err(|_| "Use a duration like 30m or 1.5h.")?;
        let multiplier = match &compact[boundary..] {
            "m" | "min" | "mins" | "minute" | "minutes" => 1.,
            "h" | "hr" | "hrs" | "hour" | "hours" => 60.,
            _ => return Err("Use minutes or hours, for example 30m or 1.5h.".into()),
        };
        let minutes = amount * multiplier;
        if !minutes.is_finite() || !(5.0..=1440.0).contains(&minutes) {
            return Err("Choose a duration between 5 minutes and 24 hours.".into());
        }
        minutes.round() as i32
    } else {
        30
    };
    let compact = clock.replace(' ', "");
    let (clock, meridiem) = if let Some(clock) = compact.strip_suffix("am") {
        (clock, Some(false))
    } else if let Some(clock) = compact.strip_suffix("pm") {
        (clock, Some(true))
    } else {
        (compact.strip_suffix('h').unwrap_or(compact.as_str()), None)
    };
    let components: Vec<_> = clock.split(':').collect();
    if components.is_empty() || components.len() > 2 {
        return Err("Use a time like 12, 14:30 or 12pm.".into());
    }
    let mut hour = components[0]
        .parse::<i32>()
        .map_err(|_| "Use a time like 12, 14:30 or 12pm.")?;
    let minute = if components.len() == 2 {
        if components[1].len() != 2 {
            return Err("Use two minute digits, like 14:30.".into());
        }
        components[1]
            .parse::<i32>()
            .map_err(|_| "Use a time like 14:30.")?
    } else {
        0
    };
    if !(0..60).contains(&minute) {
        return Err("Minutes must be between 00 and 59.".into());
    }
    if let Some(pm) = meridiem {
        if !(1..=12).contains(&hour) {
            return Err("Use an hour from 1 to 12 with am or pm.".into());
        }
        hour = hour % 12 + if pm { 12 } else { 0 };
    } else if !(0..24).contains(&hour) {
        return Err("Use an hour from 0 to 23, or add am / pm.".into());
    }
    let mut title = prefix.to_owned();
    let mut chosen = selected.clone();
    let words: Vec<_> = prefix.split_whitespace().collect();
    let date_starters = [
        "today",
        "now",
        "tomorrow",
        "yesterday",
        "next",
        "last",
        "in",
        "on",
    ];
    let month_names = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    for index in 1..words.len() {
        let suffix = words[index..].join(" ");
        if let Some(date) = parse_calendar_date(&suffix, today) {
            chosen = date;
            title = words[..index].join(" ");
            break;
        }
        let first = words[index].to_ascii_lowercase();
        let explicit = date_starters.contains(&first.as_str())
            || (first.starts_with(|c: char| c.is_ascii_digit())
                && (first.contains('-')
                    || first.contains('/')
                    || first.contains('.')
                    || words.get(index + 1).is_some_and(|word| {
                        month_names
                            .iter()
                            .any(|month| month.starts_with(&word.to_ascii_lowercase()))
                    })))
            || month_names.contains(&first.as_str());
        if explicit {
            return Err("That date is not valid. Try tomorrow, next Friday or 20 October.".into());
        }
    }
    if title.trim().is_empty() {
        return Err("Give this event a title before the date and time.".into());
    }
    let start = glib::DateTime::from_local(
        chosen.year(),
        chosen.month(),
        chosen.day_of_month(),
        hour,
        minute,
        0.,
    )
    .map_err(|_| "That local time is unavailable.")?;
    if start.hour() != hour || start.minute() != minute {
        return Err("That time does not exist in your local timezone.".into());
    }
    let end = start
        .add_minutes(minutes)
        .map_err(|_| "That duration is outside the supported date range.")?;
    Ok(Some(QuickCalendarEvent {
        title,
        start,
        end,
        minutes,
    }))
}
fn validated_calendar_draft(
    raw_title: &str,
    parsed_title: Option<&str>,
    start_day: &str,
    start_time: &str,
    end_day: &str,
    end_time: &str,
) -> Result<(String, i64, i64), &'static str> {
    // Submission reads current controls, never parses the original natural query
    // again: this preserves an explicit date/time correction made by the user.
    let parse = |day: &str, time: &str| {
        glib::DateTime::from_iso8601(
            &format!("{}T{}:00", day.trim(), time.trim()),
            Some(&glib::TimeZone::local()),
        )
        .ok()
        .map(|d| d.to_unix() * 1000)
    };
    let start = parse(start_day, start_time).ok_or("Enter a valid start date and time.")?;
    let end = parse(end_day, end_time)
        .filter(|end| *end > start)
        .ok_or("The end must be after the start.")?;
    let title = parsed_title.unwrap_or(raw_title).trim();
    if title.is_empty() {
        return Err("Give this event a title.");
    }
    Ok((title.to_owned(), start, end))
}
#[derive(Clone)]
struct GridEvent {
    event: Value,
    start: f64,
    end: f64,
    lane: usize,
    lanes: usize,
}
fn arrange_events(mut events: Vec<GridEvent>) -> Vec<GridEvent> {
    events.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));
    let mut group = 0;
    while group < events.len() {
        let mut stop = group + 1;
        let mut finish = events[group].end.max(events[group].start + 30.);
        while stop < events.len() && events[stop].start < finish {
            finish = finish.max(events[stop].end.max(events[stop].start + 30.));
            stop += 1;
        }
        let mut lanes: Vec<f64> = Vec::new();
        for event in &mut events[group..stop] {
            let lane = lanes
                .iter()
                .position(|end| *end <= event.start)
                .unwrap_or(lanes.len());
            if lane == lanes.len() {
                lanes.push(0.);
            }
            lanes[lane] = event.end.max(event.start + 30.);
            event.lane = lane;
        }
        for event in &mut events[group..stop] {
            event.lanes = lanes.len();
        }
        group = stop;
    }
    events
}
fn source_key(account: &str, calendar: &str) -> String {
    format!("{account}\n{calendar}")
}
fn event_key(event: &Value) -> String {
    format!(
        "{}:{}",
        event["calendar_id"].as_str().unwrap_or(""),
        event["id"].as_str().unwrap_or("")
    )
}
pub(super) fn leave(ui: &Rc<Ui>) {
    ui.workspace.set_visible_child_name("mail");
    ui.goto_pending.set(false);
    ui.compact_navigation.set(false);
    ui.refresh_navigation(&ui.accounts());
    ui.list.grab_focus();
}
impl Agenda {
    fn render_mini(self: &Rc<Self>) {
        while let Some(child) = self.mini_calendar.first_child() {
            self.mini_calendar.remove(&child)
        }
        let day = self.date.borrow().clone();
        let month = glib::DateTime::from_local(day.year(), day.month(), 1, 0, 0, 0.).unwrap();
        let top = gtk::Box::new(Orientation::Horizontal, 2);
        let previous = gtk::Button::with_label("‹");
        let title = gtk::Label::new(Some(&month.format("%B %Y").unwrap()));
        title.set_hexpand(true);
        title.add_css_class("heading");
        let next = gtk::Button::with_label("›");
        top.append(&previous);
        top.append(&title);
        top.append(&next);
        self.mini_calendar.append(&top);
        for (button, offset) in [(previous, -1), (next, 1)] {
            let this = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    let target = this.date.borrow().add_months(offset).ok();
                    if let Some(target) = target {
                        *this.date.borrow_mut() = target;
                        this.reload();
                    }
                }
            });
        }
        let grid = gtk::Grid::new();
        grid.set_column_homogeneous(true);
        grid.set_row_homogeneous(true);
        for (index, name) in ["M", "T", "W", "T", "F", "S", "S"].iter().enumerate() {
            let label = gtk::Label::new(Some(name));
            label.add_css_class("agenda-time");
            grid.attach(&label, index as i32, 0, 1, 1)
        }
        let first = month.add_days(1 - month.day_of_week()).unwrap();
        for index in 0..42 {
            let date = first.add_days(index).unwrap();
            let button = gtk::Button::with_label(&date.day_of_month().to_string());
            button.add_css_class("calendar-mini-day");
            if date.month() != month.month() {
                button.add_css_class("calendar-other-month")
            }
            if date.day_of_month() == day.day_of_month() && date.month() == day.month() {
                button.add_css_class("calendar-mini-selected")
            }
            button.set_tooltip_text(Some(&date.format("%A, %-d %B %Y").unwrap()));
            let this = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    *this.date.borrow_mut() = date.clone();
                    this.reload();
                }
            });
            grid.attach(&button, index % 7, index / 7 + 1, 1, 1)
        }
        self.mini_calendar.append(&grid);
    }
    fn set_busy(&self, value: bool) {
        self.busy.set(value);
        self.account.set_sensitive(!value);
    }

    fn id(&self) -> Option<String> {
        self.accounts
            .get((self.account.selected() as usize).wrapping_sub(1))
            .map(|a| a.id.clone())
    }
    fn first_day(&self) -> glib::DateTime {
        let day = self.date.borrow();
        match self.mode.get() {
            CalendarMode::Day => day.clone(),
            CalendarMode::Week => day.add_days(1 - day.day_of_week()).unwrap(),
            CalendarMode::Month => {
                glib::DateTime::from_local(day.year(), day.month(), 1, 0, 0, 0.).unwrap()
            }
            CalendarMode::Year => glib::DateTime::from_local(day.year(), 1, 1, 0, 0, 0.).unwrap(),
        }
    }
    fn range(&self) -> (i64, i64) {
        let start = self.first_day();
        let end = match self.mode.get() {
            CalendarMode::Day => start.add_days(1),
            CalendarMode::Week => start.add_days(7),
            CalendarMode::Month => start.add_months(1),
            CalendarMode::Year => start.add_years(1),
        }
        .unwrap();
        (start.to_unix() * 1000, end.to_unix() * 1000)
    }
    fn set_mode(self: &Rc<Self>, mode: CalendarMode) {
        if self.mode.replace(mode) != mode {
            self.mode_buttons[mode.index()].set_active(true);
            self.mode_menu.set_selected(mode.index() as u32);
        }
        self.reload();
    }
    fn zoom_hours(self: &Rc<Self>, direction: i32) {
        let current = self.hour_height.get();
        let next = if direction > 0 {
            if current < 58 {
                58
            } else {
                76
            }
        } else if current > 58 {
            58
        } else {
            44
        };
        if current != next {
            let adjustment = self.scroll.vadjustment();
            let offset = adjustment.value() / current as f64;
            self.hour_height.set(next);
            let events = self.all_events.borrow().clone();
            self.clear();
            self.add_events(events);
            // GTK applies the rebuilt grid allocation on the next frame. Restore
            // against that new scroll extent instead of clamping to the old one.
            let adjustment = adjustment.clone();
            let frames = Rc::new(Cell::new(0));
            self.scroll.add_tick_callback(move |_, _| {
                frames.set(frames.get() + 1);
                if frames.get() < 2 {
                    return glib::ControlFlow::Continue;
                }
                adjustment.set_value(offset * next as f64);
                glib::ControlFlow::Break
            });
        }
    }
    fn go_today(self: &Rc<Self>) {
        let now = glib::DateTime::now_local().unwrap();
        *self.date.borrow_mut() =
            glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 0, 0, 0.)
                .unwrap();
        self.reload();
    }
    fn go_day(self: &Rc<Self>, day: glib::DateTime) {
        *self.date.borrow_mut() = day;
        self.set_mode(CalendarMode::Day);
    }
    fn apply_date(self: &Rc<Self>) {
        let value = self.date_entry.text();
        let now = glib::DateTime::now_local().unwrap();
        let today =
            glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 0, 0, 0.)
                .unwrap();
        match parse_calendar_date(
            if value.trim().is_empty() {
                "today"
            } else {
                value.as_str()
            },
            &today,
        ) {
            Some(day) => {
                *self.date.borrow_mut() = day;
                self.date_popup.popdown();
                self.reload();
            }
            None => self
                .status
                .set_text("Try 20 October, next Friday, in 2 weeks, or 2026-11-20."),
        }
    }

    fn show_back(&self) {
        self.back.set_visible(true);
        self.legend_scroll.set_visible(false);
        self.footer.set_text(
            "J / K  Previous / next event    Tab  Next field    Esc  Calendar    G I  Inbox",
        );
    }
    fn clear(&self) {
        self.draft_canvases.borrow_mut().clear();
        self.rows.set_spacing(0);
        self.event_controls.borrow_mut().clear();
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child)
        }
    }
    fn request(
        self: &Rc<Self>,
        request: Value,
        done: impl FnOnce(Rc<Self>, Result<Value, String>) + 'static,
    ) {
        let weak = Rc::downgrade(self);
        super::workflows::request_async(self.ui.database_path.clone(), request, move |result| {
            if let Some(this) = weak.upgrade() {
                if this.window.is_visible() {
                    done(this, result)
                }
            }
        });
    }
    fn move_day(self: &Rc<Self>, offset: i32) {
        let first = self.first_day();
        let next = match self.mode.get() {
            CalendarMode::Day => first.add_days(offset),
            CalendarMode::Week => first.add_days(offset * 7),
            CalendarMode::Month => first.add_months(offset),
            CalendarMode::Year => first.add_years(offset),
        }
        .unwrap();
        *self.date.borrow_mut() = next;
        self.reload();
    }
    fn reload(self: &Rc<Self>) {
        let id = self.id();
        let (start, end) = self.range();
        let serial = self.generation.get() + 1;
        self.generation.set(serial);
        if let Some(popup) = self.detail_popup.borrow_mut().take() {
            popup.popdown();
        }
        self.back.set_visible(false);
        self.legend_scroll.set_visible(true);
        self.footer.set_text(CALENDAR_KEYS);
        let day = self.first_day();
        let caption = match self.mode.get() {
            CalendarMode::Day => day.format("%e %B %Y").unwrap().to_string(),
            CalendarMode::Week => format!(
                "{} – {}",
                day.format("%e %b").unwrap(),
                day.add_days(6).unwrap().format("%e %b %Y").unwrap()
            ),
            CalendarMode::Month => day.format("%B %Y").unwrap().to_string(),
            CalendarMode::Year => day.format("%Y").unwrap().to_string(),
        };
        self.footer.set_text(&format!(
            "H / L or ← →  Previous / next {}    T  Today    B  Date    ?  All shortcuts",
            self.mode.get().name()
        ));
        self.date_label.set_text(&caption);
        self.updating_mini.set(true);
        self.render_mini();
        self.updating_mini.set(false);
        drop(day);
        let hidden_sources: Vec<Value> = self
            .hidden_calendars
            .borrow()
            .iter()
            .filter_map(|key| {
                key.split_once('\n')
                    .map(|(account, calendar)| json!({"account_id":account,"calendar_id":calendar}))
            })
            .collect();
        let deduplicate_shared = id.is_none();
        let offset = self.scroll.vadjustment().value();
        self.request(
            json!({"op":"list_agenda","account_id":id,"start_ms":start,"end_ms":end,"limit":10000,"deduplicate_shared":deduplicate_shared,"hidden_sources":hidden_sources}),
            move |this, result| {
                if this.generation.get() != serial {
                    return;
                };
                match result {
                    Err(error) => this.status.set_text(&error),
                    Ok(events) => {
                        this.request(
                            json!({"op":"list_calendars","account_id":id}),
                            move |this, result| {
                                if this.generation.get() != serial {
                                    return;
                                };
                                match result {
                                    Err(error) => this.status.set_text(&error),
                                    Ok(calendars) => {
                                        *this.calendars.borrow_mut() =
                                            calendars.as_array().cloned().unwrap_or_default();
                                        this.clear();
                                        let events = events.as_array().cloned().unwrap_or_default();
                                        this.update_legend();
                                        this.add_events(events);
                                        let adjustment=this.scroll.vadjustment();let frame=Rc::new(Cell::new(0));
                                        this.scroll.add_tick_callback(move|_,_|{frame.set(frame.get()+1);if frame.get()<2{return glib::ControlFlow::Continue;}adjustment.set_value(offset);glib::ControlFlow::Break});
                                        let ids =
                                            this.id().map(|id| vec![id]).unwrap_or_else(|| {
                                                this.accounts
                                                    .iter()
                                                    .map(|account| account.id.clone())
                                                    .collect()
                                            });
                                        this.load_connection_states(serial, ids, Vec::new());
                                    }
                                }
                            },
                        );
                    }
                }
            },
        );
    }
    fn load_connection_states(
        self: &Rc<Self>,
        serial: u64,
        mut remaining: Vec<String>,
        mut connected: Vec<String>,
    ) {
        if self.generation.get() != serial {
            return;
        }
        if remaining.is_empty() {
            let any = !connected.is_empty();
            *self.connected_accounts.borrow_mut() = connected;
            self.connect.set_visible(!any);
            self.connect
                .set_sensitive(self.id().is_some() && !self.busy.get());
            self.refresh.set_sensitive(any && !self.busy.get());
            self.status.set_text(if any {
                "Saved for offline access · Checking connected calendars"
            } else if self.id().is_none() {
                "Choose an account to connect its calendar. Saved calendars stay available."
            } else {
                "Connect this account’s Google Calendar to sync events."
            });
            if any {
                self.maybe_sync();
            }
            return;
        }
        let id = remaining.remove(0);
        self.request(
            json!({"op":"calendar_status","account_id":id}),
            move |this, result| {
                if let Ok(state) = result {
                    if state["connected"].as_bool() == Some(true) {
                        connected.push(id);
                    }
                }
                this.load_connection_states(serial, remaining, connected);
            },
        );
    }
    fn add_events(self: &Rc<Self>, mut events: Vec<Value>) {
        *self.all_events.borrow_mut() = events.clone();
        events.retain(|event| {
            !self.hidden_calendars.borrow().contains(&source_key(
                event["account_id"].as_str().unwrap_or(""),
                event["calendar_id"].as_str().unwrap_or(""),
            ))
        });
        events.retain(|event| event["status"].as_str() != Some("cancelled"));
        events.sort_by_key(|event| {
            (
                event["start_ms"].as_i64().unwrap_or(0),
                event["id"].as_str().unwrap_or("").to_owned(),
            )
        });
        let selected_id = self.events.borrow().get(self.selected.get()).map(event_key);
        self.selected.set(
            selected_id
                .as_ref()
                .and_then(|id| events.iter().position(|event| event_key(event) == *id))
                .unwrap_or_else(|| self.selected.get().min(events.len().saturating_sub(1))),
        );
        *self.events.borrow_mut() = events.clone();
        if matches!(self.mode.get(), CalendarMode::Month | CalendarMode::Year) {
            self.render_overview(&events);
            return;
        }
        let first = self.first_day();
        let days = if self.mode.get() == CalendarMode::Week {
            7
        } else {
            1
        };
        let mut columns: Vec<(glib::DateTime, Vec<Value>, Vec<GridEvent>)> = Vec::new();
        let mut first_hour = 6;
        for offset in 0..days {
            let day = first.add_days(offset).unwrap();
            let day_end = day.add_days(1).unwrap();
            let lower = day.to_unix() * 1000;
            let upper = day_end.to_unix() * 1000;
            let calendar_day =
                glib::DateTime::from_utc(day.year(), day.month(), day.day_of_month(), 0, 0, 0.)
                    .unwrap()
                    .to_unix()
                    * 1000;
            let mut all_day = Vec::new();
            let mut timed = Vec::new();
            for event in &events {
                if event["status"].as_str() == Some("cancelled") {
                    continue;
                }
                let start = event["start_ms"].as_i64().unwrap_or(0);
                let end = event["end_ms"].as_i64().unwrap_or(start + 60_000);
                if event["all_day"].as_bool() == Some(true) {
                    if start < calendar_day + 86_400_000 && end > calendar_day {
                        all_day.push(event.clone());
                    }
                } else if start < upper && end > lower {
                    let minute = |ms| {
                        glib::DateTime::from_unix_local(ms / 1000)
                            .ok()
                            .map(|d| (d.hour() * 60 + d.minute()) as f64)
                            .unwrap_or(0.)
                    };
                    let begin = if start <= lower { 0. } else { minute(start) };
                    let finish = if end >= upper { 1440. } else { minute(end) };
                    first_hour = first_hour.min((begin / 60.).floor() as i32);
                    timed.push(GridEvent {
                        event: event.clone(),
                        start: begin,
                        end: finish.max(begin + 1.),
                        lane: 0,
                        lanes: 1,
                    });
                }
            }
            columns.push((day, all_day, arrange_events(timed)));
        }
        let all_day_height = (columns
            .iter()
            .map(|(_, all, _)| all.len())
            .max()
            .unwrap_or(0)
            .max(1)
            * 28
            + 8) as i32;
        let header_height = 48;
        let hour_height = self.hour_height.get();
        let pixels_per_minute = hour_height as f64 / 60.;
        let hours_height = (24 - first_hour) * hour_height;
        let outer = gtk::Box::new(Orientation::Horizontal, 0);
        outer.set_hexpand(true);
        let gutter = gtk::Box::new(Orientation::Vertical, 0);
        gutter.set_size_request(58, -1);
        let zone = text("", false);
        zone.set_size_request(58, header_height);
        gutter.append(&zone);
        let all_day_caption = text("All day", false);
        all_day_caption.add_css_class("calendar-hour");
        all_day_caption.set_size_request(58, all_day_height);
        all_day_caption.set_margin_start(8);
        all_day_caption.set_valign(gtk::Align::Start);
        gutter.append(&all_day_caption);
        let hour_axis = gtk::Fixed::new();
        hour_axis.set_size_request(58, hours_height);
        for hour in first_hour..24 {
            let caption = text(&format!("{hour:02}:00"), false);
            caption.set_selectable(false);
            caption.add_css_class("calendar-hour");
            caption.set_size_request(-1, 22);
            caption.set_yalign(0.5);
            hour_axis.put(&caption, 8., ((hour - first_hour) * hour_height) as f64);
        }
        gutter.append(&hour_axis);
        outer.append(&gutter);
        let grid = gtk::Grid::new();
        grid.set_column_homogeneous(true);
        grid.set_hexpand(true);
        let now = glib::DateTime::now_local().unwrap();
        let today = now.format("%Y-%m-%d").unwrap();
        for (index, (day, all_day, blocks)) in columns.into_iter().enumerate() {
            let column = gtk::Box::new(Orientation::Vertical, 0);
            column.set_hexpand(true);
            // Let homogeneous day columns share the available viewport, including
            // when the sources rail is open. Natural canvas widths must not add up
            // to a wider-than-visible week.
            column.set_size_request(32, -1);
            column.add_css_class("calendar-column");
            let caption = gtk::Box::new(Orientation::Vertical, 1);
            caption.set_halign(gtk::Align::Fill);
            caption.set_valign(gtk::Align::Center);
            caption.add_css_class("calendar-day-header");
            let weekday = text(&day.format("%a").unwrap().to_uppercase(), false);
            weekday.set_xalign(0.5);
            weekday.add_css_class("calendar-weekday");
            let number = text(&day.day_of_month().to_string(), true);
            number.set_xalign(0.5);
            number.add_css_class("calendar-day-number");
            caption.append(&weekday);
            caption.append(&number);
            if day.format("%Y-%m-%d").unwrap() == today {
                caption.add_css_class("calendar-today");
            }
            caption.set_size_request(-1, header_height);
            column.append(&caption);
            let banner = gtk::Box::new(Orientation::Vertical, 3);
            banner.add_css_class("calendar-all-day");
            banner.set_size_request(-1, all_day_height);
            for event in all_day {
                let button = self.event_button(&event, false, 1);
                button.set_size_request(-1, 24);
                banner.append(&button);
            }
            column.append(&banner);
            let overlay = gtk::Overlay::new();
            let canvas = gtk::DrawingArea::new();
            canvas.set_content_height(hours_height);
            canvas.set_content_width(1);
            canvas.set_hexpand(true);
            let live_draft = self.live_draft.clone();
            let draft_day = day.clone();
            self.draft_canvases.borrow_mut().push(canvas.clone());
            canvas.set_draw_func(move |_, cr, width, height| {
                cr.set_source_rgba(0.5, 0.5, 0.5, 0.14);
                cr.set_line_width(1.);
                let mut y = 0.5;
                while y < height as f64 {
                    cr.move_to(0., y);
                    cr.line_to(width as f64, y);
                    y += hour_height as f64;
                }
                let _ = cr.stroke();
                if let Some((title, start, end)) = live_draft.borrow().as_ref() {
                    let lower = draft_day.to_unix() * 1000;
                    let upper = draft_day.add_days(1).unwrap().to_unix() * 1000;
                    if *start < upper && *end > lower {
                        let minute = |ms: i64| {
                            glib::DateTime::from_unix_local(ms / 1000)
                                .map(|date| (date.hour() * 60 + date.minute()) as f64)
                                .unwrap_or(0.)
                        };
                        let from = if *start <= lower { 0. } else { minute(*start) };
                        let to = if *end >= upper { 1440. } else { minute(*end) };
                        let y = (from - first_hour as f64 * 60.) * pixels_per_minute;
                        let h = ((to - from) * pixels_per_minute).max(20.);
                        let _ = cr.save();
                        cr.rectangle(4., y, (width as f64 - 8.).max(1.), h);
                        cr.set_source_rgba(0.50, 0.64, 0.80, 0.12);
                        let _ = cr.fill_preserve();
                        cr.set_source_rgba(0.50, 0.64, 0.80, 0.85);
                        cr.set_line_width(1.5);
                        cr.set_dash(&[4., 3.], 0.);
                        let _ = cr.stroke();
                        cr.set_dash(&[], 0.);
                        cr.set_font_size(11.);
                        cr.move_to(10., y + 14.);
                        let title = title.chars().take(35).collect::<String>();
                        let _ = cr.show_text(&format!("Draft · {title}"));
                        let _ = cr.restore();
                    }
                }
            });
            let slots = gtk::GestureClick::new();
            slots.set_button(1);
            {
                let this = self.clone();
                let day = day.clone();
                let overlay_weak = overlay.downgrade();
                slots.connect_released(move |_, _, x, y| {
                    if let Some(overlay) = overlay_weak.upgrade() {
                        let mut picked = overlay.pick(x, y, gtk::PickFlags::DEFAULT);
                        while let Some(widget) = picked {
                            if widget.is::<gtk::Button>() {
                                return;
                            }
                            if widget == overlay.clone().upcast::<gtk::Widget>() {
                                break;
                            }
                            picked = widget.parent();
                        }
                    }
                    let minute = ((y / pixels_per_minute / 15.).floor() as i32 * 15
                        + first_hour * 60)
                        .clamp(0, 23 * 60 + 45);
                    if let Ok(at) = day.add_minutes(minute) {
                        this.new_event_at(Some(at), None);
                    }
                });
            }
            overlay.add_controller(slots);
            canvas.set_tooltip_text(Some("Click an empty time slot to create an event"));
            overlay.set_child(Some(&canvas));
            let fixed = gtk::Fixed::new();
            overlay.add_overlay(&fixed);
            let mut positions = Vec::new();
            for segment in blocks {
                let y = (segment.start - first_hour as f64 * 60.) * pixels_per_minute;
                let height = ((segment.end - segment.start) * pixels_per_minute)
                    .max(24.)
                    .min(hours_height as f64 - y)
                    .max(1.) as i32;
                let button =
                    self.event_button(&segment.event, true, if height < 48 { 1 } else { 3 });
                button.set_size_request(32, height);
                fixed.put(&button, 3., y);
                positions.push((button, segment.lane, segment.lanes, y, height));
            }
            canvas.connect_resize(move |_, width, _| {
                for (button, lane, count, y, height) in &positions {
                    let lane_width = ((width - 6).max(1) as f64 / *count as f64).max(1.);
                    button.set_size_request((lane_width - 3.).max(1.) as i32, *height);
                    fixed.move_(button, 3. + *lane as f64 * lane_width, *y);
                }
            });
            column.append(&overlay);
            grid.attach(&column, index as i32, 0, 1, 1);
        }
        outer.append(&grid);
        self.rows.append(&outer);
        self.highlight_selection(false);
    }
    fn event_button(self: &Rc<Self>, event: &Value, timed: bool, lines: i32) -> gtk::Button {
        let button = gtk::Button::new();
        button.add_css_class("calendar-block");
        button.add_css_class(&format!(
            "calendar-tone-{}",
            calendar_tone(event["calendar_id"].as_str().unwrap_or(""))
        ));
        let title = event["title"].as_str().unwrap_or("Untitled event");
        let content = gtk::Box::new(Orientation::Vertical, 2);
        content.set_valign(gtk::Align::Start);
        let label = gtk::Label::new(Some(title));
        label.set_xalign(0.);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_width_chars(1);
        label.set_max_width_chars(1);
        content.append(&label);
        if timed && lines > 1 {
            let time = gtk::Label::new(Some(&format!(
                "{}–{}",
                date(event["start_ms"].as_i64().unwrap_or(0)),
                date(event["end_ms"].as_i64().unwrap_or(0))
            )));
            time.set_xalign(0.);
            time.set_wrap(false);
            time.set_ellipsize(gtk::pango::EllipsizeMode::End);
            time.set_width_chars(1);
            time.set_max_width_chars(1);
            time.add_css_class("agenda-time");
            time.set_size_request(-1, 18);
            time.set_yalign(0.5);
            content.append(&time);
        }
        button.set_child(Some(&content));
        let all_day = event["all_day"].as_bool() == Some(true);
        let describe = |ms| {
            let day = if all_day {
                glib::DateTime::from_unix_utc(ms / 1000)
            } else {
                glib::DateTime::from_unix_local(ms / 1000)
            };
            day.ok()
                .and_then(|d| {
                    d.format(if all_day {
                        "%e %b %Y · All day"
                    } else {
                        "%e %b %Y %H:%M %Z"
                    })
                    .ok()
                })
                .map(|s| s.to_string())
                .unwrap_or_default()
        };
        let start = event["start_ms"].as_i64().unwrap_or(0);
        let end = event["end_ms"].as_i64().unwrap_or(start);
        let description = format!(
            "{title} · {} – {} · {} · Open event details",
            describe(start),
            describe(if all_day {
                (end - 86_400_000).max(start)
            } else {
                end
            }),
            event["location"].as_str().unwrap_or("")
        );
        button.set_tooltip_text(Some(&description));
        button.update_property(&[gtk::accessible::Property::Label(&description)]);
        self.event_controls
            .borrow_mut()
            .push((event_key(event), button.clone()));
        let this = self.clone();
        let event = event.clone();
        button.connect_clicked(move |_| {
            let index = this
                .events
                .borrow()
                .iter()
                .position(|item| event_key(item) == event_key(&event))
                .unwrap_or(0);
            this.selected.set(index);
            this.show_event(&event);
        });
        button
    }
    fn show_event(self: &Rc<Self>, event: &Value) {
        if let Some(old) = self.detail_popup.borrow_mut().take() {
            old.popdown();
        }
        let anchor = self
            .event_controls
            .borrow()
            .iter()
            .find(|(key, _)| *key == event_key(event))
            .map(|(_, button)| button.clone());
        let Some(anchor) = anchor else { return };
        let popup = gtk::Popover::new();
        popup.set_autohide(true);
        popup.set_has_arrow(false);
        popup.set_parent(&anchor);
        let details = gtk::Box::new(Orientation::Vertical, 12);
        details.set_margin_top(20);
        details.set_margin_bottom(20);
        details.set_margin_start(20);
        details.set_margin_end(20);
        details.set_size_request(300, -1);
        let close = gtk::Button::with_label("Close");
        close.add_css_class("flat");
        close.set_halign(gtk::Align::End);
        {
            let popup = popup.clone();
            close.connect_clicked(move |_| popup.popdown());
        }
        details.append(&close);
        let title = text(event["title"].as_str().unwrap_or("Untitled event"), true);
        title.add_css_class("agenda-heading");
        details.append(&title);
        let start = event["start_ms"].as_i64().unwrap_or(0);
        let end = event["end_ms"].as_i64().unwrap_or(0);
        let all_day = event["all_day"].as_bool() == Some(true);
        let format = |ms| {
            let value = if all_day {
                glib::DateTime::from_unix_utc(ms / 1000)
            } else {
                glib::DateTime::from_unix_local(ms / 1000)
            };
            value
                .ok()
                .and_then(|d| {
                    d.format(if all_day {
                        "%A, %e %B %Y"
                    } else {
                        "%A, %e %B %Y · %H:%M %Z"
                    })
                    .ok()
                })
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Time unavailable".into())
        };
        details.append(&text(
            &if all_day {
                if end - start <= 86_400_000 {
                    format!("All day · {}", format(start))
                } else {
                    format!("All day\n{} – {}", format(start), format(end - 86_400_000))
                }
            } else {
                format!("{}\n{}", format(start), format(end))
            },
            false,
        ));
        for (key, caption) in [
            ("location", "Location"),
            ("organizer_email", "Organizer"),
            ("description", "Description"),
        ] {
            if let Some(value) = event[key].as_str().filter(|v| !v.is_empty()) {
                details.append(&text(caption, true));
                details.append(&text(value, false));
            }
        }
        let writable = self.calendars.borrow().iter().any(|c| {
            c["account_id"] == event["account_id"]
                && c["id"] == event["calendar_id"]
                && c["can_write"].as_bool() == Some(true)
        });
        if writable
            && event["can_edit"].as_bool() == Some(true)
            && event["etag"].as_str().is_some_and(|v| !v.is_empty())
        {
            let edit = gtk::Button::with_label("Edit event");
            edit.add_css_class("agenda-new");
            let this = self.clone();
            let event = event.clone();
            let edit_popup = popup.clone();
            edit.connect_clicked(move |_| {
                edit_popup.popdown();
                this.new_event_at(None, Some(event.clone()));
            });
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            let edit_key = edit.clone();
            keys.connect_key_pressed(move |_, key, _, mods| {
                if matches!(key, gdk::Key::e | gdk::Key::E)
                    && !mods.intersects(
                        gdk::ModifierType::CONTROL_MASK
                            | gdk::ModifierType::ALT_MASK
                            | gdk::ModifierType::SUPER_MASK,
                    )
                {
                    edit_key.emit_clicked();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            popup.add_controller(keys);
            details.append(&edit);
        } else {
            let note = text(
                if event["all_day"].as_bool() == Some(true) {
                    "All-day event · Editing is not available yet"
                } else if writable {
                    "Refresh this calendar to check editing access"
                } else {
                    "Read-only calendar"
                },
                false,
            );
            note.add_css_class("agenda-time");
            details.append(&note);
        }
        if let Some(uri) = event["html_url"]
            .as_str()
            .and_then(|value| Url::parse(value).ok())
            .filter(|url| {
                url.scheme() == "https"
                    && (url.host_str() == Some("calendar.google.com")
                        || (url.host_str() == Some("www.google.com")
                            && url.path().starts_with("/calendar/")))
            })
        {
            let link = gtk::LinkButton::with_label(uri.as_str(), "Open in Google Calendar");
            link.set_halign(gtk::Align::Start);
            details.append(&link);
        }
        if writable && event["can_respond"].as_bool() == Some(true) {
            let actions = gtk::Box::new(Orientation::Horizontal, 8);
            for (caption, response) in [
                ("Accept", "accepted"),
                ("Maybe", "tentative"),
                ("Decline", "declined"),
            ] {
                let button = gtk::Button::with_label(caption);
                let this = self.clone();
                let event = event.clone();
                button.connect_clicked(move |_| this.respond(&event, response));
                actions.append(&button);
            }
            details.append(&actions);
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_max_content_height(440);
        scroll.set_propagate_natural_height(true);
        scroll.set_child(Some(&details));
        popup.set_child(Some(&scroll));
        popup.connect_closed(|popup| popup.unparent());
        *self.detail_popup.borrow_mut() = Some(popup.clone());
        popup.popup();
    }
    fn highlight_selection(self: &Rc<Self>, scroll_to: bool) {
        let selected = self.events.borrow().get(self.selected.get()).map(event_key);
        let mut target = None;
        for (_, button) in self.event_controls.borrow().iter() {
            button.remove_css_class("calendar-selected");
        }
        for (key, button) in self.event_controls.borrow().iter() {
            if selected.as_ref() == Some(key) {
                button.add_css_class("calendar-selected");
                if target.is_none() {
                    target = Some(button.clone());
                }
            }
        }
        if scroll_to {
            if let Some(button) = target {
                let this = Rc::downgrade(self);
                glib::idle_add_local_once(move || {
                    if let Some(this) = this.upgrade() {
                        if let Some(bounds) = button.compute_bounds(&this.rows) {
                            let vertical = this.scroll.vadjustment();
                            let y = bounds.y() as f64;
                            let bottom = y + bounds.height() as f64;
                            let next = if y < vertical.value() {
                                y
                            } else if bottom > vertical.value() + vertical.page_size() {
                                bottom - vertical.page_size()
                            } else {
                                vertical.value()
                            };
                            vertical.set_value(next.clamp(
                                vertical.lower(),
                                (vertical.upper() - vertical.page_size()).max(vertical.lower()),
                            ));
                            let horizontal = this.scroll.hadjustment();
                            let x = bounds.x() as f64;
                            let right = x + bounds.width() as f64;
                            let next = if x < horizontal.value() {
                                x
                            } else if right > horizontal.value() + horizontal.page_size() {
                                right - horizontal.page_size()
                            } else {
                                horizontal.value()
                            };
                            horizontal.set_value(
                                next.clamp(
                                    horizontal.lower(),
                                    (horizontal.upper() - horizontal.page_size())
                                        .max(horizontal.lower()),
                                ),
                            );
                        }
                    }
                });
            }
        }
    }
    fn select_event(self: &Rc<Self>, offset: i32) {
        let count = self.events.borrow().len();
        if count == 0 {
            return;
        }
        let next = (self.selected.get() as i32 + offset).clamp(0, count as i32 - 1) as usize;
        self.selected.set(next);
        if let Some(event) = self.events.borrow().get(next) {
            self.footer.set_text(&format!(
                "{} / {} · {}    ↵ Open    J / K Previous / next    ? Shortcuts",
                next + 1,
                count,
                event["title"].as_str().unwrap_or("Untitled event")
            ));
        }
        if self.back.is_visible() {
            let event = self.events.borrow()[next].clone();
            self.show_event(&event);
        } else {
            self.highlight_selection(true);
        }
    }
    pub(super) fn handle_key(
        self: &Rc<Self>,
        key: gdk::Key,
        modifiers: gdk::ModifierType,
    ) -> glib::Propagation {
        if self
            .editor_popup
            .borrow()
            .as_ref()
            .is_some_and(|popup| popup.is_visible())
        {
            if key == gdk::Key::Escape {
                if let Some(popup) = self.editor_popup.borrow_mut().take() {
                    popup.popdown();
                }
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        if self
            .detail_popup
            .borrow()
            .as_ref()
            .is_some_and(|popup| popup.is_visible())
        {
            if key == gdk::Key::Escape {
                if let Some(popup) = self.detail_popup.borrow_mut().take() {
                    popup.popdown();
                }
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::e {
                let event = self.events.borrow().get(self.selected.get()).cloned();
                if let Some(event) = event.filter(|event| {
                    event["can_edit"].as_bool() == Some(true)
                        && event["etag"].as_str().is_some_and(|v| !v.is_empty())
                        && self.calendars.borrow().iter().any(|c| {
                            c["id"] == event["calendar_id"]
                                && c["account_id"] == event["account_id"]
                                && c["can_write"].as_bool() == Some(true)
                        })
                }) {
                    if let Some(popup) = self.detail_popup.borrow_mut().take() {
                        popup.popdown();
                    }
                    self.new_event_at(None, Some(event));
                    return glib::Propagation::Stop;
                }
            }
            return glib::Propagation::Proceed;
        }
        if self.date_popup.is_visible()
            || self.help_popup.is_visible()
            || self.source_popup.is_visible()
        {
            if key == gdk::Key::Escape {
                self.date_popup.popdown();
                self.help_popup.popdown();
                self.source_popup.popdown();
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        if key == gdk::Key::Escape {
            self.ui.goto_pending.set(false);
            if self.back.is_visible() {
                self.reload();
            } else {
                leave(&self.ui);
            }
            return glib::Propagation::Stop;
        }
        let editing =
            gtk::prelude::GtkWindowExt::focus(&self.window).is_some_and(|mut widget| loop {
                if widget.is::<gtk::Entry>()
                    || widget.is::<gtk::TextView>()
                    || widget.is::<gtk::SearchEntry>()
                {
                    break true;
                }
                if let Some(parent) = widget.parent() {
                    widget = parent;
                } else {
                    break false;
                }
            });
        if editing {
            return glib::Propagation::Proceed;
        }
        if modifiers.intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) {
            return glib::Propagation::Proceed;
        }
        let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        if ctrl && matches!(key, gdk::Key::d | gdk::Key::u) {
            let adjustment = self.scroll.vadjustment();
            let delta = adjustment.page_size() / 2. * if key == gdk::Key::u { -1. } else { 1. };
            adjustment.set_value((adjustment.value() + delta).clamp(
                adjustment.lower(),
                (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
            ));
            return glib::Propagation::Stop;
        }
        if ctrl {
            return glib::Propagation::Proceed;
        }
        if self.ui.goto_pending.replace(false) {
            if key == gdk::Key::m {
                self.ui.refresh_navigation(&self.ui.accounts());
                if let Some(button) = self.ui.views_box.first_child() {
                    button.grab_focus();
                }
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::c {
                self.ui.refresh_navigation(&self.ui.accounts());
                self.footer.set_text(CALENDAR_KEYS);
                return glib::Propagation::Stop;
            }
            let destination = match key {
                gdk::Key::i | gdk::Key::u | gdk::Key::f => Some(MailView::Inbox),
                gdk::Key::s => Some(MailView::Snoozed),
                gdk::Key::w => Some(MailView::Followups),
                gdk::Key::a => Some(MailView::Archive),
                gdk::Key::d => Some(MailView::Drafts),
                gdk::Key::o => Some(MailView::Outbox),
                _ => None,
            };
            if let Some(view) = destination {
                self.ui.search.set_text("");
                let filter = match key {
                    gdk::Key::u => 1,
                    gdk::Key::f => 2,
                    _ => 0,
                };
                self.ui.filter_buttons[filter].emit_clicked();
                self.ui.set_view(view);
                self.ui.list.grab_focus();
            } else {
                self.ui.refresh_navigation(&self.ui.accounts());
            }
            return glib::Propagation::Stop;
        }
        if self.back.is_visible() && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
            return glib::Propagation::Proceed;
        }
        match key {
            gdk::Key::g => {
                self.ui.goto_pending.set(true);
                self.ui.refresh_navigation(&self.ui.accounts());
                self.footer.set_text("G then I  Inbox · U  Unread · F  Favorites · A  Archive · S  Snoozed · D  Drafts · O  Outbox · C  Calendar");
            }
            gdk::Key::j | gdk::Key::J | gdk::Key::Down => self.select_event(1),
            gdk::Key::k | gdk::Key::K | gdk::Key::Up => self.select_event(-1),
            gdk::Key::Return | gdk::Key::KP_Enter => {
                let event = self.events.borrow().get(self.selected.get()).cloned();
                if let Some(event) = event {
                    self.show_event(&event);
                }
            }
            gdk::Key::d | gdk::Key::_1 => self.set_mode(CalendarMode::Day),
            gdk::Key::w | gdk::Key::_2 => self.set_mode(CalendarMode::Week),
            gdk::Key::m | gdk::Key::_3 => self.set_mode(CalendarMode::Month),
            gdk::Key::y | gdk::Key::_4 => self.set_mode(CalendarMode::Year),
            gdk::Key::t => self.go_today(),
            gdk::Key::plus | gdk::Key::equal => self.zoom_hours(1),
            gdk::Key::minus => self.zoom_hours(-1),
            gdk::Key::b => {
                self.date_entry.set_text("");
                self.date_popup.popup();
                self.date_entry.grab_focus();
            }
            gdk::Key::question => self.help_popup.popup(),
            gdk::Key::c => self.new_event(),
            gdk::Key::slash => self.ui.open_palette("calendar-search"),
            gdk::Key::h | gdk::Key::Left => self.move_day(-1),
            gdk::Key::l | gdk::Key::Right => self.move_day(1),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }
    pub(super) fn command_items(&self, search: bool) -> Vec<parity::Command> {
        let mut commands = vec![parity::Command::new("bookings", "Bookings and meeting links", "", "scheduling analytics")];
        if !search {
            for (id, title, keys, words) in [
                (
                    "calendar:search",
                    "Search calendar",
                    "/",
                    "find event meeting title",
                ),
                (
                    "calendar:open",
                    "Open selected event",
                    "Enter",
                    "details meeting",
                ),
                (
                    "calendar:new",
                    "New event…",
                    "C",
                    "create meeting invitation",
                ),
                ("calendar:refresh", "Refresh calendar", "", "sync reload"),
                (
                    "calendar:availability",
                    "Find a time",
                    "",
                    "free available slots",
                ),
                ("calendar:grid", "Back to calendar", "Esc", "week day grid"),
                ("calendar:day", "Day view", "D / 1", "view"),
                ("calendar:week", "Week view", "W / 2", "view"),
                ("calendar:month", "Month view", "M / 3", "view"),
                ("calendar:year", "Year view", "Y / 4", "view"),
                ("calendar:today", "Go to today", "T", "current date"),
                (
                    "calendar:previous",
                    "Previous period",
                    "H / ←",
                    "last week month year",
                ),
                (
                    "calendar:next",
                    "Next period",
                    "L / →",
                    "following week month year",
                ),
                ("calendar:date", "Go to date…", "B", "jump date"),
                (
                    "calendar:help",
                    "Keyboard shortcuts",
                    "?",
                    "help navigation",
                ),
                ("inbox", "Go to Inbox", "G I", "mail email"),
                ("compose", "Compose email", "", "write compose email"),
            ] {
                commands.push(parity::Command::new(id, title, keys, words));
            }
        }
        for (index, event) in self.events.borrow().iter().enumerate() {
            let start = event["start_ms"].as_i64().unwrap_or(0);
            let detail = glib::DateTime::from_unix_local(start / 1000)
                .ok()
                .and_then(|d| d.format("%a %e %b · %H:%M %Z").ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            commands.push(
                parity::Command::new(
                    &format!("calendar:event:{index}"),
                    event["title"].as_str().unwrap_or("Untitled event"),
                    "",
                    "event meeting calendar",
                )
                .detail(detail),
            );
        }
        commands
    }
    pub(super) fn date_command(&self, query: &str) -> Option<parity::Command> {
        if query.trim().is_empty() {
            return None;
        }
        let now = glib::DateTime::now_local().ok()?;
        let day = parse_calendar_date(query, &now)?;
        Some(
            parity::Command::new(
                &format!("calendar:jump:{}", day.format("%Y-%m-%d").ok()?),
                &format!("Go to {}", day.format("%A, %-d %B %Y").ok()?),
                "↵",
                "",
            )
            .detail("Keep current calendar view · Enter to go"),
        )
    }
    pub(super) fn execute_command(self: &Rc<Self>, id: &str) {
        if let Some(value) = id.strip_prefix("calendar:jump:") {
            if let Ok(now) = glib::DateTime::now_local() {
                if let Some(day) = parse_calendar_date(value, &now) {
                    *self.date.borrow_mut() = day;
                    self.reload();
                }
            }
            return;
        }
        match id {
            "calendar:day" => self.set_mode(CalendarMode::Day),
            "calendar:week" => self.set_mode(CalendarMode::Week),
            "calendar:month" => self.set_mode(CalendarMode::Month),
            "calendar:year" => self.set_mode(CalendarMode::Year),
            "calendar:today" => self.go_today(),
            "calendar:previous" => self.move_day(-1),
            "calendar:next" => self.move_day(1),
            "calendar:date" => {
                self.date_entry.set_text("");
                self.date_popup.popup();
                self.date_entry.grab_focus();
            }
            "calendar:help" => self.help_popup.popup(),
            "calendar:search" => self.ui.open_palette("calendar-search"),
            "calendar:new" => self.new_event(),
            "calendar:refresh" => self.sync(),
            "calendar:availability" => self.availability(),
            "calendar:grid" => self.reload(),
            "calendar:open" => {
                let event = self.events.borrow().get(self.selected.get()).cloned();
                if let Some(event) = event {
                    self.show_event(&event);
                }
            }
            "inbox" => self.ui.set_view(MailView::Inbox),
            "compose" => self.ui.compose(false),
            _ => {
                if let Some(index) = id
                    .strip_prefix("calendar:event:")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    let event = self.events.borrow().get(index).cloned();
                    if let Some(event) = event {
                        self.selected.set(index);
                        self.show_event(&event);
                    }
                }
            }
        }
    }
    fn update_legend(self: &Rc<Self>) {
        while let Some(child) = self.legend.first_child() {
            self.legend.remove(&child);
        }
        let mut previous_account = String::new();
        let mut calendars = self.calendars.borrow().clone();
        calendars.sort_by_key(|calendar| {
            (
                calendar["account_id"].as_str().unwrap_or("").to_owned(),
                calendar["name"].as_str().unwrap_or("").to_owned(),
            )
        });
        for calendar in calendars.iter() {
            let owner = calendar["account_id"].as_str().unwrap_or("");
            if owner != previous_account {
                let name = self
                    .accounts
                    .iter()
                    .find(|account| account.id == owner)
                    .map(|account| account.email.as_str())
                    .unwrap_or(owner);
                let caption = text(name, false);
                caption.set_wrap(false);
                caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
                caption.set_max_width_chars(23);
                caption.add_css_class("calendar-rail-caption");
                self.legend.append(&caption);
                previous_account = owner.to_owned();
            }
            let id = calendar["id"].as_str().unwrap_or("").to_owned();
            let button =
                gtk::CheckButton::with_label(calendar["name"].as_str().unwrap_or("Calendar"));
            button.add_css_class(&format!("calendar-legend-{}", calendar_tone(&id)));
            if let Some(child) = button.child().and_downcast::<gtk::Label>() {
                child.set_ellipsize(gtk::pango::EllipsizeMode::End);
                child.set_max_width_chars(24);
            }
            let key = source_key(owner, &id);
            button.set_active(!self.hidden_calendars.borrow().contains(&key));
            button.set_tooltip_text(Some("Show or hide this calendar"));
            let this = self.clone();
            button.connect_toggled(move |button| {
                if button.is_active() {
                    this.hidden_calendars.borrow_mut().remove(&key);
                } else {
                    this.hidden_calendars.borrow_mut().insert(key.clone());
                }
                this.reload();
            });
            self.legend.append(&button);
        }
    }
    fn events_on<'a>(events: &[OverviewEntry<'a>], day: &glib::DateTime) -> Vec<&'a Value> {
        let lower = day.to_unix() * 1000;
        let upper = day.add_days(1).unwrap().to_unix() * 1000;
        let calendar_day =
            glib::DateTime::from_utc(day.year(), day.month(), day.day_of_month(), 0, 0, 0.)
                .unwrap()
                .to_unix()
                * 1000;
        events
            .iter()
            .filter_map(|entry| {
                let (begin, end) = if entry.all_day {
                    (calendar_day, calendar_day + 86_400_000)
                } else {
                    (lower, upper)
                };
                if entry.start < end && entry.end > begin {
                    Some(entry.event)
                } else {
                    None
                }
            })
            .collect()
    }
    fn month_widget(
        self: &Rc<Self>,
        month: glib::DateTime,
        events: &[Value],
        mini: bool,
    ) -> gtk::Box {
        let root = gtk::Box::new(Orientation::Vertical, 4);
        root.set_hexpand(true);
        root.set_vexpand(!mini);
        if mini {
            let title = gtk::Button::with_label(&month.format("%B").unwrap());
            title.add_css_class("calendar-month-name");
            let this = self.clone();
            let month = month.clone();
            title.connect_clicked(move |_| {
                *this.date.borrow_mut() = month.clone();
                this.set_mode(CalendarMode::Month);
            });
            root.append(&title);
        }
        let grid = gtk::Grid::new();
        grid.set_column_homogeneous(true);
        grid.set_row_homogeneous(false);
        grid.set_vexpand(!mini);
        for (index, name) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
            .iter()
            .enumerate()
        {
            let label = text(name, false);
            label.set_xalign(0.5);
            label.add_css_class("calendar-month-weekday");
            label.set_size_request(-1, 24);
            label.set_valign(gtk::Align::Center);
            label.set_margin_top(2);
            label.set_margin_bottom(2);
            grid.attach(&label, index as i32, 0, 1, 1);
        }
        let first = month.add_days(1 - month.day_of_week()).unwrap();
        // Normalize each candidate once, then use integer comparisons per day.
        // A one-day margin includes all-day dates in timezones either side of UTC.
        let lower = first.to_unix() * 1000 - 86_400_000;
        let upper = first.add_days(43).unwrap().to_unix() * 1000;
        let candidates: Vec<OverviewEntry<'_>> = events
            .iter()
            .filter_map(|event| {
                let start = event["start_ms"].as_i64().unwrap_or(0);
                let end = event["end_ms"].as_i64().unwrap_or(0);
                if start < upper && end > lower {
                    Some(OverviewEntry {
                        event,
                        start,
                        end,
                        all_day: event["all_day"].as_bool() == Some(true),
                    })
                } else {
                    None
                }
            })
            .collect();
        let now = glib::DateTime::now_local().unwrap();
        for offset in 0..42 {
            let day = first.add_days(offset).unwrap();
            let matches = Self::events_on(&candidates, &day);
            let cell = gtk::Box::new(Orientation::Vertical, 3);
            cell.add_css_class("calendar-month-cell");
            cell.set_vexpand(!mini);
            cell.set_size_request(if mini { 25 } else { 100 }, if mini { 24 } else { 68 });
            if day.month() != month.month() {
                cell.add_css_class("calendar-other-month");
            }
            let caption = if mini && !matches.is_empty() {
                format!("{} ·", day.day_of_month())
            } else {
                day.day_of_month().to_string()
            };
            let date_button = gtk::Button::with_label(&caption);
            date_button.add_css_class("calendar-date");
            if !mini {
                date_button.set_halign(gtk::Align::Start);
            }
            if day.year() == now.year() && day.day_of_year() == now.day_of_year() {
                date_button.add_css_class("calendar-today");
            }
            date_button.set_tooltip_text(Some(&format!(
                "{} · {} events · Open day",
                day.format("%A %e %B %Y").unwrap(),
                matches.len()
            )));
            {
                let this = self.clone();
                let day = day.clone();
                date_button.connect_clicked(move |_| this.go_day(day.clone()));
            }
            for event in &matches {
                self.event_controls
                    .borrow_mut()
                    .push((event_key(event), date_button.clone()));
            }
            if mini {
                if let Some(event) = matches.first() {
                    date_button.add_css_class(&format!(
                        "calendar-legend-{}",
                        calendar_tone(event["calendar_id"].as_str().unwrap_or(""))
                    ));
                }
            }
            cell.append(&date_button);
            if !mini {
                let capacity =
                    ((self.scroll.height().max(480) - 36) / 6 - 32).max(20) as usize / 24;
                for event in matches.iter().take(capacity.max(1)) {
                    let button = self.event_button(event, false, 1);
                    button.add_css_class("calendar-month-event");
                    cell.append(&button);
                }
                if matches.len() > capacity.max(1) {
                    let more = gtk::Button::with_label(&format!(
                        "+{} more",
                        matches.len() - capacity.max(1)
                    ));
                    more.add_css_class("calendar-month-more");
                    let this = self.clone();
                    let day = day.clone();
                    more.connect_clicked(move |_| this.go_day(day.clone()));
                    cell.append(&more);
                }
            }
            grid.attach(&cell, offset % 7, offset / 7 + 1, 1, 1);
        }
        root.append(&grid);
        root
    }
    fn render_overview(self: &Rc<Self>, events: &[Value]) {
        let first = self.first_day();
        if self.mode.get() == CalendarMode::Month {
            let widget = self.month_widget(first, events, false);
            self.rows.append(&widget);
            self.highlight_selection(false);
        } else {
            let flow = gtk::FlowBox::new();
            flow.set_selection_mode(gtk::SelectionMode::None);
            flow.set_min_children_per_line(1);
            flow.set_max_children_per_line(3);
            flow.set_column_spacing(18);
            flow.set_row_spacing(20);
            for month in 0..12 {
                let card = self.month_widget(first.add_months(month).unwrap(), events, true);
                card.set_size_request(210, -1);
                flow.insert(&card, -1);
            }
            self.rows.append(&flow);
            self.highlight_selection(false);
        }
    }
    fn connect(self: &Rc<Self>) {
        let Some(id) = self.id() else { return };
        if self.busy.replace(true) {
            return;
        };
        self.account.set_sensitive(false);
        self.connect.set_sensitive(false);
        self.status
            .set_text("Finish calendar permission approval in your browser…");
        self.request(
            json!({"op":"connect_calendar","account_id":id}),
            |this, result| {
                this.set_busy(false);
                match result {
                    Ok(_) => this.reload(),
                    Err(error) => {
                        this.connect.set_sensitive(true);
                        this.status.set_text(&error)
                    }
                }
            },
        );
    }
    fn maybe_sync(self: &Rc<Self>) {
        if self.ui.workspace.visible_child_name().as_deref() != Some("calendar")
            || self.back.is_visible()
            || self.busy.get()
        {
            return;
        }
        let (start, end) = self.range();
        let pending: Vec<String> = self
            .connected_accounts
            .borrow()
            .iter()
            .filter(|id| {
                !self
                    .sync_successes
                    .borrow()
                    .iter()
                    .any(|(account, lower, upper, at)| {
                        account == *id
                            && *lower <= start
                            && *upper >= end
                            && at.elapsed().as_secs() < 300
                    })
                    && !self
                        .sync_attempts
                        .borrow()
                        .iter()
                        .any(|(account, lower, upper, at)| {
                            account == *id
                                && *lower == start
                                && *upper == end
                                && at.elapsed().as_secs() < 60
                        })
            })
            .cloned()
            .collect();
        if pending.is_empty() {
            self.status
                .set_text("Saved for offline access · Up to date");
            return;
        }
        if self.busy.replace(true) {
            return;
        }
        self.account.set_sensitive(false);
        self.refresh.set_sensitive(false);
        self.sync_accounts(pending, start, end, Vec::new());
    }
    fn sync(self: &Rc<Self>) {
        if self.busy.replace(true) {
            return;
        }
        let ids = self.connected_accounts.borrow().clone();
        let (start, end) = self.range();
        self.account.set_sensitive(false);
        self.refresh.set_sensitive(false);
        self.sync_accounts(ids, start, end, Vec::new());
    }
    fn sync_accounts(
        self: &Rc<Self>,
        mut remaining: Vec<String>,
        start: i64,
        end: i64,
        mut errors: Vec<String>,
    ) {
        if remaining.is_empty() {
            self.set_busy(false);
            self.refresh.set_sensitive(true);
            self.reload();
            if !errors.is_empty() {
                self.status.set_text(&format!(
                    "Some calendars could not refresh · Saved events remain available. {}",
                    errors.join(" · ")
                ));
            }
            return;
        }
        let id = remaining.remove(0);
        self.status
            .set_text("Refreshing connected calendars… Saved events stay available.");
        self.sync_attempts
            .borrow_mut()
            .push((id.clone(), start, end, std::time::Instant::now()));
        self.request(
            json!({"op":"sync_calendar","account_id":id,"start_ms":start,"end_ms":end}),
            move |this, result| {
                match result {
                    Ok(_) => this.sync_successes.borrow_mut().push((
                        id,
                        start,
                        end,
                        std::time::Instant::now(),
                    )),
                    Err(error) => errors.push(error),
                }
                this.sync_attempts
                    .borrow_mut()
                    .retain(|(_, _, _, at)| at.elapsed().as_secs() < 300);
                this.sync_successes
                    .borrow_mut()
                    .retain(|(_, _, _, at)| at.elapsed().as_secs() < 600);
                this.sync_accounts(remaining, start, end, errors);
            },
        );
    }
    fn respond(self: &Rc<Self>, event: &Value, response: &str) {
        let Some(id) = event["account_id"]
            .as_str()
            .filter(|id| self.id().is_none() || self.id().as_deref() == Some(*id))
            .map(str::to_owned)
        else {
            return;
        };
        if self.busy.replace(true) {
            return;
        };
        self.account.set_sensitive(false);
        self.status.set_text("Sending your response…");
        self.request(json!({"op":"respond_calendar_invitation","account_id":id,"calendar_id":event["calendar_id"],"event_id":event["id"],"response":response}),|this,result|{this.set_busy(false);match result{Ok(_)=>this.reload(),Err(error)=>this.status.set_text(&error)}});
    }
    fn new_event(self: &Rc<Self>) {
        self.new_event_at(None, None);
    }
    fn new_event_at(self: &Rc<Self>, at: Option<glib::DateTime>, editing: Option<Value>) {
        let focused_id = self.id();
        if self.busy.get() {
            return;
        }
        if let Some(old) = self.editor_popup.borrow_mut().take() {
            old.popdown();
        }
        let writable: Vec<Value> = self
            .calendars
            .borrow()
            .iter()
            .filter(|c| {
                c["can_write"].as_bool() == Some(true)
                    && (focused_id.is_none() || c["account_id"].as_str() == focused_id.as_deref())
            })
            .cloned()
            .collect();
        let id = editing
            .as_ref()
            .and_then(|event| event["account_id"].as_str())
            .map(str::to_owned)
            .or(focused_id)
            .or_else(|| {
                writable
                    .first()
                    .and_then(|c| c["account_id"].as_str())
                    .map(str::to_owned)
            })
            .or_else(|| self.accounts.first().map(|account| account.id.clone()));
        let Some(id) = id else {
            return;
        };
        let popup = gtk::Popover::new();
        popup.set_has_arrow(false);
        popup.set_autohide(false);
        popup.set_parent(&self.create_button);
        let shell = gtk::Box::new(Orientation::Vertical, 10);
        shell.add_css_class("calendar-editor");
        let content = gtk::Box::new(Orientation::Vertical, 10);
        shell.set_width_request(410);
        shell.set_margin_top(16);
        shell.set_margin_bottom(16);
        shell.set_margin_start(16);
        shell.set_margin_end(16);
        let header = gtk::Box::new(Orientation::Horizontal, 8);
        let heading = text(
            if editing.is_some() {
                "Edit event"
            } else {
                "New event"
            },
            true,
        );
        heading.add_css_class("calendar-editor-caption");
        heading.set_selectable(false);
        heading.set_hexpand(true);
        header.append(&heading);
        let cancel = gtk::Button::with_label("×");
        cancel.set_tooltip_text(Some("Close · Esc"));
        header.append(&cancel);
        {
            let popup = popup.downgrade();
            cancel.connect_clicked(move |_| {
                if let Some(popup) = popup.upgrade() {
                    popup.popdown();
                }
            });
        }
        shell.append(&header);
        let title = gtk::Entry::new();
        title.set_placeholder_text(Some("Lunch tomorrow at 12 for 1h"));
        title.add_css_class("calendar-editor-title");
        title.update_property(&[gtk::accessible::Property::Label("Event title")]);
        if let Some(event) = &editing {
            title.set_text(event["title"].as_str().unwrap_or(""));
        }
        content.append(&title);
        let quick_preview = text("", false);
        quick_preview.set_selectable(false);
        quick_preview.add_css_class("calendar-draft-summary");
        let time_toggle = gtk::Button::new();
        time_toggle.set_child(Some(&quick_preview));
        time_toggle.add_css_class("flat");
        time_toggle.add_css_class("calendar-time-disclosure");
        content.append(&time_toggle);
        let quick_hint = text("Try lunch 12h · tomorrow at 12 for 1h", false);
        quick_hint.set_selectable(false);
        quick_hint.add_css_class("agenda-time");
        content.append(&quick_hint);
        let names: Vec<String> = if writable.is_empty() {
            vec!["Connect a writable calendar to save".to_owned()]
        } else {
            writable
                .iter()
                .map(|calendar| {
                    let title = calendar["name"].as_str().unwrap_or("Calendar");
                    if self.id().is_none() {
                        let account = calendar["account_id"].as_str().unwrap_or("");
                        let owner = self
                            .accounts
                            .iter()
                            .find(|a| a.id == account)
                            .map(|a| a.email.as_str())
                            .unwrap_or(account);
                        if title.eq_ignore_ascii_case(owner)
                            || title.to_lowercase().contains(&owner.to_lowercase())
                        {
                            title.to_owned()
                        } else {
                            format!("{title} · {owner}")
                        }
                    } else {
                        title.to_owned()
                    }
                })
                .collect()
        };
        let names_ref: Vec<&str> = names.iter().map(String::as_str).collect();
        let destination = gtk::DropDown::from_strings(&names_ref);
        destination.set_sensitive(editing.is_none() && !writable.is_empty());
        if let Some(event) = &editing {
            if let Some(index) = writable.iter().position(|c| {
                c["id"] == event["calendar_id"] && c["account_id"] == event["account_id"]
            }) {
                destination.set_selected(index as u32);
            }
        }
        destination.set_hexpand(true);
        let destination_row = gtk::Box::new(Orientation::Horizontal, 10);
        let destination_label = text("Calendar", false);
        destination_label.set_selectable(false);
        destination_label.set_width_chars(7);
        destination_row.append(&destination_label);
        destination_row.append(&destination);

        if writable.is_empty() {
            content.append(&text(
                "No writable calendar connected. You can draft here; connect a calendar to save.",
                false,
            ));
            let connect = gtk::Button::with_label(if self.id().is_some() {
                "Connect this calendar account"
            } else {
                "Choose an account to connect"
            });
            let this = self.clone();
            let popup = popup.downgrade();
            connect.connect_clicked(move |_| {
                if let Some(popup) = popup.upgrade() {
                    popup.popdown();
                }
                if this.id().is_some() {
                    this.connect();
                } else {
                    if this.source_popup.child().is_some() {
                        this.source_popup.popup();
                    }
                    this.account.grab_focus();
                }
            });
            content.append(&connect);
        }
        let begin = editing
            .as_ref()
            .and_then(|e| e["start_ms"].as_i64())
            .and_then(|ms| glib::DateTime::from_unix_local(ms / 1000).ok())
            .or(at)
            .unwrap_or_else(|| self.date.borrow().add_hours(9).unwrap());
        let finish = editing
            .as_ref()
            .and_then(|e| e["end_ms"].as_i64())
            .and_then(|ms| glib::DateTime::from_unix_local(ms / 1000).ok())
            .unwrap_or_else(|| begin.add_minutes(30).unwrap());
        let start_day = gtk::Entry::new();
        start_day.set_text(&begin.format("%Y-%m-%d").unwrap());
        start_day.set_width_chars(12);
        start_day.set_max_width_chars(12);
        let start_time = gtk::Entry::new();
        start_time.set_text(&begin.format("%H:%M").unwrap());
        start_time.set_width_chars(6);
        start_time.set_max_width_chars(6);
        let end_day = gtk::Entry::new();
        end_day.set_text(&finish.format("%Y-%m-%d").unwrap());
        end_day.set_width_chars(12);
        end_day.set_max_width_chars(12);
        let end_time = gtk::Entry::new();
        end_time.set_text(&finish.format("%H:%M").unwrap());
        end_time.set_width_chars(6);
        end_time.set_max_width_chars(6);
        let time_fields = gtk::Box::new(Orientation::Vertical, 8);
        let time_reveal = gtk::Revealer::new();
        time_reveal.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        time_reveal.set_child(Some(&time_fields));
        content.append(&time_reveal);
        {
            let reveal = time_reveal.clone();
            let start = start_day.clone();
            time_toggle.connect_clicked(move |_| {
                reveal.set_reveal_child(!reveal.reveals_child());
                if reveal.reveals_child() {
                    start.grab_focus();
                }
            });
        }
        for (caption, date, time) in [
            ("Starts", &start_day, &start_time),
            ("Ends", &end_day, &end_time),
        ] {
            let row = gtk::Box::new(Orientation::Horizontal, 8);
            let caption = text(caption, false);
            caption.set_selectable(false);
            caption.set_width_chars(7);
            row.append(&caption);
            date.set_hexpand(true);
            row.append(date);
            row.append(time);
            time_fields.append(&row);
        }
        content.append(&destination_row);
        let details = gtk::Box::new(Orientation::Vertical, 8);
        let more = gtk::Expander::new(Some("Add details"));
        more.set_expanded(editing.as_ref().is_some_and(|event| {
            ["location", "description"]
                .iter()
                .any(|key| event[*key].as_str().is_some_and(|value| !value.is_empty()))
        }));
        more.set_child(Some(&details));
        let location = gtk::Entry::new();
        location.set_placeholder_text(Some("Add a location"));
        location.set_text(
            editing
                .as_ref()
                .and_then(|e| e["location"].as_str())
                .unwrap_or(""),
        );
        location.update_property(&[gtk::accessible::Property::Label("Location")]);
        details.append(&location);
        let notes = gtk::Entry::new();
        notes.set_placeholder_text(Some("Add notes"));
        notes.set_text(
            editing
                .as_ref()
                .and_then(|e| e["description"].as_str())
                .unwrap_or(""),
        );
        notes.update_property(&[gtk::accessible::Property::Label("Notes")]);
        details.append(&notes);
        let attendees = gtk::Entry::new();
        attendees.set_placeholder_text(Some("Email addresses, separated by commas"));
        if editing.is_none() {
            attendees.update_property(&[gtk::accessible::Property::Label("Guests")]);
            details.append(&attendees);
        }
        let zone = begin.format("%Z").unwrap();
        let zone_hint = text(&format!("{zone} · 24-hour time"), false);
        zone_hint.set_selectable(false);
        zone_hint.add_css_class("agenda-time");
        time_fields.append(&zone_hint);
        let guest_hint = text(
            if editing.is_some() {
                "Existing guests are preserved and receive your changes when saved."
            } else {
                "Adding guests sends invitations when you create the event."
            },
            false,
        );
        guest_hint.set_max_width_chars(44);
        guest_hint.set_selectable(false);
        guest_hint.add_css_class("agenda-time");
        details.append(&guest_hint);
        content.append(&more);
        let feedback = text("", false);
        feedback.set_selectable(false);
        feedback.set_max_width_chars(44);
        let footer_feedback = feedback.clone();
        let save = gtk::Button::with_label(if editing.is_some() {
            "Save changes"
        } else {
            "Create event"
        });
        save.add_css_class("agenda-new");
        save.set_sensitive(!writable.is_empty());
        let key_hint = text("Tab next · Ctrl↵ save · Esc close", false);
        key_hint.set_max_width_chars(44);
        key_hint.set_selectable(false);
        key_hint.add_css_class("agenda-time");
        let pending: Rc<RefCell<Option<QuickCalendarEvent>>> = Rc::new(RefCell::new(None));
        let quick_error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let refresh_summary: Rc<dyn Fn()> = {
            let summary = quick_preview.downgrade();
            let title = title.downgrade();
            let pending = pending.clone();
            let quick_error = quick_error.clone();
            let live_draft = self.live_draft.clone();
            let canvases = self.draft_canvases.clone();
            let start_day = start_day.downgrade();
            let start_time = start_time.downgrade();
            let end_day = end_day.downgrade();
            let end_time = end_time.downgrade();
            Rc::new(move || {
                let (
                    Some(summary),
                    Some(title),
                    Some(start_day),
                    Some(start_time),
                    Some(end_day),
                    Some(end_time),
                ) = (
                    summary.upgrade(),
                    title.upgrade(),
                    start_day.upgrade(),
                    start_time.upgrade(),
                    end_day.upgrade(),
                    end_time.upgrade(),
                )
                else {
                    return;
                };
                let parse = |day: &gtk::Entry, time: &gtk::Entry| {
                    glib::DateTime::from_iso8601(
                        &format!("{}T{}:00", day.text().trim(), time.text().trim()),
                        Some(&glib::TimeZone::local()),
                    )
                    .ok()
                };
                let value = match (parse(&start_day, &start_time), parse(&end_day, &end_time)) {
                    (Some(start), Some(end)) if end.to_unix() > start.to_unix() => {
                        let duration = (end.to_unix() - start.to_unix()) / 60;
                        let end_format = if start.format("%Y-%m-%d").unwrap()
                            == end.format("%Y-%m-%d").unwrap()
                        {
                            "%H:%M"
                        } else {
                            "%a %-d %b %H:%M"
                        };
                        format!(
                            "{}–{} · {} min · Edit time",
                            start.format("%a %-d %b · %H:%M").unwrap(),
                            end.format(end_format).unwrap(),
                            duration
                        )
                    }
                    _ => "Check the start and end time".to_owned(),
                };
                summary.set_text(&value);
                let canonical = pending
                    .borrow()
                    .as_ref()
                    .map(|draft| draft.title.clone())
                    .unwrap_or_else(|| {
                        if title.text().trim().is_empty() {
                            "New event".to_owned()
                        } else {
                            title.text().trim().to_owned()
                        }
                    });
                *live_draft.borrow_mut() = if quick_error.borrow().is_none() {
                    validated_calendar_draft(
                        &canonical,
                        None,
                        start_day.text().as_str(),
                        start_time.text().as_str(),
                        end_day.text().as_str(),
                        end_time.text().as_str(),
                    )
                    .ok()
                } else {
                    None
                };
                for canvas in canvases.borrow().iter() {
                    canvas.queue_draw();
                }
            })
        };
        let setting_query = Rc::new(Cell::new(false));
        let previous_start = Rc::new(Cell::new(begin.to_unix()));
        for field in [&start_day, &start_time] {
            let guard = setting_query.clone();
            let previous = previous_start.clone();
            let day = start_day.downgrade();
            let time = start_time.downgrade();
            let end_day = end_day.downgrade();
            let end_time = end_time.downgrade();
            field.connect_changed(move |_| {
                let (Some(day), Some(time), Some(end_day), Some(end_time)) = (
                    day.upgrade(),
                    time.upgrade(),
                    end_day.upgrade(),
                    end_time.upgrade(),
                ) else {
                    return;
                };
                if guard.get() {
                    return;
                }
                let parse = |day: &gtk::Entry, time: &gtk::Entry| {
                    glib::DateTime::from_iso8601(
                        &format!("{}T{}:00", day.text().trim(), time.text().trim()),
                        Some(&glib::TimeZone::local()),
                    )
                    .ok()
                };
                if let Some(start) = parse(&day, &time) {
                    let delta = start.to_unix() - previous.replace(start.to_unix());
                    if let Some(end) = parse(&end_day, &end_time)
                        .and_then(|end| end.add_seconds(delta as f64).ok())
                    {
                        end_day.set_text(&end.format("%Y-%m-%d").unwrap());
                        end_time.set_text(&end.format("%H:%M").unwrap());
                    }
                }
            });
        }
        for field in [&start_day, &start_time, &end_day, &end_time] {
            let refresh = refresh_summary.clone();
            let query = title.downgrade();
            let pending = pending.clone();
            let guard = setting_query.clone();
            field.connect_changed(move |_| {
                if !guard.get() {
                    let canonical = pending.borrow().as_ref().map(|draft| draft.title.clone());
                    if let (Some(query), Some(canonical)) = (query.upgrade(), canonical) {
                        guard.set(true);
                        query.set_text(&canonical);
                        guard.set(false);
                    }
                }
                refresh();
            });
        }
        refresh_summary();
        {
            let pending = pending.clone();
            let quick_error = quick_error.clone();
            let hint = quick_hint.clone();
            let selected = begin.clone();
            let start_day = start_day.clone();
            let start_time = start_time.clone();
            let end_day = end_day.clone();
            let end_time = end_time.clone();
            let refresh = refresh_summary.clone();
            let feedback = feedback.clone();
            let setting_query = setting_query.clone();
            let previous_start = previous_start.clone();
            title.connect_changed(move |entry| {
                if setting_query.get() {
                    return;
                }
                let now = glib::DateTime::now_local().unwrap();
                match parse_quick_calendar_event(entry.text().as_str(), &selected, &now) {
                    Ok(Some(draft)) => {
                        // Only editing the query derives new times. Manual field edits
                        // subsequently remain the source of truth until the query changes.
                        setting_query.set(true);
                        previous_start.set(draft.start.to_unix());
                        start_day.set_text(&draft.start.format("%Y-%m-%d").unwrap());
                        start_time.set_text(&draft.start.format("%H:%M").unwrap());
                        end_day.set_text(&draft.end.format("%Y-%m-%d").unwrap());
                        end_time.set_text(&draft.end.format("%H:%M").unwrap());
                        setting_query.set(false);
                        *pending.borrow_mut() = Some(draft);
                        *quick_error.borrow_mut() = None;
                        hint.set_text("Date and time updated");
                        hint.remove_css_class("calendar-draft-error");
                        feedback.set_text("");
                    }
                    Ok(None) => {
                        *pending.borrow_mut() = None;
                        *quick_error.borrow_mut() = None;
                        hint.set_text("Try lunch 12h · tomorrow at 12 for 1h");
                        hint.remove_css_class("calendar-draft-error");
                        feedback.set_text("");
                    }
                    Err(error) => {
                        hint.set_text(&error);
                        hint.add_css_class("calendar-draft-error");
                        *pending.borrow_mut() = None;
                        *quick_error.borrow_mut() = Some(error);
                    }
                }
                refresh();
            });
        }
        {
            let toggle = time_toggle.clone();
            title.connect_activate(move |_| {
                toggle.grab_focus();
            });
        }
        let this = self.clone();
        let popup_save = popup.downgrade();
        let title_focus = title.clone();
        save.connect_clicked(move |_|{
            if this.busy.get()||(this.id().is_some()&&this.id().as_deref()!=Some(id.as_str())){return;}
            if let Some(error)=quick_error.borrow().as_ref(){feedback.set_text(error);return;}
            let parsed_title=pending.borrow().as_ref().map(|draft|draft.title.clone());
            let (final_title,start,end)=match validated_calendar_draft(title.text().as_str(),parsed_title.as_deref(),start_day.text().as_str(),start_time.text().as_str(),end_day.text().as_str(),end_time.text().as_str()){Ok(draft)=>draft,Err(error)=>{feedback.set_text(error);if error=="Give this event a title."{title.grab_focus();}return;}};
            let request=if let Some(event)=&editing {json!({"op":"update_calendar_event","account_id":id,"calendar_id":event["calendar_id"],"event_id":event["id"],"expected_etag":event["etag"],"edit_scope":if event["recurring_event_id"].as_str().is_some_and(|v|!v.is_empty()){"instance"}else{"single"},"title":final_title,"start_ms":start,"end_ms":end,"location":location.text().as_str(),"description":notes.text().as_str()})}
            else {let recipients:Vec<String>=attendees.text().split(',').map(str::trim).filter(|s|!s.is_empty()).map(str::to_owned).collect();json!({"op":"create_calendar_event","account_id":writable[destination.selected() as usize]["account_id"],"calendar_id":writable[destination.selected() as usize]["id"],"title":final_title,"start_ms":start,"end_ms":end,"location":location.text().as_str(),"description":notes.text().as_str(),"attendee_emails":recipients})};
            this.set_busy(true);feedback.set_text("Saving event…");let feedback=feedback.clone();let popup=popup_save.clone();
            this.request(request,move|this,result|{this.set_busy(false);match result{Ok(_)=>{if let Some(popup)=popup.upgrade(){popup.popdown();}this.reload();},Err(error)=>feedback.set_text(&error)}});
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let save = save.clone();
            let popup = popup.downgrade();
            keys.connect_key_pressed(move |_, key, _, mods| {
                if key == gdk::Key::Escape {
                    if let Some(popup) = popup.upgrade() {
                        popup.popdown();
                    }
                    return glib::Propagation::Stop;
                }
                if mods.contains(gdk::ModifierType::CONTROL_MASK)
                    && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
                {
                    if save.is_sensitive() {
                        save.emit_clicked();
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        popup.add_controller(keys);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_max_content_height((self.ui.calendar_host.height() - 240).clamp(200, 420));
        scroll.set_propagate_natural_height(true);
        scroll.set_child(Some(&content));
        shell.append(&scroll);
        shell.append(&footer_feedback);
        let footer = gtk::Box::new(Orientation::Horizontal, 10);
        key_hint.set_hexpand(true);
        footer.append(&key_hint);
        footer.append(&save);
        shell.append(&footer);
        popup.set_child(Some(&shell));
        {
            let draft = self.live_draft.clone();
            let canvases = self.draft_canvases.clone();
            let owner = Rc::downgrade(self);
            popup.connect_closed(move |popup| {
                if let Some(owner) = owner.upgrade() {
                    // Escape may already be taking this slot while emitting closed.
                    if let Ok(mut slot) = owner.editor_popup.try_borrow_mut() {
                        slot.take();
                    }
                }
                *draft.borrow_mut() = None;
                for canvas in canvases.borrow().iter() {
                    canvas.queue_draw();
                }
                popup.unparent();
            });
        }
        *self.editor_popup.borrow_mut() = Some(popup.clone());
        popup.popup();
        title_focus.grab_focus();
        glib::idle_add_local_once(move || {
            title_focus.grab_focus();
        });
    }
    fn availability(self: &Rc<Self>) {
        let id = self.id();
        let (start, end) = {
            let d = self.date.borrow();
            let begin = glib::DateTime::from_local(d.year(), d.month(), d.day_of_month(), 9, 0, 0.)
                .unwrap();
            let finish =
                glib::DateTime::from_local(d.year(), d.month(), d.day_of_month(), 17, 0, 0.)
                    .unwrap();
            (begin.to_unix() * 1000, finish.to_unix() * 1000)
        };
        let serial = self.generation.get() + 1;
        self.generation.set(serial);
        self.request(json!({"op":"availability","account_id":id,"start_ms":start,"end_ms":end,"duration_minutes":30}),move|this,result|{if this.generation.get()!=serial{return};match result{Err(error)=>this.status.set_text(&error),Ok(result)=>{
        this.clear();this.show_back();this.rows.set_spacing(10);this.status.set_text(if result["coverage_complete"].as_bool()==Some(true){"30-minute suggestions, 09:00–17:00 on the selected day. Other calendars may conflict; confirm before sharing."}else{"09:00–17:00 on the selected day. Calendar coverage is incomplete; these are suggestions, not confirmed availability."});
        let mut lines=Vec::new();for slot in result["slots"].as_array().into_iter().flatten().take(5){let line=format!("{} – {}",date(slot["start_ms"].as_i64().unwrap_or(0)),date(slot["end_ms"].as_i64().unwrap_or(0)));this.rows.append(&text(&line,true));lines.push(line);}
        if lines.is_empty(){this.rows.append(&text("No suggestions in the saved calendar window.",false));}else{let copy=gtk::Button::with_label("Copy suggested times");let day=this.date.borrow().format("%e %b %Y · %Z").unwrap().to_string();let value=format!("Suggested times — {day}\n{}\nPlease confirm availability; based on saved calendar data.",lines.join("\n"));copy.connect_clicked(move|button|button.clipboard().set_text(&value));this.rows.append(&copy);}
    }}});
    }
}
pub(super) fn open(ui: &Rc<Ui>) {
    // G C and the palette follow the sidebar: no Calendar where none can be connected.
    if !ui.calendar_available(&ui.accounts()) {
        ui.window.error_bell();
        return;
    }
    ui.workspace.set_visible_child_name("calendar");
    ui.compact_navigation.set(false);
    ui.goto_pending.set(false);
    ui.refresh_navigation(&ui.accounts());
    let existing = ui.calendar.borrow().clone();
    if let Some(calendar) = existing {
        ui.calendar_host.grab_focus();
        calendar.maybe_sync();
        return;
    }
    let window = ui.window.clone();
    let css = gtk::CssProvider::new();
    let mut design_css = String::from(
        ".agenda-root { background: @surface; color: @ink; padding: 0; } .agenda-heading { font-size: 20px; font-weight: 600; } .agenda-toolbar { padding-bottom: 10px; } .agenda-toolbar button { background: transparent; } .agenda-toolbar button:hover { background: @hover; } .agenda-toolbar .agenda-new { background: @selected; font-weight: 600; } .agenda-day { font-size: 12px; color: @secondary; padding: 20px 0 9px; font-weight: 600; } .agenda-event { padding: 14px 0; border-bottom: 1px solid @line; } .agenda-event-title { font-size: 13px; font-weight: 500; } .agenda-time { font-size: 11px; color: @muted; } .agenda-status, .agenda-footer { font-size: 11px; color: @muted; } .agenda-footer { border-top: 1px solid @line; padding-top: 10px; } .agenda-modes button { color: @muted; } .agenda-modes button:checked { background: @selected; color: @ink; } .calendar-column { border-left: 1px solid @line; } .calendar-day-header { font-size: 12px; font-weight: 600; border-bottom: 1px solid @line; } .calendar-weekday { font-size: 11px; color: @muted; padding: 2px 0; min-height: 16px; } .calendar-month-weekday { font-size: 11px; color: @muted; min-height: 24px; padding: 3px 0; } .calendar-day-number { font-size: 19px; font-weight: 500; } .calendar-today { background: @selected; } .calendar-all-day { padding: 3px; border-bottom: 1px solid @line; } .calendar-hour { font-size: 11px; color: @muted; min-height: 20px; padding: 2px 0; } button.calendar-block { background: @selected; border-left: 2px solid @muted; border-radius: 5px; padding: 3px 5px; min-width: 0; min-height: 0; } button.calendar-block:hover { background: @hover; } button.calendar-block.calendar-selected, button.calendar-date.calendar-selected { outline: 1px solid @ink; outline-offset: -1px; } .calendar-block label { font-size: 11px; }",
    );
    design_css.push_str(" .calendar-month-cell { border: 1px solid @line; padding: 3px; } .calendar-other-month { opacity: 0.4; } .calendar-date { padding: 0 4px; min-height: 20px; } .calendar-month-name { font-weight: 600; } button.calendar-month-event { min-height: 18px; padding: 1px 3px; } button.calendar-month-more { min-height: 14px; padding: 0; font-size: 10px; color: @muted; } ");
    for (index, color) in CALENDAR_COLORS.iter().enumerate() {
        design_css.push_str(&format!("button.calendar-block.calendar-tone-{index} {{border-left-color: {color}; background: alpha({color},0.22);}} .calendar-legend-{index} {{color: {color};font-size:11px;}}"));
    }
    design_css.push_str(" .calendar-header {padding: 18px 18px 16px; border-bottom:1px solid @line; background:@surface;} .calendar-header button {min-height:30px;min-width:26px;padding:0 9px;border-radius:7px;background:transparent;} .calendar-header button:hover {background:@hover;} .calendar-header .agenda-new {background:@ink;color:@surface;font-weight:600;padding:0 14px;} .calendar-header .agenda-new:hover {opacity:0.9;} .calendar-header dropdown button {background:@hover;} .calendar-header-secondary {padding-top:0;padding-bottom:12px;} .calendar-rail {padding:20px 14px 12px;border-right:1px solid @line;background:@surface;} .calendar-jump {font-size:12px;min-width:252px;} .calendar-jump grid label {min-height:30px;min-width:30px;padding:0;} .calendar-jump header button {min-height:30px;} .calendar-rail-caption {font-size:10px;letter-spacing:1px;color:@muted;font-weight:600;} .calendar-account {font-size:11px;} .calendar-mini {font-size:10px;background:transparent;padding:0;min-width:0;} .calendar-mini header {font-size:11px;} .calendar-mini label {padding:1px;min-width:14px;min-height:14px;} .calendar-mini grid label {border-radius:4px;} .calendar-rail checkbutton {min-height:24px;font-size:12px;} .calendar-rail checkbutton check {min-width:12px;min-height:12px;border-radius:4px;} .agenda-status {padding:7px 16px 0;font-size:10px;} .agenda-footer {padding:7px 16px;border:none;font-size:10px;} .calendar-day-header {padding:8px 0;} .calendar-day-number {font-size:22px;font-weight:500;} .calendar-today .calendar-day-number {color:@ink;} button.calendar-block {border-left-width:3px;padding:5px 7px;border-radius:6px;} .calendar-block label {font-size:12px;font-weight:500;} .calendar-block .agenda-time {font-size:11px;font-weight:400;color:@secondary;min-height:18px;padding-top:2px;padding-bottom:1px;} ");
    design_css.push_str(" .calendar-rail {padding:18px 14px 12px;} .calendar-rail dropdown button {min-height:26px;padding:2px 6px;background:transparent;border:1px solid @line;border-radius:6px;} .calendar-mini {border:none;box-shadow:none;} .calendar-mini header {min-height:22px;padding:0;} .calendar-mini header button {min-width:16px;min-height:18px;padding:0 2px;background:transparent;border:none;box-shadow:none;} .calendar-mini grid label {min-width:20px;min-height:20px;padding:0;font-size:10px;} .calendar-mini grid label:selected {background:@selected;color:@ink;} .calendar-rail checkbutton {padding:3px 0;min-height:20px;} .calendar-rail checkbutton check {background:transparent;border:1px solid @muted;box-shadow:none;color:@ink;} .calendar-rail checkbutton check:checked {background:@selected;border-color:@muted;color:@ink;} .calendar-command entry {min-height:36px;padding:4px 10px;border-radius:8px;} .calendar-date-preview {min-height:44px;border:none;box-shadow:none;background:@selected;border-radius:8px;padding:8px 12px;font-weight:600;} .calendar-date-preview:disabled {background:transparent;color:@muted;} .calendar-command expander {margin-top:6px;} .calendar-keycap {background:@hover;border:1px solid @line;border-radius:5px;padding:3px 7px;font-size:11px;color:@secondary;} .calendar-shortcuts label {min-height:20px;} ");
    design_css.push_str(" .calendar-mini-custom {min-width:184px;} .calendar-mini-custom button {min-width:20px;min-height:20px;padding:0;border:none;box-shadow:none;background:transparent;border-radius:5px;font-size:10px;} .calendar-mini-custom button:hover {background:@hover;} .calendar-mini-custom button.calendar-mini-selected {background:@selected;color:@ink;font-weight:700;} .calendar-mini-custom .heading {font-size:11px;} ");
    design_css.push_str(" .calendar-editor {min-width:0;} .calendar-editor entry {min-height:28px;padding:4px 8px;border-radius:6px;} .calendar-editor entry.calendar-editor-title {font-size:20px;font-weight:600;background:transparent;border:none;box-shadow:none;padding:6px 0;min-height:34px;} .calendar-editor-caption {font-size:11px;color:@muted;font-weight:600;} .calendar-editor dropdown button {min-height:28px;padding:3px 8px;border-radius:6px;background:@hover;} .calendar-editor button {min-height:28px;padding:4px 10px;border-radius:6px;} .calendar-editor .agenda-new {background:@ink;color:@surface;font-weight:600;} .calendar-editor expander {font-size:12px;color:@secondary;} ");
    design_css.push_str(" .calendar-draft-summary {font-size:12px;font-weight:500;color:@ink;padding:2px 0;} .calendar-time-disclosure {padding:4px 0;min-height:24px;} .calendar-draft-error {color:#b55d54;} ");
    css.load_from_data(&design_css);
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(&window),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let root = gtk::Box::new(Orientation::Vertical, 0);
    root.add_css_class("agenda-root");
    root.set_size_request(-1, 372);
    root.set_hexpand(true);
    root.set_vexpand(true);
    let records = ui.accounts();
    let titles: Vec<&str> = std::iter::once("All accounts")
        .chain(records.iter().map(|a| a.email.as_str()))
        .collect();
    let account = gtk::DropDown::from_strings(&titles);
    account.add_css_class("calendar-account");
    let body = gtk::Box::new(Orientation::Horizontal, 0);
    body.set_hexpand(true);
    body.set_vexpand(true);
    let rail_slot = gtk::Box::new(Orientation::Vertical, 0);
    let rail = gtk::Box::new(Orientation::Vertical, 10);
    rail.add_css_class("calendar-rail");
    rail.set_width_request(184);
    let mini_calendar = gtk::Box::new(Orientation::Vertical, 6);
    mini_calendar.add_css_class("calendar-mini-custom");
    rail.append(&mini_calendar);
    let source_label = text("MY CALENDARS", false);
    source_label.add_css_class("calendar-rail-caption");
    rail.append(&source_label);
    rail.append(&account);
    let legend = gtk::Box::new(Orientation::Vertical, 5);
    let legend_scroll = gtk::ScrolledWindow::new();
    legend_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    legend_scroll.set_vexpand(true);
    legend_scroll.set_child(Some(&legend));
    rail.append(&legend_scroll);
    let connect = gtk::Button::with_label("Connect calendar");
    connect.add_css_class("flat");
    rail.append(&connect);
    let timezone = text(
        &glib::DateTime::now_local().unwrap().format("%Z").unwrap(),
        false,
    );
    timezone.add_css_class("agenda-time");
    rail.append(&timezone);
    rail_slot.append(&rail);
    body.append(&rail_slot);
    let main = gtk::Box::new(Orientation::Vertical, 0);
    main.set_hexpand(true);
    main.set_vexpand(true);
    let heading = gtk::Box::new(Orientation::Horizontal, 6);
    heading.add_css_class("calendar-header");
    let source_menu = gtk::MenuButton::new();
    source_menu.set_child(Some(&mail_icon("x-office-calendar-symbolic")));
    source_menu.update_property(&[gtk::accessible::Property::Label("Calendars")]);
    source_menu.set_tooltip_text(Some("Show calendars and date navigator"));
    let source_popup = gtk::Popover::new();
    source_popup.set_has_arrow(false);
    source_menu.set_popover(Some(&source_popup));
    source_menu.set_visible(false);
    heading.append(&source_menu);
    let back_to_mail = gtk::Button::with_label("‹");
    back_to_mail.set_tooltip_text(Some("Back to mail · G I"));
    {
        let ui = ui.clone();
        back_to_mail.connect_clicked(move |_| leave(&ui));
    }
    heading.append(&back_to_mail);
    let date_label = text("Calendar", true);
    date_label.add_css_class("agenda-heading");
    date_label.set_wrap(false);
    date_label.set_selectable(false);
    date_label.set_hexpand(true);
    date_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    date_label.set_width_chars(1);
    heading.append(&date_label);
    let prev = gtk::Button::with_label("‹");
    prev.set_tooltip_text(Some("Previous period · H / ←"));
    let today = gtk::Button::with_label("Today");
    today.set_tooltip_text(Some("Go to today · T"));
    let next = gtk::Button::with_label("›");
    next.set_tooltip_text(Some("Next period · L / →"));
    heading.append(&today);
    heading.append(&prev);
    heading.append(&next);
    let mut mode_buttons: Vec<gtk::ToggleButton> = Vec::new();
    for caption in ["Day", "Week", "Month", "Year"] {
        let button = gtk::ToggleButton::with_label(caption);
        if let Some(first) = mode_buttons.first() {
            button.set_group(Some(first));
        }
        mode_buttons.push(button);
    }
    mode_buttons[1].set_active(true);
    let compact_mode = gtk::DropDown::from_strings(&["Day", "Week", "Month", "Year"]);
    compact_mode.set_selected(1);
    compact_mode.set_tooltip_text(Some("Calendar view · D / W / M / Y"));
    heading.append(&compact_mode);
    let search = icon_button("system-search-symbolic", "Search calendar · /");
    {
        let ui = ui.clone();
        search.connect_clicked(move |_| ui.open_palette("calendar-search"));
    }
    heading.append(&search);
    let create = gtk::Button::with_label("New event");
    create.add_css_class("agenda-new");
    create.set_tooltip_text(Some("New event · C"));
    heading.append(&create);
    let go_date = gtk::MenuButton::new();
    go_date.set_child(Some(&gtk::Label::new(Some("⌄"))));
    go_date.set_tooltip_text(Some("Go to date · B"));
    let date_popup = gtk::Popover::new();
    let date_content = gtk::Box::new(Orientation::Vertical, 8);
    date_content.add_css_class("calendar-command");
    date_content.set_width_request(300);
    date_content.set_margin_top(16);
    date_content.set_margin_bottom(16);
    date_content.set_margin_start(12);
    date_content.set_margin_end(12);
    let date_title = text("Go to date", true);
    date_title.set_selectable(false);
    date_content.append(&date_title);
    let jump_calendar = gtk::Calendar::new();
    jump_calendar.set_show_heading(true);
    jump_calendar.set_show_day_names(true);
    jump_calendar.set_width_request(252);
    jump_calendar.add_css_class("calendar-jump");
    let date_entry = gtk::Entry::new();
    date_entry.set_placeholder_text(Some("20 October, next Friday, in 2 weeks…"));
    date_content.append(&date_entry);
    let date_preview = gtk::Button::with_label(&format!(
        "Go to {}  ↵",
        glib::DateTime::now_local()
            .unwrap()
            .format("%a, %-d %B %Y")
            .unwrap()
    ));
    date_preview.add_css_class("calendar-date-preview");
    date_content.append(&date_preview);
    let browse = gtk::Expander::new(Some("Browse calendar"));
    browse.set_child(Some(&jump_calendar));
    date_content.append(&browse);
    let date_hint = text("Return to jump · Tab to browse days · Esc to close", false);
    date_hint.set_selectable(false);
    date_hint.set_max_width_chars(38);
    date_content.append(&date_hint);
    let date_examples = text("Try 20 October · +2w · next Friday", false);
    date_examples.set_selectable(false);
    date_content.append(&date_examples);
    date_popup.set_child(Some(&date_content));
    go_date.set_popover(Some(&date_popup));
    heading.insert_child_after(&go_date, Some(&date_label));
    let help = gtk::MenuButton::new();
    help.set_label("?");
    help.set_tooltip_text(Some("Calendar shortcuts · ?"));
    let help_popup = gtk::Popover::new();
    let help_content = gtk::Box::new(Orientation::Vertical, 14);
    help_content.add_css_class("calendar-shortcuts");
    help_content.set_margin_top(20);
    help_content.set_margin_bottom(20);
    help_content.set_margin_start(20);
    help_content.set_margin_end(20);
    help_content.set_width_request(330);
    help_content.append(&text("Calendar shortcuts", true));
    for (section, rows) in [
        (
            "Move through time",
            vec![
                ("Previous / next period", "H  L"),
                ("Today", "T"),
                ("Go to date", "B"),
            ],
        ),
        (
            "Change view",
            vec![("Day · Week · Month · Year", "D  W  M  Y")],
        ),
        (
            "Work with events",
            vec![
                ("Previous / next event", "K  J"),
                ("Open selected event", "Enter"),
                ("Edit event details", "E"),
                ("New event", "C"),
                ("Search events", "/"),
            ],
        ),
        (
            "Get around",
            vec![
                ("Scroll half a page", "Ctrl D / U"),
                ("Inbox / Calendar", "G I / G C"),
                ("Close / go back", "Esc"),
            ],
        ),
    ] {
        let caption = text(section, false);
        caption.add_css_class("calendar-rail-caption");
        help_content.append(&caption);
        for (title, key) in rows {
            let row = gtk::Box::new(Orientation::Horizontal, 18);
            let label = text(title, false);
            label.remove_css_class("dim-label");
            label.set_hexpand(true);
            row.append(&label);
            let keys = gtk::Label::new(Some(key));
            keys.add_css_class("calendar-keycap");
            row.append(&keys);
            help_content.append(&row);
        }
    }
    help_popup.set_child(Some(&help_content));
    help.set_popover(Some(&help_popup));
    rail.append(&help);
    let refresh = icon_button("view-refresh-symbolic", "Refresh calendar");
    rail.append(&refresh);
    let bookings = gtk::Button::with_label("Bookings and links");
    bookings.set_visible(booking::configured());
    {let ui = ui.clone(); bookings.connect_clicked(move |_|booking::open(&ui,None));}
    let free = gtk::Button::with_label("Find free time");
    free.add_css_class("flat");
    free.set_tooltip_text(Some("Find available 30-minute slots"));
    rail.append(&bookings);
    rail.append(&free);
    main.append(&heading);
    let secondary_header = gtk::Box::new(Orientation::Horizontal, 6);
    secondary_header.add_css_class("calendar-header");
    secondary_header.add_css_class("calendar-header-secondary");
    secondary_header.set_visible(false);
    main.append(&secondary_header);
    let back = gtk::Button::with_label("‹ Back to calendar");
    back.set_halign(gtk::Align::Start);
    back.set_visible(false);
    main.append(&back);
    let rows = gtk::Box::new(Orientation::Vertical, 0);
    rows.set_vexpand(true);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&rows));
    main.append(&scroll);
    let status = text("Loading saved calendars…", false);
    status.add_css_class("agenda-status");
    status.set_wrap(false);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.set_tooltip_text(Some("Saved events remain available offline"));
    main.append(&status);
    let footer = text(CALENDAR_KEYS, false);
    footer.add_css_class("agenda-footer");
    main.append(&footer);
    body.append(&main);
    root.append(&body);
    let narrow = Rc::new(Cell::new(false));
    let toolbar_narrow = Rc::new(Cell::new(false));
    root.add_tick_callback({
        let rail = rail.clone();
        let rail_slot = rail_slot.clone();
        let source_popup = source_popup.clone();
        let source_menu = source_menu.clone();
        let create = create.clone();
        let today = today.clone();
        let prev = prev.clone();
        let next = next.clone();
        let search = search.clone();
        let heading = heading.clone();
        let secondary_header = secondary_header.clone();
        let compact_mode = compact_mode.clone();
        move |root, _| {
            let compact = root.width() < 1000;
            if narrow.replace(compact) != compact {
                if compact {
                    rail_slot.remove(&rail);
                    source_popup.set_child(Some(&rail));
                } else {
                    source_popup.popdown();
                    source_popup.set_child(None::<&gtk::Widget>);
                    rail_slot.append(&rail);
                }
                rail_slot.set_visible(!compact);
                source_menu.set_visible(compact);
            }
            let split = root.width() < 760;
            if toolbar_narrow.replace(split) != split {
                for button in [&today, &prev, &next, &search, &create] {
                    if split {
                        heading.remove(button);
                        secondary_header.append(button);
                    } else {
                        secondary_header.remove(button);
                        heading.append(button);
                    }
                }
                // Keep the view picker next to the date in the first row.
                if !split {
                    heading.reorder_child_after(&compact_mode, Some(&next));
                }
                secondary_header.set_visible(split);
            }
            create.set_label(if root.width() < 400 { "+" } else { "New event" });
            glib::ControlFlow::Continue
        }
    });
    ui.calendar_host.append(&root);
    let now = glib::DateTime::now_local().unwrap();
    let day =
        glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 0, 0, 0.).unwrap();
    let agenda = Rc::new(Agenda {
        ui: ui.clone(),
        window: window.clone(),
        accounts: records,
        account: account.clone(),
        date: RefCell::new(day),
        date_label,
        status,
        rows,
        connect: connect.clone(),
        refresh: refresh.clone(),
        calendars: RefCell::new(Vec::new()),
        generation: Cell::new(0),
        busy: Cell::new(false),
        connected_accounts: RefCell::new(Vec::new()),
        mode: Cell::new(CalendarMode::Week),
        mode_buttons: mode_buttons.clone(),
        mode_menu: compact_mode.clone(),
        mini_calendar: mini_calendar.clone(),
        updating_mini: Cell::new(false),
        hour_height: Cell::new(58),
        detail_popup: RefCell::new(None),
        editor_popup: RefCell::new(None),
        create_button: create.clone(),
        legend,
        legend_scroll,
        hidden_calendars: RefCell::new(std::collections::HashSet::new()),
        all_events: RefCell::new(Vec::new()),
        live_draft: Rc::new(RefCell::new(None)),
        draft_canvases: Rc::new(RefCell::new(Vec::new())),
        date_popup,
        date_entry: date_entry.clone(),
        help_popup,
        source_popup: source_popup.clone(),
        back: back.clone(),
        scroll: scroll.clone(),
        footer,
        events: RefCell::new(Vec::new()),
        event_controls: RefCell::new(Vec::new()),
        selected: Cell::new(0),
        sync_successes: RefCell::new(Vec::new()),
        sync_attempts: RefCell::new(Vec::new()),
    });
    {
        let this = agenda.clone();
        back.connect_clicked(move |_| this.reload());
    }
    {
        let this = agenda.clone();
        prev.connect_clicked(move |_| this.move_day(-1));
    }
    {
        let this = agenda.clone();
        next.connect_clicked(move |_| this.move_day(1));
    }
    {
        let this = agenda.clone();
        today.connect_clicked(move |_| this.go_today());
    }
    {
        let this = agenda.clone();
        account.connect_selected_notify(move |_| this.reload());
    }
    {
        let this = agenda.clone();
        connect.connect_clicked(move |_| this.connect());
    }
    {
        let this = agenda.clone();
        refresh.connect_clicked(move |_| this.sync());
    }
    {
        let this = agenda.clone();
        free.connect_clicked(move |_| this.availability());
    }
    {
        let this = agenda.clone();
        compact_mode.connect_selected_notify(move |dropdown| {
            let mode = [
                CalendarMode::Day,
                CalendarMode::Week,
                CalendarMode::Month,
                CalendarMode::Year,
            ][dropdown.selected().min(3) as usize];
            if this.mode.get() != mode {
                this.set_mode(mode);
            }
        });
    }
    for (index, button) in mode_buttons.iter().enumerate() {
        let compact_mode = compact_mode.clone();
        button.connect_toggled(move |button| {
            if button.is_active() {
                compact_mode.set_selected(index as u32);
            }
        });
    }
    for (button, mode) in mode_buttons.into_iter().zip([
        CalendarMode::Day,
        CalendarMode::Week,
        CalendarMode::Month,
        CalendarMode::Year,
    ]) {
        let this = agenda.clone();
        button.connect_toggled(move |button| {
            if button.is_active() && this.mode.replace(mode) != mode {
                this.reload();
            }
        });
    }
    {
        let weak = Rc::downgrade(&agenda);
        let mini = jump_calendar.clone();
        let entry = date_entry.clone();
        agenda.date_popup.connect_show(move |_| {
            if let Some(this) = weak.upgrade() {
                mini.select_day(&this.date.borrow());
                entry.set_text("");
                entry.grab_focus();
                let entry = entry.clone();
                glib::idle_add_local_once(move || {
                    entry.grab_focus();
                });
            }
        });
    }
    {
        let entry = date_entry.clone();
        jump_calendar.connect_day_selected(move |calendar| {
            entry.set_text(&calendar.date().format("%Y-%m-%d").unwrap());
        });
    }
    {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let this = agenda.clone();
        let browse = browse.clone();
        let mini = jump_calendar.clone();
        let entry = date_entry.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::Tab
                && !mods.contains(gdk::ModifierType::SHIFT_MASK)
                && entry.has_focus()
            {
                browse.set_expanded(true);
                mini.grab_focus();
                return glib::Propagation::Stop;
            }
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
                this.apply_date();
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::Escape {
                this.date_popup.popdown();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        agenda.date_popup.add_controller(keys);
    }
    {
        let preview = date_preview.clone();
        date_entry.connect_changed(move |entry| {
            let now = glib::DateTime::now_local().unwrap();
            let value = entry.text();
            let result = parse_calendar_date(
                if value.trim().is_empty() {
                    "today"
                } else {
                    value.as_str()
                },
                &now,
            );
            preview.set_sensitive(result.is_some());
            preview.set_label(
                &result
                    .map(|day| format!("Go to {}  ↵", day.format("%a, %-d %B %Y").unwrap()))
                    .unwrap_or_else(|| {
                        if entry.text().is_empty() {
                            "Type a date to jump".into()
                        } else {
                            "Date not understood — try 20 October".into()
                        }
                    }),
            );
        });
        let this = agenda.clone();
        date_preview.connect_clicked(move |_| this.apply_date());
    }
    {
        let this = agenda.clone();
        date_entry.connect_activate(move |_| this.apply_date());
    }
    {
        let this = agenda.clone();
        create.connect_clicked(move |_| this.new_event());
    }
    *ui.calendar.borrow_mut() = Some(agenda.clone());
    window.present();
    ui.calendar_host.grab_focus();
    if agenda.accounts.is_empty() {
        agenda
            .status
            .set_text("Add a Gmail account in Email to connect your calendar.");
        connect.set_sensitive(false);
        refresh.set_sensitive(false);
    } else {
        agenda.reload();
    }
}

#[cfg(test)]
mod calendar_grid_tests {
    use super::*;
    fn event(start: f64, end: f64) -> GridEvent {
        GridEvent {
            event: Value::Null,
            start,
            end,
            lane: 0,
            lanes: 1,
        }
    }
    #[test]
    fn overlapping_events_get_separate_lanes_and_consecutive_reuse_width() {
        let layout = arrange_events(vec![
            event(540., 600.),
            event(570., 630.),
            event(660., 690.),
        ]);
        assert_eq!((layout[0].lane, layout[0].lanes), (0, 2));
        assert_eq!((layout[1].lane, layout[1].lanes), (1, 2));
        assert_eq!((layout[2].lane, layout[2].lanes), (0, 1));
    }
    #[test]
    fn month_day_membership_uses_exclusive_end_boundaries() {
        let day = glib::DateTime::from_utc(2026, 11, 9, 0, 0, 0.).unwrap();
        let start = day.to_unix() * 1000;
        let event =
            json!({"id":"all-day","all_day":true,"start_ms":start,"end_ms":start+86_400_000});
        let entries = [OverviewEntry {
            event: &event,
            start,
            end: start + 86_400_000,
            all_day: true,
        }];
        assert_eq!(Agenda::events_on(&entries, &day).len(), 1);
        assert!(Agenda::events_on(&entries, &day.add_days(1).unwrap()).is_empty());
    }
    #[test]
    fn jump_date_accepts_relative_dates_and_weekdays() {
        let today = glib::DateTime::from_local(2026, 11, 9, 0, 0, 0.).unwrap();
        for (value, expected) in [
            ("today", 9),
            ("tomorrow", 10),
            ("next week", 16),
            ("Friday", 13),
            ("next Monday", 16),
            ("2026-11-20", 20),
        ] {
            assert_eq!(
                parse_calendar_date(value, &today).unwrap().day_of_month(),
                expected
            );
        }
        for (value, expected) in [
            ("20 october", "2026-10-20"),
            ("October 20 2027", "2027-10-20"),
            ("go to 20 October", "2026-10-20"),
            ("in 2 weeks", "2026-11-23"),
            ("+2w", "2026-11-23"),
            ("3 days ago", "2026-11-06"),
            ("last Friday", "2026-11-06"),
            ("next month", "2026-12-09"),
            ("20.10.2026", "2026-10-20"),
        ] {
            assert_eq!(
                parse_calendar_date(value, &today)
                    .unwrap()
                    .format("%Y-%m-%d")
                    .unwrap()
                    .as_str(),
                expected,
                "{value}"
            );
        }
        for value in [
            "nonsense",
            "32 october",
            "2026-02-30",
            "31.02.2026",
            "in 999999999 weeks",
            "-2147483648d",
            "20junk october",
            "0 october",
        ] {
            assert!(parse_calendar_date(value, &today).is_none(), "{value}");
        }
    }
    #[test]
    fn quick_event_preserves_title_and_parses_date_clock_duration() {
        let today = glib::DateTime::from_local(2026, 11, 9, 13, 0, 0.).unwrap();
        let selected = today.add_days(7).unwrap();
        assert!(
            parse_quick_calendar_event("jan 8-12 bbq w/ Sam", &selected, &today)
                .err()
                .unwrap()
                .contains("Is this January 8–12")
        );
        for (raw, title, day, hour, minute, duration) in [
            ("Lunch tomorrow at 12 for 1h", "Lunch", 10, 12, 0, 60),
            (
                "Design Review next Friday at 2:30pm for 1.5h",
                "Design Review",
                13,
                14,
                30,
                90,
            ),
            ("Coffee at 12am", "Coffee", 16, 0, 0, 30),
            ("Coffee at 12pm", "Coffee", 16, 12, 0, 30),
            ("lunch 12h", "lunch", 16, 12, 0, 30),
            ("lunch tomorrow 12h", "lunch", 10, 12, 0, 30),
            ("lunch 12:30", "lunch", 16, 12, 30, 30),
            ("lunch 12pm", "lunch", 16, 12, 0, 30),
            ("lunch 12 pm", "lunch", 16, 12, 0, 30),
            ("lunch at 12h", "lunch", 16, 12, 0, 30),
            ("lunch tomorrow 12h for1h", "lunch", 10, 12, 0, 60),
            (
                "Birthday 20 November at 18 for 45 minutes",
                "Birthday",
                20,
                18,
                0,
                45,
            ),
        ] {
            let event = parse_quick_calendar_event(raw, &selected, &today)
                .unwrap()
                .unwrap();
            assert_eq!(
                (
                    event.title.as_str(),
                    event.start.day_of_month(),
                    event.start.hour(),
                    event.start.minute(),
                    event.minutes
                ),
                (title, day, hour, minute, duration),
                "{raw}"
            );
        }
        assert!(
            parse_quick_calendar_event("Ordinary title", &selected, &today)
                .unwrap()
                .is_none()
        );
        assert!(
            parse_quick_calendar_event("Meeting at home", &selected, &today)
                .unwrap()
                .is_none()
        );
        for raw in [
            "Lunch tomorrow at 25",
            "lunch 25h",
            "lunch tomorrow 25h",
            "Lunch at 12:70",
            "Lunch at 0pm",
            "Lunch at 13am",
            "Lunch at 12 for 1m",
            "Lunch at 12 for 25h",
            "Lunch at 12 for NaNh",
            "Lunch at 12 for 99999999999999999999999999999h",
            "Lunch 32 October at 12",
            "Lunch 2026-02-30 at 12",
        ] {
            assert!(
                parse_quick_calendar_event(raw, &selected, &today).is_err(),
                "{raw}"
            );
        }
    }
    #[test]
    fn submission_preserves_manual_times_after_natural_query() {
        let today = glib::DateTime::from_local(2026, 11, 9, 0, 0, 0.).unwrap();
        let query = "Lunch tomorrow 12h for1h";
        let parsed = parse_quick_calendar_event(query, &today, &today)
            .unwrap()
            .unwrap();
        assert_eq!(parsed.start.hour(), 12);
        let (title, start, end) = validated_calendar_draft(
            query,
            Some(&parsed.title),
            "2026-11-10",
            "14:30",
            "2026-11-10",
            "15:45",
        )
        .unwrap();
        assert_eq!(title, "Lunch");
        assert_eq!(
            glib::DateTime::from_unix_local(start / 1000)
                .unwrap()
                .format("%H:%M")
                .unwrap()
                .as_str(),
            "14:30"
        );
        assert_eq!(end - start, 75 * 60 * 1000);
        let plain = validated_calendar_draft(
            "Design review",
            None,
            "2026-11-12",
            "09:00",
            "2026-11-12",
            "09:30",
        )
        .unwrap();
        assert_eq!(plain.0, "Design review");
        assert!(validated_calendar_draft(
            "Lunch",
            Some("Lunch"),
            "2026-11-10",
            "14:30",
            "2026-11-10",
            "14:00"
        )
        .is_err());
    }
    #[test]
    fn short_event_hit_targets_do_not_cover_another_event() {
        let layout = arrange_events(vec![event(540., 545.), event(550., 560.)]);
        assert_ne!(layout[0].lane, layout[1].lane);
    }
}
