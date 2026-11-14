//! Agents in the GTK shell: Agent requests in the main window (sends, bulk
//! changes, sender cleanup and refused agents that want an account), and the
//! Agent access settings window (what agents may do per account, connected
//! agents, activity). Mirrors apps/macos/Sources/EmailUI/AgentRequests.swift
//! and AgentAccess.swift. Titles, previews, button verbs and the turn-on
//! sheet's text come from mail-service, so both shells show the same words.
//! Only the app can turn access on or approve a request; the MCP server has no
//! code path for either.
use super::*;

/// The composer's undo window; approved agent sends get the same one.
pub(super) const UNDO_SECONDS: u64 = 10;

type Job = (Value, mpsc::Sender<Result<Value, String>>);

thread_local! {
    /// One long-lived service on a worker thread for polls and window reads,
    /// so a 5-second poll does not reopen the store each time.
    static WORKER: RefCell<Option<mpsc::Sender<Job>>> = const { RefCell::new(None) };
    /// Notification ids already shown, loaded once from disk.
    static SEEN: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    /// The Agent access window. Closing hides it, so reopening shows the same window.
    static OPEN_PANEL: RefCell<Option<Rc<Panel>>> = const { RefCell::new(None) };
    static POLL_IN_FLIGHT: Cell<bool> = const { Cell::new(false) };
    static REVISION: RefCell<String> = const { RefCell::new(String::new()) };
    /// Requests waiting (approvals plus refused agents) and whether any
    /// account is on: they decide the sidebar's Agent requests item.
    static TOTAL: Cell<usize> = const { Cell::new(0) };
    static ACCESS_ON: Cell<bool> = const { Cell::new(false) };
    /// The rows shown in Agent requests.
    static REQUESTS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    static READER: RefCell<Option<Rc<Reader>>> = const { RefCell::new(None) };
}

/// The sidebar entry for the settings window.
pub(super) fn sidebar_button() -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("side-item");
    button.set_tooltip_text(Some("Agent access: what AI agents may do with each account"));
    let content = gtk::Box::new(Orientation::Horizontal, 10);
    content.append(&mail_icon("security-high-symbolic"));
    let title = gtk::Label::new(Some("Agent access"));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&title);
    button.set_child(Some(&content));
    button
}

/// Whether the sidebar shows Agent requests: an account is on, or something waits.
pub(super) fn requests_item_visible() -> bool {
    ACCESS_ON.with(Cell::get) || TOTAL.with(Cell::get) > 0
}

pub(super) fn requests_total() -> usize {
    TOTAL.with(Cell::get)
}

/// Run a service request on the agent worker and deliver the result on the
/// GTK main loop.
fn request(ui: &Ui, op: Value, done: impl FnOnce(Result<Value, String>) + 'static) {
    let sender = WORKER.with(|slot| {
        slot.borrow_mut()
            .get_or_insert_with(|| {
                let (tx, rx) = mpsc::channel::<Job>();
                let path = ui.database_path.clone();
                thread::spawn(move || {
                    let mut service: Option<MailService> = None;
                    while let Ok((op, reply)) = rx.recv() {
                        // Open lazily and retry after a failed open.
                        if service.is_none() {
                            match MailService::open(&path) {
                                Ok(opened) => service = Some(opened),
                                Err(error) => {
                                    let _ = reply.send(Err(error));
                                    continue;
                                }
                            }
                        }
                        let result = service.as_ref().map_or_else(
                            || Err("Mail service unavailable".to_string()),
                            |service| service.request_json(&op.to_string()),
                        );
                        let _ = reply.send(result);
                    }
                });
                tx
            })
            .clone()
    });
    let (reply, result) = mpsc::channel();
    if sender.send((op, reply)).is_err() {
        WORKER.with(|slot| *slot.borrow_mut() = None);
        return done(Err("Agent worker stopped. Please retry.".into()));
    }
    let mut done = Some(done);
    glib::timeout_add_local(std::time::Duration::from_millis(35), move || match result.try_recv() {
        Ok(value) => {
            if let Some(done) = done.take() {
                done(value);
            }
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(_) => {
            if let Some(done) = done.take() {
                done(Err("Agent worker stopped. Please retry.".into()));
            }
            glib::ControlFlow::Break
        }
    });
}

/// Cheap local poll: the Agent requests item and its count, notifications,
/// and an open requests list or settings window.
pub(super) fn poll(ui: &Rc<Ui>) {
    if POLL_IN_FLIGHT.with(Cell::get) {
        return;
    }
    POLL_IN_FLIGHT.with(|flag| flag.set(true));
    let weak = Rc::downgrade(ui);
    request(ui, json!({"op":"agent_pending_summary"}), move |result| {
        POLL_IN_FLIGHT.with(|flag| flag.set(false));
        let (Some(ui), Ok(summary)) = (weak.upgrade(), result) else { return };
        let total = summary["total"].as_u64().unwrap_or(0) as usize;
        let access_on = summary["access_on"].as_bool() == Some(true);
        let total_changed = TOTAL.with(|t| t.replace(total)) != total;
        let access_changed = ACCESS_ON.with(|a| a.replace(access_on)) != access_on;
        if total_changed || access_changed {
            ui.refresh_navigation(&ui.accounts());
        }
        notify(&ui, &summary);
        // Re-read the list only when a request, decision or consent changed.
        let revision = summary["revision"].as_str().unwrap_or_default().to_string();
        let changed = REVISION.with(|seen| seen.replace(revision.clone()) != revision);
        if changed {
            if matches!(*ui.view.borrow(), MailView::AgentRequests) {
                refresh_keeping_place(&ui);
            }
            if let Some(panel) = OPEN_PANEL.with(|slot| slot.borrow().clone())
                && panel.window.is_visible()
            {
                panel.reload();
            }
        }
    });
}

/// True while requests wait, for the command palette.
pub(super) fn waiting() -> bool {
    TOTAL.with(Cell::get) > 0
}

fn seen_path(ui: &Ui) -> PathBuf {
    ui.database_path.with_extension("agent-notifications")
}

/// One notification per new request ("Claude Code" / "wants to send “Design
/// review time” to Maya Chen"), remembered across launches. Previews off keeps
/// subjects and recipients out of it. Clicking opens that request.
fn notify(ui: &Rc<Ui>, summary: &Value) {
    let path = seen_path(ui);
    let mut seen: Vec<String> = SEEN.with(|slot| {
        slot.borrow_mut()
            .get_or_insert_with(|| {
                std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_default()
            })
            .clone()
    });
    let previews = ui.notification_previews.get();
    let mut fresh: Vec<(String, String, String)> = Vec::new();
    for notice in summary["notices"].as_array().into_iter().flatten() {
        let Some(id) = notice["id"].as_str() else { continue };
        if seen.iter().any(|known| known == id) {
            continue;
        }
        seen.push(id.into());
        let body: String = notice["body"].as_str().unwrap_or_default().chars().take(160).collect();
        fresh.push(if previews {
            (id.to_string(), notice["title"].as_str().unwrap_or("An agent").to_string(), body)
        } else {
            (id.to_string(), "Email".into(), "An agent is waiting for you".into())
        });
    }
    if fresh.is_empty() {
        return;
    }
    let keep = seen.len().saturating_sub(400);
    let kept = seen[keep..].to_vec();
    let _ = write_private(&path, &serde_json::to_vec(&kept).unwrap_or_default());
    SEEN.with(|slot| *slot.borrow_mut() = Some(kept));
    if !ui.notifications.get() {
        return;
    }
    let Some(app) = ui.window.application() else { return };
    let total = fresh.len();
    for (id, title, body) in fresh.into_iter().take(3) {
        let notification = gtk::gio::Notification::new(&title);
        notification.set_body(Some(&body));
        notification.set_default_action_and_target_value("app.open-agent-request", Some(&id.to_variant()));
        app.send_notification(Some(&format!("agent-{id}")), &notification);
    }
    if total > 3 {
        let notification = gtk::gio::Notification::new("Email");
        notification.set_body(Some(&format!("{} more agent requests", total - 3)));
        notification.set_default_action_and_target_value("app.open-agent-request", Some(&String::new().to_variant()));
        app.send_notification(Some("agent-batch"), &notification);
    }
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)?.write_all(bytes)
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.add_css_class(class);
    label
}

fn link(text: &str) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("agent-link");
    button.set_valign(gtk::Align::Center);
    button
}

fn date_time(ms: Option<i64>) -> String {
    ms.and_then(|ms| glib::DateTime::from_unix_local(ms / 1000).ok())
        .and_then(|date| date.format("%e %b %Y at %H:%M").ok())
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// As the macOS inbox list shows times (`AgentRequestRows.time`): today the
/// time, older the date as the Linux inbox shows it.
pub(super) fn short_time(ms: Option<i64>) -> String {
    let Some(ms) = ms else { return String::new() };
    let Some(date) = glib::DateTime::from_unix_local(ms / 1000).ok() else { return String::new() };
    let today = glib::DateTime::now_local().ok().and_then(|now| now.format("%Y%m%d").ok());
    if date.format("%Y%m%d").ok() == today {
        date.format("%H:%M").map(|t| t.to_string()).unwrap_or_default()
    } else {
        super::message_time(ms)
    }
}

fn size(bytes: u64) -> String {
    glib::format_size(bytes).to_string()
}

pub(super) fn status_title(status: &str) -> &'static str {
    match status {
        "approved" => "Approved",
        "rejected" => "Declined",
        "edited" => "Opened to edit",
        "failed" => "Could not apply",
        "blocked" => "Blocked",
        "replaced" => "Replaced by a newer request",
        "pending" => "Waiting",
        _ => "Done",
    }
}

/// "Name <address>" when the agent gave a name, so the real address is always
/// visible next to it.
fn addresses(details: &Value, key: &str) -> String {
    match details["addresses"][key].as_array() {
        Some(parts) => parts
            .iter()
            .map(|part| {
                let address = part["address"].as_str().unwrap_or_default();
                match part["name"].as_str().filter(|n| !n.is_empty()) {
                    Some(name) => format!("{name} <{address}>"),
                    None => address.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        None => details["draft"][key]
            .as_array()
            .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
            .unwrap_or_default(),
    }
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

// MARK: Agent requests in the main window

/// The rows for Agent requests, kept for the reader. Called by `Ui::refresh`.
pub(super) fn request_rows(ui: &Ui) -> Vec<Mail> {
    let rows: Vec<Value> = ui
        .request(json!({"op":"agent_requests"}))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    TOTAL.with(|t| t.set(rows.len()));
    let mails = rows
        .iter()
        .map(|row| Mail {
            thread_id: row["id"].as_str().unwrap_or_default().to_string(),
            account_id: String::new(),
            account: String::new(),
            sender: row["agent"].as_str().unwrap_or("An agent").to_string(),
            subject: row["title"].as_str().unwrap_or_default().to_string(),
            preview: row["preview"].as_str().unwrap_or_default().to_string(),
            received: short_time(row["created_at_ms"].as_i64()),
            unread: false,
            labels: Vec::new(),
        })
        .collect();
    REQUESTS.with(|slot| *slot.borrow_mut() = rows);
    mails
}

/// One row: the agent and the time, the title, and one preview line.
pub(super) fn request_row(item: &Mail) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let content = gtk::Box::new(Orientation::Vertical, 0);
    content.add_css_class("agent-request-row");
    let top = gtk::Box::new(Orientation::Horizontal, 8);
    let agent = gtk::Label::new(Some(&item.sender));
    agent.add_css_class("agent-row-agent");
    agent.set_xalign(0.0);
    agent.set_hexpand(true);
    agent.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let time = gtk::Label::new(Some(&item.received));
    time.add_css_class("account");
    top.append(&agent);
    top.append(&time);
    let title = gtk::Label::new(Some(&item.subject));
    title.add_css_class("subject");
    title.add_css_class("agent-row-title");
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let preview = gtk::Label::new(Some(&item.preview));
    preview.add_css_class("preview");
    preview.set_xalign(0.0);
    preview.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&top);
    content.append(&title);
    content.append(&preview);
    row.set_child(Some(&content));
    row.update_property(&[gtk::accessible::Property::Label(&format!("{}: {}", item.sender, item.subject))]);
    row
}

/// The reader for one request: a strip saying what the agent wants with the
/// buttons, then the email exactly as it will go out (or the conversations a
/// bulk change touches, or the account a refused agent wants).
pub(super) struct Reader {
    pub(super) root: gtk::Box,
    strip: gtk::Box,
    ask: gtk::Label,
    buttons: gtk::Box,
    scroll: gtk::ScrolledWindow,
    content: gtk::Box,
    keys: gtk::Box,
    empty: gtk::Label,
    undo_bar: gtk::Box,
    undo_text: gtk::Label,
    undo_action: gtk::Button,
    undo_handler: RefCell<Option<Rc<dyn Fn()>>>,
    undo_generation: Cell<u64>,
    approve: RefCell<Option<gtk::Button>>,
    request: RefCell<Option<Value>>,
    /// A decision is accepted only a moment after a request appears, so a
    /// double press can't approve the next one by accident.
    ready: Cell<bool>,
    ready_generation: Cell<u64>,
    busy: Cell<bool>,
}

/// The request reader, created once and placed next to the mail reader.
pub(super) fn request_reader() -> gtk::Box {
    let root = gtk::Box::new(Orientation::Vertical, 0);
    root.add_css_class("agent-reader");
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_visible(false);
    let strip = gtk::Box::new(Orientation::Horizontal, 12);
    strip.add_css_class("agent-strip");
    let ask = gtk::Label::new(None);
    ask.set_xalign(0.0);
    ask.set_hexpand(true);
    ask.set_wrap(true);
    ask.add_css_class("agent-ask");
    let buttons = gtk::Box::new(Orientation::Horizontal, 8);
    buttons.set_valign(gtk::Align::Center);
    strip.append(&ask);
    strip.append(&buttons);
    root.append(&strip);
    let overlay = gtk::Overlay::new();
    overlay.set_vexpand(true);
    let content = gtk::Box::new(Orientation::Vertical, 0);
    content.add_css_class("agent-mail");
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&content));
    overlay.set_child(Some(&scroll));
    let empty = gtk::Label::new(Some("Nothing waiting"));
    empty.add_css_class("agent-empty");
    empty.set_halign(gtk::Align::Center);
    empty.set_valign(gtk::Align::Center);
    overlay.add_overlay(&empty);
    let undo_bar = gtk::Box::new(Orientation::Horizontal, 10);
    undo_bar.add_css_class("agent-undo");
    undo_bar.set_halign(gtk::Align::Center);
    undo_bar.set_valign(gtk::Align::End);
    undo_bar.set_margin_bottom(12);
    let undo_text = gtk::Label::new(None);
    let undo_action = link("Undo");
    undo_bar.append(&undo_text);
    undo_bar.append(&undo_action);
    undo_bar.set_visible(false);
    overlay.add_overlay(&undo_bar);
    root.append(&overlay);
    let keys = gtk::Box::new(Orientation::Horizontal, 18);
    keys.add_css_class("agent-keys");
    root.append(&keys);
    let reader = Rc::new(Reader {
        root: root.clone(),
        strip,
        ask,
        buttons,
        scroll,
        content,
        keys,
        empty,
        undo_bar,
        undo_text,
        undo_action: undo_action.clone(),
        undo_handler: RefCell::new(None),
        undo_generation: Cell::new(0),
        approve: RefCell::new(None),
        request: RefCell::new(None),
        ready: Cell::new(false),
        ready_generation: Cell::new(0),
        busy: Cell::new(false),
    });
    let weak = Rc::downgrade(&reader);
    undo_action.connect_clicked(move |_| {
        let handler = weak.upgrade().and_then(|reader| reader.undo_handler.borrow().clone());
        if let Some(handler) = handler {
            handler();
        }
    });
    reader.show(None, None);
    READER.with(|slot| *slot.borrow_mut() = Some(reader));
    root
}

fn reader() -> Option<Rc<Reader>> {
    READER.with(|slot| slot.borrow().clone())
}

/// Show the request reader in place of the mail reader while the view is
/// Agent requests (macOS covers the reader the same way).
pub(super) fn set_reader_visible(visible: bool) {
    if let Some(reader) = reader() {
        reader.root.set_visible(visible);
    }
}

/// Show the selected row's request, or the empty state.
pub(super) fn show_selected(ui: &Rc<Ui>) {
    let id = ui.selected().and_then(|i| ui.items.borrow().get(i).map(|m| m.thread_id.clone()));
    let request = id.and_then(|id| REQUESTS.with(|slot| slot.borrow().iter().find(|r| r["id"] == id.as_str()).cloned()));
    if let Some(reader) = reader() {
        reader.show(Some(ui), request);
    }
}

/// Re-read the list after a decision, keeping the row in the decided one's
/// place so J and K pick up where you were.
fn refresh_keeping_place(ui: &Rc<Ui>) {
    if !matches!(*ui.view.borrow(), MailView::AgentRequests) {
        return;
    }
    let index = ui.list.selected_row().map(|r| r.index()).unwrap_or(0);
    let current = ui.selected().and_then(|i| ui.items.borrow().get(i).map(|m| m.thread_id.clone()));
    ui.refresh();
    let still_there = current.is_some_and(|id| ui.items.borrow().iter().any(|m| m.thread_id == id));
    if !still_there {
        let last = ui.items.borrow().len() as i32 - 1;
        if let Some(row) = ui.list.row_at_index(index.min(last).max(0)) {
            ui.list.select_row(Some(&row));
        }
    }
    show_selected(ui);
}

impl Reader {
    fn show(self: &Rc<Self>, ui: Option<&Rc<Ui>>, request: Option<Value>) {
        let same = request
            .as_ref()
            .is_some_and(|r| self.request.borrow().as_ref().is_some_and(|c| c["id"] == r["id"]));
        *self.request.borrow_mut() = request.clone();
        let Some(request) = request else {
            self.strip.set_visible(false);
            self.scroll.set_visible(false);
            self.keys.set_visible(false);
            self.empty.set_visible(true);
            *self.approve.borrow_mut() = None;
            return;
        };
        self.strip.set_visible(true);
        self.scroll.set_visible(true);
        self.keys.set_visible(true);
        self.empty.set_visible(false);
        self.ask.set_markup(&format!(
            "<b>{}</b> <span alpha=\"62%\">{}</span>",
            glib::markup_escape_text(request["agent"].as_str().unwrap_or("An agent")),
            glib::markup_escape_text(request["ask"].as_str().unwrap_or_default())
        ));
        if let Some(ui) = ui {
            self.build_buttons(ui, &request);
            self.build_content(ui, &request);
        }
        self.build_keys(&request);
        if !same {
            self.scroll.vadjustment().set_value(0.0);
            self.set_ready(false);
            let generation = self.ready_generation.get() + 1;
            self.ready_generation.set(generation);
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                if let Some(reader) = weak.upgrade()
                    && reader.ready_generation.get() == generation
                {
                    reader.set_ready(true);
                }
            });
        } else {
            self.set_ready(self.ready.get());
        }
    }

    /// Dimmed, not insensitive, so its label stays readable; clicks and
    /// Ctrl+Enter wait for `ready`.
    fn set_ready(&self, ready: bool) {
        self.ready.set(ready);
        if let Some(button) = self.approve.borrow().as_ref() {
            if ready {
                button.remove_css_class("agent-not-ready");
            } else {
                button.add_css_class("agent-not-ready");
            }
        }
    }

    fn build_buttons(self: &Rc<Self>, ui: &Rc<Ui>, request: &Value) {
        while let Some(child) = self.buttons.first_child() {
            self.buttons.remove(&child);
        }
        let reject = gtk::Button::with_label(request["reject_label"].as_str().unwrap_or("Reject"));
        reject.add_css_class("agent-ghost");
        let weak = Rc::downgrade(ui);
        reject.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                reject_current(&ui);
            }
        });
        self.buttons.append(&reject);
        if request["can_edit"].as_bool() == Some(true) {
            let edit = gtk::Button::with_label("Edit");
            edit.add_css_class("agent-button");
            let weak = Rc::downgrade(ui);
            edit.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    edit_current(&ui);
                }
            });
            self.buttons.append(&edit);
        }
        let title = request["approve_label"].as_str().unwrap_or("Approve").to_string();
        let approve = gtk::Button::new();
        let text = if request["type"] == "access" {
            glib::markup_escape_text(&title).to_string()
        } else {
            format!("{}  <span size=\"85%\" alpha=\"60%\">Ctrl ↵</span>", glib::markup_escape_text(&title))
        };
        let markup = gtk::Label::new(None);
        markup.set_markup(&text);
        approve.set_child(Some(&markup));
        approve.add_css_class("agent-approve");
        approve.update_property(&[gtk::accessible::Property::Label(&format!("{title}, Ctrl+Enter"))]);
        let weak = Rc::downgrade(ui);
        approve.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                approve_current(&ui);
            }
        });
        self.buttons.append(&approve);
        *self.approve.borrow_mut() = Some(approve);
    }

    fn build_keys(&self, request: &Value) {
        while let Some(child) = self.keys.first_child() {
            self.keys.remove(&child);
        }
        let mut items = vec![("Ctrl ↵", request["approve_label"].as_str().unwrap_or("Approve").to_string())];
        if request["can_edit"].as_bool() == Some(true) {
            items.push(("E", "Edit".into()));
        }
        items.push(("⌫", request["reject_label"].as_str().unwrap_or("Reject").to_string()));
        items.push(("J K", "Next request".into()));
        for (key, text) in items {
            let item = gtk::Box::new(Orientation::Horizontal, 6);
            let cap = gtk::Label::new(Some(key));
            cap.add_css_class("agent-keycap");
            item.append(&cap);
            let text = gtk::Label::new(Some(&text));
            text.add_css_class("agent-keytext");
            item.append(&text);
            self.keys.append(&item);
        }
    }

    fn build_content(self: &Rc<Self>, ui: &Rc<Ui>, request: &Value) {
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
        let details = &request["details"];
        match request["type"].as_str() {
            Some("send") => {
                let draft = &details["draft"];
                let subject = draft["subject"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no subject)");
                self.content.append(&label(subject, "agent-mail-subject"));
                let header = gtk::Grid::new();
                header.set_row_spacing(5);
                header.set_column_spacing(10);
                header.set_margin_top(14);
                let mut rows = vec![("From", draft["from"].as_str().unwrap_or_default().to_string())];
                for (name, key) in [("To", "to"), ("Cc", "cc"), ("Bcc", "bcc"), ("Reply-To", "reply_to")] {
                    let value = addresses(details, key);
                    if !value.is_empty() || key == "to" {
                        rows.push((name, value));
                    }
                }
                if request["kind"] == "schedule_draft" {
                    rows.push(("When", date_time(details["send_at_ms"].as_i64())));
                }
                for (index, (name, value)) in rows.iter().enumerate() {
                    let key = label(name, "agent-mail-key");
                    key.set_hexpand(false);
                    key.set_width_chars(7);
                    header.attach(&key, 0, index as i32, 1, 1);
                    let field = label(value, "agent-mail-field");
                    field.set_selectable(true);
                    header.attach(&field, 1, index as i32, 1, 1);
                }
                self.content.append(&header);
                let invisible = details["invisible_count"].as_u64().unwrap_or(0);
                if invisible > 0 {
                    let note = label(
                        &format!("Contains {invisible} invisible character{} used by emoji or some scripts.", if invisible == 1 { "" } else { "s" }),
                        "agent-note",
                    );
                    note.set_margin_top(10);
                    self.content.append(&note);
                }
                let body = draft["body_plain"].as_str().unwrap_or_default();
                let text = label(if body.is_empty() { "(empty message)" } else { body }, "agent-mail-body");
                text.set_selectable(true);
                text.set_margin_top(20);
                self.content.append(&text);
                let files = draft["attachments"].as_array().cloned().unwrap_or_default();
                if !files.is_empty() {
                    let chips = gtk::Box::new(Orientation::Horizontal, 8);
                    chips.set_margin_top(18);
                    chips.set_halign(gtk::Align::Start);
                    for file in &files {
                        let filename = file["filename"].as_str().unwrap_or("file").to_string();
                        let chip = gtk::Button::new();
                        chip.add_css_class("agent-chip");
                        chip.set_tooltip_text(Some("Save a copy"));
                        chip.update_property(&[gtk::accessible::Property::Label(&format!("Save a copy of {filename}"))]);
                        let inner = gtk::Box::new(Orientation::Horizontal, 8);
                        let icon = gtk::Label::new(Some("▮"));
                        icon.add_css_class("agent-chip-icon");
                        inner.append(&icon);
                        let names = gtk::Box::new(Orientation::Vertical, 1);
                        let name = gtk::Label::new(Some(&filename));
                        name.set_xalign(0.0);
                        let meta = gtk::Label::new(Some(&size(file["size"].as_u64().unwrap_or(0))));
                        meta.add_css_class("agent-note");
                        meta.set_xalign(0.0);
                        names.append(&name);
                        names.append(&meta);
                        inner.append(&names);
                        chip.set_child(Some(&inner));
                        let weak = Rc::downgrade(ui);
                        let action_id = request["id"].as_str().unwrap_or_default().to_string();
                        let attachment_id = file["id"].as_str().unwrap_or_default().to_string();
                        chip.connect_clicked(move |_| {
                            if let Some(ui) = weak.upgrade() {
                                save_attachment(&ui, &action_id, &attachment_id, &filename);
                            }
                        });
                        chips.append(&chip);
                    }
                    self.content.append(&chips);
                }
            }
            Some("access") => {
                self.content.append(&label(request["preview"].as_str().unwrap_or_default(), "agent-mail-muted"));
            }
            _ => {
                let card = gtk::Box::new(Orientation::Vertical, 0);
                card.add_css_class("agent-rows");
                let threads = request["threads"].as_array().cloned().unwrap_or_default();
                for (index, thread) in threads.iter().enumerate() {
                    if index > 0 {
                        card.append(&gtk::Separator::new(Orientation::Horizontal));
                    }
                    let row = gtk::Box::new(Orientation::Vertical, 1);
                    row.add_css_class("agent-rows-row");
                    let top = gtk::Box::new(Orientation::Horizontal, 8);
                    let who = gtk::Label::new(Some(sender_name(thread["sender"].as_str().unwrap_or_default())));
                    who.add_css_class("agent-row-agent");
                    who.set_xalign(0.0);
                    who.set_hexpand(true);
                    who.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    let when = gtk::Label::new(Some(&short_time(thread["last_message_at"].as_i64())));
                    when.add_css_class("account");
                    top.append(&who);
                    top.append(&when);
                    let what = gtk::Label::new(Some(thread["subject"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no subject)")));
                    what.add_css_class("agent-rows-subject");
                    what.set_xalign(0.0);
                    what.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    let snippet = gtk::Label::new(Some(thread["snippet"].as_str().unwrap_or_default()));
                    snippet.add_css_class("preview");
                    snippet.set_xalign(0.0);
                    snippet.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    row.append(&top);
                    row.append(&what);
                    row.append(&snippet);
                    card.append(&row);
                }
                self.content.append(&card);
                let count = details["count"].as_u64().unwrap_or(0) as usize;
                if count > threads.len() {
                    let more = label(&format!("and {} more", count - threads.len()), "agent-mail-muted");
                    more.set_margin_top(10);
                    self.content.append(&more);
                }
                if let Some(note) = request["note"].as_str() {
                    let note = label(note, "agent-mail-muted");
                    note.set_margin_top(12);
                    self.content.append(&note);
                }
            }
        }
    }

    /// After a decision: what happened and its undo, until `until_ms` passes.
    fn show_undo(self: &Rc<Self>, text: &str, action: &str, until_ms: Option<i64>, handler: Rc<dyn Fn()>) {
        let generation = self.undo_generation.get() + 1;
        self.undo_generation.set(generation);
        self.undo_text.set_text(text);
        self.undo_action.set_label(action);
        *self.undo_handler.borrow_mut() = Some(handler);
        self.undo_bar.set_visible(true);
        if let Some(until) = until_ms {
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(std::time::Duration::from_millis((until - now_ms()).max(0) as u64), move || {
                if let Some(reader) = weak.upgrade()
                    && reader.undo_generation.get() == generation
                {
                    reader.hide_undo();
                }
            });
        }
    }

    fn hide_undo(&self) {
        self.undo_bar.set_visible(false);
        *self.undo_handler.borrow_mut() = None;
    }
}

fn current() -> Option<Value> {
    reader().and_then(|reader| reader.request.borrow().clone())
}

/// Ctrl+Enter, E and Backspace on a request; mail shortcuts that would act on
/// a conversation do nothing here. Returns whether the key was taken.
pub(super) fn handle_key(ui: &Rc<Ui>, key: gdk::Key, mods: gdk::ModifierType) -> bool {
    let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
    if ctrl && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
        approve_current(ui);
        return true;
    }
    if mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) {
        return false;
    }
    match key {
        gdk::Key::e | gdk::Key::E => {
            if current().is_some_and(|r| r["can_edit"].as_bool() == Some(true)) {
                edit_current(ui);
            }
            true
        }
        gdk::Key::BackSpace | gdk::Key::Delete | gdk::Key::KP_Delete => {
            reject_current(ui);
            true
        }
        gdk::Key::x
        | gdk::Key::m
        | gdk::Key::u
        | gdk::Key::f
        | gdk::Key::r
        | gdk::Key::R
        | gdk::Key::s
        | gdk::Key::l
        | gdk::Key::z
        | gdk::Key::exclam
        | gdk::Key::numbersign
        | gdk::Key::Return => true,
        _ => false,
    }
}

fn approve_current(ui: &Rc<Ui>) {
    let (Some(reader), Some(request)) = (reader(), current()) else { return };
    if !reader.ready.get() || reader.busy.get() {
        return;
    }
    if request["type"] == "access" {
        return allow(ui, &request);
    }
    let Some(id) = request["id"].as_str().map(str::to_owned) else { return };
    let kind = request["kind"].as_str().unwrap_or_default().to_string();
    let accounts = strings(&request["account_ids"]);
    reader.busy.set(true);
    let ui = ui.clone();
    workflows::request_async(
        ui.database_path.clone(),
        json!({"op":"approve_agent_action","id":id,"undo_seconds":UNDO_SECONDS}),
        move |result| {
            reader.busy.set(false);
            match result {
                Ok(action) => {
                    let result = &action["result"];
                    match kind.as_str() {
                        "send_draft" | "queue_send_draft" | "schedule_draft" => {
                            let message_id = result["message_id"].as_str().unwrap_or_default().to_string();
                            let send_at = result["send_at_ms"].as_i64().unwrap_or(0);
                            let scheduled = result["status"].as_str() == Some("scheduled");
                            // The composer's undo affordance, here and in the reader.
                            *ui.last_queued_draft.borrow_mut() = Some(message_id.clone());
                            ui.undo_button.set_visible(true);
                            let weak = Rc::downgrade(&ui);
                            let cancel: Rc<dyn Fn()> = Rc::new(move || {
                                if let Some(ui) = weak.upgrade() {
                                    cancel_send(&ui, &message_id);
                                }
                            });
                            if scheduled {
                                reader.show_undo(&format!("Scheduled for {}", date_time(Some(send_at))), "Cancel", None, cancel);
                            } else {
                                let seconds = ((send_at - now_ms()) as f64 / 1000.0).round().max(1.0) as i64;
                                reader.show_undo(&format!("Sending in {seconds} seconds"), "Undo", Some(send_at), cancel);
                            }
                        }
                        "batch_triage" => {
                            if result["undoable"].as_bool() == Some(true) {
                                let operation = result["operation_id"].as_str().unwrap_or_default().to_string();
                                *ui.last_triage.borrow_mut() = Some((operation.clone(), accounts.clone()));
                                ui.update_selection_bar();
                                let weak = Rc::downgrade(&ui);
                                let undo: Rc<dyn Fn()> = Rc::new(move || {
                                    if let Some(ui) = weak.upgrade() {
                                        undo_batch(&ui, &operation);
                                    }
                                });
                                reader.show_undo("Done", "Undo", Some(now_ms() + 30_000), undo);
                            }
                        }
                        "undo_triage" => {
                            let operation = result["operation_id"].as_str().unwrap_or_default();
                            if ui.last_triage.borrow().as_ref().is_some_and(|(id, _)| id == operation) {
                                *ui.last_triage.borrow_mut() = None;
                                ui.update_selection_bar();
                            }
                        }
                        _ => {}
                    }
                }
                Err(error) => ui.status.set_text(&error),
            }
            refresh_keeping_place(&ui);
            poll(&ui);
        },
    );
}

fn reject_current(ui: &Rc<Ui>) {
    let (Some(reader), Some(request)) = (reader(), current()) else { return };
    if reader.busy.get() {
        return;
    }
    reader.busy.set(true);
    let op = if request["type"] == "access" {
        json!({"op":"dismiss_agent_access_request","account_id":request["account_id"]})
    } else {
        json!({"op":"reject_agent_action","id":request["id"]})
    };
    let ui = ui.clone();
    workflows::request_async(ui.database_path.clone(), op, move |result| {
        reader.busy.set(false);
        if let Err(error) = result {
            ui.status.set_text(&error);
        }
        refresh_keeping_place(&ui);
        poll(&ui);
    });
}

/// Edit closes the request without sending and opens its draft in the
/// composer; only the user's own Send sends it.
fn edit_current(ui: &Rc<Ui>) {
    let (Some(reader), Some(request)) = (reader(), current()) else { return };
    if reader.busy.get() {
        return;
    }
    reader.busy.set(true);
    let ui = ui.clone();
    workflows::request_async(
        ui.database_path.clone(),
        json!({"op":"edit_agent_action","id":request["id"]}),
        move |result| {
            reader.busy.set(false);
            match result {
                Ok(action) => {
                    let message_id = action["details"]["request"]["message_id"].as_str().unwrap_or_default().to_string();
                    ui.search.set_text("");
                    ui.set_view(MailView::Drafts);
                    let index = ui.items.borrow().iter().position(|m| m.thread_id == message_id);
                    if let Some(row) = index.and_then(|i| ui.list.row_at_index(i as i32)) {
                        ui.list.select_row(Some(&row));
                        ui.compose(false);
                    }
                    poll(&ui);
                }
                Err(error) => {
                    ui.status.set_text(&error);
                    refresh_keeping_place(&ui);
                }
            }
        },
    );
}

/// Allow… on a refused agent: the turn-on sheet with the level it needs.
fn allow(ui: &Rc<Ui>, request: &Value) {
    let Some(account_id) = request["account_id"].as_str().map(str::to_owned) else { return };
    let level = request["level"].as_str().unwrap_or("read").to_string();
    let grants = strings(&request["grants"]);
    let changing = request["changing"].as_bool() == Some(true);
    let after = ui.clone();
    turn_on(ui, ui.window.upcast_ref(), &account_id, grants, level == "custom", changing, move |_| {
        refresh_keeping_place(&after);
        poll(&after);
        if let Some(panel) = OPEN_PANEL.with(|slot| slot.borrow().clone()) {
            panel.reload();
        }
    });
}

fn cancel_send(ui: &Rc<Ui>, message_id: &str) {
    let ui = ui.clone();
    let id = message_id.to_string();
    workflows::request_async(
        ui.database_path.clone(),
        json!({"op":"cancel_scheduled_draft","message_id":message_id}),
        move |result| match result {
            Ok(_) => {
                if ui.last_queued_draft.borrow().as_deref() == Some(id.as_str()) {
                    *ui.last_queued_draft.borrow_mut() = None;
                    ui.undo_button.set_visible(false);
                }
                ui.status.set_text("Send cancelled · back in Drafts");
                if let Some(reader) = reader() {
                    reader.hide_undo();
                }
            }
            Err(error) => ui.status.set_text(&error),
        },
    );
}

fn undo_batch(ui: &Rc<Ui>, operation: &str) {
    let ui = ui.clone();
    let operation = operation.to_string();
    workflows::request_async(
        ui.database_path.clone(),
        json!({"op":"undo_triage","operation_id":operation}),
        move |result| match result {
            Ok(_) => {
                if ui.last_triage.borrow().as_ref().is_some_and(|(id, _)| *id == operation) {
                    *ui.last_triage.borrow_mut() = None;
                    ui.update_selection_bar();
                }
                if let Some(reader) = reader() {
                    reader.hide_undo();
                }
            }
            Err(error) => ui.status.set_text(&error),
        },
    );
}

fn save_attachment(ui: &Rc<Ui>, action_id: &str, attachment_id: &str, filename: &str) {
    let ui = ui.clone();
    let filename = filename.to_string();
    workflows::request_async(
        ui.database_path.clone(),
        json!({"op":"agent_request_attachment","id":action_id,"attachment_id":attachment_id}),
        move |result| {
            let Ok(bytes) = result.and_then(|file| {
                STANDARD
                    .decode(file["data_base64"].as_str().unwrap_or_default())
                    .map_err(|_| "The attachment could not be read.".to_string())
            }) else {
                return;
            };
            let dialog = gtk::FileDialog::builder().initial_name(&filename).modal(true).build();
            let status = ui.status.clone();
            dialog.save(Some(&ui.window), None::<&gtk::gio::Cancellable>, move |chosen| {
                let Ok(file) = chosen else { return };
                let Some(path) = file.path() else { return };
                // Written without execute permission: a copy to look at.
                if let Err(error) = std::fs::write(&path, &bytes) {
                    status.set_text(&error.to_string());
                }
            });
        },
    );
}

/// From a notification or the settings window: the main window on that request.
pub(super) fn open_request(ui: &Rc<Ui>, id: Option<&str>) {
    ui.window.present();
    ui.search.set_text("");
    ui.set_view(MailView::AgentRequests);
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        let index = ui.items.borrow().iter().position(|m| m.thread_id == id);
        if let Some(row) = index.and_then(|i| ui.list.row_at_index(i as i32)) {
            ui.list.select_row(Some(&row));
        }
    }
    show_selected(ui);
}

// MARK: The turn-on sheet

/// Turning an account on or allowing more, from the settings dropdown or from
/// Allow… on a request: show the sheet, record the choice.
fn turn_on(
    ui: &Rc<Ui>,
    parent: &gtk::Window,
    account_id: &str,
    grants: Vec<String>,
    custom: bool,
    changing: bool,
    done: impl Fn(bool) + 'static,
) {
    let disclosure = match ui.request(json!({"op":"agent_access_disclosure","account_id":account_id})) {
        Ok(value) => value,
        Err(error) => {
            ui.status.set_text(&error);
            return done(false);
        }
    };
    let ui = ui.clone();
    let account_id = account_id.to_string();
    let version = disclosure["version"].clone();
    disclosure_dialog(parent, &disclosure, grants, custom, changing, move |chosen| {
        let Some(grants) = chosen else { return done(false) };
        let granted = ui
            .request(json!({"op":"grant_agent_access","account_id":account_id,"grants":grants,"disclosure_version":version.clone()}))
            .is_ok();
        done(granted);
    });
}

/// "Let agents use {account}?": three choices with Read only as the default,
/// the provider-terms note, Custom… for the individual switches (calendar
/// changes only there) and "What can agents see?" for the full list.
/// `chosen` runs once with the grants, or None when cancelled.
pub(super) fn disclosure_dialog(
    parent: &gtk::Window,
    disclosure: &Value,
    initial: Vec<String>,
    custom: bool,
    changing: bool,
    chosen: impl Fn(Option<Vec<String>>) + 'static,
) -> gtk::Window {
    let dialog = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Agent access")
        .default_width(470)
        .build();
    dialog.add_css_class("agent-window");
    dialog.add_css_class("agent-sheet");
    let root = gtk::Box::new(Orientation::Vertical, 0);
    let column = gtk::Box::new(Orientation::Vertical, 8);
    column.set_margin_top(22);
    column.set_margin_bottom(6);
    column.set_margin_start(24);
    column.set_margin_end(24);
    column.append(&label(disclosure["title"].as_str().unwrap_or("Agent access"), "agent-sheet-title"));
    let levels: Vec<(String, Vec<String>)> = disclosure["levels"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| (l["id"].as_str().unwrap_or_default().to_string(), strings(&l["grants"])))
        .collect();
    let mut radios: Vec<(String, gtk::CheckButton, gtk::Box)> = Vec::new();
    let mut group: Option<gtk::CheckButton> = None;
    for (index, level) in disclosure["levels"].as_array().into_iter().flatten().enumerate() {
        let id = level["id"].as_str().unwrap_or_default().to_string();
        let card = gtk::Box::new(Orientation::Horizontal, 0);
        card.add_css_class("agent-option");
        if index == 0 {
            card.set_margin_top(10);
        }
        let radio = gtk::CheckButton::new();
        if let Some(group) = &group {
            radio.set_group(Some(group));
        } else {
            group = Some(radio.clone());
        }
        let text = gtk::Box::new(Orientation::Vertical, 1);
        text.append(&label(level["label"].as_str().unwrap_or(&id), "agent-option-title"));
        text.append(&label(level["sub"].as_str().unwrap_or_default(), "agent-option-sub"));
        radio.set_child(Some(&text));
        radio.set_hexpand(true);
        radio.set_widget_name(&format!("agent-level-{id}"));
        card.append(&radio);
        column.append(&card);
        radios.push((id, radio, card));
    }
    let custom_box = gtk::Box::new(Orientation::Vertical, 8);
    custom_box.set_margin_top(10);
    let mut switches: Vec<(String, gtk::Switch)> = Vec::new();
    for grant in disclosure["custom"].as_array().into_iter().flatten() {
        let id = grant["id"].as_str().unwrap_or_default().to_string();
        let row = gtk::Box::new(Orientation::Horizontal, 9);
        let switch = gtk::Switch::new();
        switch.set_valign(gtk::Align::Center);
        switch.set_widget_name(&format!("agent-grant-{id}"));
        let name = grant["label"].as_str().unwrap_or(&id).to_string();
        if let Some(detail) = grant["detail"].as_str() {
            switch.set_tooltip_text(Some(detail));
            switch.update_property(&[gtk::accessible::Property::Label(&name), gtk::accessible::Property::Description(detail)]);
        }
        row.append(&switch);
        let title = gtk::Label::new(Some(&name));
        title.add_css_class("agent-check-label");
        row.append(&title);
        if let Some(note) = grant["note"].as_str() {
            let note = gtk::Label::new(Some(&format!("·  {note}")));
            note.add_css_class(if grant["warning"].as_bool() == Some(true) { "agent-check-warning" } else { "agent-check-note" });
            row.append(&note);
        }
        custom_box.append(&row);
        switches.push((id, switch));
    }
    custom_box.set_visible(custom);
    column.append(&custom_box);
    let note = label(disclosure["note"].as_str().unwrap_or_default(), "agent-note");
    note.set_margin_top(10);
    column.append(&note);
    let details = gtk::Box::new(Orientation::Vertical, 6);
    details.set_margin_top(8);
    for item in disclosure["details"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let row = gtk::Box::new(Orientation::Horizontal, 8);
        let bullet = gtk::Label::new(Some("•"));
        bullet.set_valign(gtk::Align::Start);
        bullet.add_css_class("agent-detail");
        row.append(&bullet);
        row.append(&label(item, "agent-detail"));
        details.append(&row);
    }
    details.set_visible(false);
    column.append(&details);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_propagate_natural_height(true);
    scroll.set_max_content_height(600);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&column));
    root.append(&scroll);

    // Radios and switches follow each other: a choice sets the switches; a
    // switch selects the matching choice, or none.
    let syncing = Rc::new(Cell::new(false));
    let radios = Rc::new(radios);
    let switches = Rc::new(switches);
    let levels = Rc::new(levels);
    let set_switches: Rc<dyn Fn(&[String])> = {
        let (switches, syncing) = (switches.clone(), syncing.clone());
        Rc::new(move |grants: &[String]| {
            syncing.set(true);
            for (id, switch) in switches.iter() {
                switch.set_active(grants.contains(id));
            }
            syncing.set(false);
        })
    };
    let sync_radios: Rc<dyn Fn()> = {
        let (switches, radios, levels, syncing) = (switches.clone(), radios.clone(), levels.clone(), syncing.clone());
        Rc::new(move || {
            let mut current: Vec<String> = switches.iter().filter(|(_, s)| s.is_active()).map(|(id, _)| id.clone()).collect();
            current.sort();
            syncing.set(true);
            for (id, radio, card) in radios.iter() {
                let matches = levels.iter().find(|(level, _)| level == id).is_some_and(|(_, grants)| {
                    let mut grants = grants.clone();
                    grants.sort();
                    grants == current
                });
                radio.set_active(matches);
                if matches {
                    card.add_css_class("selected");
                } else {
                    card.remove_css_class("selected");
                }
            }
            syncing.set(false);
        })
    };
    for (id, radio, _) in radios.iter() {
        let (levels, syncing, sync_radios, set_switches) = (levels.clone(), syncing.clone(), sync_radios.clone(), set_switches.clone());
        let id = id.clone();
        radio.connect_toggled(move |radio| {
            if syncing.get() || !radio.is_active() {
                return;
            }
            let grants = levels.iter().find(|(level, _)| *level == id).map(|(_, g)| g.clone()).unwrap_or_default();
            set_switches(&grants);
            sync_radios();
        });
    }
    for (_, switch) in switches.iter() {
        let (syncing, sync_radios) = (syncing.clone(), sync_radios.clone());
        switch.connect_active_notify(move |_| {
            if !syncing.get() {
                sync_radios();
            }
        });
    }
    set_switches(&initial);
    sync_radios();

    let actions = gtk::Box::new(Orientation::Horizontal, 8);
    actions.set_margin_top(14);
    actions.set_margin_bottom(18);
    actions.set_margin_start(24);
    actions.set_margin_end(24);
    let custom_link = link(disclosure["custom_link"].as_str().unwrap_or("Custom…"));
    let details_link = link(disclosure["details_link"].as_str().unwrap_or("What can agents see?"));
    let dot = gtk::Label::new(Some("·"));
    dot.add_css_class("agent-note");
    let spacer = gtk::Box::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = gtk::Button::with_label(disclosure["cancel"].as_str().unwrap_or("Cancel"));
    cancel.add_css_class("agent-button");
    let confirm = gtk::Button::with_label(
        disclosure[if changing { "change_confirm" } else { "confirm" }].as_str().unwrap_or("Allow"),
    );
    confirm.add_css_class("agent-approve");
    actions.append(&custom_link);
    actions.append(&dot);
    actions.append(&details_link);
    actions.append(&spacer);
    actions.append(&cancel);
    actions.append(&confirm);
    root.append(&actions);
    dialog.set_child(Some(&root));
    let grow: Rc<dyn Fn()> = {
        let (scroll, column) = (scroll.clone(), column.clone());
        Rc::new(move || {
            // Grow the window to what opened, up to the scroll limit; GTK does
            // not grow a mapped window for a larger natural size on its own.
            let natural = column.measure(Orientation::Vertical, 422).1;
            scroll.set_min_content_height(natural.min(600));
        })
    };
    {
        let (custom_box, grow) = (custom_box.clone(), grow.clone());
        custom_link.connect_clicked(move |_| {
            custom_box.set_visible(!custom_box.is_visible());
            grow();
        });
    }
    {
        let (details, grow) = (details.clone(), grow.clone());
        details_link.connect_clicked(move |_| {
            details.set_visible(!details.is_visible());
            grow();
        });
    }
    let chosen = Rc::new(chosen);
    // Exactly one answer: a choice, Cancel, Escape or closing the window.
    let answered = Rc::new(Cell::new(false));
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let (d, done, answer, chosen_switches) = (dialog.clone(), chosen.clone(), answered.clone(), switches.clone());
    confirm.connect_clicked(move |_| {
        let grants = chosen_switches.iter().filter(|(_, s)| s.is_active()).map(|(id, _)| id.clone()).collect();
        answer.set(true);
        d.close();
        done(Some(grants));
    });
    let (done, answer) = (chosen.clone(), answered.clone());
    dialog.connect_close_request(move |_| {
        if !answer.replace(true) {
            done(None);
        }
        glib::Propagation::Proceed
    });
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
    cancel.grab_focus();
    dialog.present();
    dialog
}

// MARK: The Agent access settings window

pub(super) struct Panel {
    ui: Rc<Ui>,
    pub(super) window: gtk::Window,
    sections: gtk::Box,
    accounts: RefCell<Vec<Value>>,
    history: RefCell<Vec<Value>>,
    levels: RefCell<Vec<Value>>,
    version: RefCell<Value>,
    waiting: Cell<usize>,
    /// The `agent_clients` answer: CLIs found, registrations, the helper.
    clients: RefCell<Option<Value>>,
    clients_loading: Cell<bool>,
    connecting: RefCell<Option<String>>,
    client_errors: RefCell<std::collections::HashMap<String, String>>,
    /// Connected from this window: "Start a new session to use it".
    just_connected: RefCell<std::collections::HashSet<String>>,
    presenting: Cell<bool>,
}

pub(super) fn open(ui: &Rc<Ui>) -> Rc<Panel> {
    if let Some(panel) = OPEN_PANEL.with(|slot| slot.borrow().clone()) {
        panel.window.present();
        panel.reload();
        panel.refresh_clients();
        return panel;
    }
    let window = gtk::Window::builder()
        .title("Agent access")
        .transient_for(&ui.window)
        .default_width(620)
        .default_height(560)
        .build();
    window.set_size_request(480, 420);
    window.set_hide_on_close(true);
    window.add_css_class("agent-window");
    let root = gtk::Box::new(Orientation::Vertical, 0);
    let sections = gtk::Box::new(Orientation::Vertical, 8);
    sections.set_margin_top(18);
    sections.set_margin_bottom(22);
    sections.set_margin_start(30);
    sections.set_margin_end(30);
    sections.set_valign(gtk::Align::Start);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&sections));
    root.append(&scroll);
    root.append(&gtk::Separator::new(Orientation::Horizontal));
    let footer = gtk::Box::new(Orientation::Horizontal, 10);
    footer.set_margin_top(10);
    footer.set_margin_bottom(12);
    footer.set_margin_start(30);
    footer.set_margin_end(30);
    let activity = link("Activity");
    footer.append(&activity);
    let spacer = gtk::Box::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    let done = gtk::Button::with_label("Done");
    done.add_css_class("agent-button");
    done.set_tooltip_text(Some("Close (Esc)"));
    let w = window.clone();
    done.connect_clicked(move |_| w.close());
    footer.append(&done);
    root.append(&footer);
    window.set_child(Some(&root));
    let keys = gtk::EventControllerKey::new();
    let w = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            w.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);
    let panel = Rc::new(Panel {
        ui: ui.clone(),
        window,
        sections,
        accounts: RefCell::new(Vec::new()),
        history: RefCell::new(Vec::new()),
        levels: RefCell::new(Vec::new()),
        version: RefCell::new(Value::Null),
        waiting: Cell::new(0),
        clients: RefCell::new(None),
        clients_loading: Cell::new(false),
        connecting: RefCell::new(None),
        client_errors: RefCell::new(Default::default()),
        just_connected: RefCell::new(Default::default()),
        presenting: Cell::new(false),
    });
    let weak = Rc::downgrade(&panel);
    activity.connect_clicked(move |_| {
        if let Some(panel) = weak.upgrade() {
            panel.show_activity();
        }
    });
    OPEN_PANEL.with(|slot| *slot.borrow_mut() = Some(panel.clone()));
    panel.window.present();
    panel.reload();
    panel.refresh_clients();
    panel
}

/// A list card: rows with a title and sub line on the left, a control on the
/// right, and a hairline between rows.
fn list_card() -> gtk::Box {
    let card = gtk::Box::new(Orientation::Vertical, 0);
    card.add_css_class("agent-list");
    card
}

fn list_row(card: &gtk::Box, title: &str, sub: &str, sub_class: &str, trailing: Option<&gtk::Widget>) {
    if card.first_child().is_some() {
        card.append(&gtk::Separator::new(Orientation::Horizontal));
    }
    let row = gtk::Box::new(Orientation::Horizontal, 12);
    row.add_css_class("agent-list-row");
    let names = gtk::Box::new(Orientation::Vertical, 1);
    names.set_hexpand(true);
    names.set_valign(gtk::Align::Center);
    let heading = label(title, "agent-row-title");
    heading.set_wrap(false);
    heading.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    names.append(&heading);
    if !sub.is_empty() {
        names.append(&label(sub, sub_class));
    }
    row.append(&names);
    if let Some(trailing) = trailing {
        trailing.set_valign(gtk::Align::Center);
        row.append(trailing);
    }
    card.append(&row);
}

impl Panel {
    pub(super) fn reload(self: &Rc<Self>) {
        let panel = self.clone();
        request(&self.ui, json!({"op":"agent_access_overview","history_limit":60}), move |result| {
            let Ok(state) = result else { return };
            *panel.accounts.borrow_mut() = state["accounts"].as_array().cloned().unwrap_or_default();
            *panel.history.borrow_mut() = state["history"].as_array().cloned().unwrap_or_default();
            *panel.levels.borrow_mut() = state["levels"].as_array().cloned().unwrap_or_default();
            *panel.version.borrow_mut() = state["disclosure_version"].clone();
            panel.waiting.set(state["pending"]["total"].as_u64().unwrap_or(0) as usize);
            panel.render();
        });
    }

    /// Look for Claude Code and Codex again. Runs their CLIs on a separate
    /// service, so only when the window opens and after Connect.
    fn refresh_clients(self: &Rc<Self>) {
        if self.clients_loading.replace(true) {
            return;
        }
        let panel = self.clone();
        workflows::request_async(self.ui.database_path.clone(), json!({"op":"agent_clients"}), move |result| {
            panel.clients_loading.set(false);
            if let Ok(state) = result {
                *panel.clients.borrow_mut() = Some(state);
            }
            panel.render();
        });
    }

    fn add(&self, widget: &impl IsA<gtk::Widget>) {
        self.sections.append(widget);
    }

    fn group_label(&self, text: &str) {
        let heading = label(text, "agent-group-label");
        heading.set_margin_top(16);
        self.add(&heading);
    }

    fn render(self: &Rc<Self>) {
        while let Some(child) = self.sections.first_child() {
            self.sections.remove(&child);
        }
        let waiting = self.waiting.get();
        if waiting > 0 {
            let banner = gtk::Box::new(Orientation::Horizontal, 6);
            banner.add_css_class("agent-waiting");
            banner.set_margin_bottom(10);
            banner.append(&gtk::Label::new(Some(&format!(
                "{waiting} request{} waiting",
                if waiting == 1 { "" } else { "s" }
            ))));
            let dot = gtk::Label::new(Some("·"));
            dot.add_css_class("agent-note");
            banner.append(&dot);
            let review = link("Review");
            let ui = self.ui.clone();
            review.connect_clicked(move |_| open_request(&ui, None));
            banner.append(&review);
            self.add(&banner);
        }
        self.add(&label("Agent access", "agent-title"));
        self.add(&label(
            "Let AI tools like Claude Code and Codex use your mail. Nothing is sent without your OK.",
            "agent-lead",
        ));
        self.group_label("What agents can do");
        let accounts = list_card();
        let rows = self.accounts.borrow().clone();
        for account in &rows {
            let dropdown = self.account_dropdown(account);
            list_row(&accounts, account["email"].as_str().unwrap_or_default(), "", "agent-row-subtitle", Some(dropdown.upcast_ref()));
        }
        if rows.is_empty() {
            list_row(&accounts, "Add a mail account first.", "", "agent-row-subtitle", None);
        }
        self.add(&accounts);
        self.group_label("Agents");
        let agents = list_card();
        let clients = self.clients.borrow().clone();
        let rows = clients
            .as_ref()
            .and_then(|c| c["clients"].as_array().cloned())
            .unwrap_or_else(|| vec![json!({"id":"claude","name":"Claude Code"}), json!({"id":"codex","name":"Codex"})]);
        let helper_found = clients.as_ref().is_some_and(|c| c["helper"].is_string());
        for row in rows {
            let id = row["id"].as_str().unwrap_or_default().to_string();
            let name = row["name"].as_str().unwrap_or(&id).to_string();
            let mut sub = if clients.is_none() { String::new() } else { row["subtitle"].as_str().unwrap_or_default().to_string() };
            let trailing: Option<gtk::Widget> = if row["connected"].as_bool() == Some(true) {
                if self.just_connected.borrow().contains(&id) {
                    sub = "Start a new session to use it".into();
                }
                let connected = gtk::Label::new(Some("✓ Connected"));
                connected.add_css_class("agent-connected");
                Some(connected.upcast())
            } else if row["installed"].as_bool() == Some(true) {
                let busy = self.connecting.borrow().as_deref() == Some(id.as_str());
                let button = gtk::Button::with_label(if busy { "Connecting…" } else { "Connect" });
                button.add_css_class("agent-button");
                button.set_sensitive(helper_found && self.connecting.borrow().is_none());
                button.update_property(&[gtk::accessible::Property::Label(&format!("Connect {name}"))]);
                let panel = self.clone();
                let id = id.clone();
                button.connect_clicked(move |_| panel.connect_client(&id));
                Some(button.upcast())
            } else {
                if clients.is_some() {
                    sub = "Not installed".into();
                }
                None
            };
            let error = self.client_errors.borrow().get(&id).cloned();
            let class = if error.is_some() { "agent-error" } else { "agent-row-subtitle" };
            list_row(&agents, &name, error.as_deref().unwrap_or(&sub), class, trailing.as_ref());
        }
        let setup = gtk::Button::with_label("Set up…");
        setup.add_css_class("agent-button");
        setup.set_sensitive(clients.as_ref().is_some_and(|c| c["manual"].is_object()));
        setup.update_property(&[gtk::accessible::Property::Label("Set up another app: copies the command and JSON config")]);
        let panel = self.clone();
        setup.connect_clicked(move |button| panel.copy_manual_config(button));
        list_row(&agents, "Another app", "Any app that supports MCP", "agent-row-subtitle", Some(setup.upcast_ref()));
        self.add(&agents);
        if let Some(problem) = clients.as_ref().and_then(|c| c["helper_problem"].as_str().map(str::to_owned)) {
            let note = label(&problem, "agent-note");
            note.set_margin_top(8);
            self.add(&note);
        }
    }

    fn account_dropdown(self: &Rc<Self>, account: &Value) -> gtk::DropDown {
        let id = account["account_id"].as_str().unwrap_or_default().to_string();
        let email = account["email"].as_str().unwrap_or(&id).to_string();
        let level = account["level"].as_str().unwrap_or("off").to_string();
        let grants = strings(&account["consent"]["grants"]);
        let mut choices: Vec<(String, String)> = vec![("off".into(), "Off".into())];
        for level in self.levels.borrow().iter() {
            choices.push((level["id"].as_str().unwrap_or_default().into(), level["label"].as_str().unwrap_or_default().into()));
        }
        choices.push(("custom".into(), "Custom…".into()));
        let titles: Vec<&str> = choices.iter().map(|(_, title)| title.as_str()).collect();
        let dropdown = gtk::DropDown::from_strings(&titles);
        dropdown.add_css_class("agent-dropdown");
        dropdown.set_selected(choices.iter().position(|(choice, _)| *choice == level).unwrap_or(0) as u32);
        dropdown.update_property(&[gtk::accessible::Property::Label(&format!("What agents can do with {email}"))]);
        let panel = self.clone();
        dropdown.connect_selected_notify(move |dropdown| {
            let Some((chosen, _)) = choices.get(dropdown.selected() as usize).cloned() else { return };
            let panel = panel.clone();
            let (id, email, level, grants) = (id.clone(), email.clone(), level.clone(), grants.clone());
            // After the dropdown's own handler, so the window can re-render.
            glib::idle_add_local_once(move || panel.choose(&chosen, &id, &email, &level, &grants));
        });
        dropdown
    }

    /// The dropdown's choice. Off asks first; turning on or allowing more
    /// opens the sheet with that choice; allowing less applies at once.
    fn choose(self: &Rc<Self>, chosen: &str, account_id: &str, email: &str, level: &str, grants: &[String]) {
        if chosen == level && chosen != "custom" {
            return;
        }
        if self.presenting.replace(true) {
            return;
        }
        if chosen == "off" {
            let confirm = gtk::AlertDialog::builder()
                .message(format!("Turn off agent access for {email}?"))
                .detail("Agents lose access right away, and requests waiting for you are dropped. Mail you already approved stays in the Outbox.")
                .buttons(["Cancel", "Turn off"])
                .cancel_button(0)
                .default_button(0)
                .build();
            let panel = self.clone();
            let account_id = account_id.to_string();
            confirm.choose(Some(&self.window), None::<&gtk::gio::Cancellable>, move |result| {
                panel.presenting.set(false);
                if result == Ok(1) {
                    let _ = panel.ui.request(json!({"op":"revoke_agent_access","account_id":account_id}));
                    poll(&panel.ui);
                }
                panel.reload();
            });
            return;
        }
        let preset: Option<Vec<String>> = self
            .levels
            .borrow()
            .iter()
            .find(|l| l["id"] == chosen)
            .map(|l| strings(&l["grants"]));
        if let Some(preset) = &preset
            && level != "off"
            && preset.iter().all(|g| grants.contains(g))
        {
            let version = self.version.borrow().clone();
            let _ = self.ui.request(json!({"op":"grant_agent_access","account_id":account_id,"grants":preset,"disclosure_version":version}));
            self.presenting.set(false);
            self.reload();
            poll(&self.ui);
            return;
        }
        let initial = if chosen == "custom" {
            if grants.is_empty() { vec!["sync".to_string()] } else { grants.to_vec() }
        } else {
            preset.unwrap_or_else(|| vec!["sync".into()])
        };
        let panel = self.clone();
        turn_on(&self.ui, &self.window, account_id, initial, chosen == "custom", level != "off", move |_| {
            panel.presenting.set(false);
            panel.reload();
            poll(&panel.ui);
        });
    }

    fn connect_client(self: &Rc<Self>, id: &str) {
        if self.connecting.borrow().is_some() {
            return;
        }
        *self.connecting.borrow_mut() = Some(id.to_string());
        self.client_errors.borrow_mut().remove(id);
        self.render();
        let panel = self.clone();
        let id = id.to_string();
        workflows::request_async(
            self.ui.database_path.clone(),
            json!({"op":"connect_agent_client","client":id}),
            move |result| {
                match result {
                    Ok(_) => {
                        panel.just_connected.borrow_mut().insert(id.clone());
                    }
                    Err(error) => {
                        panel.client_errors.borrow_mut().insert(id.clone(), error);
                    }
                }
                *panel.connecting.borrow_mut() = None;
                panel.refresh_clients();
            },
        );
    }

    fn copy_manual_config(self: &Rc<Self>, button: &gtk::Button) {
        let text = self.clients.borrow().as_ref().and_then(|c| c["manual"]["copy_text"].as_str().map(str::to_owned));
        let Some(text) = text else { return };
        button.clipboard().set_text(&text);
        button.set_label("Copied");
        let weak = button.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
            if let Some(button) = weak.upgrade() {
                button.set_label("Set up…");
            }
        });
    }

    /// The activity log: decisions, changes agents made, consent changes and
    /// refusals, newest first.
    fn show_activity(self: &Rc<Self>) {
        let dialog = gtk::Window::builder()
            .transient_for(&self.window)
            .modal(true)
            .title("Activity")
            .default_width(520)
            .default_height(460)
            .build();
        dialog.add_css_class("agent-window");
        let root = gtk::Box::new(Orientation::Vertical, 0);
        let column = gtk::Box::new(Orientation::Vertical, 10);
        column.set_margin_top(20);
        column.set_margin_start(24);
        column.set_margin_end(24);
        column.set_margin_bottom(10);
        column.append(&label("Activity", "agent-sheet-title"));
        let history = self.history.borrow().clone();
        if history.is_empty() {
            column.append(&label("Nothing yet.", "agent-note"));
        }
        for item in &history {
            column.append(&history_row(item));
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&column));
        root.append(&scroll);
        let done = gtk::Button::with_label("Done");
        done.add_css_class("agent-button");
        done.set_halign(gtk::Align::End);
        done.set_margin_top(10);
        done.set_margin_bottom(14);
        done.set_margin_end(24);
        let d = dialog.clone();
        done.connect_clicked(move |_| d.close());
        root.append(&done);
        dialog.set_child(Some(&root));
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
        dialog.present();
    }
}

fn history_row(item: &Value) -> gtk::Box {
    let column = gtk::Box::new(Orientation::Vertical, 2);
    let when = date_time(item["decided_at_ms"].as_i64().or_else(|| item["created_at_ms"].as_i64()));
    let mut meta = format!("{when} · {}", status_title(item["status"].as_str().unwrap_or_default()));
    if let Some(client) = item["client"].as_str().filter(|c| !c.is_empty()) {
        meta.push_str(&format!(" · {client}"));
    }
    column.append(&label(&meta, "agent-meta"));
    column.append(&label(item["summary"].as_str().unwrap_or_default(), "agent-body"));
    if let Some(error) = item["error"].as_str().filter(|e| !e.is_empty()) {
        column.append(&label(error, if item["status"].as_str() == Some("failed") { "agent-error" } else { "agent-note" }));
    }
    column
}

pub(super) const CSS: &str = r#"
.agent-window { background: @canvas; color: @ink; }
.agent-title { font-size: 22px; font-weight: 700; }
.agent-sheet-title { font-size: 16px; font-weight: 700; }
.agent-lead { font-size: 13.5px; color: @muted; }
.agent-group-label { font-size: 12px; font-weight: 600; color: @muted; }
.agent-waiting { background: alpha(#3a7afe, 0.16); border-radius: 10px; padding: 10px 14px; font-size: 13px; }
.agent-note { font-size: 12px; color: @muted; }
.agent-meta { font-size: 10.5px; color: @muted; }
.agent-body { font-size: 12px; }
.agent-detail { font-size: 11.5px; }
.agent-list { background: @surface; border-radius: 10px; padding: 0 14px; }
.agent-list > separator { background: @line; min-height: 1px; }
.agent-list-row { padding: 11px 0; }
.agent-row-title { font-size: 13px; }
.agent-row-subtitle { font-size: 12px; color: @muted; }
.agent-connected { font-size: 12.5px; font-weight: 600; color: #34c759; }
.agent-link { background: none; border: 0; box-shadow: none; padding: 0 2px; min-height: 0; font-size: 12.5px; color: @link; }
.agent-link:hover { background: none; text-decoration: underline; }
.agent-button { background: @selected; padding: 4px 12px; font-size: 12.5px; }
.agent-button:hover { background: @hover; }
.agent-button:disabled { opacity: 0.5; }
.agent-ghost { background: none; padding: 4px 12px; font-size: 12.5px; color: alpha(@ink, 0.8); }
.agent-approve { background: @ink; color: @canvas; border: 0; border-radius: 7px; padding: 5px 14px; font-size: 12.5px; font-weight: 600; }
.agent-approve:hover { background: @ink; opacity: 0.88; }
.agent-not-ready { opacity: 0.6; }
.agent-error { font-size: 12px; color: #b55d54; }
.agent-dropdown { font-size: 12.5px; }
.agent-dropdown button { background: @selected; border-radius: 6px; padding: 3px 10px; min-height: 26px; }
.agent-option { border: 1px solid @line; border-radius: 9px; padding: 8px 10px; }
.agent-option.selected { border-color: #3a7afe; background: alpha(#3a7afe, 0.12); }
.agent-option-title { font-size: 13px; font-weight: 600; }
.agent-option-sub { font-size: 12px; color: @muted; }
.agent-check-label { font-size: 13px; }
.agent-check-note { font-size: 12px; color: @muted; }
.agent-check-warning { font-size: 12px; color: @warning; }
.agent-reader { background: @canvas; }
.agent-strip { background: @surface; border-bottom: 1px solid @line; padding: 14px 28px; min-height: 36px; }
.agent-ask { font-size: 13.5px; }
.agent-mail { padding: 26px 28px; }
.agent-mail-subject { font-size: 22px; font-weight: 700; }
.agent-mail-key { font-size: 13px; color: @muted; }
.agent-mail-field { font-size: 13px; }
.agent-mail-body { font-size: 14px; }
.agent-mail-muted { font-size: 13px; color: @muted; }
.agent-empty { font-size: 15px; font-weight: 500; color: @muted; }
.agent-chip { background: none; border: 1px solid @line; border-radius: 8px; padding: 8px 12px; font-size: 12.5px; }
.agent-chip-icon { color: #c7533f; font-size: 18px; }
.agent-rows { background: @surface; border: 1px solid @line; border-radius: 9px; }
.agent-rows > separator { background: @line; min-height: 1px; }
.agent-rows-row { padding: 10px 16px; }
.agent-rows-subject { font-size: 13px; font-weight: 500; }
.agent-row-agent { font-size: 12px; font-weight: 600; }
.agent-request-row { padding: 7px 14px 7px 18px; }
.agent-keys { border-top: 1px solid @line; margin: 0 28px; padding: 9px 0 11px 0; }
.agent-keycap { font-family: monospace; font-size: 9px; border: 1px solid @line; border-radius: 4px; padding: 1px 5px; background: alpha(@selected, 0.55); }
.agent-keytext { font-size: 10px; color: @muted; }
.agent-undo { background: @selected; border-radius: 9px; padding: 8px 14px; font-size: 12.5px; }
.agent-count-pill { background: #3a7afe; color: #ffffff; border-radius: 9px; padding: 0 6px; font-size: 10.5px; font-weight: 600; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_read_like_the_macos_shell() {
        assert_eq!(status_title("edited"), "Opened to edit");
        assert_eq!(status_title("rejected"), "Declined");
        assert_eq!(status_title("approved"), "Approved");
    }

    #[test]
    fn display_names_never_hide_the_address() {
        let details = json!({"addresses":{"to":[{"address":"maya@example.org","name":"Maya Chen"},{"address":"rowan@example.net","name":null}]}});
        assert_eq!(addresses(&details, "to"), "Maya Chen <maya@example.org>, rowan@example.net");
        assert_eq!(addresses(&json!({"draft":{"cc":["a@example.test"]}}), "cc"), "a@example.test");
    }
}
