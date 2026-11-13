//! Agent access: per-account consent for agents connected through Email's local
//! MCP server, requests that wait for the user's approval, and agent history.
//! Mirrors apps/macos/Sources/EmailUI/AgentAccess.swift. The disclosure text
//! comes from mail-service so both shells show the same words. Only this window
//! can turn access on or approve a request; the MCP server has no code path
//! for either.
use super::*;

/// The composer's undo window; approved agent sends get the same one.
pub(super) const UNDO_SECONDS: u64 = 10;

pub(super) struct Panel {
    ui: Rc<Ui>,
    pub(super) window: gtk::Window,
    sections: gtk::Box,
    status: gtk::Label,
    status_action: gtk::Button,
    done: gtk::Button,
    status_handler: RefCell<Option<Rc<dyn Fn()>>>,
    status_generation: Cell<u64>,
    accounts: RefCell<Vec<Value>>,
    pending: RefCell<Vec<Value>>,
    history: RefCell<Vec<Value>>,
    blocked: RefCell<Option<Value>>,
    expanded: RefCell<std::collections::HashSet<String>>,
    busy: Cell<bool>,
    signature: RefCell<String>,
    /// Decision clicks right after the list changed are ignored, so a card
    /// that just moved under the pointer cannot be approved by accident.
    rendered_at: Cell<i64>,
}

type Job = (Value, mpsc::Sender<Result<Value, String>>);

thread_local! {
    /// One long-lived service on a worker thread for polls and window reads,
    /// so a 5-second poll does not reopen the store each time.
    static WORKER: RefCell<Option<mpsc::Sender<Job>>> = const { RefCell::new(None) };
    /// Notification ids already shown, loaded once from disk.
    static SEEN: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    /// The Agent access window. Closing hides it, as on macOS, so reopening
    /// shows the same window with its expanded previews.
    static OPEN_PANEL: RefCell<Option<Rc<Panel>>> = const { RefCell::new(None) };
    static SIDEBAR: RefCell<Option<(gtk::Button, gtk::Label)>> = const { RefCell::new(None) };
    static POLL_IN_FLIGHT: Cell<bool> = const { Cell::new(false) };
    static REVISION: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The sidebar entry. Its badge shows the number of waiting requests, or "!"
/// after an agent was refused because access is off.
pub(super) fn sidebar_button() -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("side-item");
    button.set_tooltip_text(Some(
        "Agent access: accounts AI agents may use and requests waiting for your approval",
    ));
    let content = gtk::Box::new(Orientation::Horizontal, 10);
    content.append(&mail_icon("security-high-symbolic"));
    let title = gtk::Label::new(Some("Agent access"));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&title);
    let badge = gtk::Label::new(None);
    badge.add_css_class("agent-badge");
    badge.set_visible(false);
    content.append(&badge);
    button.set_child(Some(&content));
    SIDEBAR.with(|slot| *slot.borrow_mut() = Some((button.clone(), badge)));
    button
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
        return done(Err("Agent access worker stopped. Please retry.".into()));
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
                done(Err("Agent access worker stopped. Please retry.".into()));
            }
            glib::ControlFlow::Break
        }
    });
}

/// Cheap local poll: badge, notifications and an open Agent access window.
pub(super) fn poll(ui: &Rc<Ui>) {
    if POLL_IN_FLIGHT.with(Cell::get) {
        return;
    }
    POLL_IN_FLIGHT.with(|flag| flag.set(true));
    let weak = Rc::downgrade(ui);
    request(
        ui,
        json!({"op":"agent_pending_summary"}),
        move |result| {
            POLL_IN_FLIGHT.with(|flag| flag.set(false));
            let (Some(ui), Ok(summary)) = (weak.upgrade(), result) else { return };
            let count = summary["count"].as_u64().unwrap_or(0);
            // "!" only while a refused agent still needs access turned on.
            let blocked = summary["blocked"]["needs_access"].as_bool() == Some(true);
            let text = if count > 99 {
                "99+".to_string()
            } else if count > 0 {
                count.to_string()
            } else if blocked {
                "!".to_string()
            } else {
                String::new()
            };
            let attention = if count > 0 {
                format!(
                    "{count} request{} waiting for your approval",
                    if count == 1 { "" } else { "s" }
                )
            } else if blocked {
                "An agent was blocked because agent access is off".into()
            } else {
                "No requests waiting".into()
            };
            SIDEBAR.with(|slot| {
                if let Some((button, badge)) = slot.borrow().as_ref() {
                    badge.set_text(&text);
                    badge.set_visible(!text.is_empty());
                    button.set_tooltip_text(Some(&format!("Agent access · {attention}")));
                    button.update_property(&[gtk::accessible::Property::Label(&format!(
                        "Agent access, {attention}"
                    ))]);
                }
            });
            notify(&ui, &summary);
            // Re-read the full window only when a request, decision or consent changed.
            let revision = summary["revision"].as_str().unwrap_or_default().to_string();
            let changed = REVISION.with(|seen| seen.replace(revision.clone()) != revision);
            if changed
                && let Some(panel) = OPEN_PANEL.with(|slot| slot.borrow().clone())
                && panel.window.is_visible()
            {
                panel.reload(false);
            }
        },
    );
}

/// True while the sidebar badge shows requests or a refusal needing access.
pub(super) fn waiting() -> bool {
    SIDEBAR.with(|slot| slot.borrow().as_ref().is_some_and(|(_, badge)| badge.is_visible()))
}

fn seen_path(ui: &Ui) -> PathBuf {
    ui.database_path.with_extension("agent-notifications")
}

/// One alert per new request or new blocked-access entry, remembered across
/// launches. Previews off keeps subjects and recipients out of the alert.
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
    let mut fresh: Vec<(String, String, String)> = Vec::new();
    let previews = ui.notification_previews.get();
    for item in summary["items"].as_array().into_iter().flatten() {
        let Some(id) = item["id"].as_str() else { continue };
        if seen.iter().any(|known| known == id) {
            continue;
        }
        seen.push(id.into());
        let text: String = item["summary"].as_str().unwrap_or_default().chars().take(120).collect();
        fresh.push((
            format!("agent-{id}"),
            if previews { format!("Agent request: {text}") } else { "An agent is waiting for your approval".into() },
            "Review it in Agent access.".into(),
        ));
    }
    if let Some(id) = summary["blocked"]["id"].as_str()
        && !seen.iter().any(|known| known == id)
    {
        seen.push(id.into());
        let text: String = summary["blocked"]["summary"].as_str().unwrap_or_default().chars().take(140).collect();
        fresh.push((
            format!("agent-blocked-{id}"),
            "Agent access needed".into(),
            if previews {
                format!("{text}. Open Agent access to turn it on.")
            } else {
                "An agent tried to use your mail. Open Agent access to review.".into()
            },
        ));
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
        notification.set_default_action("app.open-agent-access");
        app.send_notification(Some(&id), &notification);
    }
    if total > 3 {
        let notification = gtk::gio::Notification::new(&format!("{} more agent requests", total - 3));
        notification.set_body(Some("Review them in Agent access."));
        notification.set_default_action("app.open-agent-access");
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

pub(super) fn open(ui: &Rc<Ui>) -> Rc<Panel> {
    if let Some(panel) = OPEN_PANEL.with(|slot| slot.borrow().clone()) {
        panel.window.present();
        panel.reload(true);
        return panel;
    }
    let window = gtk::Window::builder()
        .title("Agent access")
        .transient_for(&ui.window)
        .default_width(600)
        .default_height(680)
        .build();
    window.set_size_request(440, 420);
    window.set_hide_on_close(true);
    window.add_css_class("agent-window");
    let root = gtk::Box::new(Orientation::Vertical, 0);
    let sections = gtk::Box::new(Orientation::Vertical, 14);
    sections.set_margin_top(20);
    sections.set_margin_bottom(20);
    sections.set_margin_start(24);
    sections.set_margin_end(24);
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
    footer.set_margin_start(24);
    footer.set_margin_end(24);
    let status = label("", "agent-footnote");
    status.set_hexpand(true);
    status.set_lines(3);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    footer.append(&status);
    let status_action = gtk::Button::new();
    status_action.set_visible(false);
    footer.append(&status_action);
    let done = gtk::Button::with_label("Done");
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
        status,
        status_action,
        done,
        status_handler: RefCell::new(None),
        status_generation: Cell::new(0),
        accounts: RefCell::new(Vec::new()),
        pending: RefCell::new(Vec::new()),
        history: RefCell::new(Vec::new()),
        blocked: RefCell::new(None),
        expanded: RefCell::new(Default::default()),
        busy: Cell::new(false),
        signature: RefCell::new(String::new()),
        rendered_at: Cell::new(0),
    });
    let weak = Rc::downgrade(&panel);
    panel.status_action.connect_clicked(move |_| {
        let handler = weak.upgrade().and_then(|panel| panel.status_handler.borrow().clone());
        if let Some(handler) = handler {
            handler();
        }
    });
    let weak = Rc::downgrade(&panel);
    panel.window.connect_close_request(move |_| {
        if let Some(panel) = weak.upgrade() {
            panel.show_status_action(None, None, None);
        }
        glib::Propagation::Proceed
    });
    OPEN_PANEL.with(|slot| *slot.borrow_mut() = Some(panel.clone()));
    panel.window.present();
    panel.reload(true);
    panel
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

fn field(name: &str, value: &str) -> gtk::Label {
    let label = label("", "agent-field");
    label.set_markup(&format!(
        "<span font_weight=\"600\" fgalpha=\"70%\">{}</span>  {}",
        glib::markup_escape_text(name),
        glib::markup_escape_text(value)
    ));
    label.set_selectable(true);
    label
}

/// Bare addresses first; a display name is shown as what it is, since it can
/// imitate another address.
fn addresses(details: &Value, key: &str) -> String {
    match details["addresses"][key].as_array() {
        Some(parts) => parts
            .iter()
            .map(|part| {
                let address = part["address"].as_str().unwrap_or_default();
                match part["name"].as_str() {
                    Some(name) => format!("{address} (shown as “{name}”)"),
                    None => address.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        None => list(&details["draft"][key]),
    }
}

fn list(value: &Value) -> String {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
        .unwrap_or_default()
}

fn date_time(ms: Option<i64>) -> String {
    ms.and_then(|ms| glib::DateTime::from_unix_local(ms / 1000).ok())
        .and_then(|date| date.format("%e %b %Y at %H:%M").ok())
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

fn date(ms: Option<i64>) -> String {
    ms.and_then(|ms| glib::DateTime::from_unix_local(ms / 1000).ok())
        .and_then(|date| date.format("%e %b %Y").ok())
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

fn size(bytes: u64) -> String {
    glib::format_size(bytes).to_string()
}

pub(super) fn kind_title(kind: &str) -> &'static str {
    match kind {
        "send_draft" | "queue_send_draft" => "SEND EMAIL",
        "schedule_draft" => "SCHEDULE EMAIL",
        "batch_triage" => "CHANGE CONVERSATIONS",
        "undo_triage" => "UNDO A CHANGE",
        "confirm_sender_cleanup" => "SENDER CLEANUP",
        _ => "REQUEST",
    }
}

pub(super) fn approve_title(kind: &str) -> &'static str {
    match kind {
        "send_draft" | "queue_send_draft" => "Approve and Send",
        "schedule_draft" => "Approve and Schedule",
        _ => "Approve",
    }
}

fn status_title(status: &str) -> &'static str {
    match status {
        "approved" => "Approved",
        "rejected" => "Rejected",
        "failed" => "Could not apply",
        "blocked" => "Blocked",
        "replaced" => "Replaced by a newer request",
        "pending" => "Waiting",
        _ => "Done",
    }
}

fn when(kind: &str, details: &Value) -> String {
    match kind {
        "schedule_draft" => format!(
            "Scheduled for {}, or after a {UNDO_SECONDS}-second undo window if that time has passed",
            date_time(details["send_at_ms"].as_i64())
        ),
        "queue_send_draft" => format!(
            "After approval, with {} seconds to undo",
            details["undo_seconds"].as_u64().unwrap_or(UNDO_SECONDS).max(UNDO_SECONDS)
        ),
        _ => format!("After approval, with {UNDO_SECONDS} seconds to undo"),
    }
}

/// The accounts a refusal still affects, as decided by mail-core.
pub(super) fn blocked_accounts(blocked: &Value, accounts: &[Value]) -> Vec<Value> {
    let ids: Vec<&str> = blocked["affected_account_ids"]
        .as_array()
        .map(|ids| ids.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    accounts
        .iter()
        .filter(|account| ids.contains(&account["account_id"].as_str().unwrap_or_default()))
        .cloned()
        .collect()
}

impl Panel {
    /// Re-read state. Polls skip rendering when nothing changed, so an open
    /// window does not jump while the user reads a request.
    pub(super) fn reload(self: &Rc<Self>, force: bool) {
        let panel = self.clone();
        request(
            &self.ui,
            json!({"op":"agent_access_overview","history_limit":60}),
            move |result| match result {
                Ok(state) => {
                    let (pending, history) = (state["requests"].clone(), state["history"].clone());
                    let signature = signature(&state, &pending, &history);
                    if force || signature != *panel.signature.borrow() {
                        // Only a change to the cards arms the click guard; history
                        // updates below them must not drop a decision click.
                        let cards = |value: &Value| value.as_array().map(|rows| rows.iter().map(|r| r["id"].to_string()).collect::<Vec<_>>().join(",")).unwrap_or_default();
                        if cards(&pending) != cards(&Value::Array(panel.pending.borrow().clone())) {
                            panel.rendered_at.set(now_ms());
                        }
                        *panel.accounts.borrow_mut() = state["accounts"].as_array().cloned().unwrap_or_default();
                        *panel.blocked.borrow_mut() = Some(state["pending"]["blocked"].clone()).filter(|b| !b.is_null());
                        *panel.pending.borrow_mut() = pending.as_array().cloned().unwrap_or_default();
                        *panel.history.borrow_mut() = history.as_array().cloned().unwrap_or_default();
                        *panel.signature.borrow_mut() = signature;
                        panel.render();
                    }
                }
                Err(error) => panel.set_status(&error, true),
            },
        );
    }

    fn add(&self, widget: &impl IsA<gtk::Widget>) {
        self.sections.append(widget);
    }

    fn section(&self, text: &str) {
        let heading = label(text, "agent-section");
        heading.set_margin_top(8);
        self.add(&heading);
    }

    pub(super) fn render(self: &Rc<Self>) {
        // Move focus off widgets about to be destroyed (a clicked Approve, for
        // example) and away from selectable text. Same rule as macOS.
        if gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_none_or(|focused| focused.is_ancestor(&self.sections))
        {
            self.done.grab_focus();
        }
        while let Some(child) = self.sections.first_child() {
            self.sections.remove(&child);
        }
        self.add(&label("Agent access", "agent-heading"));
        self.add(&label(
            "AI agents and tools connected through Email's local MCP server. They can use only the accounts you turn on, and sending mail or bulk changes always waits for your approval here.",
            "agent-secondary",
        ));
        let blocked = self.blocked.borrow().clone();
        if let Some(banner) = blocked.and_then(|blocked| self.blocked_banner(&blocked)) {
            self.add(&banner);
        }
        let pending = self.pending.borrow().clone();
        self.section(&if pending.is_empty() {
            "WAITING FOR YOUR APPROVAL".to_string()
        } else {
            format!("WAITING FOR YOUR APPROVAL · {}", pending.len())
        });
        if pending.is_empty() {
            self.add(&label(
                "No requests are waiting. When an agent asks to send mail or change many conversations, it appears here.",
                "agent-secondary",
            ));
        }
        for item in &pending {
            self.add(&self.pending_card(item));
        }
        self.section("ACCOUNTS");
        let accounts = self.accounts.borrow().clone();
        if accounts.is_empty() {
            self.add(&label("Add a mail account first.", "agent-secondary"));
        }
        for account in &accounts {
            self.add(&self.account_row(account));
        }
        self.add(&label(
            "Turning access off applies to the agent's next request. Each MCP registration's own permissions also still apply.",
            "agent-footnote",
        ));
        self.section("HISTORY");
        let history = self.history.borrow().clone();
        if history.is_empty() {
            self.add(&label("No agent activity yet.", "agent-secondary"));
        }
        for item in &history {
            self.add(&history_row(item));
        }
    }

    fn blocked_banner(self: &Rc<Self>, blocked: &Value) -> Option<gtk::Box> {
        let affected = blocked_accounts(blocked, &self.accounts.borrow());
        if affected.is_empty() {
            return None;
        }
        let card = gtk::Box::new(Orientation::Vertical, 6);
        card.add_css_class("agent-banner");
        card.append(&label("Agent access needed", "agent-strong"));
        card.append(&label(blocked["summary"].as_str().unwrap_or_default(), "agent-body"));
        if let Some(reason) = blocked["reason"].as_str().filter(|r| !r.is_empty()) {
            card.append(&label(reason, "agent-footnote"));
        }
        if let [account] = affected.as_slice() {
            let id = account["account_id"].as_str().unwrap_or_default().to_string();
            let email = account["email"].as_str().unwrap_or_default().to_string();
            let current = account["current"].as_bool() == Some(true);
            let grants: Option<Vec<String>> = current.then(|| {
                account["consent"]["grants"]
                    .as_array()
                    .map(|g| g.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                    .unwrap_or_default()
            });
            let title = if current {
                format!("Change Access for {email}…")
            } else {
                format!("Turn On for {email}…")
            };
            let button = gtk::Button::with_label(&title);
            button.set_halign(gtk::Align::Start);
            let panel = self.clone();
            button.connect_clicked(move |_| panel.turn_on(&id, &email, grants.clone()));
            card.append(&button);
        } else {
            card.append(&label(
                "Turn on agent access below for each account the agent may use.",
                "agent-footnote",
            ));
        }
        Some(card)
    }

    fn pending_card(self: &Rc<Self>, item: &Value) -> gtk::Box {
        let id = item["id"].as_str().unwrap_or_default().to_string();
        let kind = item["kind"].as_str().unwrap_or_default().to_string();
        let details = &item["details"];
        let card = gtk::Box::new(Orientation::Vertical, 6);
        card.add_css_class("agent-card");
        card.append(&label(kind_title(&kind), "agent-kind"));
        card.append(&label(item["summary"].as_str().unwrap_or_default(), "agent-summary"));
        let mut meta = Vec::new();
        if let Some(email) = details["account_email"].as_str() {
            meta.push(email.to_string());
        }
        if let Some(client) = item["client"].as_str().filter(|c| !c.is_empty()) {
            meta.push(format!("requested by {client}"));
        }
        meta.push(date_time(item["created_at_ms"].as_i64()));
        card.append(&label(&meta.join(" · "), "agent-footnote"));
        match kind.as_str() {
            "send_draft" | "queue_send_draft" | "schedule_draft" => {
                let draft = &details["draft"];
                card.append(&field("From", draft["from"].as_str().unwrap_or_default()));
                for (name, key) in [("To", "to"), ("Cc", "cc"), ("Bcc", "bcc"), ("Reply-To", "reply_to")] {
                    let value = addresses(details, key);
                    if !value.is_empty() || key == "to" {
                        card.append(&field(name, &value));
                    }
                }
                let subject = draft["subject"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no subject)");
                card.append(&field("Subject", subject));
                let files = draft["attachments"].as_array().cloned().unwrap_or_default();
                for file in &files {
                    let row = gtk::Box::new(Orientation::Horizontal, 8);
                    let filename = file["filename"].as_str().unwrap_or("file").to_string();
                    let text = field(
                        "Attachment",
                        &format!(
                            "{filename} ({}, {})",
                            size(file["size"].as_u64().unwrap_or(0)),
                            file["mime_type"].as_str().unwrap_or("file")
                        ),
                    );
                    row.append(&text);
                    let save = gtk::Button::with_label("Save…");
                    save.set_valign(gtk::Align::Center);
                    save.update_property(&[gtk::accessible::Property::Label(&format!("Save a copy of {filename}"))]);
                    let panel = self.clone();
                    let (action_id, attachment_id) = (id.clone(), file["id"].as_str().unwrap_or_default().to_string());
                    save.connect_clicked(move |_| panel.save_attachment(&action_id, &attachment_id, &filename));
                    row.append(&save);
                    card.append(&row);
                }
                if !files.is_empty() {
                    card.append(&label(
                        "The agent attached these files. Save a copy to view one before approving; never run a file you did not expect.",
                        "agent-footnote",
                    ));
                }
                card.append(&field("When", &when(&kind, details)));
                let invisible = details["invisible_count"].as_u64().unwrap_or(0);
                if invisible > 0 {
                    card.append(&label(
                        &format!("Contains {invisible} invisible character{} used by emoji or some scripts.", if invisible == 1 { "" } else { "s" }),
                        "agent-footnote",
                    ));
                }
                let body = draft["body_plain"].as_str().unwrap_or_default();
                let expanded = self.expanded.borrow().contains(&id);
                let shortened = body.chars().count() > 1200 || body.replace("\r\n", "\n").split('\n').count() > 14;
                // Expanded shows every character that would be sent; nothing is cut.
                let text: String = if body.is_empty() {
                    "(empty message)".into()
                } else if expanded {
                    body.to_string()
                } else {
                    let opening: String = body.replace("\r\n", "\n").split('\n').take(14).collect::<Vec<_>>().join("\n").chars().take(1200).collect();
                    if shortened { format!("{opening}…") } else { opening }
                };
                let quote = gtk::Box::new(Orientation::Vertical, 0);
                quote.add_css_class("agent-quote");
                // Never clip visually: what the preview holds is shown in full, and
                // anything left out is announced with the note and the toggle.
                let preview = label(&text, "agent-body");
                preview.set_selectable(true);
                quote.append(&preview);
                card.append(&quote);
                if shortened && !expanded {
                    card.append(&label(
                        "Only the beginning is shown. Show the full message to review everything that would be sent.",
                        "agent-footnote",
                    ));
                }
                if shortened {
                    let toggle = gtk::Button::with_label(if expanded { "Show less" } else { "Show full message" });
                    toggle.add_css_class("flat");
                    toggle.set_halign(gtk::Align::Start);
                    let panel = self.clone();
                    let key = id.clone();
                    toggle.connect_clicked(move |_| {
                        {
                            let mut expanded = panel.expanded.borrow_mut();
                            if !expanded.insert(key.clone()) {
                                expanded.remove(&key);
                            }
                        }
                        panel.render();
                    });
                    card.append(&toggle);
                }
            }
            "batch_triage" | "undo_triage" => {
                let count = details["count"].as_u64().unwrap_or(0) as usize;
                card.append(&field("Accounts", &list(&details["account_emails"])));
                card.append(&field("Conversations", &count.to_string()));
                let threads = details["threads"].as_array().cloned().unwrap_or_default();
                for thread in threads.iter().take(8) {
                    let subject = thread["subject"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no subject)");
                    card.append(&label(
                        &format!("{} — {subject}", sender_name(thread["sender"].as_str().unwrap_or_default())),
                        "agent-secondary",
                    ));
                }
                let shown = threads.len().min(8);
                if count > shown {
                    card.append(&label(&format!("and {} more", count - shown), "agent-footnote"));
                }
                if kind == "undo_triage" {
                    card.append(&label(
                        "These conversations return to where they were before the change.",
                        "agent-footnote",
                    ));
                } else if matches!(details["kind"].as_str(), Some("archive" | "restore" | "spam")) {
                    card.append(&label("You can undo this change after approving.", "agent-footnote"));
                }
            }
            "confirm_sender_cleanup" => {
                card.append(&field("Sender", details["sender"].as_str().unwrap_or_default()));
                card.append(&field("Unread messages", &details["matched"].as_u64().unwrap_or(0).to_string()));
                for subject in details["sample_subjects"].as_array().into_iter().flatten().take(3).filter_map(Value::as_str) {
                    card.append(&label(subject, "agent-secondary"));
                }
                card.append(&label(
                    "Only messages found in the preview are marked read in Gmail. Completed changes have no automatic undo.",
                    "agent-footnote",
                ));
            }
            _ => {}
        }
        let actions = gtk::Box::new(Orientation::Horizontal, 8);
        let reject = gtk::Button::with_label("Reject");
        let approve = gtk::Button::with_label(approve_title(&kind));
        approve.add_css_class("agent-approve");
        let summary = item["summary"].as_str().unwrap_or_default().to_string();
        reject.update_property(&[gtk::accessible::Property::Label(&format!("Reject: {summary}"))]);
        approve.update_property(&[gtk::accessible::Property::Label(&format!("{}: {summary}", approve_title(&kind)))]);
        let panel = self.clone();
        let reject_id = id.clone();
        reject.connect_clicked(move |_| panel.reject(&reject_id));
        let panel = self.clone();
        let approved = item.clone();
        approve.connect_clicked(move |_| panel.approve(&approved));
        actions.append(&reject);
        actions.append(&approve);
        card.append(&actions);
        card
    }

    fn account_row(self: &Rc<Self>, account: &Value) -> gtk::Box {
        let id = account["account_id"].as_str().unwrap_or_default().to_string();
        let email = account["email"].as_str().unwrap_or(&id).to_string();
        let consent = account["consent"].clone();
        let current = account["current"].as_bool() == Some(true);
        let row = gtk::Box::new(Orientation::Horizontal, 10);
        let names = gtk::Box::new(Orientation::Vertical, 3);
        names.set_hexpand(true);
        let title = label(&email, "agent-strong");
        title.set_wrap(false);
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        names.append(&title);
        let state = if consent.is_null() {
            "Off".to_string()
        } else {
            let since = date(consent["granted_at_ms"].as_i64());
            if current {
                format!("On since {since} · {}", list(&account["grant_labels"]))
            } else {
                format!("Needs review: the disclosure changed since {since}")
            }
        };
        names.append(&label(&state, "agent-footnote"));
        row.append(&names);
        let mut buttons = Vec::new();
        if consent.is_null() || !current {
            let button = gtk::Button::with_label("Turn On…");
            let panel = self.clone();
            let (id, email) = (id.clone(), email.clone());
            button.connect_clicked(move |_| panel.turn_on(&id, &email, None));
            buttons.push(button);
        } else {
            let grants: Vec<String> = consent["grants"]
                .as_array()
                .map(|g| g.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            let change = gtk::Button::with_label("Change…");
            let panel = self.clone();
            let (cid, cemail) = (id.clone(), email.clone());
            change.connect_clicked(move |_| panel.turn_on(&cid, &cemail, Some(grants.clone())));
            buttons.push(change);
            let off = gtk::Button::with_label("Turn Off");
            let panel = self.clone();
            let (oid, oemail) = (id.clone(), email.clone());
            off.connect_clicked(move |_| panel.turn_off(&oid, &oemail));
            buttons.push(off);
        }
        for button in buttons {
            button.set_valign(gtk::Align::Center);
            let text = button.label().unwrap_or_default();
            button.update_property(&[gtk::accessible::Property::Label(&format!("{text} agent access for {email}"))]);
            row.append(&button);
        }
        row
    }

    fn set_status(&self, text: &str, error: bool) {
        self.status.set_text(text);
        if error {
            self.status.add_css_class("agent-error");
        } else {
            self.status.remove_css_class("agent-error");
        }
    }

    fn show_status_action(&self, title: Option<&str>, until_ms: Option<i64>, handler: Option<Rc<dyn Fn()>>) {
        let generation = self.status_generation.get() + 1;
        self.status_generation.set(generation);
        // Hiding a focused button would hand focus to the first selectable text.
        if title.is_none() && self.status_action.has_focus() {
            self.done.grab_focus();
        }
        self.status_action.set_label(title.unwrap_or_default());
        self.status_action.set_visible(title.is_some());
        *self.status_handler.borrow_mut() = handler;
        if let (Some(until), Some(_)) = (until_ms, title) {
            let delay = (until - now_ms()).max(0) as u64;
            let action = self.status_action.clone();
            let generation_cell = Rc::new(self.status_generation.get());
            let weak_panel = OPEN_PANEL
                .with(|slot| slot.borrow().as_ref().map(Rc::downgrade))
                .unwrap_or_default();
            glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
                if let Some(panel) = weak_panel.upgrade()
                    && panel.status_generation.get() == *generation_cell
                {
                    if action.has_focus() {
                        panel.done.grab_focus();
                    }
                    action.set_visible(false);
                    *panel.status_handler.borrow_mut() = None;
                }
            });
        }
    }

    fn list_just_changed(&self) -> bool {
        if now_ms() - self.rendered_at.get() >= 800 {
            return false;
        }
        self.set_status("The list just changed. Check the request, then choose again.", false);
        true
    }

    fn approve(self: &Rc<Self>, item: &Value) {
        if self.busy.get() || self.list_just_changed() {
            return;
        }
        let Some(id) = item["id"].as_str().map(str::to_owned) else { return };
        let kind = item["kind"].as_str().unwrap_or_default().to_string();
        let accounts: Vec<String> = item["account_ids"]
            .as_array()
            .map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        self.busy.set(true);
        self.set_status("Approving…", false);
        let panel = self.clone();
        workflows::request_async(
            self.ui.database_path.clone(),
            json!({"op":"approve_agent_action","id":id,"undo_seconds":UNDO_SECONDS}),
            move |result| {
                panel.busy.set(false);
                match result {
                    Ok(action) => {
                        let result = &action["result"];
                        match kind.as_str() {
                            "send_draft" | "queue_send_draft" | "schedule_draft" => {
                                let message_id = result["message_id"].as_str().unwrap_or_default().to_string();
                                let send_at = result["send_at_ms"].as_i64().unwrap_or(0);
                                let scheduled = result["status"].as_str() == Some("scheduled");
                                // Reuse the composer's undo affordance for approved agent sends.
                                *panel.ui.last_queued_draft.borrow_mut() = Some(message_id.clone());
                                panel.ui.undo_button.set_visible(true);
                                let seconds = ((send_at - now_ms()) as f64 / 1000.0).round().max(1.0) as i64;
                                panel.ui.status.set_text(&if scheduled {
                                    "Agent request approved · Scheduled · View in Outbox".to_string()
                                } else {
                                    format!("Agent request approved · Sending in {seconds} seconds · Undo send")
                                });
                                let weak = Rc::downgrade(&panel);
                                let cancel: Rc<dyn Fn()> = Rc::new(move || {
                                    if let Some(panel) = weak.upgrade() {
                                        panel.cancel_send(&message_id);
                                    }
                                });
                                if scheduled {
                                    panel.set_status(
                                        &format!("Approved · scheduled for {} · in Outbox", date_time(Some(send_at))),
                                        false,
                                    );
                                    panel.show_status_action(Some("Cancel Send"), None, Some(cancel));
                                } else {
                                    panel.set_status(&format!("Approved · sending in {seconds} seconds"), false);
                                    panel.show_status_action(Some("Undo Send"), Some(send_at), Some(cancel));
                                }
                            }
                            "undo_triage" => {
                                let operation = result["operation_id"].as_str().unwrap_or_default();
                                let changed = result["changed"].as_u64().unwrap_or(0);
                                let current = panel.ui.last_triage.borrow().as_ref().map(|(id, _)| id.clone());
                                if current.as_deref() == Some(operation) {
                                    *panel.ui.last_triage.borrow_mut() = None;
                                    panel.ui.update_selection_bar();
                                }
                                panel.set_status(
                                    &format!(
                                        "Approved · change undone for {changed} conversation{}",
                                        if changed == 1 { "" } else { "s" }
                                    ),
                                    false,
                                );
                                panel.show_status_action(None, None, None);
                            }
                            "batch_triage" => {
                                let operation = result["operation_id"].as_str().unwrap_or_default().to_string();
                                let changed = result["changed"].as_u64().unwrap_or(0);
                                // Only an undoable change becomes the main window's undo.
                                if result["undoable"].as_bool() == Some(true) {
                                    *panel.ui.last_triage.borrow_mut() = Some((operation.clone(), accounts.clone()));
                                    panel.ui.update_selection_bar();
                                }
                                panel.set_status(
                                    &format!(
                                        "Approved · {changed} conversation{} updated · saved locally",
                                        if changed == 1 { "" } else { "s" }
                                    ),
                                    false,
                                );
                                if result["undoable"].as_bool() == Some(true) {
                                    let weak = Rc::downgrade(&panel);
                                    let undo: Rc<dyn Fn()> = Rc::new(move || {
                                        if let Some(panel) = weak.upgrade() {
                                            panel.undo_triage(&operation);
                                        }
                                    });
                                    panel.show_status_action(Some("Undo"), None, Some(undo));
                                } else {
                                    panel.show_status_action(None, None, None);
                                }
                            }
                            _ => {
                                panel.set_status(
                                    "Approved · Email marks the messages read in batches. Open Cleanup history to follow progress.",
                                    false,
                                );
                                panel.show_status_action(None, None, None);
                            }
                        }
                        panel.changed();
                    }
                    Err(error) => {
                        panel.set_status(&error, true);
                        panel.show_status_action(None, None, None);
                    }
                }
                panel.reload(true);
            },
        );
    }

    fn save_attachment(self: &Rc<Self>, action_id: &str, attachment_id: &str, filename: &str) {
        let panel = self.clone();
        let filename = filename.to_string();
        workflows::request_async(
            self.ui.database_path.clone(),
            json!({"op":"agent_request_attachment","id":action_id,"attachment_id":attachment_id}),
            move |result| {
                let bytes = match result.and_then(|file| {
                    STANDARD
                        .decode(file["data_base64"].as_str().unwrap_or_default())
                        .map_err(|_| "The attachment could not be read.".to_string())
                }) {
                    Ok(bytes) => bytes,
                    Err(error) => return panel.set_status(&error, true),
                };
                let dialog = gtk::FileDialog::builder().initial_name(&filename).modal(true).build();
                let saver = panel.clone();
                dialog.save(Some(&panel.window), None::<&gtk::gio::Cancellable>, move |chosen| {
                    let Ok(file) = chosen else { return };
                    let Some(path) = file.path() else { return };
                    match std::fs::write(&path, &bytes) {
                        Ok(()) => saver.set_status(
                            &format!(
                                "Saved {}. View it with an app you trust before approving, and never run a file you did not expect.",
                                path.file_name().and_then(|n| n.to_str()).unwrap_or("the file")
                            ),
                            false,
                        ),
                        Err(error) => saver.set_status(&error.to_string(), true),
                    }
                });
            },
        );
    }

    fn reject(self: &Rc<Self>, id: &str) {
        if self.busy.get() || self.list_just_changed() {
            return;
        }
        self.busy.set(true);
        let panel = self.clone();
        workflows::request_async(
            self.ui.database_path.clone(),
            json!({"op":"reject_agent_action","id":id}),
            move |result| {
                panel.busy.set(false);
                match result {
                    Ok(_) => {
                        panel.set_status("Rejected · nothing was sent or changed", false);
                        panel.show_status_action(None, None, None);
                    }
                    Err(error) => panel.set_status(&error, true),
                }
                panel.reload(true);
                panel.changed();
            },
        );
    }

    fn cancel_send(self: &Rc<Self>, message_id: &str) {
        let panel = self.clone();
        let id = message_id.to_string();
        workflows::request_async(
            self.ui.database_path.clone(),
            json!({"op":"cancel_scheduled_draft","message_id":message_id}),
            move |result| match result {
                Ok(_) => {
                    if panel.ui.last_queued_draft.borrow().as_deref() == Some(id.as_str()) {
                        *panel.ui.last_queued_draft.borrow_mut() = None;
                        panel.ui.undo_button.set_visible(false);
                        panel.ui.status.set_text("Send cancelled · Message returned to Drafts");
                    }
                    panel.set_status("Send cancelled · the message is back in Drafts", false);
                    panel.show_status_action(None, None, None);
                    panel.changed();
                }
                Err(error) => panel.set_status(&error, true),
            },
        );
    }

    fn undo_triage(self: &Rc<Self>, operation: &str) {
        let panel = self.clone();
        workflows::request_async(
            self.ui.database_path.clone(),
            json!({"op":"undo_triage","operation_id":operation}),
            move |result| match result {
                Ok(_) => {
                    *panel.ui.last_triage.borrow_mut() = None;
                    panel.ui.update_selection_bar();
                    panel.set_status("Change undone", false);
                    panel.show_status_action(None, None, None);
                    panel.changed();
                }
                Err(error) => panel.set_status(&error, true),
            },
        );
    }

    fn changed(self: &Rc<Self>) {
        self.ui.refresh();
        poll(&self.ui);
    }

    fn turn_on(self: &Rc<Self>, account_id: &str, email: &str, current: Option<Vec<String>>) {
        let disclosure = match self.ui.request(json!({"op":"agent_access_disclosure","account_id":account_id})) {
            Ok(value) => value,
            Err(error) => return self.set_status(&error, true),
        };
        let panel = self.clone();
        let (account_id, email) = (account_id.to_string(), email.to_string());
        let changing = current.is_some();
        let version = disclosure["version"].clone();
        disclosure_dialog(&self.window, &disclosure, current, move |grants| {
            match panel.ui.request(json!({"op":"grant_agent_access","account_id":account_id,"grants":grants,"disclosure_version":version.clone()})) {
                Ok(_) => panel.set_status(
                    &if changing {
                        format!("Agent access updated for {email}")
                    } else {
                        format!("Agent access is on for {email}")
                    },
                    false,
                ),
                Err(error) => panel.set_status(&error, true),
            }
            panel.reload(true);
            panel.changed();
        });
    }

    fn turn_off(self: &Rc<Self>, account_id: &str, email: &str) {
        let confirm = gtk::AlertDialog::builder()
            .message(format!("Turn off agent access for {email}?"))
            .detail("Agents lose access to this account on their next request. Requests waiting for your approval are rejected. Mail you already approved stays in the Outbox, where you can cancel it.")
            .buttons(["Cancel", "Turn Off"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let panel = self.clone();
        let (account_id, email) = (account_id.to_string(), email.to_string());
        confirm.choose(Some(&self.window), None::<&gtk::gio::Cancellable>, move |result| {
            if result == Ok(1) {
                panel.revoke(&account_id, &email);
            }
        });
    }

    pub(super) fn revoke(self: &Rc<Self>, account_id: &str, email: &str) {
        match self.ui.request(json!({"op":"revoke_agent_access","account_id":account_id})) {
            Ok(_) => self.set_status(&format!("Agent access is off for {email}"), false),
            Err(error) => self.set_status(&error, true),
        }
        self.reload(true);
        self.changed();
    }
}

fn signature(state: &Value, pending: &Value, history: &Value) -> String {
    let mut parts: Vec<String> = state["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| format!("{}={}", a["account_id"], a["consent"]["granted_at_ms"]))
        .collect();
    parts.push(state["pending"]["blocked"]["id"].to_string());
    // The revision also changes when a coalesced entry updates in place.
    parts.push(state["pending"]["revision"].to_string());
    parts.extend(pending.as_array().into_iter().flatten().map(|p| p["id"].to_string()));
    parts.extend(
        history
            .as_array()
            .into_iter()
            .flatten()
            .take(5)
            .map(|h| format!("{}:{}", h["id"], h["status"])),
    );
    parts.join("|")
}

fn history_row(item: &Value) -> gtk::Box {
    let column = gtk::Box::new(Orientation::Vertical, 2);
    let when = date_time(item["decided_at_ms"].as_i64().or_else(|| item["created_at_ms"].as_i64()));
    let mut meta = format!("{when} · {}", status_title(item["status"].as_str().unwrap_or_default()));
    let count = item["details"]["count"].as_u64().unwrap_or(1);
    if count > 1 && !matches!(item["kind"].as_str(), Some("batch_triage" | "undo_triage")) {
        meta.push_str(&format!(" · {count} times"));
    }
    let attempts = item["details"]["attempts"].as_u64().unwrap_or(1);
    if attempts > 1 {
        meta.push_str(&format!(" · {attempts} attempts"));
    }
    if let Some(client) = item["client"].as_str().filter(|c| !c.is_empty()) {
        meta.push_str(&format!(" · {client}"));
    }
    column.append(&label(&meta, "agent-meta"));
    column.append(&label(item["summary"].as_str().unwrap_or_default(), "agent-body"));
    if let Some(error) = item["error"].as_str().filter(|e| !e.is_empty()) {
        column.append(&label(
            error,
            if item["status"].as_str() == Some("failed") { "agent-error" } else { "agent-footnote" },
        ));
    }
    column
}

/// The consent disclosure. Read access is required and always on; every other
/// grant is opt-in. `chosen` runs with the selected grants on confirmation.
pub(super) fn disclosure_dialog(
    parent: &gtk::Window,
    disclosure: &Value,
    current: Option<Vec<String>>,
    chosen: impl Fn(Vec<String>) + 'static,
) -> gtk::Window {
    let changing = current.is_some();
    let title = disclosure[if changing { "change_title" } else { "title" }]
        .as_str()
        .unwrap_or("Agent access");
    let dialog = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Agent access")
        .default_width(500)
        .default_height(640)
        .build();
    dialog.add_css_class("agent-window");
    let root = gtk::Box::new(Orientation::Vertical, 0);
    let column = gtk::Box::new(Orientation::Vertical, 8);
    column.set_margin_top(22);
    column.set_margin_bottom(16);
    column.set_margin_start(24);
    column.set_margin_end(24);
    column.append(&label(title, "agent-heading"));
    column.append(&label(disclosure["intro"].as_str().unwrap_or_default(), "agent-body"));
    for category in disclosure["data_categories"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        column.append(&label(&format!("•  {category}"), "agent-body"));
    }
    if let Some(note) = disclosure["shared_note"].as_str() {
        column.append(&label(note, "agent-secondary"));
    }
    column.append(&label(disclosure["provider_terms"].as_str().unwrap_or_default(), "agent-strong"));
    column.append(&label(disclosure["safeguards"].as_str().unwrap_or_default(), "agent-body"));
    column.append(&label(disclosure["grants_heading"].as_str().unwrap_or_default(), "agent-strong"));
    let read = gtk::CheckButton::with_label(
        disclosure["read_label"].as_str().unwrap_or("Read this account's data (required)"),
    );
    read.set_active(true);
    read.set_sensitive(false);
    column.append(&read);
    let mut boxes = Vec::new();
    for grant in disclosure["grants"].as_array().into_iter().flatten() {
        let Some(id) = grant["id"].as_str() else { continue };
        let check = gtk::CheckButton::with_label(grant["label"].as_str().unwrap_or(id));
        check.set_active(current.as_ref().is_some_and(|c| c.iter().any(|g| g == id)));
        check.set_widget_name(&format!("agent-grant-{id}"));
        column.append(&check);
        let detail = label(grant["detail"].as_str().unwrap_or_default(), "agent-footnote");
        detail.set_margin_start(28);
        detail.set_margin_bottom(4);
        column.append(&detail);
        boxes.push((id.to_string(), check));
    }
    column.append(&label(disclosure["revoke"].as_str().unwrap_or_default(), "agent-footnote"));
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&column));
    root.append(&scroll);
    root.append(&gtk::Separator::new(Orientation::Horizontal));
    let buttons = gtk::Box::new(Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(10);
    buttons.set_margin_bottom(12);
    buttons.set_margin_start(24);
    buttons.set_margin_end(24);
    let cancel = gtk::Button::with_label("Cancel");
    let confirm = gtk::Button::with_label(
        disclosure[if changing { "change_confirm" } else { "confirm" }]
            .as_str()
            .unwrap_or("Turn On Agent Access"),
    );
    confirm.add_css_class("agent-approve");
    buttons.append(&cancel);
    buttons.append(&confirm);
    root.append(&buttons);
    dialog.set_child(Some(&root));
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let d = dialog.clone();
    confirm.connect_clicked(move |_| {
        let grants = boxes
            .iter()
            .filter(|(_, check)| check.is_active())
            .map(|(id, _)| id.clone())
            .collect();
        d.close();
        chosen(grants);
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

pub(super) const CSS: &str = r#"
.agent-window { background: @canvas; color: @ink; }
.agent-heading { font-size: 17px; font-weight: 600; }
.agent-section { font-size: 10px; font-weight: 600; color: @muted; }
.agent-secondary { font-size: 12px; color: @muted; }
.agent-footnote { font-size: 11px; color: @muted; }
.agent-meta { font-size: 10.5px; color: @muted; }
.agent-body { font-size: 12px; }
.agent-field { font-size: 12px; }
.agent-strong { font-size: 12.5px; font-weight: 600; }
.agent-kind { font-size: 10px; font-weight: 600; color: @muted; }
.agent-summary { font-size: 13.5px; font-weight: 600; }
.agent-card { background: @canvas; border: 1px solid @line; border-radius: 10px; padding: 12px 14px; }
.agent-banner { background: @selected; border: 1px solid @line; border-radius: 10px; padding: 12px 14px; }
.agent-quote { background: @surface; border: 1px solid @line; border-radius: 10px; padding: 12px 14px; }
.agent-approve { background: @ink; color: @canvas; border: 0; border-radius: 7px; padding: 5px 14px; font-size: 12px; font-weight: 600; }
.agent-approve:hover { opacity: 0.88; }
.agent-error { color: #b55d54; }
.agent-badge { color: @ink; font-size: 10px; font-weight: 600; font-family: monospace; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_prompt_names_only_the_accounts_core_says_are_affected() {
        let a = json!({"account_id":"a","email":"a@example.test"});
        let b = json!({"account_id":"b","email":"b@example.test"});
        let blocked = json!({"account_ids":["a","b"],"affected_account_ids":["b"]});
        let affected = blocked_accounts(&blocked, &[a.clone(), b]);
        assert_eq!(affected.len(), 1);
        assert_eq!(affected[0]["account_id"], "b");
        assert!(blocked_accounts(&json!({}), &[a]).is_empty());
    }

    #[test]
    fn labels_match_the_macos_shell() {
        assert_eq!(kind_title("send_draft"), "SEND EMAIL");
        assert_eq!(kind_title("batch_triage"), "CHANGE CONVERSATIONS");
        assert_eq!(kind_title("undo_triage"), "UNDO A CHANGE");
        assert_eq!(approve_title("schedule_draft"), "Approve and Schedule");
        assert_eq!(approve_title("confirm_sender_cleanup"), "Approve");
        assert_eq!(when("send_draft", &json!({})), "After approval, with 10 seconds to undo");
        assert_eq!(
            when("queue_send_draft", &json!({"undo_seconds":30})),
            "After approval, with 30 seconds to undo"
        );
    }
}
