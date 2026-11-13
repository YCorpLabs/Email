//! Agent access: requests waiting for the user's approval, connecting Claude
//! Code or Codex to Email's local MCP server, per-account consent, and recent
//! agent activity. Mirrors apps/macos/Sources/EmailUI/AgentAccess.swift. The
//! turn-on window's text comes from mail-service so both shells show the same
//! words. Only this window can turn access on or approve a request; the MCP
//! server has no code path for either.
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
    history_expanded: Cell<bool>,
    /// The `agent_clients` answer: CLIs found, registrations, the helper.
    clients: RefCell<Option<Value>>,
    clients_loading: Cell<bool>,
    connecting: RefCell<Option<String>>,
    copy_button: RefCell<Option<gtk::Button>>,
    presenting: Cell<bool>,
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
        panel.refresh_clients();
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
    let sections = gtk::Box::new(Orientation::Vertical, 10);
    sections.set_margin_top(22);
    sections.set_margin_bottom(22);
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
    status_action.add_css_class("agent-button");
    status_action.set_visible(false);
    footer.append(&status_action);
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
        history_expanded: Cell::new(false),
        clients: RefCell::new(None),
        clients_loading: Cell::new(false),
        connecting: RefCell::new(None),
        copy_button: RefCell::new(None),
        presenting: Cell::new(false),
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
    panel.refresh_clients();
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
        "<span fgalpha=\"62%\">{}</span>  {}",
        glib::markup_escape_text(name),
        glib::markup_escape_text(value)
    ));
    label.set_selectable(true);
    label
}

/// A title with a chevron that turns when it opens: Recent activity, Show
/// message and the turn-on window's full list.
fn disclosure_button(title: &str, expanded: bool) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("agent-disclosure");
    button.set_halign(gtk::Align::Start);
    set_disclosure(&button, title, expanded);
    button
}

fn set_disclosure(button: &gtk::Button, title: &str, expanded: bool) {
    let content = gtk::Box::new(Orientation::Horizontal, 6);
    let text = gtk::Label::new(Some(title));
    text.add_css_class("agent-disclosure-title");
    content.append(&text);
    content.append(&gtk::Image::from_icon_name(if expanded { "pan-down-symbolic" } else { "pan-end-symbolic" }));
    button.set_child(Some(&content));
    button.update_property(&[gtk::accessible::Property::Label(title)]);
    button.update_state(&[gtk::accessible::State::Expanded(Some(expanded))]);
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
        "send_draft" | "queue_send_draft" => "Approve and send",
        "schedule_draft" => "Approve and schedule",
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

/// The first sentence of an agent-facing refusal, for the banner. For a missing
/// grant it names the grant, which is all the user needs there.
fn first_sentence(text: &str) -> String {
    let sentence = text.split(". ").next().unwrap_or_default();
    if sentence.is_empty() || sentence.ends_with('.') {
        sentence.to_string()
    } else {
        format!("{sentence}.")
    }
}

fn grants_of(consent: &Value) -> Vec<String> {
    consent["grants"]
        .as_array()
        .map(|g| g.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// A list card: rows with a title and subtitle on the left, a control on the
/// right, and a hairline between rows.
fn list_card() -> gtk::Box {
    let card = gtk::Box::new(Orientation::Vertical, 0);
    card.add_css_class("agent-list");
    card
}

fn list_row(card: &gtk::Box, title: &str, subtitle: &impl IsA<gtk::Widget>, trailing: Option<&gtk::Widget>) {
    if card.first_child().is_some() {
        card.append(&gtk::Separator::new(Orientation::Horizontal));
    }
    let row = gtk::Box::new(Orientation::Horizontal, 12);
    row.add_css_class("agent-list-row");
    let names = gtk::Box::new(Orientation::Vertical, 2);
    names.set_hexpand(true);
    let heading = label(title, "agent-row-title");
    heading.set_wrap(false);
    heading.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    names.append(&heading);
    names.append(subtitle);
    row.append(&names);
    if let Some(trailing) = trailing {
        trailing.set_valign(gtk::Align::Center);
        row.append(trailing);
    }
    card.append(&row);
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

    /// Look for Claude Code and Codex again. Runs their CLIs on a separate
    /// service, so only when the window opens and after Connect, never on the
    /// five-second poll.
    fn refresh_clients(self: &Rc<Self>) {
        if self.clients_loading.replace(true) {
            return;
        }
        let panel = self.clone();
        workflows::request_async(self.ui.database_path.clone(), json!({"op":"agent_clients"}), move |result| {
            panel.clients_loading.set(false);
            match result {
                Ok(state) => *panel.clients.borrow_mut() = Some(state),
                Err(error) => panel.set_status(&error, true),
            }
            panel.render();
        });
    }

    fn add(&self, widget: &impl IsA<gtk::Widget>) {
        self.sections.append(widget);
    }

    fn section(&self, text: &str, count: Option<usize>) {
        let row = gtk::Box::new(Orientation::Horizontal, 7);
        row.set_margin_top(16);
        let title = gtk::Label::new(Some(text));
        title.add_css_class("agent-section-title");
        row.append(&title);
        if let Some(count) = count {
            let badge = gtk::Label::new(Some(&if count > 99 { "99+".into() } else { count.to_string() }));
            badge.add_css_class("agent-count");
            badge.set_valign(gtk::Align::Center);
            row.append(&badge);
            row.update_property(&[gtk::accessible::Property::Label(&format!("{text}, {count}"))]);
        }
        self.add(&row);
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
        *self.copy_button.borrow_mut() = None;
        self.add(&label("Agent access", "agent-title"));
        self.add(&label(
            "Let AI tools like Claude Code and Codex work with your mail. You approve every send.",
            "agent-lead",
        ));
        let blocked = self.blocked.borrow().clone();
        if let Some(banner) = blocked.and_then(|blocked| self.blocked_banner(&blocked)) {
            banner.set_margin_top(8);
            self.add(&banner);
        }
        let pending = self.pending.borrow().clone();
        if !pending.is_empty() {
            self.section("Waiting for you", Some(pending.len()));
            for item in &pending {
                self.add(&self.pending_card(item));
            }
        }
        self.section("Connect an agent", None);
        self.add(&self.connect_list());
        let note = self.clients.borrow().as_ref().and_then(|c| c["helper_problem"].as_str().map(str::to_owned));
        self.add(&label(
            note.as_deref().unwrap_or("Connected agents can use only the accounts you turn on below."),
            "agent-note",
        ));
        self.section("Accounts", None);
        let accounts = self.accounts.borrow().clone();
        if accounts.is_empty() {
            self.add(&label("Add a mail account first.", "agent-secondary"));
        } else {
            self.add(&self.account_list(&accounts));
        }
        let expanded = self.history_expanded.get();
        let toggle = disclosure_button("Recent activity", expanded);
        toggle.set_margin_top(12);
        let panel = self.clone();
        toggle.connect_clicked(move |_| {
            panel.history_expanded.set(!panel.history_expanded.get());
            panel.render();
        });
        self.add(&toggle);
        if expanded {
            let history = self.history.borrow().clone();
            if history.is_empty() {
                self.add(&label("Nothing yet.", "agent-secondary"));
            }
            for item in &history {
                self.add(&history_row(item));
            }
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
        // The refusal is written for the agent; a missing grant's first
        // sentence says which grant.
        if blocked["needs_access"].as_bool() == Some(false)
            && let Some(reason) = blocked["reason"].as_str().filter(|r| !r.is_empty())
        {
            card.append(&label(&first_sentence(reason), "agent-footnote"));
        }
        if let [account] = affected.as_slice() {
            let id = account["account_id"].as_str().unwrap_or_default().to_string();
            let email = account["email"].as_str().unwrap_or_default().to_string();
            let current = account["current"].as_bool() == Some(true);
            let grants: Option<Vec<String>> = current.then(|| grants_of(&account["consent"]));
            let title = if current {
                format!("Change access for {email}…")
            } else {
                format!("Turn on for {email}…")
            };
            let button = gtk::Button::with_label(&title);
            button.add_css_class("agent-button");
            button.set_halign(gtk::Align::Start);
            let panel = self.clone();
            button.connect_clicked(move |_| panel.turn_on(&id, &email, grants.clone()));
            card.append(&button);
        } else {
            card.append(&label(
                "Turn on each account the agent may use under Accounts.",
                "agent-footnote",
            ));
        }
        Some(card)
    }

    fn pending_card(self: &Rc<Self>, item: &Value) -> gtk::Box {
        let id = item["id"].as_str().unwrap_or_default().to_string();
        let kind = item["kind"].as_str().unwrap_or_default().to_string();
        let details = &item["details"];
        let card = gtk::Box::new(Orientation::Vertical, 5);
        card.add_css_class("agent-card");
        card.append(&label(kind_title(&kind), "agent-kind"));
        let mut toggle: Option<gtk::Button> = None;
        match kind.as_str() {
            "send_draft" | "queue_send_draft" | "schedule_draft" => {
                let draft = &details["draft"];
                let subject = draft["subject"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no subject)");
                let headline = label(subject, "agent-summary");
                headline.set_margin_bottom(2);
                card.append(&headline);
                for (name, key) in [("To", "to"), ("Cc", "cc"), ("Bcc", "bcc"), ("Reply-To", "reply_to")] {
                    let value = addresses(details, key);
                    if !value.is_empty() || key == "to" {
                        card.append(&field(name, &value));
                    }
                }
                card.append(&field("From", draft["from"].as_str().unwrap_or_default()));
                if kind == "schedule_draft" {
                    card.append(&field("When", &when(&kind, details)));
                }
                let invisible = details["invisible_count"].as_u64().unwrap_or(0);
                if invisible > 0 {
                    card.append(&label(
                        &format!("Contains {invisible} invisible character{} used by emoji or some scripts.", if invisible == 1 { "" } else { "s" }),
                        "agent-footnote",
                    ));
                }
                let files = draft["attachments"].as_array().cloned().unwrap_or_default();
                let expanded = self.expanded.borrow().contains(&id);
                if expanded {
                    let body = draft["body_plain"].as_str().unwrap_or_default();
                    // Expanded shows every character that would be sent; nothing is cut.
                    let quote = gtk::Box::new(Orientation::Vertical, 0);
                    quote.add_css_class("agent-quote");
                    quote.set_margin_top(5);
                    let preview = label(if body.is_empty() { "(empty message)" } else { body }, "agent-body");
                    preview.set_selectable(true);
                    quote.append(&preview);
                    card.append(&quote);
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
                        save.add_css_class("agent-button");
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
                    if kind != "schedule_draft" {
                        card.append(&field("When", &when(&kind, details)));
                    }
                }
                let attachments = match files.len() {
                    0 => String::new(),
                    1 => " · 1 attachment".into(),
                    n => format!(" · {n} attachments"),
                };
                let button = disclosure_button(
                    &format!("{}{attachments}", if expanded { "Hide message" } else { "Show message" }),
                    expanded,
                );
                let panel = self.clone();
                let key = id.clone();
                button.connect_clicked(move |_| {
                    {
                        let mut expanded = panel.expanded.borrow_mut();
                        if !expanded.insert(key.clone()) {
                            expanded.remove(&key);
                        }
                    }
                    panel.render();
                });
                toggle = Some(button);
            }
            "batch_triage" | "undo_triage" => {
                card.append(&label(item["summary"].as_str().unwrap_or_default(), "agent-summary"));
                let count = details["count"].as_u64().unwrap_or(0) as usize;
                card.append(&field("Accounts", &list(&details["account_emails"])));
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
                card.append(&label(item["summary"].as_str().unwrap_or_default(), "agent-summary"));
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
            _ => card.append(&label(item["summary"].as_str().unwrap_or_default(), "agent-summary")),
        }
        let mut meta = Vec::new();
        if let Some(client) = item["client"].as_str().filter(|c| !c.is_empty()) {
            meta.push(format!("Requested by {client}"));
        }
        meta.push(date_time(item["created_at_ms"].as_i64()));
        card.append(&label(&meta.join(" · "), "agent-footnote"));
        let actions = gtk::Box::new(Orientation::Horizontal, 8);
        actions.set_margin_top(5);
        if let Some(toggle) = toggle {
            actions.append(&toggle);
        }
        let spacer = gtk::Box::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        actions.append(&spacer);
        let reject = gtk::Button::with_label("Reject");
        reject.add_css_class("agent-button");
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

    fn connect_list(self: &Rc<Self>) -> gtk::Box {
        let card = list_card();
        let clients = self.clients.borrow().clone();
        let rows = clients
            .as_ref()
            .and_then(|c| c["clients"].as_array().cloned())
            .unwrap_or_else(|| vec![json!({"id":"claude","name":"Claude Code"}), json!({"id":"codex","name":"Codex"})]);
        let helper_found = clients.as_ref().is_some_and(|c| c["helper"].is_string());
        for row in rows {
            let id = row["id"].as_str().unwrap_or_default().to_string();
            let name = row["name"].as_str().unwrap_or(&id).to_string();
            let subtitle = if clients.is_none() { "Checking…" } else { row["subtitle"].as_str().unwrap_or_default() };
            let trailing: Option<gtk::Widget> = if row["connected"].as_bool() == Some(true) {
                let connected = gtk::Label::new(None);
                connected.set_markup("<span weight=\"bold\" foreground=\"#34c759\">✓</span> Connected");
                connected.add_css_class("agent-connected");
                connected.update_property(&[gtk::accessible::Property::Label("Connected")]);
                Some(connected.upcast())
            } else if row["installed"].as_bool() == Some(true) {
                let busy = self.connecting.borrow().as_deref() == Some(id.as_str());
                let button = gtk::Button::with_label(if busy { "Connecting…" } else { "Connect" });
                button.add_css_class("agent-button");
                button.set_sensitive(helper_found && self.connecting.borrow().is_none());
                button.update_property(&[gtk::accessible::Property::Label(&format!("Connect {name}"))]);
                let panel = self.clone();
                let (id, name) = (id.clone(), name.clone());
                button.connect_clicked(move |_| panel.connect_client(&id, &name));
                Some(button.upcast())
            } else {
                None
            };
            list_row(&card, &name, &label(subtitle, "agent-row-subtitle"), trailing.as_ref());
        }
        let copy = gtk::Button::with_label("Copy");
        copy.add_css_class("agent-button");
        copy.set_sensitive(clients.as_ref().is_some_and(|c| c["manual"].is_object()));
        copy.update_property(&[gtk::accessible::Property::Label("Copy the command and JSON config for another MCP client")]);
        let panel = self.clone();
        copy.connect_clicked(move |button| panel.copy_manual_config(button));
        *self.copy_button.borrow_mut() = Some(copy.clone());
        list_row(&card, "Other MCP client", &label("Copy the command and a JSON config", "agent-row-subtitle"), Some(copy.upcast_ref::<gtk::Widget>()));
        card
    }

    fn connect_client(self: &Rc<Self>, id: &str, name: &str) {
        if self.connecting.borrow().is_some() {
            return;
        }
        *self.connecting.borrow_mut() = Some(id.to_string());
        self.render();
        let panel = self.clone();
        let name = name.to_string();
        workflows::request_async(
            self.ui.database_path.clone(),
            json!({"op":"connect_agent_client","client":id}),
            move |result| {
                match result {
                    Ok(state) => panel.set_status(
                        &if state["already_registered"].as_bool() == Some(true) {
                            format!("{name} already had Email set up.")
                        } else {
                            format!("{name} is connected. Start a new {name} session to use it.")
                        },
                        false,
                    ),
                    Err(error) => panel.set_status(&error, true),
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
        self.set_status("Copied the mail-mcp command and a JSON config. Paste whichever your client takes.", false);
        button.set_label("Copied");
        let weak = button.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
            if let Some(button) = weak.upgrade() {
                button.set_label("Copy");
            }
        });
    }

    fn account_list(self: &Rc<Self>, accounts: &[Value]) -> gtk::Box {
        let card = list_card();
        for account in accounts {
            let id = account["account_id"].as_str().unwrap_or_default().to_string();
            let email = account["email"].as_str().unwrap_or(&id).to_string();
            let consent = account["consent"].clone();
            let on = !consent.is_null() && account["current"].as_bool() == Some(true);
            let subtitle = gtk::Box::new(Orientation::Horizontal, 6);
            if on {
                let summary = gtk::Label::new(Some(account["grant_summary"].as_str().unwrap_or("Read")));
                summary.add_css_class("agent-row-subtitle");
                summary.set_wrap(true);
                summary.set_xalign(0.0);
                subtitle.append(&summary);
                let edit = gtk::Button::new();
                let edit_label = gtk::Label::new(None);
                edit_label.set_markup("<u>Edit</u>");
                edit.set_child(Some(&edit_label));
                edit.add_css_class("agent-link");
                edit.set_valign(gtk::Align::Center);
                edit.update_property(&[gtk::accessible::Property::Label(&format!("Edit agent access for {email}"))]);
                let panel = self.clone();
                let (eid, eemail, grants) = (id.clone(), email.clone(), grants_of(&consent));
                edit.connect_clicked(move |_| panel.turn_on(&eid, &eemail, Some(grants.clone())));
                subtitle.append(&edit);
            } else {
                subtitle.append(&label(
                    if consent.is_null() { "Off" } else { "Off · the terms changed; switch on to review them" },
                    "agent-row-subtitle",
                ));
            }
            let switch = gtk::Switch::new();
            switch.set_active(on);
            switch.update_property(&[gtk::accessible::Property::Label(&format!("Agent access for {email}"))]);
            let panel = self.clone();
            switch.connect_state_set(move |_, active| {
                if active {
                    panel.turn_on(&id, &email, None);
                } else {
                    panel.turn_off(&id, &email);
                }
                glib::Propagation::Proceed
            });
            list_row(&card, account["email"].as_str().unwrap_or_default(), &subtitle, Some(switch.upcast_ref::<gtk::Widget>()));
        }
        card
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
                                    panel.show_status_action(Some("Cancel send"), None, Some(cancel));
                                } else {
                                    panel.set_status(&format!("Approved · sending in {seconds} seconds"), false);
                                    panel.show_status_action(Some("Undo send"), Some(send_at), Some(cancel));
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

    /// Opens the turn-on window: from the switch (`current` None), the banner,
    /// or Edit with the current grants. Cancelling puts the switch back.
    fn turn_on(self: &Rc<Self>, account_id: &str, email: &str, current: Option<Vec<String>>) {
        if self.presenting.replace(true) {
            return self.rerender_later();
        }
        let disclosure = match self.ui.request(json!({"op":"agent_access_disclosure","account_id":account_id})) {
            Ok(value) => value,
            Err(error) => {
                self.presenting.set(false);
                self.set_status(&error, true);
                return self.rerender_later();
            }
        };
        let panel = self.clone();
        let (account_id, email) = (account_id.to_string(), email.to_string());
        let changing = current.is_some();
        let version = disclosure["version"].clone();
        disclosure_dialog(&self.window, &disclosure, current, move |grants| {
            panel.presenting.set(false);
            let Some(grants) = grants else { return panel.rerender_later() };
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

    /// Re-render after the current signal handler returns, so a switch is
    /// never destroyed inside its own state-set handler.
    fn rerender_later(self: &Rc<Self>) {
        let panel = self.clone();
        glib::idle_add_local_once(move || panel.render());
    }

    fn turn_off(self: &Rc<Self>, account_id: &str, email: &str) {
        if self.presenting.replace(true) {
            return self.rerender_later();
        }
        let confirm = gtk::AlertDialog::builder()
            .message(format!("Turn off agent access for {email}?"))
            .detail("Agents lose access on their next request, and requests waiting for you are rejected. Mail you already approved stays in the Outbox.")
            .buttons(["Cancel", "Turn off"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let panel = self.clone();
        let (account_id, email) = (account_id.to_string(), email.to_string());
        confirm.choose(Some(&self.window), None::<&gtk::gio::Cancellable>, move |result| {
            panel.presenting.set(false);
            if result == Ok(1) {
                panel.revoke(&account_id, &email);
            } else {
                panel.rerender_later();
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

/// The style of a grant's note: the warning color for a grant that takes
/// effect without approval, muted otherwise.
fn grant_note_class(warning: bool) -> &'static str {
    if warning { "agent-check-warning" } else { "agent-check-note" }
}

/// "Let agents use {email}?". Read is required and always on; every other
/// grant starts unchecked. `chosen` runs once with the selected grants, or
/// None when cancelled.
pub(super) fn disclosure_dialog(
    parent: &gtk::Window,
    disclosure: &Value,
    current: Option<Vec<String>>,
    chosen: impl Fn(Option<Vec<String>>) + 'static,
) -> gtk::Window {
    let changing = current.is_some();
    let dialog = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Agent access")
        .default_width(460)
        .build();
    dialog.add_css_class("agent-window");
    dialog.add_css_class("agent-sheet");
    let root = gtk::Box::new(Orientation::Vertical, 0);
    let column = gtk::Box::new(Orientation::Vertical, 8);
    column.set_margin_top(22);
    column.set_margin_bottom(8);
    column.set_margin_start(24);
    column.set_margin_end(24);
    column.append(&label(disclosure["title"].as_str().unwrap_or("Agent access"), "agent-sheet-title"));
    column.append(&label(disclosure["lead"].as_str().unwrap_or_default(), "agent-lead"));
    let checks = gtk::Box::new(Orientation::Vertical, 4);
    checks.set_margin_top(10);
    let check = |text: &str, note: Option<&str>, warning: bool| {
        let check = gtk::CheckButton::new();
        let content = gtk::Box::new(Orientation::Horizontal, 8);
        let title = gtk::Label::new(Some(text));
        title.add_css_class("agent-check-label");
        content.append(&title);
        if let Some(note) = note {
            let note = gtk::Label::new(Some(&format!("·  {note}")));
            note.set_wrap(true);
            note.set_xalign(0.0);
            note.add_css_class(grant_note_class(warning));
            content.append(&note);
        }
        check.set_child(Some(&content));
        check
    };
    let read = &disclosure["read"];
    let read_box = check(read["label"].as_str().unwrap_or("Read mail and calendar"), read["note"].as_str(), false);
    read_box.set_active(true);
    read_box.set_sensitive(false);
    checks.append(&read_box);
    let mut boxes = Vec::new();
    for grant in disclosure["grants"].as_array().into_iter().flatten() {
        let Some(id) = grant["id"].as_str() else { continue };
        let check = check(
            grant["label"].as_str().unwrap_or(id),
            grant["note"].as_str(),
            grant["warning"].as_bool() == Some(true),
        );
        // Reading is the only default: every optional grant starts off.
        check.set_active(current.as_ref().is_some_and(|c| c.iter().any(|g| g == id)));
        check.set_widget_name(&format!("agent-grant-{id}"));
        if let Some(detail) = grant["detail"].as_str() {
            check.set_tooltip_text(Some(detail));
            check.update_property(&[gtk::accessible::Property::Description(detail)]);
        }
        checks.append(&check);
        boxes.push((id.to_string(), check));
    }
    column.append(&checks);
    let callout = gtk::Box::new(Orientation::Vertical, 0);
    callout.add_css_class("agent-callout");
    callout.set_margin_top(10);
    callout.append(&label(disclosure["provider_terms"].as_str().unwrap_or_default(), "agent-body"));
    column.append(&callout);
    let revoke = label(disclosure["revoke"].as_str().unwrap_or_default(), "agent-note");
    revoke.set_margin_top(4);
    column.append(&revoke);
    let details_title = disclosure["details_title"].as_str().unwrap_or("What exactly can agents see?").to_string();
    let toggle = disclosure_button(&details_title, false);
    toggle.set_margin_top(4);
    column.append(&toggle);
    let details = gtk::Box::new(Orientation::Vertical, 6);
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
    let expanded = Cell::new(false);
    let (list, scroller, content) = (details.clone(), scroll.clone(), column.clone());
    toggle.connect_clicked(move |button| {
        let open = !expanded.get();
        expanded.set(open);
        set_disclosure(button, &details_title, open);
        list.set_visible(open);
        // Grow the window to the list, up to the scroll limit; GTK does not
        // grow a mapped window for a larger natural size on its own.
        let natural = content.measure(Orientation::Vertical, 412).1;
        scroller.set_min_content_height(if open { natural.min(600) } else { -1 });
    });
    root.append(&scroll);
    let buttons = gtk::Box::new(Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(12);
    buttons.set_margin_bottom(18);
    buttons.set_margin_start(24);
    buttons.set_margin_end(24);
    let cancel = gtk::Button::with_label(disclosure["cancel"].as_str().unwrap_or("Cancel"));
    cancel.add_css_class("agent-button");
    let confirm = gtk::Button::with_label(
        disclosure[if changing { "change_confirm" } else { "confirm" }]
            .as_str()
            .unwrap_or("Turn on"),
    );
    confirm.add_css_class("agent-approve");
    buttons.append(&cancel);
    buttons.append(&confirm);
    root.append(&buttons);
    dialog.set_child(Some(&root));
    let chosen = Rc::new(chosen);
    // Exactly one answer: a choice, Cancel, Escape or closing the window.
    let answered = Rc::new(Cell::new(false));
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let (d, done, answer) = (dialog.clone(), chosen.clone(), answered.clone());
    confirm.connect_clicked(move |_| {
        let grants = boxes
            .iter()
            .filter(|(_, check)| check.is_active())
            .map(|(id, _)| id.clone())
            .collect();
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

pub(super) const CSS: &str = r#"
.agent-window { background: @canvas; color: @ink; }
.agent-title { font-size: 20px; font-weight: 600; }
.agent-sheet-title { font-size: 16px; font-weight: 600; }
.agent-lead { font-size: 12.5px; color: @muted; }
.agent-section-title { font-size: 13px; font-weight: 600; }
.agent-count { background: @ink; color: @canvas; border-radius: 9px; padding: 0 6px; min-width: 6px; font-size: 11px; font-weight: 600; }
.agent-secondary { font-size: 12px; color: @muted; }
.agent-footnote { font-size: 11px; color: @muted; }
.agent-note { font-size: 11.5px; color: @muted; }
.agent-meta { font-size: 10.5px; color: @muted; }
.agent-body { font-size: 12px; }
.agent-detail { font-size: 11.5px; }
.agent-field { font-size: 12px; }
.agent-strong { font-size: 12.5px; font-weight: 600; }
.agent-kind { font-size: 10px; font-weight: 600; color: @muted; letter-spacing: 0.6px; }
.agent-summary { font-size: 14px; font-weight: 600; }
.agent-card { background: @canvas; border: 1px solid @line; border-radius: 10px; padding: 12px 14px; }
.agent-list { background: @canvas; border: 1px solid @line; border-radius: 10px; padding: 0 14px; }
.agent-list > separator { background: @line; min-height: 1px; }
.agent-list-row { padding: 10px 0; }
.agent-row-title { font-size: 13px; font-weight: 600; }
.agent-row-subtitle { font-size: 11.5px; color: @muted; }
.agent-connected { font-size: 12.5px; font-weight: 500; }
.agent-link { background: none; border: 0; box-shadow: none; padding: 0 2px; min-height: 0; font-size: 11.5px; font-weight: 600; color: @ink; }
.agent-disclosure { background: none; border: 0; box-shadow: none; padding: 2px 0; min-height: 0; }
.agent-disclosure:hover { opacity: 0.8; }
.agent-disclosure-title { font-size: 13px; font-weight: 600; }
.agent-card .agent-disclosure-title, .agent-sheet .agent-disclosure-title { font-size: 12px; }
.agent-disclosure image { color: @muted; -gtk-icon-size: 12px; }
.agent-check-label { font-size: 13px; }
.agent-check-note { font-size: 12px; color: @muted; }
.agent-check-warning { font-size: 12px; color: @warning; }
.agent-banner { background: @selected; border: 1px solid @line; border-radius: 10px; padding: 12px 14px; }
.agent-callout { background: @selected; border: 1px solid @line; border-radius: 10px; padding: 12px 14px; }
.agent-quote { background: @surface; border: 1px solid @line; border-radius: 10px; padding: 12px 14px; }
.agent-button { background: @selected; padding: 4px 12px; font-size: 12.5px; }
.agent-button:hover { background: @hover; }
.agent-button:disabled { opacity: 0.5; }
.agent-banner .agent-button { background: @canvas; border: 1px solid @line; }
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
        assert_eq!(approve_title("send_draft"), "Approve and send");
        assert_eq!(approve_title("schedule_draft"), "Approve and schedule");
        assert_eq!(approve_title("confirm_sender_cleanup"), "Approve");
        assert_eq!(when("send_draft", &json!({})), "After approval, with 10 seconds to undo");
        assert_eq!(
            when("queue_send_draft", &json!({"undo_seconds":30})),
            "After approval, with 30 seconds to undo"
        );
    }

    #[test]
    fn banner_shows_only_the_grant_sentence_of_a_refusal() {
        assert_eq!(
            first_sentence("Agent access for this account does not include “Ask to send”. Ask the user to turn on “Ask to send” for this account under Agent access in the Email app."),
            "Agent access for this account does not include “Ask to send”."
        );
        assert_eq!(first_sentence("No sentence end"), "No sentence end.");
        assert_eq!(first_sentence(""), "");
    }

    #[test]
    fn grant_notes_use_the_warning_color_only_for_unapproved_changes() {
        assert_eq!(grant_note_class(true), "agent-check-warning");
        assert_eq!(grant_note_class(false), "agent-check-note");
    }
}
