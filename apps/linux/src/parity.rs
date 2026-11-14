//! Desktop interaction contract shared with the Mac shell: native controls, no web app shell.
use super::*;

#[derive(Clone)]
pub(super) struct Command {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub keys: String,
    pub words: String,
}
impl Command {
    pub(super) fn new(id: &str, title: &str, keys: &str, words: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            detail: String::new(),
            keys: keys.into(),
            words: words.into(),
        }
    }
    pub(super) fn detail(mut self, text: impl Into<String>) -> Self {
        self.detail = text.into();
        self
    }
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, c) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, d) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(above + 1)
                .min(diagonal + usize::from(c != *d));
            diagonal = above;
        }
    }
    row[b.len()]
}
fn score(item: &Command, query: &str) -> Option<usize> {
    let text = format!("{} {} {}", item.title, item.words, item.detail).to_lowercase();
    let mut total = 0;
    for token in query.split_whitespace() {
        let best = text
            .split_whitespace()
            .map(|word| {
                if word == token {
                    0
                } else if word.starts_with(token) {
                    1
                } else if word.contains(token) {
                    2
                } else if token.chars().count() >= 3 {
                    let d = distance(token, word);
                    if d <= 2 {
                        4 + d
                    } else {
                        100
                    }
                } else {
                    100
                }
            })
            .min()
            .unwrap_or(100);
        if best == 100 {
            return None;
        }
        total += best;
    }
    Some(total)
}
fn calendar_reminder(input: &str, now: &glib::DateTime) -> Option<(i64, String, String)> {
    let lower = input.to_lowercase();
    let mut words: Vec<&str> = lower.split_whitespace().collect();
    if words.first().is_some_and(|w| ["remind", "snooze"].iter().any(|v| distance(w, v) <= 2 && w.len() >= 4)) {
        words.remove(0);
        if words.first() == Some(&"me") { words.remove(0); }
        if words.first().is_some_and(|w| ["in", "for", "at", "on"].contains(w)) { words.remove(0); }
    }
    let query = words.join(" ");
    let name = ["tonight", "this evening", "tomorrow morning", "tomorrow evening", "tomorrow", "today"]
        .into_iter().find(|name| query == *name || query.starts_with(&format!("{name} ")))?;
    let clock = query[name.len()..].trim().trim_start_matches("at ").replace(' ', "");
    let explicit = !clock.is_empty();
    let mut hour = if name == "tonight" || name.contains("evening") { 20 } else { 9 };
    let mut minute = 0;
    if explicit {
        let meridiem = if clock.ends_with("am") { "am" } else if clock.ends_with("pm") { "pm" } else { "" };
        let value = clock.strip_suffix(meridiem).unwrap_or(&clock);
        let parts: Vec<_> = value.split(':').collect();
        if parts.is_empty() || parts.len() > 2 { return None; }
        let h: i32 = parts[0].parse().ok()?;
        let m: i32 = if parts.len() == 2 { parts[1].parse().ok()? } else { 0 };
        if !(0..=59).contains(&m) || !(if meridiem.is_empty() { 0..=23 } else { 1..=12 }).contains(&h) { return None; }
        hour = if meridiem.is_empty() { h } else { h % 12 + if meridiem == "pm" { 12 } else { 0 } };
        minute = m;
    }
    let day = now.add_days(if name.starts_with("tomorrow") { 1 } else { 0 }).ok()?;
    let mut date = glib::DateTime::new(&now.timezone(), day.year(), day.month(), day.day_of_month(), hour, minute, 0.).ok()?;
    if date.to_unix() <= now.to_unix() {
        if explicit || !["tonight", "this evening"].contains(&name) || now.hour() >= 23 { return None; }
        date = glib::DateTime::new(&now.timezone(), day.year(), day.month(), day.day_of_month(), now.hour()+1, 0, 0.).ok()?;
    }
    let label = if name.starts_with("tomorrow") { "Tomorrow" } else if ["tonight", "this evening"].contains(&name) { "Tonight" } else { "Today" };
    Some((date.to_unix()*1000, format!("{label} at {}", date.format("%H:%M").ok()?), format!("Until {} · Enter to confirm", date.format("%d %b %Y, %H:%M").ok()?)))
}

pub(super) fn duration_query(input: &str) -> Option<(u64, String)> {
    let lower = input.trim().to_lowercase();
    let mut words: Vec<&str> = lower.split_whitespace().collect();
    if words.first().is_some_and(|w| {
        ["snooze", "remind"]
            .iter()
            .any(|v| distance(w, v) <= 2 && w.len() >= 4)
    }) {
        words.remove(0);
    }
    words.retain(|w| !["me", "in", "for"].contains(w));
    if words.is_empty() || words.len() > 2 {
        return None;
    }
    let digits: String = words[0].chars().take_while(char::is_ascii_digit).collect();
    let amount: u64 = digits.parse().ok()?;
    let suffix = if words.len() == 2 {
        words[1]
    } else {
        &words[0][digits.len()..]
    };
    let (unit, name) = match suffix {
        "" | "m" | "min" | "mins" | "minute" | "minutes" => (60, "minute"),
        "h" | "hr" | "hrs" | "hour" | "hours" => (3600, "hour"),
        "d" | "day" | "days" => (86400, "day"),
        "w" | "week" | "weeks" => (604800, "week"),
        _ => return None,
    };
    let seconds = amount.checked_mul(unit)?;
    if seconds == 0 || seconds > 366 * 86400 {
        return None;
    }
    Some((
        seconds,
        format!("In {amount} {name}{}", if amount == 1 { "" } else { "s" }),
    ))
}
pub(super) fn address(sender: &str) -> String {
    sender
        .split_once('<')
        .map(|(_, r)| r.split('>').next().unwrap_or(r))
        .unwrap_or(sender)
        .trim()
        .to_lowercase()
}
fn plain_label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class(class);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

/// True when a composer that opened with recipients now has none in To, Cc or Bcc.
pub(super) fn recipients_cleared(initial: [&str; 3], current: [&str; 3]) -> bool {
    let any = |fields: [&str; 3]| fields.iter().any(|field| !field.trim().is_empty());
    any(initial) && !any(current)
}

pub(super) fn draft_recipient_summary(to: &str, cc: &str, bcc: &str) -> String {
    for (label, value) in [("To", to), ("Cc", cc), ("Bcc", bcc)] {
        if !value.trim().is_empty() {
            return format!("{label}: {value}");
        }
    }
    "No recipient".into()
}

pub(super) const SEARCH_HELP: &str = "Search downloaded mail only; unsynced messages are excluded. Current mailbox, account and filter stay active.\n\nfrom:person@example.com · to:team@example.com · cc:person@example.com\nsubject:\"project update\" · label:\"Customer Support\"\nis:unread · is:read · is:starred · has:attachment\nin:archive · before:2026-11-01 · after:2026-10-01\n\nUse Archive to search archived mail. Conflicting scopes return no matches. Esc clears search and returns to J/K navigation.";

pub(super) fn quote_search_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Context is independent of query text: operators cannot discard these restrictions.
pub(super) fn thread_search_request(
    view: &MailView,
    query: &str,
    filter: u8,
    limit: u32,
    offset: u32,
) -> Value {
    let mut request = json!({"op":"search", "query":query, "limit":limit, "offset":offset, "unread_only":filter==1, "starred_only":filter==2});
    match view {
        MailView::Account(id) => {
            request["account_id"] = json!(id);
            request["is_inbox"] = json!(true);
        }
        MailView::Split(id) => {
            request["split_id"] = json!(id);
            request["is_inbox"] = json!(true);
        }
        MailView::Label(name) => request["label"] = json!(name),
        MailView::Inbox if filter == 2 => {}
        MailView::Archive => request["folder"] = json!("archive"),
        MailView::Sent => request["folder"] = json!("sent"),
        MailView::Snoozed => request["snoozed_only"] = json!(true),
        MailView::Followups => request["followups_only"] = json!(true),
        _ => request["is_inbox"] = json!(true),
    }
    request
}

pub(super) fn search_thread_pages(
    service: &MailService,
    mut request: Value,
    target: u32,
) -> Result<Vec<ThreadSummary>, String> {
    let mut rows = Vec::new();
    while rows.len() < target as usize {
        let page_size = (target as usize - rows.len()).min(500);
        request["limit"] = json!(page_size);
        request["offset"] = json!(rows.len());
        let value = service
            .request_json(&request.to_string())
            .map_err(|e| e.to_string())?;
        let page: Vec<ThreadSummary> = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let count = page.len();
        rows.extend(page);
        if count < page_size {
            break;
        }
    }
    Ok(rows)
}

impl Ui {
    pub(super) fn schedule_search(self: &Rc<Self>) {
        let generation = self.search_generation.get().wrapping_add(1);
        self.search_generation.set(generation);
        self.search_preview.borrow_mut().take();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if ui.search_generation.get() != generation || matches!(*ui.view.borrow(), MailView::AgentRequests) {
                return;
            }
            if matches!(*ui.view.borrow(), MailView::Drafts | MailView::Outbox) {
                ui.refresh();
                return;
            }
            ui.status.set_text("Searching downloaded mail…");
            let scope = ui.search_scope_key();
            let request = thread_search_request(
                &ui.view.borrow(),
                ui.search.text().as_str(),
                ui.inbox_filter.get(),
                500,
                0,
            );
            let target = ui.loaded_limit.get();
            let path = ui.database_path.clone();
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let result = MailService::open(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|service| search_thread_pages(&service, request, target));
                let _ = tx.send(result);
            });
            let weak = Rc::downgrade(&ui);
            glib::timeout_add_local(std::time::Duration::from_millis(35), move || {
                let Some(ui) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if ui.search_generation.get() != generation || ui.search_scope_key() != scope {
                    return glib::ControlFlow::Break;
                }
                match rx.try_recv() {
                    Ok(result) => {
                        *ui.search_preview.borrow_mut() = Some((scope.clone(), result));
                        ui.refresh();
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(_) => {
                        ui.status
                            .set_text("Search worker stopped. Edit the query to retry.");
                        glib::ControlFlow::Break
                    }
                }
            });
        });
    }
    /// Agent requests have no search field; searching goes back to the inbox.
    pub(super) fn focus_search(self: &Rc<Self>) {
        if matches!(*self.view.borrow(), MailView::AgentRequests) {
            self.set_view(MailView::Inbox);
        }
        self.search.grab_focus();
    }
    pub(super) fn search_filters(self: &Rc<Self>) {
        if matches!(*self.view.borrow(), MailView::AgentRequests) {
            self.set_view(MailView::Inbox);
        }
        if matches!(*self.view.borrow(), MailView::Drafts | MailView::Outbox) {
            self.status
                .set_text("Open a mailbox to use structured search filters.");
            return;
        }
        let initial = search_filter_rules::decompose(self.search.text().as_str());
        let dialog = gtk::Window::builder()
            .title("Refine search")
            .transient_for(&self.window)
            .modal(true)
            .default_width(440)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 12);
        content.set_margin_top(20);
        content.set_margin_bottom(20);
        content.set_margin_start(20);
        content.set_margin_end(20);
        let heading = gtk::Label::new(Some("Refine search"));
        heading.set_xalign(0.0);
        heading.add_css_class("section-title");
        content.append(&heading);
        let hint = gtk::Label::new(Some(
            "Downloaded mail in your current view. Existing filters are filled in; other search terms stay unchanged.",
        ));
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        hint.add_css_class("hint");
        content.append(&hint);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(10);
        grid.set_column_spacing(12);
        let mut entries = Vec::new();
        for (index, (key, title, placeholder)) in [
            ("from", "From", "Name or email"),
            ("to", "To", "Recipient or alias"),
            ("cc", "Cc", "Copied recipient"),
            ("subject", "Subject", "Words or phrase"),
            ("after", "On or after", "YYYY-MM-DD"),
            ("before", "Before", "YYYY-MM-DD"),
        ]
        .iter()
        .enumerate()
        {
            let label = gtk::Label::new(Some(title));
            label.set_xalign(0.0);
            label.add_css_class("hint");
            let entry = gtk::Entry::new();
            entry.set_placeholder_text(Some(placeholder));
            entry.set_hexpand(true);
            entry.set_text(initial.fields.get(*key).map(String::as_str).unwrap_or(""));
            grid.attach(&label, 0, index as i32, 1, 1);
            grid.attach(&entry, 1, index as i32, 1, 1);
            entries.push((key.to_string(), entry));
        }
        content.append(&grid);
        let options = gtk::Box::new(Orientation::Horizontal, 16);
        let read = gtk::DropDown::from_strings(&["Any read status", "Unread", "Read"]);
        read.set_selected(initial.read as u32);
        options.append(&read);
        let attachments = gtk::CheckButton::with_label("Has attachments");
        attachments.set_active(initial.attachments);
        options.append(&attachments);
        content.append(&options);
        let preview = gtk::Label::new(None);
        preview.set_wrap(true);
        preview.set_xalign(0.0);
        preview.add_css_class("hint");
        preview.set_max_width_chars(52);
        preview.set_lines(3);
        preview.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.append(&preview);
        let footer = gtk::Box::new(Orientation::Horizontal, 8);
        footer.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_label("Cancel");
        let apply = gtk::Button::with_label("Apply filters");
        footer.append(&cancel);
        footer.append(&apply);
        content.append(&footer);
        let fields = entries.clone();
        let read_value = read.clone();
        let attachments_value = attachments.clone();
        let base = initial.remainder.clone();
        let build: Rc<dyn Fn() -> Result<String, String>> = Rc::new(move || {
            let values = fields
                .iter()
                .map(|(key, entry)| (key.clone(), entry.text().trim().to_string()))
                .collect();
            search_filter_rules::query(
                &base,
                &values,
                read_value.selected() as usize,
                attachments_value.is_active(),
            )
        });
        let values = entries.clone();
        let read_value = read.clone();
        let attachments_value = attachments.clone();
        let p = preview.clone();
        let a = apply.clone();
        let query = build.clone();
        let update: Rc<dyn Fn()> = Rc::new(move || match query() {
            Ok(query) => {
                p.set_text(if query.is_empty() {
                    "Choose filters above · Tab moves between fields"
                } else {
                    &query
                });
                let changed = values.iter().any(|(key, entry)| {
                    entry.text().trim() != initial.fields.get(key).map(String::as_str).unwrap_or("")
                }) || read_value.selected() as usize != initial.read
                    || attachments_value.is_active() != initial.attachments;
                a.set_sensitive(changed);
            }
            Err(error) => {
                p.set_text(&error);
                a.set_sensitive(false);
            }
        });
        for (_, entry) in &entries {
            let update = update.clone();
            entry.connect_changed(move |_| update());
        }
        let change = update.clone();
        read.connect_selected_notify(move |_| change());
        let change = update.clone();
        attachments.connect_toggled(move |_| change());
        update();
        let d = dialog.clone();
        cancel.connect_clicked(move |_| d.close());
        let ui = self.clone();
        let d = dialog.clone();
        let query = build.clone();
        apply.connect_clicked(move |button| {
            if button.is_sensitive() {
                if let Ok(query) = query() {
                    ui.search.set_text(&query);
                    ui.list.grab_focus();
                    d.close();
                }
            }
        });
        dialog.set_default_widget(Some(&apply));
        let keys = gtk::EventControllerKey::new();
        let d = dialog.clone();
        let a = apply.clone();
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape => {
                d.close();
                glib::Propagation::Stop
            }
            gdk::Key::Return => {
                if a.is_sensitive() {
                    a.emit_clicked();
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        dialog.add_controller(keys);
        dialog.set_child(Some(&content));
        dialog.present();
        if let Some((_, first)) = entries.first() {
            first.grab_focus();
        }
    }
    fn search_help(&self) {
        let dialog = gtk::Window::builder()
            .title("Search syntax and coverage")
            .transient_for(&self.window)
            .modal(true)
            .default_width(540)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 16);
        content.set_margin_top(24);
        content.set_margin_bottom(24);
        content.set_margin_start(24);
        content.set_margin_end(24);
        let label = gtk::Label::new(Some(SEARCH_HELP));
        label.set_wrap(true);
        label.set_xalign(0.0);
        label.set_selectable(true);
        content.append(&label);
        let close = gtk::Button::with_label("Close");
        let d = dialog.clone();
        close.connect_clicked(move |_| d.close());
        content.append(&close);
        let keys = gtk::EventControllerKey::new();
        let d = dialog.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                d.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.add_controller(keys);
        dialog.set_child(Some(&content));
        dialog.present();
    }
}

impl Ui {
    pub(super) fn set_shortcuts(&self, reading: bool) {
        self.shortcut_bar.set_text(if !self.marked.borrow().is_empty() && !reading {
            "X  Select     Shift J K  Extend     M U  Read / unread     E  Archive     Esc  Clear"
        } else if reading {
            "READING     J K  Scroll     Ctrl D/U  Half page     H L  History     Esc  Inbox"
        } else if matches!(*self.view.borrow(), MailView::Drafts) {
            if self.last_discarded_draft.borrow().is_some() {
                "J K  Next / previous     ↵  Edit draft     Delete  Discard     Z  Undo discard     Ctrl K  Commands"
            } else {
                "J K  Next / previous     ↵  Edit draft     Delete  Discard     C  New message     Ctrl K  Commands"
            }
        } else {
            "J K  Next / previous     ↵  Read     R  Reply     E  Archive     Ctrl K  Commands"
        });
    }
    pub(super) fn open_palette(self: &Rc<Self>, page: &str) {
        *self.palette_page.borrow_mut() = page.into();
        self.command.set_text("");
        self.update_palette();
        self.palette.present();
        self.command.grab_focus();
        self.check_saved_open_activity(); // tracked mail can arrive after launch
    }
    pub(super) fn update_palette(self: &Rc<Self>) {
        let query = self.command.text().trim().to_lowercase();
        let page = self.palette_page.borrow().clone();
        self.palette_heading.set_text(match page.as_str() {
            "snooze" => "Snooze until",
            "labels" => "Labels",
            "accounts" => "Switch account",
            "snippets" => "Insert snippet",
            "calendar-search" => "Search calendar",
            _ => "Commands",
        });
        let mut items = vec![Command::new("bookings", "Bookings and meeting links", "", "booking analytics scheduling")];
        if self.offers_open_activity.get() {
            items.push(Command::new("tracking activity", "Recent opens", "", "sent tracking image requests analytics"));
        }
        if self.workspace.visible_child_name().as_deref() == Some("calendar") {
            if let Some(calendar) = self.calendar.borrow().as_ref() {
                items = calendar.command_items(page == "calendar-search");
            }
        } else if self.compose_pane.borrow().is_some() && page == "root" {
            for (id, title, keys, words) in [
                ("send", "Send message", "Ctrl Enter", "deliver"),
                ("body", "Write message", "", "body text"),
                (
                    "from",
                    "Choose From address",
                    "",
                    "sender identity alias send as",
                ),
                ("to", "Edit recipient", "", "address"),
                ("cc", "Edit Cc recipients", "", "copy recipients"),
                ("bcc", "Edit Bcc recipients", "", "blind hidden recipients"),
                ("subject", "Edit subject", "", "title"),
                ("attach", "Attach file…", "", "attachment upload"),
                (
                    "snippets",
                    "Insert snippet…",
                    "",
                    "template saved reply reuse text",
                ),
                (
                    "save snippet",
                    "Save message as snippet…",
                    "",
                    "create template save reply",
                ),
                ("schedule", "Schedule send…", "", "send later"),
                ("signature", "Signature…", "", "identity footer signatures"),
                ("booking", "Insert booking link…", "", "meeting schedule availability"),
                ("tracking", "Toggle open tracking", "", "optional email open image privacy"),
                ("save", "Save draft", "Ctrl S", ""),
                ("discard", "Discard draft", "", "delete trash remove throw away"),
                ("close", "Save and close", "Esc", ""),
            ] {
                if id == "tracking" && !super::OPEN_TRACKING {
                    continue;
                }
                items.push(Command::new(&format!("composer:{id}"), title, keys, words));
            }
        } else if page == "snippets" {
            if let Ok(value) = self.request(json!({"op":"list_snippets"})) {
                for snippet in value.as_array().into_iter().flatten() {
                    if let (Some(id), Some(name)) =
                        (snippet["id"].as_str(), snippet["name"].as_str())
                    {
                        let preview = snippet["body"]
                            .as_str()
                            .unwrap_or("")
                            .replace('\n', " ")
                            .chars()
                            .take(100)
                            .collect::<String>();
                        items.push(
                            Command::new(
                                &format!("snippet:{id}"),
                                name,
                                "",
                                "template saved reply",
                            )
                            .detail(preview),
                        );
                    }
                }
            }
        } else if page == "root" && !self.marked.borrow().is_empty() {
            let count = self.marked.borrow().len();
            for (id, title, keys) in [
                ("mark read", format!("Mark {count} selected as read"), "M"),
                (
                    "mark unread",
                    format!("Mark {count} selected as unread"),
                    "U",
                ),
                (
                    "archive",
                    if matches!(*self.view.borrow(), MailView::Archive) {
                        format!("Move {count} selected to Inbox")
                    } else {
                        format!("Archive {count} selected")
                    },
                    "E",
                ),
                ("spam", format!("Move {count} selected to Junk"), "!"),
                (
                    "select loaded",
                    "Select all loaded conversations".into(),
                    "Ctrl A",
                ),
                ("clear selection", "Clear selection".into(), "Esc"),
            ] {
                items.push(Command::new(id, &title, keys, "selection bulk"));
            }
            if self.last_triage.borrow().is_some() {
                items.push(Command::new(
                    "undo triage",
                    "Undo last move",
                    "Z",
                    "recover unarchive",
                ));
            }
        } else if page == "snooze" {
            for (seconds, title, keys) in [
                (10800, "In 3 hours", ""),
                (86400, "Tomorrow", ""),
                (604800, "Next week", ""),
            ] {
                items.push(
                    Command::new(&format!("snooze:{seconds}"), title, keys, "")
                        .detail("Return this conversation to your inbox"),
                );
            }
        } else if page == "labels" {
            if let Ok(value) = self.request(json!({"op":"list_labels"})) {
                let active = self
                    .selected()
                    .and_then(|i| self.items.borrow().get(i).map(|m| m.labels.clone()))
                    .unwrap_or_default();
                for label in serde_json::from_value::<Vec<LabelSummary>>(value).unwrap_or_default()
                {
                    if [
                        "INBOX", "UNREAD", "SENT", "DRAFT", "TRASH", "SPAM", "STARRED",
                    ]
                    .contains(&label.name.as_str())
                    {
                        continue;
                    }
                    items.push(
                        Command::new(&format!("label:{}", label.name), &label.name, "", "tag")
                            .detail(if active.contains(&label.name) {
                                "Applied · Enter to remove"
                            } else {
                                "Enter to apply"
                            }),
                    );
                }
            }
            if !query.is_empty() && !items.iter().any(|i| i.title.eq_ignore_ascii_case(&query)) {
                items.insert(
                    0,
                    Command::new(
                        &format!("label:{}", self.command.text().trim()),
                        &format!("Create label “{}”", self.command.text().trim()),
                        "",
                        &query,
                    ),
                );
            }
        } else {
            if page != "accounts" && matches!(*self.view.borrow(), MailView::Drafts) {
                if self.last_discarded_draft.borrow().is_some() {
                    items.push(
                        Command::new("undo discard draft", "Undo discard", "Z", "undo restore recover undelete draft")
                            .detail("Restore the last discarded draft"),
                    );
                }
                if self.selected().is_some() {
                    items.push(Command::new("edit draft", "Edit draft", "Enter", "open continue writing"));
                    items.push(
                        Command::new("discard draft", "Discard draft", "Delete", "delete trash remove throw away")
                            .detail("Z to undo"),
                    );
                }
            }
            if page != "accounts" {
                if self.last_triage.borrow().is_some() && !matches!(*self.view.borrow(), MailView::Drafts) {
                    items.push(Command::new(
                        "undo triage",
                        "Undo last move",
                        "Z",
                        "recover unarchive",
                    ));
                }
                items.push(Command::new(
                    "search filters",
                    "Refine search…",
                    "Ctrl Shift F",
                    "filters advanced search from to cc dates attachments",
                ));
                items.push(Command::new(
                    "search help",
                    "Search syntax and coverage",
                    "",
                    "from to cc label operators local cache",
                ));
                items.push(Command::new(
                    "cleanup jobs",
                    "Cleanup history…",
                    "",
                    "history jobs progress resume",
                ));
                if self
                    .selected()
                    .and_then(|i| self.items.borrow().get(i).cloned())
                    .is_some()
                {
                    items.push(Command::new(
                        "bulk read sender",
                        "Bulk mark sender as read…",
                        "",
                        "bulk mass read mark all read newsletter sender cleanup",
                    ).detail("Preview unread messages from the selected sender before changing anything"));
                    items.push(
                        Command::new(
                            "bulk read downloaded",
                            "Mark downloaded mail from sender as read…",
                            "",
                            "downloaded local sender read",
                        )
                        .detail("Only matching conversations already saved on this device"),
                    );
                }
                items.push(Command::new(
                    "select loaded",
                    "Select all loaded conversations",
                    "Ctrl A",
                    "select bulk mass",
                ));
                for (id, title, keys, words) in [
                    ("reply", "Reply", "R", "respond answer"),
                    (
                        "reply all",
                        "Reply all",
                        "Shift R",
                        "respond everyone recipients",
                    ),
                    ("archive", "Archive", "E", "done"),
                    ("mark read", "Mark as read", "M", "seen"),
                    ("mark unread", "Mark as unread", "U", "unseen"),
                    (
                        "favorite",
                        "Toggle favorite",
                        "F",
                        "pin important star starred",
                    ),
                    ("labels", "Apply label…", "L", "tag folder"),
                    ("snooze", "Snooze…", "S", "remind later snooz snoz"),
                    ("spam", "Move to Junk", "!", "spam unwanted"),
                    (
                        "unsubscribe",
                        "Unsubscribe",
                        "Ctrl Shift U",
                        "stop newsletter",
                    ),
                    ("compose", "New message", "C", "write send email"),
                    ("search", "Search mail", "/", "find"),
                    ("inbox", "Go to Inbox", "G I", "all inboxes"),
                    ("archive-view", "Go to Archive", "G A", "archived"),
                    ("unread", "Go to Unread", "G U", "unseen"),
                    ("favorites", "Go to Favorites", "G F", "pinned starred"),
                    ("snoozed", "Go to Snoozed", "G S", "reminders"),
                    ("drafts", "Go to Drafts", "G D", "unfinished"),
                    ("sent", "Go to Sent", "G T", "sent mail delivered messages"),
                    ("outbox", "Go to Outbox", "G O", "scheduled undo send"),
                    (
                        "contact",
                        "Toggle contact pane",
                        "0",
                        "person sender profile",
                    ),
                    ("new split", "New split inbox…", "", "filter saved search"),
                    (
                        "add account",
                        "Add account…",
                        "",
                        "connect gmail icloud login",
                    ),
                    ("manage", "Manage accounts…", "", "remove account settings"),
                    ("agent", "Agent access…", "", "mcp ai agent codex claude assistant connect"),
                    ("agent requests", "Go to Agent requests", "", "approve approval agent claude codex waiting"),
                    ("sync", "Refresh mail", "Ctrl R", "sync reload"),
                    (
                        "images",
                        if self.remote_images_default.get() {
                            "Block remote images by default"
                        } else {
                            "Load remote images by default"
                        },
                        "I",
                        "toggle pictures privacy tracking",
                    ),
                    ("light", "Appearance: Light", "", "theme"),
                    ("dark", "Appearance: Dark", "", "theme"),
                    ("system", "Appearance: Follow System", "", "theme automatic"),
                    ("support", "Ask for support…", "", "help bug logs screenshot"),
                    ("feedback", "Give feedback…", "", "suggestion idea request"),
                    ("about", "About Email", "", "version build updated"),
                    (
                        "notifications",
                        "Toggle mail and follow-up notifications",
                        "",
                        "alerts banners",
                    ),
                    ("notification-previews", "Toggle notification previews", "", "privacy sender subject alerts"),
                    ("notification-test", "Send a test notification", "", "check alerts banners desktop"),
                    (
                        "calendar",
                        "Go to Calendar",
                        "G C",
                        "agenda schedule week day",
                    ),
                    (
                        "invitations",
                        "Calendar invitations…",
                        "",
                        "calendar agenda meeting ics event",
                    ),
                    (
                        "followup",
                        "Follow up unless they reply…",
                        "",
                        "waiting reminder followup",
                    ),
                    (
                        "followups",
                        "Go to Follow-ups",
                        "G W",
                        "waiting for reply due",
                    ),
                    ("help", "Keyboard shortcuts", "?", "help keys"),
                ] {
                    let command = Command::new(id, title, keys, words);
                    if id == "agent requests" && !agent::requests_item_visible() {
                        continue;
                    }
                    items.push(if id == "agent requests" && agent::waiting() {
                        command.detail(&format!("{} waiting", agent::requests_total()))
                    } else {
                        command
                    });
                }
                if let Ok(value) = self.request(json!({"op":"list_splits"})) {
                    for split in
                        serde_json::from_value::<Vec<SplitInbox>>(value).unwrap_or_default()
                    {
                        items.push(
                            Command::new(
                                &format!("split:{}", split.id),
                                &format!("Go to {}", split.name),
                                "",
                                &split.query,
                            )
                            .detail("Split inbox"),
                        );
                    }
                }
            }
            for account in self.accounts() {
                items.push(
                    Command::new(
                        &format!("account:{}", account.id),
                        &account.email,
                        "",
                        &account.display_name,
                    )
                    .detail("Account inbox"),
                );
            }
        }
        if !self.calendar_available(&self.accounts()) {
            items.retain(|item| !matches!(item.id.as_str(), "calendar" | "invitations"));
        }
        if !booking::configured() {
            // Scheduling commands exist only when the profile names a service.
            items.retain(|item| {
                !matches!(item.id.as_str(), "bookings" | "tracking activity" | "composer:booking" | "composer:tracking")
            });
        }
        if page != "labels" {
            let mut scored: Vec<_> = items
                .into_iter()
                .enumerate()
                .filter_map(|(index, item)| score(&item, &query).map(|s| (s, index, item)))
                .collect();
            scored.sort_by_key(|(s, index, _)| (*s, *index));
            items = scored.into_iter().map(|(_, _, item)| item).collect();
        } else {
            items.retain(|i| score(i, &query).is_some());
        }
        if page == "snooze" || page == "root" {
            if let Some((seconds, label)) = duration_query(&query) {
                items.insert(
                    0,
                    Command::new(&format!("snooze:{seconds}"), &label, "↵", "")
                        .detail("Snooze this conversation · Press Enter to confirm"),
                );
            }
        }
        if (page == "snooze" || page == "root") && self.workspace.visible_child_name().as_deref() != Some("calendar") {
            if let Ok(now) = glib::DateTime::now_local() {
                if let Some((timestamp, label, detail)) = calendar_reminder(&query, &now) {
                    items.insert(0, Command::new(&format!("snooze-at:{timestamp}"), &label, "↵", "").detail(&detail));
                }
            }
        }
        if self.workspace.visible_child_name().as_deref() == Some("calendar") && page == "root" {
            if let Some(calendar) = self.calendar.borrow().as_ref() {
                if let Some(command) = calendar.date_command(&query) {
                    items.insert(0, command);
                }
            }
        }
        while let Some(child) = self.palette_rows.first_child() {
            self.palette_rows.remove(&child);
        }
        for item in items.iter().take(60) {
            let row = gtk::ListBoxRow::new();
            let line = gtk::Box::new(Orientation::Horizontal, 12);
            let words = gtk::Box::new(Orientation::Vertical, 3);
            words.set_hexpand(true);
            words.append(&plain_label(&item.title, "command-title"));
            if !item.detail.is_empty() {
                words.append(&plain_label(&item.detail, "command-detail"));
            }
            line.append(&words);
            line.append(&plain_label(&item.keys, "keycap"));
            row.set_child(Some(&line));
            self.palette_rows.append(&row);
        }
        if let Some(row) = self.palette_rows.row_at_index(0) {
            self.palette_rows.select_row(Some(&row));
        }
        self.palette_empty.set_visible(items.is_empty());
        *self.palette_items.borrow_mut() = items;
    }
    pub(super) fn run_palette(self: &Rc<Self>) {
        let index = self
            .palette_rows
            .selected_row()
            .map(|r| r.index() as usize)
            .unwrap_or(0);
        let item = self.palette_items.borrow().get(index).cloned();
        if let Some(item) = item {
            self.palette.hide();
            self.execute_command(&item.id);
        }
    }
    pub(super) fn execute_command(self: &Rc<Self>, id: &str) {
        if id == "bookings" { booking::open(self, None); return; }
        if id == "tracking activity" { booking::activity(self,None); return; }
        if id == "composer:tracking" {
            if let Some(button)=self.compose_fields.borrow().get("tracking").and_then(|widget|widget.clone().downcast::<gtk::CheckButton>().ok()) {
                if button.is_sensitive(){button.set_active(!button.is_active());}else{self.status.set_text("Connect Scheduling to enable optional open tracking.");}
            }
            return;
        }
        // Calendar commands are an allowlist. A stale palette item must never
        // archive, send, or otherwise act on the hidden mailbox selection.
        if self.workspace.visible_child_name().as_deref() == Some("calendar") {
            let calendar = self.calendar.borrow().clone();
            if let Some(calendar) = calendar {
                calendar.execute_command(id);
            }
            return;
        }
        match id {
            "bulk read sender" => self.bulk_read_sender(),
            "bulk read downloaded" => self.bulk_read_sender_downloaded(),
            "search help" => self.search_help(),
            "search filters" => self.search_filters(),
            "cleanup jobs" => cleanup::open(self, None),
            "calendar" => agenda::open(self),
            "invitations" => workflows::open(self, true),
            "followup" => workflows::open(self, false),
            "followups" => self.set_view(MailView::Followups),
            "reply" => self.compose(true),
            "reply all" => self.compose_with_mode(true, true),
            "compose" => self.compose(false),
            "archive" => self.archive(),
            "undo triage" => self.undo_triage(),
            "edit draft" => {
                if matches!(*self.view.borrow(), MailView::Drafts) && self.selected().is_some() {
                    self.compose(false);
                }
            }
            "discard draft" => self.discard_selected_draft(),
            "undo discard draft" => self.undo_discard_draft(),
            "select loaded" => self.select_loaded(),
            "clear selection" => self.clear_marked(),
            "mark read" => self.mark(false),
            "mark unread" => self.mark(true),
            "favorite" => self.favorite(),
            "spam" => self.spam(),
            "labels" => self.open_palette("labels"),
            "snooze" => self.open_palette("snooze"),
            "unsubscribe" => self.unsubscribe(),
            "search" => self.focus_search(),
            "inbox" | "unread" | "favorites" => {
                self.search.set_text("");
                self.inbox_filter.set(match id {
                    "unread" => 1,
                    "favorites" => 2,
                    _ => 0,
                });
                self.set_view(MailView::Inbox);
            }
            "archive-view" => self.set_view(MailView::Archive),
            "snoozed" => self.set_view(MailView::Snoozed),
            "drafts" => self.set_view(MailView::Drafts),
            "sent" => self.set_view(MailView::Sent),
            "outbox" => self.set_view(MailView::Outbox),
            "contact" => self.toggle_contact(),
            "new split" => self.create_split(),
            "add account" => self.add_account(),
            "manage" => self.manage_accounts(),
            "agent" => {
                agent::open(self);
            }
            "agent requests" => agent::open_request(self, None),
            "sync" => self.sync_all(),
            "images" => self.toggle_remote_images(),
            "dark" | "light" | "system" => self.set_appearance(id),
            "help" => self.shortcuts_help(),
            "support" => self.show_support(false),
            "feedback" => self.show_support(true),
            "about" => {
                let dialog = gtk::Window::builder()
                    .title("About Email")
                    .transient_for(&self.window)
                    .modal(true)
                    .default_width(360)
                    .build();
                let content = gtk::Box::new(Orientation::Vertical, 12);
                content.add_css_class("settings-card");
                let label = gtk::Label::new(Some(&format!(
                    "Email {}\nBuild: {}",
                    env!("CARGO_PKG_VERSION"),
                    option_env!("EMAIL_BUILD_ID").unwrap_or("development")
                )));
                label.set_selectable(true);
                content.append(&label);
                let close = gtk::Button::with_label("Close");
                let d = dialog.clone();
                close.connect_clicked(move |_| d.close());
                content.append(&close);
                let keys = gtk::EventControllerKey::new();
                let d = dialog.clone();
                keys.connect_key_pressed(move |_, key, _, _| {
                    if key == gdk::Key::Escape {
                        d.close();
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                });
                dialog.add_controller(keys);
                dialog.set_child(Some(&content));
                dialog.present();
            }
            "notification-previews" => {
                self.notification_generation.set(self.notification_generation.get().wrapping_add(1));
                self.notification_previews.set(!self.notification_previews.get());
                let _ = std::fs::write(self.database_path.with_extension("notification-previews"), if self.notification_previews.get() { "on" } else { "off" });
                self.status.set_text(if self.notification_previews.get() { "Notification previews enabled" } else { "Notification previews hidden" });
            }
            "notification-test" => {
                if self.notifications.get() {
                    if let Some(app) = self.window.application() {
                        let notification = gtk::gio::Notification::new("Email notifications are ready");
                        notification.set_body(Some("Mail and follow-up alerts can appear while Email is running."));
                        app.send_notification(Some("email-notification-test"), &notification);
                        self.status.set_text("Test notification requested. Check your desktop notification settings if it does not appear.");
                    }
                } else { self.status.set_text("Enable notifications first, then send a test notification."); }
            }
            "notifications" => {
                // Observe cached arrivals while still off so enabling cannot replay old mail.
                if !self.notifications.get() { self.notify_new_mail(); self.notify_followups(); }
                self.notification_generation.set(self.notification_generation.get().wrapping_add(1));
                self.notification_since.set(now_ms());
                self.notifications.set(!self.notifications.get());
                let _ = std::fs::write(
                    self.database_path.with_extension("notifications"),
                    if self.notifications.get() {
                        "on"
                    } else {
                        "off"
                    },
                );
                self.status.set_text(if self.notifications.get() {
                    "Mail and follow-up notifications enabled"
                } else {
                    "Mail and follow-up notifications disabled"
                });
            }
            _ => {
                if id == "composer:snippets" {
                    self.open_palette("snippets");
                    return;
                }
                if id == "composer:save snippet" {
                    self.save_compose_snippet();
                    return;
                }
                if let Some(snippet_id) = id.strip_prefix("snippet:") {
                    self.insert_compose_snippet(snippet_id);
                    return;
                }
                if let Some(id) = id.strip_prefix("composer:") {
                    let action = self.compose_actions.borrow().get(id).cloned();
                    let field = self.compose_fields.borrow().get(id).cloned();
                    if let Some(button) = action {
                        button.emit_clicked();
                    } else if let Some(field) = field {
                        if id != "body" {
                            if let Some(toggle) = self
                                .compose_fields
                                .borrow()
                                .get("details-toggle")
                                .and_then(|w| w.clone().downcast::<gtk::ToggleButton>().ok())
                            {
                                toggle.set_active(true);
                            }
                            if let Some(details) = self.compose_fields.borrow().get("details-panel")
                            {
                                details.set_visible(true);
                            }
                        }
                        if let Some(parent) = field.parent() {
                            parent.set_visible(true);
                            if let Some(details) = parent.parent() {
                                details.set_visible(true);
                            }
                        }
                        field.grab_focus();
                    }
                } else if let Some(id) = id.strip_prefix("account:") {
                    self.search.set_text("");
                    self.set_view(MailView::Account(id.into()));
                } else if let Some(id) = id.strip_prefix("split:") {
                    self.search.set_text("");
                    self.set_view(MailView::Split(id.into()));
                } else if let Some(value) = id.strip_prefix("snooze-at:") {
                    if let (Ok(timestamp), Some(item)) = (value.parse::<i64>(), self.selected().and_then(|i| self.items.borrow().get(i).cloned())) {
                        match self.request(json!({"op":"snooze_thread","thread_id":item.thread_id,"wake_at_ms":timestamp})) {
                            Ok(_) => { self.status.set_text("Snoozed until the selected time."); self.refresh(); }
                            Err(error) => self.status.set_text(&format!("Snooze failed: {error}")),
                        }
                    }
                } else if let Some(value) = id.strip_prefix("snooze:") {
                    if let Ok(seconds) = value.parse::<u64>() {
                        if let Some(item) = self
                            .selected()
                            .and_then(|i| self.items.borrow().get(i).cloned())
                        {
                            self.snooze_for(&item, seconds, "Snoozed");
                        }
                    }
                } else if let Some(label) = id.strip_prefix("label:") {
                    if let Some(item) = self
                        .selected()
                        .and_then(|i| self.items.borrow().get(i).cloned())
                    {
                        let action = if item.labels.contains(&label.to_string()) {
                            MailAction::RemoveLabel {
                                thread_id: item.thread_id,
                                label: label.into(),
                            }
                        } else {
                            MailAction::AddLabel {
                                thread_id: item.thread_id,
                                label: label.into(),
                            }
                        };
                        self.apply_action(action, &item.account_id);
                    }
                }
            }
        }
    }
    fn compose_body(&self) -> Option<gtk::TextView> {
        self.compose_fields
            .borrow()
            .get("body")
            .cloned()?
            .downcast::<gtk::TextView>()
            .ok()
    }
    fn insert_compose_snippet(&self, id: &str) {
        let Some(body) = self.compose_body() else {
            return;
        };
        let Ok(snippets) = self.request(json!({"op":"list_snippets"})) else {
            return;
        };
        if let Some(text) = snippets
            .as_array()
            .into_iter()
            .flatten()
            .find(|snippet| snippet["id"].as_str() == Some(id))
            .and_then(|snippet| snippet["body"].as_str())
        {
            let buffer = body.buffer();
            buffer.begin_user_action();
            buffer.delete_selection(true, true);
            buffer.insert_at_cursor(text);
            buffer.end_user_action();
            body.grab_focus();
        }
    }
    fn save_compose_snippet(self: &Rc<Self>) {
        let Some(body) = self.compose_body() else {
            return;
        };
        let buffer = body.buffer();
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        if text.trim().is_empty() {
            self.status
                .set_text("Write a message before saving a snippet.");
            return;
        }
        let dialog = gtk::Window::builder()
            .title("Save as snippet")
            .transient_for(&self.window)
            .modal(true)
            .default_width(380)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 12);
        content.add_css_class("settings-card");
        let help = gtk::Label::new(Some(
            "Reuse this message text in any reply or draft. Snippets stay on this device.",
        ));
        help.set_wrap(true);
        content.append(&help);
        let name = gtk::Entry::new();
        name.set_placeholder_text(Some("Snippet name"));
        name.set_max_length(80);
        content.append(&name);
        let error = gtk::Label::new(None);
        error.set_wrap(true);
        error.set_xalign(0.0);
        error.add_css_class("command-detail");
        content.append(&error);
        let save = gtk::Button::with_label("Save snippet");
        content.append(&save);
        let ui = self.clone();
        let close = dialog.clone();
        let field = name.clone();
        save.connect_clicked(move |_| {
            match ui
                .request(json!({"op":"upsert_snippet","name":field.text().to_string(),"body":text}))
            {
                Ok(_) => {
                    ui.status
                        .set_text("Snippet saved · Commands → Insert snippet");
                    close.close();
                    body.grab_focus();
                }
                Err(message) => error.set_text(&message),
            }
        });
        let button = save.clone();
        name.connect_activate(move |_| button.emit_clicked());
        let keys = gtk::EventControllerKey::new();
        let close = dialog.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                close.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.add_controller(keys);
        dialog.set_child(Some(&content));
        dialog.present();
        name.grab_focus();
    }
    pub(super) fn spam(self: &Rc<Self>) {
        self.selection_triage("spam");
    }
    pub(super) fn set_appearance(&self, mode: &str) {
        self.theme_mode.replace(mode.into());
        let system_dark =
            gtk::Settings::default().is_some_and(|s| s.is_gtk_application_prefer_dark_theme());
        self.dark.set(if mode == "system" {
            system_dark
        } else {
            mode == "dark"
        });
        self.css.load_from_data(&appearance_css(self.dark.get()));
        let _ = std::fs::write(self.database_path.with_extension("theme"), mode);
        if let Some(html) = self.current_html.borrow().clone() {
            self.render_html(&html);
        }
    }
    pub(super) fn shortcuts_help(&self) {
        let dialog = gtk::Window::builder()
            .transient_for(&self.window)
            .modal(true)
            .title("Keyboard shortcuts")
            .default_width(520)
            .build();
        let box_ = gtk::Box::new(Orientation::Vertical, 12);
        box_.add_css_class("settings-card");
        for (key, meaning) in [
            ("INBOX", ""),
            ("J / K", "Next / previous conversation"),
            ("Ctrl D / U", "Half page down / up"),
            ("Enter / Esc", "Enter reading mode / return to inbox"),
            ("R · Shift R · C", "Reply · Reply all · New message"),
            ("E · M · U · F", "Archive · Read · Unread · Favorite"),
            ("S · L · I · 0", "Snooze · Labels · Images · Contact"),
            ("Ctrl Shift U", "Review unsubscribe link"),
            (
                "G then I / A / U / F / S / D / O",
                "Inbox / Archive / Unread / Favorites / Snoozed / Drafts / Outbox",
            ),
            ("READING", ""),
            ("J / K · Space / Shift Space", "Scroll lines · Half page"),
            (
                "H / L · G G / Shift G",
                "Previous / next message · Top / bottom",
            ),
            ("COMPOSE", ""),
            (
                "Tab / Shift Tab · Ctrl Enter",
                "Next / previous field · Send",
            ),
            ("Ctrl S · Esc", "Save draft · Save and close"),
            ("Ctrl K · / · ?", "Commands · Search · Shortcuts"),
        ] {
            let row = gtk::Box::new(Orientation::Horizontal, 12);
            row.append(&plain_label(key, "keycap"));
            let text = plain_label(meaning, "command-detail");
            text.set_wrap(true);
            text.set_max_width_chars(42);
            row.append(&text);
            box_.append(&row);
        }
        let close = gtk::Button::with_label("Done");
        let win = dialog.clone();
        close.connect_clicked(move |_| win.close());
        box_.append(&close);
        dialog.set_child(Some(&box_));
        dialog.present();
    }
    pub(super) fn manage_accounts(self: &Rc<Self>) {
        let dialog = gtk::Window::builder()
            .transient_for(&self.window)
            .modal(true)
            .title("Accounts")
            .default_width(450)
            .build();
        let box_ = gtk::Box::new(Orientation::Vertical, 12);
        box_.add_css_class("settings-card");
        for account in self.accounts() {
            let line = gtk::Box::new(Orientation::Horizontal, 12);
            let label = plain_label(&account.email, "command-title");
            label.set_hexpand(true);
            line.append(&label);
            let remove = gtk::Button::with_label("Remove");
            let ui = self.clone();
            let parent = dialog.clone();
            remove.connect_clicked(move |button|{let confirm=gtk::AlertDialog::builder().message("Remove this account from Email?").detail("This removes its local cache and credentials. Your provider mailbox is unchanged.").buttons(["Cancel","Remove account"]).cancel_button(0).default_button(0).build();let ui=ui.clone();let id=account.id.clone();let parent=parent.clone();let button=button.clone();confirm.choose(Some(&parent.clone()),None::<&gtk::gio::Cancellable>,move |result|{
                if result!=Ok(1){return;}
                // Removal can wait a few seconds on Google's token revoke; run it off the main loop.
                button.set_sensitive(false);
                workflows::request_async(ui.database_path.clone(),json!({"op":"remove_account","account_id":id}),move |result| match result {
                    Ok(_)=>{parent.close();ui.set_view(MailView::Inbox);},
                    Err(e)=>{button.set_sensitive(true);ui.status.set_text(&e);}
                });
            });});
            line.append(&remove);
            box_.append(&line);
        }
        let add = gtk::Button::with_label("Add account…");
        let ui = self.clone();
        let close = dialog.clone();
        add.connect_clicked(move |_| {
            close.close();
            ui.add_account();
        });
        box_.append(&add);
        let agent_access = gtk::Button::with_label("Agent access…");
        let ui = self.clone();
        let close = dialog.clone();
        agent_access.connect_clicked(move |_| {
            close.close();
            agent::open(&ui);
        });
        box_.append(&agent_access);
        dialog.set_child(Some(&box_));
        dialog.present();
    }
    pub(super) fn toggle_contact(self: &Rc<Self>) {
        let activity = self.activity_panel.borrow().clone();
        if let Some(activity) = activity { activity.dismiss(); }
        let visible = !self.contact_requested.get();
        self.contact_requested.set(visible);
        self.contact.root.set_visible(visible);
        let _ = std::fs::write(
            self.database_path.with_extension("contact-pane"),
            if visible { "on" } else { "off" },
        );
        if visible {
            self.refresh_contact();
        }
    }
    pub(super) fn refresh_contact(self: &Rc<Self>) {
        let Some(message) = self.current_message.borrow().clone() else {
            return;
        };
        self.contact.show(self, &message);
        self.contact.show_open_activity(None);
        self.refresh_contact_activity(false);
    }
    pub(super) fn refresh_contact_activity(self: &Rc<Self>, refresh: bool) {
        if !booking::configured() {
            self.contact.show_open_activity(None);
            return;
        }
        let Some(message) = self.current_message.borrow().clone().filter(|m| m.is_sent && !m.is_draft) else {
            self.contact.show_open_activity(None); return;
        };
        let id = message.id;
        let weak = Rc::downgrade(self);
        workflows::request_async(self.database_path.clone(), json!({"op":"tracking_activity","message_id":id,"refresh":refresh}), move |result| {
            let Some(ui) = weak.upgrade() else { return; };
            if ui.current_message.borrow().as_ref().map(|m| m.id.as_str()) != Some(id.as_str()) { return; }
            if let Ok(data) = result {
                let item = data["items"].as_array().and_then(|items| items.first());
                ui.contact.show_open_activity(item);
                if !refresh && item.is_some() && ui.contact.root.is_visible() { ui.refresh_contact_activity(true); }
            }
        });
    }
    pub(super) fn reader_scroll(&self, amount: f64, boundary: Option<bool>) {
        if let Some(reader) = self.reader.borrow().as_ref().filter(|r| r.is_visible()) {
            let script = match boundary {
                Some(true) => "window.scrollTo(0,0)".into(),
                Some(false) => "window.scrollTo(0,document.documentElement.scrollHeight)".into(),
                None => format!(
                    "window.scrollBy(0,{amount} * {})",
                    if amount.abs() < 2.0 {
                        "window.innerHeight"
                    } else {
                        "1"
                    }
                ),
            };
            reader.evaluate_javascript(
                &script,
                Some("email-reader-controls"),
                None,
                None::<&gtk::gio::Cancellable>,
                |_| {},
            );
        } else {
            let a = self.plain_scroll.vadjustment();
            let next = match boundary {
                Some(true) => a.lower(),
                Some(false) => a.upper() - a.page_size(),
                None => {
                    a.value()
                        + if amount.abs() < 2.0 {
                            amount * a.page_size()
                        } else {
                            amount
                        }
                }
            };
            a.set_value(next.clamp(a.lower(), (a.upper() - a.page_size()).max(a.lower())));
        }
    }
    pub(super) fn navigate_history(self: &Rc<Self>, delta: i32) {
        let messages = self.history.borrow();
        if messages.is_empty() {
            return;
        }
        let current = self.current_message.borrow().as_ref().map(|m| m.id.clone());
        let i = messages
            .iter()
            .position(|m| Some(&m.id) == current.as_ref())
            .unwrap_or(messages.len() - 1) as i32;
        let message = messages[(i + delta).clamp(0, messages.len() as i32 - 1) as usize].clone();
        drop(messages);
        self.show_message(&message);
    }
}

pub(super) struct ContactPane {
    pub root: gtk::ScrolledWindow,
    avatar: gtk::Label,
    photo: gtk::Picture,
    name: gtk::Label,
    email: gtk::Label,
    recent: gtk::Box,
    activity_section: gtk::Box,
    activity_body: gtk::Box,
    pub activity_refresh: gtk::Button,
    write: gtk::Button,
    find: gtk::Button,
    picture: gtk::Button,
    close: gtk::Button,
}
impl ContactPane {
    pub fn new() -> Self {
        let root = gtk::ScrolledWindow::new();
        root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        root.set_width_request(270);
        root.set_propagate_natural_width(false);
        root.add_css_class("contact-pane");
        let content = gtk::Box::new(Orientation::Vertical, 0);
        content.set_margin_top(24);
        content.set_margin_start(22);
        content.set_margin_end(18);
        root.set_child(Some(&content));
        let heading = gtk::Box::new(Orientation::Horizontal, 8);
        let title = plain_label("CONTACT", "contact-heading");
        title.set_hexpand(true);
        heading.append(&title);
        let close = gtk::Button::with_label("×");
        close.add_css_class("contact-close");
        heading.append(&close);
        content.append(&heading);
        let identity = gtk::Box::new(Orientation::Horizontal, 12);
        identity.set_margin_top(24);
        let avatar = gtk::Label::new(None);
        avatar.add_css_class("avatar");
        avatar.set_size_request(46, 46);
        let photo = gtk::Picture::new();
        photo.set_size_request(46, 46);
        photo.set_can_shrink(true);
        photo.set_content_fit(gtk::ContentFit::Cover);
        photo.add_css_class("avatar-photo");
        photo.set_visible(false);
        let overlay = gtk::Overlay::new();
        overlay.set_size_request(46, 46);
        overlay.set_child(Some(&avatar));
        overlay.add_overlay(&photo);
        identity.append(&overlay);
        let names = gtk::Box::new(Orientation::Vertical, 4);
        names.set_hexpand(true);
        let name = plain_label("", "contact-name");
        let email = plain_label("", "contact-email");
        names.append(&name);
        names.append(&email);
        identity.append(&names);
        content.append(&identity);
        let buttons = gtk::Box::new(Orientation::Horizontal, 8);
        buttons.set_margin_top(18);
        let write = gtk::Button::with_label("Write");
        let find = gtk::Button::with_label("Find mail");
        let picture = icon_button("avatar-default-symbolic", "Picture options");
        for b in [&write, &find, &picture] {
            b.add_css_class("contact-action");
            buttons.append(b);
        }
        content.append(&buttons);
        let heading = plain_label("RECENT MAIL", "contact-heading");
        heading.set_margin_top(34);
        heading.set_margin_bottom(10);
        content.append(&heading);
        let recent = gtk::Box::new(Orientation::Vertical, 0);
        content.append(&recent);
        let activity_section = gtk::Box::new(Orientation::Vertical, 10);
        activity_section.set_margin_top(24); activity_section.set_visible(false);
        let activity_header = gtk::Box::new(Orientation::Horizontal, 8); activity_header.add_css_class("activity-controls");
        let title = plain_label("Open activity", "contact-heading"); title.set_hexpand(true); activity_header.append(&title);
        let info = gtk::Button::from_icon_name("help-about-symbolic"); info.add_css_class("flat");
        info.set_tooltip_text(Some("Privacy protection and mail scanners can affect open tracking. Opens in the same minute are grouped. Recipients are not identified.")); activity_header.append(&info);
        let activity_refresh = gtk::Button::from_icon_name("view-refresh-symbolic"); activity_refresh.add_css_class("flat");
        activity_refresh.set_tooltip_text(Some("Refresh open activity")); activity_header.append(&activity_refresh);
        activity_section.append(&activity_header);
        let activity_body = gtk::Box::new(Orientation::Vertical, 8); activity_section.append(&activity_body);
        content.append(&activity_section);
        Self {
            root,
            avatar,
            photo,
            name,
            email,
            recent,
            activity_section, activity_body, activity_refresh,
            write,
            find,
            picture,
            close,
        }
    }
    pub fn show_open_activity(&self, item: Option<&Value>) {
        while let Some(child) = self.activity_body.first_child() { self.activity_body.remove(&child); }
        self.activity_section.set_visible(item.is_some());
        if let Some(item) = item { self.activity_body.append(&booking::open_activity_summary(item)); }
    }
    pub fn connect(&self, ui: &Rc<Ui>) {
        let weak = Rc::downgrade(ui);
        self.activity_refresh.connect_clicked(move |_| if let Some(ui) = weak.upgrade() { ui.refresh_contact_activity(true); });
        let weak = Rc::downgrade(ui);
        self.close.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.toggle_contact();
            }
        });
        let weak = Rc::downgrade(ui);
        self.find.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                let sender = ui
                    .current_message
                    .borrow()
                    .as_ref()
                    .map(|m| address(&m.sender));
                if let Some(sender) = sender {
                    ui.search
                        .set_text(&format!("from:{}", quote_search_value(&sender)));
                    ui.set_view(MailView::Inbox);
                }
            }
        });
        let weak = Rc::downgrade(ui);
        self.write.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                let sender = ui
                    .current_message
                    .borrow()
                    .as_ref()
                    .map(|m| address(&m.sender));
                if let Some(sender) = sender {
                    ui.compose(false);
                    if let Some(to) = ui.compose_to.borrow().as_ref() {
                        to.set_text(&sender);
                        to.grab_focus();
                    }
                }
            }
        });
        let weak = Rc::downgrade(ui);
        self.picture.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.picture_options();
            }
        });
    }
    fn show(&self, ui: &Rc<Ui>, message: &Message) {
        let email = address(&message.sender);
        let name = sender_name(&message.sender);
        self.name.set_text(name);
        self.name.set_tooltip_text(Some(name));
        self.email.set_text(&email);
        self.email.set_tooltip_text(Some(&email));
        let initials = name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .take(2)
            .filter_map(|s| s.chars().next())
            .collect::<String>()
            .to_uppercase();
        self.avatar.set_text(&initials);
        self.photo.set_visible(false);
        if let Ok(bytes) = std::fs::read(ui.portrait_path(&email)) {
            self.set_photo(&bytes);
        } else if let Some(bytes) = ui.portraits.borrow().get(&email).and_then(|v| v.as_ref()) {
            self.set_photo(bytes);
        }
        if self.root.is_visible()
            && !ui.portraits.borrow().contains_key(&email)
            && !message.account_id.starts_with("demo:")
            && crate::account_setup::build_features(&ui.service).google_contact_photos
        {
            ui.portraits.borrow_mut().insert(email.clone(), None);
            ui.background_attachment(AttachmentEvent::Portrait {
                account_id: message.account_id.clone(),
                address: email.clone(),
                result: Ok(Value::Null),
            });
        }
        while let Some(child) = self.recent.first_child() {
            self.recent.remove(&child);
        }
        if let Ok(value) =
            ui.request(json!({"op":"search","query":format!("from:{email}"),"limit":8}))
        {
            for thread in serde_json::from_value::<Vec<ThreadSummary>>(value)
                .unwrap_or_default()
                .into_iter()
                .filter(|t| t.id != message.thread_id)
                .take(6)
            {
                let button = gtk::Button::new();
                button.set_child(Some(&plain_label(&thread.subject, "contact-recent")));
                button.set_tooltip_text(Some(&thread.subject));
                button.add_css_class("contact-recent-row");
                let weak = Rc::downgrade(ui);
                let id = thread.id;
                button.connect_clicked(move |_| {
                    if let Some(ui) = weak.upgrade() {
                        ui.search.set_text("");
                        ui.set_view(MailView::Inbox);
                        let index = ui.items.borrow().iter().position(|m| m.thread_id == id);
                        if let Some(i) = index {
                            ui.list.select_row(ui.list.row_at_index(i as i32).as_ref());
                        }
                    }
                });
                self.recent.append(&button);
            }
        }
    }
    pub fn set_photo(&self, bytes: &[u8]) {
        if bytes.len() > 5 * 1024 * 1024 {
            return;
        }
        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
            self.photo.set_paintable(Some(&texture));
            self.photo.set_visible(true);
        }
    }
}
impl Ui {
    fn portrait_path(&self, address: &str) -> PathBuf {
        let key = address
            .as_bytes()
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>();
        self.database_path
            .with_extension("portraits")
            .join(format!("{key}.image"))
    }
    fn picture_options(self: &Rc<Self>) {
        let Some(message) = self.current_message.borrow().clone() else {
            return;
        };
        let address = address(&message.sender);
        let dialog = gtk::Window::builder()
            .transient_for(&self.window)
            .modal(true)
            .title("Sender picture")
            .default_width(340)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 10);
        content.add_css_class("settings-card");
        let choose = gtk::Button::with_label("Choose a picture…");
        let ui = self.clone();
        let email = address.clone();
        let parent = dialog.clone();
        choose.connect_clicked(move |_| {
            let chooser = gtk::FileDialog::new();
            let ui = ui.clone();
            let email = email.clone();
            chooser.open(
                Some(&parent),
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            if let Ok(meta) = std::fs::metadata(&path) {
                                if meta.len() > 5 * 1024 * 1024 {
                                    return;
                                }
                            }
                            if let Ok(bytes) = std::fs::read(path) {
                                if gdk::Texture::from_bytes(&glib::Bytes::from(&bytes)).is_ok() {
                                    let target = ui.portrait_path(&email);
                                    if let Some(parent) = target.parent() {
                                        let _ = std::fs::create_dir_all(parent);
                                    }
                                    if std::fs::write(target, &bytes).is_ok() {
                                        ui.portraits.borrow_mut().insert(email, Some(bytes));
                                        ui.refresh_contact();
                                    }
                                }
                            }
                        }
                    }
                },
            );
        });
        content.append(&choose);
        let remove = gtk::Button::with_label("Remove custom picture");
        let ui = self.clone();
        let email = address.clone();
        let close = dialog.clone();
        remove.connect_clicked(move |_| {
            let _ = std::fs::remove_file(ui.portrait_path(&email));
            ui.portraits.borrow_mut().remove(&email);
            ui.refresh_contact();
            close.close();
        });
        content.append(&remove);
        // Distribution builds do not request Contacts access, which would need its own Google review.
        if crate::account_setup::build_features(&self.service).google_contact_photos
            && self
                .accounts()
                .iter()
                .any(|a| a.id == message.account_id && a.provider == mail_core::Provider::Gmail)
        {
            let enable = gtk::Button::with_label("Use Google contact photos…");
            let ui = self.clone();
            let close = dialog.clone();
            enable.connect_clicked(move |_| {
                ui.background_attachment(AttachmentEvent::Portrait {
                    account_id: message.account_id.clone(),
                    address: address.clone(),
                    result: Ok(json!({"authorize":true})),
                });
                close.close();
            });
            content.append(&enable);
        }
        dialog.set_child(Some(&content));
        dialog.present();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clearing_every_recipient_is_detected_only_when_some_existed() {
        assert!(recipients_cleared(["a@example.com", "", ""], ["", "  ", ""]));
        assert!(recipients_cleared(["", "", "b@example.com"], ["", "", ""]));
        assert!(!recipients_cleared(["a@example.com", "", ""], ["", "c@example.com", ""]));
        assert!(!recipients_cleared(["", "", ""], ["", "", ""]));
    }

    #[test]
    fn private_draft_recipient_labels_include_copy_only_recipients() {
        assert_eq!(
            draft_recipient_summary("", "copy@example.com", "hidden@example.com"),
            "Cc: copy@example.com"
        );
        assert_eq!(
            draft_recipient_summary("", "", "hidden@example.com"),
            "Bcc: hidden@example.com"
        );
        assert_eq!(
            draft_recipient_summary("to@example.com", "copy@example.com", ""),
            "To: to@example.com"
        );
    }

    #[test]
    fn search_scope_stays_separate_from_query_and_limit() {
        let query = "from:person@example.org is:read";
        let request =
            thread_search_request(&MailView::Account("account-b".into()), query, 1, 100, 500);
        assert_eq!(request["account_id"], "account-b");
        assert_eq!(request["query"], query);
        assert_eq!(request["unread_only"], true);
        assert_eq!(request["is_inbox"], true);
        assert_eq!(request["offset"], 500);
        let request = thread_search_request(&MailView::Split("split-1".into()), "", 2, 500, 0);
        assert_eq!(request["split_id"], "split-1");
        assert_eq!(request["starred_only"], true);
        let label = thread_search_request(
            &MailView::Label("Customer Support".into()),
            "subject:urgent",
            1,
            500,
            0,
        );
        assert_eq!(label["label"], "Customer Support");
        assert_eq!(label["query"], "subject:urgent");
        assert_eq!(label["unread_only"], true);
        assert!(label.get("is_inbox").is_none());
        let favorites = thread_search_request(&MailView::Inbox, "", 2, 500, 0);
        assert_eq!(favorites["starred_only"], true);
        assert!(favorites.get("is_inbox").is_none());
        let archive = thread_search_request(&MailView::Archive, "in:inbox", 0, 500, 0);
        assert_eq!(archive["folder"], "archive");
        assert!(archive.get("is_inbox").is_none());
        let snoozed = thread_search_request(&MailView::Snoozed, "subject:urgent", 1, 500, 0);
        assert_eq!(snoozed["snoozed_only"], true);
        assert_eq!(snoozed["unread_only"], true);
        assert!(snoozed.get("is_inbox").is_none());
        assert_eq!(
            quote_search_value("Customer Support"),
            "\"Customer Support\""
        );
        assert_eq!(quote_search_value("Team \"A\""), "\"Team \\\"A\\\"\"");
    }

    #[test]
    fn reply_all_excludes_own_aliases_and_deduplicates() {
        let (to, cc) = reply_recipients(
            "Sender <sender@example.com>",
            &["Help <help@example.com>".into()],
            &[
                "Me <me@example.com>".into(),
                "Other <other@example.com>".into(),
            ],
            &[
                "help@example.com".into(),
                "copy@example.com".into(),
                "alias@example.com".into(),
            ],
            &["me@example.com".into(), "alias@example.com".into()],
            true,
        );
        assert_eq!(
            to,
            vec!["Help <help@example.com>", "Other <other@example.com>"]
        );
        assert_eq!(cc, vec!["copy@example.com"]);
        let (to, cc) = reply_recipients(
            "Me <alias@example.com>",
            &[],
            &["Other <other@example.com>".into()],
            &[],
            &["alias@example.com".into()],
            false,
        );
        assert_eq!(to, vec!["Other <other@example.com>"]);
        assert!(cc.is_empty());
    }

    #[test]
    fn caps_lock_does_not_break_shortcuts() {
        assert_eq!(
            shortcut_key(gdk::Key::R, gdk::ModifierType::LOCK_MASK),
            gdk::Key::r
        );
        assert_eq!(
            shortcut_key(
                gdk::Key::K,
                gdk::ModifierType::LOCK_MASK | gdk::ModifierType::CONTROL_MASK
            ),
            gdk::Key::k
        );
        assert_eq!(
            shortcut_key(
                gdk::Key::g,
                gdk::ModifierType::LOCK_MASK | gdk::ModifierType::SHIFT_MASK
            ),
            gdk::Key::G
        );
    }
    #[test]
    fn named_snooze_phrases_preview_exact_dates() {
        let zone = glib::TimeZone::utc();
        let now = glib::DateTime::new(&zone, 2026, 11, 10, 10, 0, 0.).unwrap();
        for (query, expected) in [("remind tonight", "2026-11-10 20:00"), ("tonight", "2026-11-10 20:00"), ("remind me tonight at 8pm", "2026-11-10 20:00"), ("tomorrow morning", "2026-11-11 09:00"), ("snooze tomorrow 9", "2026-11-11 09:00")] {
            let (timestamp, _, _) = calendar_reminder(query, &now).unwrap();
            assert_eq!(glib::DateTime::from_unix_utc(timestamp/1000).unwrap().format("%Y-%m-%d %H:%M").unwrap(), expected, "{query}");
        }
        for query in ["today at 9", "tonight 25:00", "remind tonight maybe", "tonight 8:99pm"] { assert!(calendar_reminder(query, &now).is_none(), "{query}"); }
        let late = glib::DateTime::new(&zone, 2026, 11, 10, 21, 30, 0.).unwrap();
        assert_eq!(calendar_reminder("tonight", &late).unwrap().1, "Tonight at 22:00");
    }

    #[test]
    fn typo_matching_and_reminders() {
        let item = Command::new("archive", "Archive selected", "E", "done");
        assert!(score(&item, "arhive").is_some());
        assert!(score(&item, "avalanche").is_none());
        assert_eq!(
            duration_query("snoz 5").unwrap(),
            (300, "In 5 minutes".into())
        );
        assert_eq!(duration_query("remind me in 2 hours").unwrap().0, 7200);
        assert!(duration_query("snooze 0").is_none());
        assert!(duration_query("999999999999999999999999d").is_none());
    }
}

/// Prefer the actual receiving alias, then the provider default; never lexical order.
fn preferred_identity(
    account: &str,
    preferred: &[String],
    identities: &std::collections::BTreeMap<String, (String, String, bool)>,
) -> Option<String> {
    preferred
        .iter()
        .find_map(|recipient| {
            let recipient = address(recipient);
            identities
                .iter()
                .find(|(_, (id, email, _))| id == account && email.eq_ignore_ascii_case(&recipient))
                .map(|(id, _)| id.clone())
        })
        .or_else(|| {
            identities
                .iter()
                .find(|(_, (id, _, default))| id == account && *default)
                .map(|(id, _)| id.clone())
        })
}

pub(super) fn own_addresses(ui: &Ui, accounts: &[Account]) -> Vec<String> {
    let mut result: Vec<String> = accounts.iter().map(|a| address(&a.email)).collect();
    for account in accounts {
        if let Ok(bytes) = std::fs::read(
            ui.database_path
                .with_extension(format!("{}.send-as", account.id)),
        ) {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                result.extend(
                    value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v["email"].as_str().map(address)),
                );
            }
        }
    }
    result
}

/// Reply-All never repeats a recipient or copies hidden Bcc recipients.
pub(super) fn reply_recipients(
    sender: &str,
    reply_to: &[String],
    to: &[String],
    cc: &[String],
    own: &[String],
    all: bool,
) -> (Vec<String>, Vec<String>) {
    let mut seen: std::collections::HashSet<String> =
        own.iter().map(|a| address(a).to_lowercase()).collect();
    let mut unique = |values: &[String]| -> Vec<String> {
        values
            .iter()
            .filter(|v| {
                let mailbox = address(v).to_lowercase();
                !mailbox.is_empty() && seen.insert(mailbox)
            })
            .cloned()
            .collect()
    };
    let fallback = vec![sender.to_string()];
    let targets: Vec<String> = reply_to
        .iter()
        .filter(|v| !v.trim().is_empty())
        .cloned()
        .collect();
    let mut primary = unique(if targets.is_empty() {
        &fallback
    } else {
        &targets
    });
    if primary.is_empty() {
        primary = unique(to);
    }
    if all {
        primary.extend(unique(to));
    }
    let copies = if all { unique(cc) } else { Vec::new() };
    (primary, copies)
}

/// Identities arrive on a worker. Switching From never blocks typing or redraw.
pub(super) fn populate_identities(
    ui: &Rc<Ui>,
    combo: &gtk::ComboBoxText,
    preferred: Vec<String>,
    accounts: Vec<Account>,
) {
    let selected = combo.active_id().map(|s| s.to_string()).unwrap_or_default();
    let selected = selected.split('\t').next().unwrap_or(&selected).to_string();
    let path = ui.database_path.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = MailService::open(&path);
        for account in accounts {
            let cache = path.with_extension(format!("{}.send-as", account.id));
            if let Ok(bytes) = std::fs::read(&cache) {
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    let _ = tx.send((account.clone(), value));
                }
            }
            if account.id.starts_with("demo:") {
                continue;
            }
            if let Ok(service) = &result {
                if let Ok(value) = service
                    .request_json(&json!({"op":"list_send_as","account_id":account.id}).to_string())
                {
                    let _ = std::fs::write(&cache, serde_json::to_vec(&value).unwrap_or_default());
                    let _ = tx.send((account, value));
                }
            }
        }
    });
    let automatic = Rc::new(Cell::new(false));
    let user_picked = Rc::new(Cell::new(false));
    let changing = automatic.clone();
    let picked = user_picked.clone();
    combo.connect_changed(move |_| {
        if !changing.get() {
            picked.set(true);
        }
    });
    let weak = combo.downgrade();
    let mut identities = std::collections::BTreeMap::<String, (String, String, bool)>::new();
    glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        let Some(combo) = weak.upgrade().filter(|c| c.root().is_some()) else {
            return glib::ControlFlow::Break;
        };
        loop {
            match rx.try_recv() {
                Ok((account, value)) => {
                    automatic.set(true);
                    for alias in value.as_array().into_iter().flatten() {
                        if let Some(email) = alias["email"].as_str() {
                            let id = format!("{}\t{}", account.id, email);
                            let display = alias["display_name"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .unwrap_or(&account.display_name);
                            if !identities.contains_key(&id) {
                                combo.append(Some(&id), &format!("{display} · {email}"));
                            }
                            identities.insert(
                                id,
                                (
                                    account.id.clone(),
                                    email.into(),
                                    alias["is_default"].as_bool().unwrap_or(false),
                                ),
                            );
                        }
                    }
                    if !user_picked.get() {
                        if let Some(id) = preferred_identity(&selected, &preferred, &identities) {
                            combo.set_active_id(Some(&id));
                        }
                    }
                    automatic.set(false);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return glib::ControlFlow::Break,
            }
        }
        glib::ControlFlow::Continue
    });
}

pub(super) fn navigation_counts(counts: &Value) -> (u64, u64, u64) {
    (counts["total"].as_u64().or_else(||counts["__total"].as_u64()).unwrap_or(0),
     counts["followups"]["due"].as_u64().unwrap_or(0), counts["followups"]["active"].as_u64().unwrap_or(0))
}
fn consume_mail_arrival(seen: &mut std::collections::HashMap<String, i64>, key: String, received_at: i64, started_at: i64) -> bool {
    let previous=seen.get(&key).copied().unwrap_or(started_at);
    seen.insert(key,previous.max(received_at));
    if seen.len()>4096 {
        if let Some(oldest)=seen.iter().min_by_key(|(_,time)|**time).map(|(key,_)|key.clone()) { seen.remove(&oldest); }
    }
    received_at>previous && received_at>started_at
}
fn incoming_notification_message(message: &Value, account_id: &str, thread_id: &str) -> bool {
    message["account_id"].as_str()==Some(account_id)&&message["thread_id"].as_str()==Some(thread_id)&&message["is_sent"].as_bool()==Some(false)&&message["is_draft"].as_bool()==Some(false)
}
fn followup_key(reminder: &mail_core::FollowupReminder) -> String {
    json!([reminder.account_id,reminder.thread_id,reminder.created_at_ms,reminder.due_at_ms]).to_string()
}
fn consume_due_followups<'a>(reminders: &'a [mail_core::FollowupReminder], now: i64, seen: &mut std::collections::HashSet<String>) -> Vec<&'a mail_core::FollowupReminder> {
    reminders.iter().filter(|r|matches!(r.status.as_str(),"waiting"|"due")&&r.due_at_ms<=now&&seen.insert(followup_key(r))).collect()
}
pub(super) fn load_followup_notifications(path: &std::path::Path) -> std::collections::HashSet<String> {
    let path=path.with_extension("followup-notifications");
    let Ok(metadata)=std::fs::symlink_metadata(&path) else { return Default::default() };
    if metadata.file_type().is_symlink() || metadata.len()>512*1024 { return Default::default() }
    std::fs::read(&path).ok().and_then(|bytes|serde_json::from_slice::<Vec<String>>(&bytes).ok()).unwrap_or_default().into_iter().take(2000).collect()
}
fn save_followup_notifications(path: &std::path::Path, seen: &std::collections::HashSet<String>) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let target=path.with_extension("followup-notifications");
    let temporary=path.with_extension(format!("followup-notifications-{}.tmp",std::process::id()));
    let mut file=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
    let result=(|| { file.write_all(&serde_json::to_vec(&seen.iter().collect::<Vec<_>>())?)?; file.sync_all()?; std::fs::rename(&temporary,&target) })();
    if result.is_err() { let _=std::fs::remove_file(&temporary); }
    result
}
impl Ui {
    pub(super) fn notify_opens(self: &Rc<Self>) {
        let now = now_ms();
        if now - self.last_open_check.get() < 60_000 || self.open_check_in_flight.replace(true) { return; }
        self.last_open_check.set(now);
        let generation = self.notification_generation.get();
        let weak = Rc::downgrade(self);
        workflows::request_async(self.database_path.clone(), json!({"op":"tracking_notifications","enabled":self.notifications.get(),"session_since":self.notification_since.get().max(self.started_at)}), move |result| {
            let Some(ui) = weak.upgrade() else { return; };
            ui.open_check_in_flight.set(false);
            if !ui.notifications.get() || generation != ui.notification_generation.get() { return; }
            let Ok(data) = result else { return; };
            let Some(rows) = data["items"].as_array() else { return; };
            let Some(app) = ui.window.application() else { return; };
            for row in rows.iter().take(3) {
                let (Some(id), Some(thread)) = (row["id"].as_str(), row["thread_id"].as_str()) else { continue; };
                let notification = gtk::gio::Notification::new("Recent opens");
                let body = if ui.notification_previews.get() {
                    format!("{}\nOpened", notification_preview(row["subject"].as_str().unwrap_or(""), "").chars().take(180).collect::<String>())
                } else { "Your email was opened.".into() };
                notification.set_body(Some(&body));
                notification.set_default_action_and_target_value("app.open-thread",Some(&format!("open:{thread}").to_variant()));
                app.send_notification(Some(id), &notification);
            }
            if rows.len() > 3 {
                let notification = gtk::gio::Notification::new("More emails opened");
                notification.set_body(Some(&format!("{} more emails were opened.",rows.len()-3)));
                notification.set_default_action_and_target_value("app.open-thread", Some(&"open:".to_variant()));
                app.send_notification(Some("open-batch"), &notification);
            }
        });
    }

    pub(super) fn notify_new_mail(&self) {
        let Ok(value) = self.request(json!({"op":"list_threads","unread_only":true,"limit":100})) else { return };
        let Ok(threads)=serde_json::from_value::<Vec<ThreadSummary>>(value) else { return };
        let arrivals:Vec<_>=threads.into_iter().filter_map(|mut thread| {
            let key=json!([thread.account_id,thread.id]).to_string();
            let fresh=consume_mail_arrival(&mut self.notified.borrow_mut(),key,thread.last_message_at,self.started_at);
            if !fresh || !self.notifications.get() { return None; }
            let message = self.request(json!({"op":"get_latest_message_for_thread","thread_id":thread.id})).ok()?;
            if !incoming_notification_message(&message,&thread.account_id,&thread.id) { return None; }
            thread.sender = message["sender"].as_str().unwrap_or(&thread.sender).to_owned();
            thread.subject = message["subject"].as_str().unwrap_or(&thread.subject).to_owned();
            thread.snippet = message["notification_snippet"].as_str().unwrap_or_default().to_owned();
            Some(thread)
        }).collect();
        if !self.notifications.get() { return }
        let Some(app)=self.window.application() else { return };
        if arrivals.len()>3 {
            let notification=gtk::gio::Notification::new(&format!("{} new emails",arrivals.len()));
            let summary=if self.notification_previews.get() { notification_batch(&arrivals.iter().map(|thread|(thread.sender.as_str(),thread.subject.as_str())).collect::<Vec<_>>()) } else { "Click to open your inbox.".into() };
            notification.set_body(Some(&summary));
            notification.set_default_action_and_target_value("app.open-thread",Some(&arrivals[0].id.to_variant()));
            app.send_notification(Some("mail-batch"),&notification);
        } else { for thread in arrivals {
            let title=if self.notification_previews.get() { sender_name(&thread.sender) } else { "New email" };
            let notification=gtk::gio::Notification::new(title);
            let body = if self.notification_previews.get() { notification_preview(&thread.subject, &thread.snippet) } else { "Click to read, or mark it read or archive it from here.".into() };
            notification.set_body(Some(&body));
            notification.set_default_action_and_target_value("app.open-thread",Some(&thread.id.to_variant()));
            for (label, kind) in [("Mark as Read","read"),("Archive","archive")] {
                notification.add_button_with_target_value(label,"app.notification-triage",Some(&json!([kind,thread.account_id,thread.id]).to_string().to_variant()));
            }
            app.send_notification(Some(&format!("mail-{}-{}",thread.account_id,thread.id)),&notification);
        }}
    }
    pub(super) fn notify_followups(&self) {
        let Ok(value)=self.request(json!({"op":"list_followups"})) else { return };
        let Ok(reminders)=serde_json::from_value::<Vec<mail_core::FollowupReminder>>(value) else { return };
        let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        let active:std::collections::HashSet<_>=reminders.iter().filter(|r|matches!(r.status.as_str(),"waiting"|"due")).map(followup_key).collect();
        let mut seen=self.notified_followups.borrow_mut();let before=seen.clone();seen.retain(|key|active.contains(key));
        // Consume due reminders even while disabled: enabling must not replay a backlog.
        let due=consume_due_followups(&reminders,now,&mut seen);
        if !self.notifications.get() {
            if *seen!=before { let _=save_followup_notifications(&self.database_path,&seen); }
            return
        }
        let Some(app)=self.window.application() else { return };
        if due.len()>3 {
            let notification=gtk::gio::Notification::new(&format!("{} follow-ups due",due.len()));
            notification.set_body(Some("Open Email to review your follow-ups."));
            notification.set_default_action_and_target_value("app.open-thread",Some(&format!("followup:{}",due[0].thread_id).to_variant()));
            app.send_notification(Some("followup-batch"),&notification);
        } else { for reminder in due {
            let notification=gtk::gio::Notification::new("Follow-up due");
            notification.set_body(Some(if self.notification_previews.get() { &reminder.subject } else { "Open Email to review this follow-up." }));
            notification.set_default_action_and_target_value("app.open-thread",Some(&format!("followup:{}",reminder.thread_id).to_variant()));
            app.send_notification(Some(&format!("followup-{}-{}",reminder.thread_id,reminder.created_at_ms)),&notification);
        }}
        if *seen!=before { let _=save_followup_notifications(&self.database_path,&seen); }
    }
}

// Match the shortcut strip's native text rendering. Reserve the whole baseline
// explicitly instead of letting theme-dependent label metrics clip small headings.
pub(super) struct PaletteHeading {
    pub root: gtk::DrawingArea,
    text: Rc<RefCell<String>>,
}
impl PaletteHeading {
    pub fn new() -> Self {
        let root = gtk::DrawingArea::new();
        root.add_css_class("command-heading");
        root.set_content_height(30);
        root.set_hexpand(true);
        let text = Rc::new(RefCell::new(String::from("Commands")));
        let draw = text.clone();
        root.set_draw_func(move |widget, cr, _, _| {
            let color = widget.color();
            cr.set_source_rgba(
                color.red() as f64,
                color.green() as f64,
                color.blue() as f64,
                color.alpha() as f64,
            );
            cr.select_font_face(
                "sans-serif",
                gtk::cairo::FontSlant::Normal,
                gtk::cairo::FontWeight::Normal,
            );
            cr.set_font_size(13.0);
            cr.move_to(0.0, 20.0);
            let _ = cr.show_text(&draw.borrow());
        });
        let heading = Self { root, text };
        heading.set_text("Commands");
        heading
    }
    pub fn set_text(&self, text: &str) {
        self.text.replace(text.into());
        self.root
            .update_property(&[gtk::accessible::Property::Label(text)]);
        self.root.queue_draw();
    }
}

pub(super) struct ShortcutStrip {
    pub root: gtk::DrawingArea,
    text: Rc<RefCell<String>>,
}
impl ShortcutStrip {
    pub fn new() -> Self {
        let root = gtk::DrawingArea::new();
        root.set_content_height(34);
        root.set_hexpand(true);
        root.set_margin_start(24);
        root.set_margin_end(24);
        let text = Rc::new(RefCell::new(String::new()));
        let draw = text.clone();
        root.set_draw_func(move |widget, cr, width, _| {
            let color = widget.color();
            let (r, g, b) = (
                color.red() as f64,
                color.green() as f64,
                color.blue() as f64,
            );
            cr.set_source_rgba(r, g, b, 0.15);
            cr.set_line_width(0.5);
            cr.move_to(0.0, 0.5);
            cr.line_to(width as f64, 0.5);
            let _ = cr.stroke();
            let text = draw.borrow();
            let items: &[(&str, &str)] = if text.starts_with("Tab") {
                &[
                    ("Tab", "Next field"),
                    ("⇧Tab", "Previous"),
                    ("Ctrl ↵", "Send"),
                    ("Esc", "Close reply"),
                ]
            } else if text.starts_with("X ") {
                &[
                    ("X", "Select"),
                    ("⇧J ⇧K", "Extend"),
                    ("M U", "Read / unread"),
                    ("E", "Archive"),
                    ("Esc", "Clear"),
                ]
            } else if text.starts_with("READING") {
                &[
                    ("J K", "Scroll"),
                    ("Ctrl D/U", "Half page"),
                    ("H L", "History"),
                    ("Esc", "Inbox"),
                ]
            } else {
                &[
                    ("J K", "Next / previous"),
                    ("↵", "Read"),
                    ("R", "Reply"),
                    ("E", "Archive"),
                    ("Ctrl K", "Commands"),
                ]
            };
            let mut x = 0.0;
            for (key, label) in items {
                cr.select_font_face(
                    "monospace",
                    gtk::cairo::FontSlant::Normal,
                    gtk::cairo::FontWeight::Normal,
                );
                cr.set_font_size(9.0);
                let key_width = cr.text_extents(key).map(|e| e.x_advance()).unwrap_or(20.0) + 10.0;
                cr.select_font_face(
                    "Inter",
                    gtk::cairo::FontSlant::Normal,
                    gtk::cairo::FontWeight::Normal,
                );
                cr.set_font_size(10.0);
                let label_width = cr
                    .text_extents(label)
                    .map(|e| e.x_advance())
                    .unwrap_or(50.0);
                if x + key_width + 6.0 + label_width > width as f64 {
                    break;
                }
                cr.set_source_rgba(r, g, b, 0.07);
                cr.rectangle(x, 8.0, key_width, 17.0);
                let _ = cr.fill();
                cr.set_source_rgba(r, g, b, 0.85);
                cr.select_font_face(
                    "monospace",
                    gtk::cairo::FontSlant::Normal,
                    gtk::cairo::FontWeight::Normal,
                );
                cr.set_font_size(9.0);
                cr.move_to(x + 5.0, 20.0);
                let _ = cr.show_text(key);
                cr.set_source_rgba(r, g, b, 0.6);
                cr.select_font_face(
                    "Inter",
                    gtk::cairo::FontSlant::Normal,
                    gtk::cairo::FontWeight::Normal,
                );
                cr.set_font_size(10.0);
                cr.move_to(x + key_width + 6.0, 20.0);
                let _ = cr.show_text(label);
                x += key_width + label_width + 30.0;
            }
        });
        Self { root, text }
    }
    pub fn set_text(&self, text: &str) {
        self.text.replace(text.into());
        self.root.set_tooltip_text(Some(text));
        self.root.queue_draw();
    }
}

/// Menu labels must describe the same scope that action dispatch will mutate.
pub(super) fn draft_context_menu_actions() -> Vec<(&'static str, String)> {
    vec![
        ("edit draft", "Edit draft   Enter".into()),
        ("discard draft", "Discard draft   Delete".into()),
    ]
}

pub(super) fn context_menu_actions(count: usize, archived: bool) -> Vec<(&'static str, String)> {
    if count > 0 {
        return vec![
            (
                "archive",
                if archived {
                    format!("Move {count} selected to Inbox")
                } else {
                    format!("Archive {count} selected")
                },
            ),
            ("spam", format!("Move {count} selected to Junk")),
            ("mark read", format!("Mark {count} selected as read")),
            ("mark unread", format!("Mark {count} selected as unread")),
        ];
    }
    vec![
        ("reply", "Reply   R".into()),
        ("reply all", "Reply all   Shift R".into()),
        (
            "archive",
            if archived {
                "Move to Inbox   Shift E".into()
            } else {
                "Archive   E".into()
            },
        ),
        ("mark read", "Mark read   M".into()),
        ("mark unread", "Mark unread   U".into()),
        (
            "bulk read sender",
            "Mark all from this sender as read…".into(),
        ),
        ("favorite", "Toggle favorite   F".into()),
        ("labels", "Labels…   L".into()),
        ("snooze", "Snooze…   S".into()),
        ("spam", "Move to Junk   !".into()),
        ("unsubscribe", "Unsubscribe   Ctrl Shift U".into()),
    ]
}

/// Bounded provider passes expose the next allowed continuation, never busy-loop.
pub(super) fn pending_retry_delay(value: &Value) -> Option<u64> {
    if let Some(rows) = value.as_array() {
        return rows.iter().filter_map(pending_retry_delay).min();
    }
    if let Some(data) = value.get("data") {
        return pending_retry_delay(data);
    }
    if value["pending_actions"].as_u64().unwrap_or(0) == 0 {
        return None;
    }
    Some(
        value["actions_retry_after_ms"]
            .as_u64()
            .unwrap_or(5000)
            .clamp(1000, 3_600_000),
    )
}

/// Modified desktop/editor bindings must not fall through to destructive mail actions.
pub(super) fn allows_mail_shortcuts(mods: gdk::ModifierType) -> bool {
    !mods.intersects(
        gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK,
    )
}

/// Bindings follow Shift, not Caps Lock. Typing widgets still receive the original event.
pub(super) fn shortcut_key(key: gdk::Key, mods: gdk::ModifierType) -> gdk::Key {
    if mods.contains(gdk::ModifierType::CONTROL_MASK)
        || !mods.contains(gdk::ModifierType::SHIFT_MASK)
    {
        key.to_lower()
    } else {
        key.to_upper()
    }
}

#[cfg(test)]
mod workflow_tests {
    use super::*;
    #[test]
    fn from_identity_prefers_receiving_alias_then_provider_default() {
        let identities = std::collections::BTreeMap::from([
            (
                "a\ta@example.com".into(),
                ("a".into(), "a@example.com".into(), false),
            ),
            (
                "a\tz@example.com".into(),
                ("a".into(), "z@example.com".into(), true),
            ),
            (
                "b\tother@example.com".into(),
                ("b".into(), "other@example.com".into(), true),
            ),
        ]);
        assert_eq!(
            preferred_identity("a", &[], &identities).as_deref(),
            Some("a\tz@example.com")
        );
        assert_eq!(
            preferred_identity("a", &["Display <A@example.com>".into()], &identities).as_deref(),
            Some("a\ta@example.com")
        );
        assert_eq!(
            preferred_identity("a", &["other@example.com".into()], &identities).as_deref(),
            Some("a\tz@example.com")
        );
    }
    #[test]
    fn pending_pass_retry_honors_provider_backoff() {
        assert_eq!(pending_retry_delay(&json!({"pending_actions":0})), None);
        assert_eq!(
            pending_retry_delay(
                &json!({"data":{"pending_actions":2,"actions_retry_after_ms":60000}})
            ),
            Some(60000)
        );
        assert_eq!(
            pending_retry_delay(
                &json!([{ "data":{"pending_actions":2,"actions_retry_after_ms":60000} }, {"data":{"pending_actions":5,"actions_retry_after_ms":5000}}])
            ),
            Some(5000)
        );
        assert_eq!(
            pending_retry_delay(&json!({"pending_actions":2,"actions_retry_after_ms":0})),
            Some(1000)
        );
    }
    #[test]
    fn draft_context_actions_only_edit_or_discard() {
        let actions = draft_context_menu_actions();
        let ids: Vec<_> = actions.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, ["edit draft", "discard draft"]);
        assert!(actions[1].1.contains("Delete"));
    }
    #[test]
    fn context_actions_name_batch_scope_and_hide_single_message_actions() {
        let actions = context_menu_actions(3, false);
        assert_eq!(actions.len(), 4);
        assert!(actions
            .iter()
            .all(|(_, title)| title.contains("3 selected")));
        assert!(!actions.iter().any(|(id, _)| [
            "reply",
            "favorite",
            "labels",
            "snooze",
            "unsubscribe"
        ]
        .contains(id)));
        assert_eq!(
            context_menu_actions(2, true)[0].1,
            "Move 2 selected to Inbox"
        );
        assert!(context_menu_actions(0, false)
            .iter()
            .any(|(id, _)| *id == "reply"));
    }
    #[test]
    fn desktop_modifiers_do_not_trigger_plain_mail_actions() {
        assert!(allows_mail_shortcuts(gdk::ModifierType::empty()));
        assert!(allows_mail_shortcuts(gdk::ModifierType::SHIFT_MASK));
        for modifier in [
            gdk::ModifierType::CONTROL_MASK,
            gdk::ModifierType::ALT_MASK,
            gdk::ModifierType::SUPER_MASK,
        ] {
            assert!(!allows_mail_shortcuts(modifier));
            assert!(!allows_mail_shortcuts(
                modifier | gdk::ModifierType::SHIFT_MASK
            ));
        }
    }
}

#[cfg(test)]
mod sent_navigation_tests {
    use super::*;
    #[test]
    fn sent_scope_does_not_inherit_the_inbox_constraint() {
        let request = thread_search_request(&MailView::Sent, "subject:hello", 0, 500, 0);
        assert_eq!(request["folder"], "sent");
        assert!(request.get("is_inbox").is_none());
    }
}

#[cfg(test)]
mod notification_tests {
    use super::*;
    #[test]
    fn reply_notifications_are_monotonic_per_account_thread_and_consume_disabled_snapshots() {
        let mut seen=std::collections::HashMap::new();
        assert!(!consume_mail_arrival(&mut seen,"a/thread".into(),99,100));
        assert!(consume_mail_arrival(&mut seen,"a/thread".into(),101,100));
        assert!(!consume_mail_arrival(&mut seen,"a/thread".into(),101,100));
        assert!(!consume_mail_arrival(&mut seen,"a/thread".into(),98,100));
        assert!(consume_mail_arrival(&mut seen,"a/thread".into(),102,100));
        assert!(consume_mail_arrival(&mut seen,"b/thread".into(),101,100));
        // While disabled, callers still consume an arrival before skipping delivery.
        assert!(consume_mail_arrival(&mut seen,"a/thread".into(),103,100));
        assert!(!consume_mail_arrival(&mut seen,"a/thread".into(),103,100));
    }
    #[test]
    fn sidebar_counts_use_exact_cached_totals_and_due_not_active_followups() {
        assert_eq!(navigation_counts(&json!({"total":1200,"accounts":{"a":400},"followups":{"active":12,"due":3}})),(1200,3,12));
        assert_eq!(navigation_counts(&json!({"__total":5})),(5,0,0));
        assert_eq!(navigation_counts(&json!({"total":-1,"followups":{"due":"3"}})),(0,0,0));
    }
    #[test]
    fn due_reminder_markers_survive_restart_and_rescheduling_uses_a_new_key() {
        use std::os::unix::fs::PermissionsExt;
        let directory=std::env::temp_dir().join(format!("email-reminder-markers-{}",std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();let path=directory.join("mail.sqlite");
        let mut reminder=mail_core::FollowupReminder{thread_id:"t".into(),account_id:"a".into(),subject:"private".into(),sender:"private@example.com".into(),due_at_ms:200,created_at_ms:100,only_if_no_reply:true,status:"due".into(),reply_message_id:None};
        let key=followup_key(&reminder);let seen=std::collections::HashSet::from([key.clone()]);save_followup_notifications(&path,&seen).unwrap();
        assert_eq!(load_followup_notifications(&path),seen);assert_eq!(std::fs::metadata(path.with_extension("followup-notifications")).unwrap().permissions().mode()&0o777,0o600);
        let text=std::fs::read_to_string(path.with_extension("followup-notifications")).unwrap();assert!(!text.contains("private"));
        reminder.created_at_ms=101;assert_ne!(key,followup_key(&reminder));
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod followup_notification_disabled_tests {
 use super::*;
 #[test]
 fn disabled_snapshots_consume_due_but_not_future_or_replied_reminders() {
  let make=|id:&str,due,status:&str|mail_core::FollowupReminder{thread_id:id.into(),account_id:"a".into(),subject:"Synthetic".into(),sender:"test@example.com".into(),due_at_ms:due,created_at_ms:1,only_if_no_reply:false,status:status.into(),reply_message_id:None};
  let mut reminders=vec![make("due",10,"due"),make("future",30,"waiting"),make("answered",5,"replied")];let mut seen=std::collections::HashSet::new();
  assert_eq!(consume_due_followups(&reminders,20,&mut seen).len(),1);
  assert!(consume_due_followups(&reminders,20,&mut seen).is_empty(),"enabling after a disabled snapshot cannot replay due reminders");
  assert_eq!(consume_due_followups(&reminders,30,&mut seen)[0].thread_id,"future");
  reminders[0].created_at_ms=2;assert_eq!(consume_due_followups(&reminders,30,&mut seen)[0].thread_id,"due","a newly created reminder is independently eligible");
 }
}

#[cfg(test)]
mod notification_message_direction_tests {
 use super::*;
 #[test]
 fn unread_thread_does_not_make_own_sent_reply_or_draft_an_incoming_alert() {
  let mut message=json!({"account_id":"a","thread_id":"t","is_sent":false,"is_draft":false});
  assert!(incoming_notification_message(&message,"a","t"));
  message["is_sent"]=json!(true);assert!(!incoming_notification_message(&message,"a","t"));
  message["is_sent"]=json!(false);message["is_draft"]=json!(true);assert!(!incoming_notification_message(&message,"a","t"));
  message["is_draft"]=json!(false);assert!(!incoming_notification_message(&message,"b","t"));assert!(!incoming_notification_message(&message,"a","other"));
  assert!(!incoming_notification_message(&json!({}),"a","t"));
 }
}

fn notification_preview(subject: &str, snippet: &str) -> String {
    let subject = subject.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(140).collect::<String>();
    let snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(180).collect::<String>();
    let subject = if subject.is_empty() { "(No subject)" } else { &subject };
    if snippet.is_empty() { subject.to_owned() } else { format!("{subject}\n{snippet}") }.chars().take(320).collect()
}

/// Up to three newest "Sender — Subject" lines for a batch alert.
fn notification_batch(items: &[(&str, &str)]) -> String {
    let mut lines: Vec<String> = items.iter().take(3).map(|(sender, subject)| {
        let subject = subject.split_whitespace().collect::<Vec<_>>().join(" ");
        format!("{} — {}", sender_name(sender), if subject.is_empty() { "(No subject)" } else { &subject }).chars().take(90).collect()
    }).collect();
    if items.len() > 3 { lines.push(format!("and {} more", items.len() - 3)); }
    lines.join("\n")
}

#[cfg(test)]
mod notification_preview_tests {
    use super::*;
    #[test]
    fn banners_include_a_compact_body_and_subject_fallback() {
        assert_eq!(notification_preview("  Update\n ", "Hello\n\tworld"), "Update\nHello world");
        assert_eq!(notification_preview(" ", ""), "(No subject)");
        assert_eq!(notification_preview("Hi", &"é".repeat(300)).chars().count(), 183);
    }
    #[test]
    fn batches_name_senders() {
        assert_eq!(notification_batch(&[("A <a@x>","One"),("B",""),("C","Three"),("D","Four")]), "A — One\nB — (No subject)\nC — Three\nand 1 more");
    }
}
