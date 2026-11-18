//! Filters in the GTK shell (docs/filters.md): the sidebar section, the
//! setup sheet, the filter view's header, the reader bar, the editor, the
//! settings sheet and the background sort loop. macOS is the reference;
//! this shell says "this computer" where macOS says "this Mac".
//!
//! Sorting and the model download block, so they run on worker threads with
//! their own `MailService`; `filters_status` is cheap and read on the main
//! loop (the download's progress is process-wide in mail-service).
use super::*;
use mail_core::{FilterAction, ThreadFilterTag};
use std::time::Duration;

const ACTIONS: [&str; 3] = ["inbox", "skip_inbox", "ignore"];
const ACTION_TITLES: [&str; 3] = ["Keep in inbox", "Skip inbox", "Ignore"];
const WAKE_BUDGET_MS: u64 = 20_000;
/// mail-core's limit.
const MAX_FILTERS: usize = 8;

thread_local! {
    static SECTION: RefCell<Option<Rc<Section>>> = const { RefCell::new(None) };
    static EDIT: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
    static BAR: RefCell<Option<Rc<Bar>>> = const { RefCell::new(None) };
    static SETTINGS: RefCell<Option<Rc<Settings>>> = const { RefCell::new(None) };
    /// The last `filters_status` answer.
    static STATUS: RefCell<Value> = RefCell::new(Value::Null);
    /// Filters are on and the model is installed: new mail waits for sorting.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static SORTING: Cell<bool> = const { Cell::new(false) };
    static SORT_AGAIN: Cell<bool> = const { Cell::new(false) };
    /// This shell's download worker is running.
    static DOWNLOADING: Cell<bool> = const { Cell::new(false) };
    static POLLING: Cell<bool> = const { Cell::new(false) };
}

fn action_index(action: &str) -> usize {
    ACTIONS.iter().position(|a| *a == action).unwrap_or(0)
}

fn action_title(action: &str) -> &'static str {
    ACTION_TITLES[action_index(action)]
}

fn action_icon(action: &str) -> &'static str {
    match action {
        "skip_inbox" => "filter-skip-symbolic",
        "ignore" => "filter-ignore-symbolic",
        _ => "tag-symbolic",
    }
}

/// The editor's line under "When it arrives".
fn action_note(action: &str) -> &'static str {
    match action {
        "skip_inbox" => "Archived. Read it here when you want.",
        "ignore" => "Archived and marked read.",
        _ => "Stays in your inbox with a tag.",
    }
}

/// The filter view's line under its title.
fn subtitle(action: &str, last_7_days: u64) -> String {
    let what = match action {
        "skip_inbox" => "Skips your inbox",
        "ignore" => "Ignored",
        _ => "Tagged in your inbox",
    };
    format!("{what} · {last_7_days} this week")
}

/// "2.5 GB" with one decimal, or whole MB below a gigabyte, as on macOS.
fn gigabytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else {
        format!("{:.0} MB", bytes as f64 / 1e6)
    }
}

fn tag_action(tag: &ThreadFilterTag) -> &'static str {
    match tag.action {
        FilterAction::Inbox => "inbox",
        FilterAction::SkipInbox => "skip_inbox",
        FilterAction::Ignore => "ignore",
    }
}

/// Read `filters_status` and remember it.
pub(super) fn load_status(ui: &Ui) -> Value {
    let status = ui.request(json!({"op":"filters_status"})).unwrap_or(Value::Null);
    ACTIVE.with(|a| a.set(status["enabled"] == true && status["model"]["installed"] == true));
    STATUS.with(|s| *s.borrow_mut() = status.clone());
    status
}

fn cached() -> Value {
    STATUS.with(|s| s.borrow().clone())
}

fn filters_of(status: &Value) -> Vec<Value> {
    status["filters"].as_array().cloned().unwrap_or_default()
}

pub(super) fn filter(id: &str) -> Option<Value> {
    filters_of(&cached()).into_iter().find(|f| f["id"] == id)
}

fn downloading(status: &Value) -> bool {
    status["model"]["downloading"] == true || DOWNLOADING.with(Cell::get)
}

fn percent(model: &Value) -> u64 {
    let total = model["download_bytes"].as_u64().unwrap_or(0).max(1);
    (model["downloaded_bytes"].as_u64().unwrap_or(0).saturating_mul(100) / total).min(100)
}

fn background(ui: &Ui, op: Value, done: impl FnOnce(Result<Value, String>) + 'static) {
    let path = ui.database_path.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = MailService::open(&path)
            .map_err(|error| error.to_string())
            .and_then(|service| service.request_json(&op.to_string()));
        let _ = tx.send(result);
    });
    let mut done = Some(done);
    glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
        Ok(result) => {
            if let Some(done) = done.take() {
                done(result);
            }
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(_) => {
            if let Some(done) = done.take() {
                done(Err("Filters worker stopped".into()));
            }
            glib::ControlFlow::Break
        }
    });
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.add_css_class(class);
    label
}

/// Esc closes; Enter runs `confirm` (when given) unless a menu has the key
/// or Shift is held (a new line in the description).
fn sheet_keys(dialog: &gtk::Window, confirm: Option<gtk::Button>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let d = dialog.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let in_menu = gtk::prelude::GtkWindowExt::focus(&d)
            .and_then(|w| w.ancestor(gtk::Popover::static_type()))
            .is_some();
        match key {
            gdk::Key::Escape if !in_menu => {
                d.close();
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter
                if !in_menu && confirm.is_some() && !mods.contains(gdk::ModifierType::SHIFT_MASK) =>
            {
                if let Some(button) = confirm.as_ref().filter(|b| b.is_sensitive()) {
                    button.emit_clicked();
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    dialog.add_controller(keys);
}

fn sheet(ui: &Ui, title: &str, width: i32) -> gtk::Window {
    let dialog = gtk::Window::builder()
        .transient_for(&ui.window)
        .modal(true)
        .title(title)
        .default_width(width)
        .resizable(false)
        .build();
    dialog.add_css_class("agent-window");
    dialog.add_css_class("agent-sheet");
    // Focus goes back to the list, as after the palette.
    let list = ui.list.clone();
    dialog.connect_close_request(move |_| {
        list.grab_focus();
        glib::Propagation::Proceed
    });
    dialog
}

fn list_card() -> gtk::Box {
    let card = gtk::Box::new(Orientation::Vertical, 0);
    card.add_css_class("agent-list");
    card
}

fn card_row(card: &gtk::Box, row: &impl IsA<gtk::Widget>) {
    if card.first_child().is_some() {
        card.append(&gtk::Separator::new(Orientation::Horizontal));
    }
    card.append(row);
}

// MARK: Sidebar

struct Section {
    rows: gtk::Box,
    signature: RefCell<String>,
    progress: RefCell<Option<gtk::Label>>,
}

/// The FILTERS heading and its rows, placed between the workspace items and
/// SPLITS; filled by [`refresh_sidebar`].
pub(super) fn section() -> gtk::Box {
    let root = gtk::Box::new(Orientation::Vertical, 0);
    root.append(&sidebar_heading("FILTERS"));
    let rows = gtk::Box::new(Orientation::Vertical, 0);
    root.append(&rows);
    SECTION.with(|slot| {
        *slot.borrow_mut() = Some(Rc::new(Section {
            rows,
            signature: RefCell::new(String::new()),
            progress: RefCell::new(None),
        }))
    });
    root
}

/// A sidebar row like `navigation_button`, returning its title and its
/// right-hand label (the count).
fn side_row(icon: &str, name: &str, count: &str) -> (gtk::Button, gtk::Label, gtk::Label) {
    let button = navigation_button(icon, name, count);
    let child = |c: Option<gtk::Widget>| {
        c.and_then(|c| c.downcast::<gtk::Label>().ok()).unwrap_or_else(|| gtk::Label::new(None))
    };
    let content = button.child();
    let title = child(content.as_ref().and_then(|c| c.first_child()).and_then(|c| c.next_sibling()));
    let hint = child(content.and_then(|c| c.last_child()));
    (button, title, hint)
}

/// Status rows are longer than the narrow sidebar: two lines, not cut off.
fn wrap(title: &gtk::Label) {
    title.set_ellipsize(gtk::pango::EllipsizeMode::None);
    title.set_wrap(true);
    title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    title.set_max_width_chars(14);
}

/// The sidebar is narrow, so the percentage sits where counts go.
fn progress_percent(status: &Value) -> String {
    format!("{}%", percent(&status["model"]))
}

fn settings_progress(status: &Value) -> String {
    format!("Downloading… {}", progress_percent(status))
}

pub(super) fn refresh_sidebar(ui: &Rc<Ui>) {
    let status = load_status(ui);
    let Some(section) = SECTION.with(|s| s.borrow().clone()) else { return };
    let enabled = status["enabled"] == true;
    let filters = filters_of(&status);
    let model = &status["model"];
    let installed = model["installed"] == true;
    let state = if !enabled && filters.is_empty() {
        "setup"
    } else if !enabled {
        "off"
    } else if downloading(&status) {
        "downloading"
    } else if !installed && model["error"].is_string() {
        "failed"
    } else if !installed {
        "missing"
    } else {
        ""
    };
    let current = match &*ui.view.borrow() {
        MailView::Filter(id) if ui.workspace.visible_child_name().as_deref() == Some("mail") => id.clone(),
        _ => String::new(),
    };
    let signature = format!(
        "{enabled}|{state}|{current}|{}",
        filters
            .iter()
            .map(|f| format!("{}:{}:{}:{}", f["id"], f["name"], f["action"], f["unread"]))
            .collect::<Vec<_>>()
            .join(",")
    );
    if *section.signature.borrow() == signature {
        // Only the download moved on: keep the rows (and any open menu).
        if let Some(progress) = section.progress.borrow().as_ref() {
            progress.set_text(&progress_percent(&status));
        }
        return;
    }
    *section.signature.borrow_mut() = signature;
    *section.progress.borrow_mut() = None;
    while let Some(child) = section.rows.first_child() {
        section.rows.remove(&child);
    }
    let weak = Rc::downgrade(ui);
    let open_settings = move |_: &gtk::Button| {
        if let Some(ui) = weak.upgrade() {
            settings(&ui);
        }
    };
    match state {
        "setup" => {
            let (row, _, _) = side_row("view-filter-symbolic", "Set up filters", "");
            row.set_tooltip_text(Some("Sort mail automatically on this computer"));
            let weak = Rc::downgrade(ui);
            row.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    setup(&ui);
                }
            });
            section.rows.append(&row);
            return;
        }
        "off" => {
            let (row, _, _) = side_row("filter-paused-symbolic", "Sorting is off", "");
            row.add_css_class("filter-status");
            row.connect_clicked(open_settings.clone());
            section.rows.append(&row);
        }
        "downloading" => {
            let (row, title, hint) = side_row("filter-download-symbolic", "Getting ready…", &progress_percent(&status));
            title.set_ellipsize(gtk::pango::EllipsizeMode::None);
            hint.add_css_class("filter-side-note");
            row.add_css_class("filter-status");
            row.set_tooltip_text(Some("Downloading the sorting model. Sorting starts when it finishes."));
            row.connect_clicked(open_settings.clone());
            section.rows.append(&row);
            *section.progress.borrow_mut() = Some(hint);
        }
        "failed" => {
            let (row, title, _) = side_row("filter-failed-symbolic", "Download failed – Retry", "");
            wrap(&title);
            row.add_css_class("filter-status");
            row.set_tooltip_text(model["error"].as_str());
            row.connect_clicked(open_settings.clone());
            section.rows.append(&row);
        }
        "missing" => {
            let (row, title, _) = side_row("filter-download-symbolic", "Download model to start sorting", "");
            wrap(&title);
            row.add_css_class("filter-status");
            row.connect_clicked(open_settings.clone());
            section.rows.append(&row);
        }
        _ => {}
    }
    for filter in &filters {
        let id = filter["id"].as_str().unwrap_or_default().to_string();
        let action = filter["action"].as_str().unwrap_or("inbox");
        let unread = filter["unread"].as_u64().unwrap_or(0);
        let count = if action != "ignore" && unread > 0 { unread.to_string() } else { String::new() };
        let (row, _, _) = side_row(action_icon(action), filter["name"].as_str().unwrap_or_default(), &count);
        row.set_tooltip_text(Some(&format!(
            "{} · {}",
            filter["name"].as_str().unwrap_or_default(),
            action_title(action)
        )));
        if id == current {
            row.add_css_class("active");
        }
        let weak = Rc::downgrade(ui);
        let view_id = id.clone();
        row.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.set_view(MailView::Filter(view_id.clone()));
            }
        });
        context_menu(ui, &row, filter.clone());
        section.rows.append(&row);
    }
    if filters.len() >= MAX_FILTERS {
        return;
    }
    let (new_filter, _, _) = side_row("list-add-symbolic", "New filter", "");
    let weak = Rc::downgrade(ui);
    new_filter.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            editor(&ui, None);
        }
    });
    section.rows.append(&new_filter);
}

/// Secondary click on a filter row: Edit filter…, Delete filter….
fn context_menu(ui: &Rc<Ui>, row: &gtk::Button, filter: Value) {
    let menu = gtk::Popover::new();
    menu.set_parent(row);
    menu.set_has_arrow(false);
    {
        // A plain button does not unparent extra children when it is destroyed.
        let menu = menu.clone();
        row.connect_destroy(move |_| menu.unparent());
    }
    let choices = gtk::Box::new(Orientation::Vertical, 2);
    choices.add_css_class("filter-menu");
    menu.set_child(Some(&choices));
    for (title, delete) in [("Edit filter…", false), ("Delete filter…", true)] {
        let button = gtk::Button::with_label(title);
        if let Some(label) = button.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            label.set_xalign(0.0);
        }
        let (weak, menu, filter) = (Rc::downgrade(ui), menu.clone(), filter.clone());
        button.connect_clicked(move |_| {
            menu.popdown();
            if let Some(ui) = weak.upgrade() {
                if delete {
                    confirm_delete(&ui, None, &filter);
                } else {
                    editor(&ui, Some(filter.clone()));
                }
            }
        });
        choices.append(&button);
    }
    let gesture = gtk::GestureClick::new();
    gesture.set_button(3);
    gesture.connect_pressed(move |_, _, x, y| {
        menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        menu.popup();
    });
    row.add_controller(gesture);
}

// MARK: Filter view

/// The filter view's Edit button, next to the list title.
pub(super) fn header_edit_button() -> gtk::Button {
    let button = gtk::Button::with_label("Edit");
    button.add_css_class("action");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some("Edit this filter"));
    button.set_visible(false);
    EDIT.with(|slot| *slot.borrow_mut() = Some(button.clone()));
    button
}

pub(super) fn connect_header(ui: &Rc<Ui>) {
    let Some(button) = EDIT.with(|slot| slot.borrow().clone()) else { return };
    let weak = Rc::downgrade(ui);
    button.connect_clicked(move |_| {
        let Some(ui) = weak.upgrade() else { return };
        let id = match &*ui.view.borrow() {
            MailView::Filter(id) => id.clone(),
            _ => return,
        };
        if let Some(filter) = filter(&id) {
            editor(&ui, Some(filter));
        }
    });
}

/// Title, subtitle and the Edit button for the current view. Returns the
/// filter's name in a filter view.
pub(super) fn update_header(ui: &Ui, view: &MailView) -> Option<String> {
    let current = match view {
        MailView::Filter(id) => filter(id),
        _ => None,
    };
    if let Some(button) = EDIT.with(|slot| slot.borrow().clone()) {
        button.set_visible(current.is_some());
    }
    let current = current?;
    let action = current["action"].as_str().unwrap_or("inbox");
    ui.inbox_hint.set_visible(true);
    ui.inbox_hint
        .set_text(&subtitle(action, current["last_7_days"].as_u64().unwrap_or(0)));
    Some(current["name"].as_str().unwrap_or_default().to_string())
}

pub(super) fn threads(ui: &Ui, id: &str) -> Result<Vec<ThreadSummary>, String> {
    ui.request(json!({"op":"list_filter_threads","filter_id":id,"limit":500,"offset":0}))
        .and_then(|value| serde_json::from_value(value).map_err(|e| e.to_string()))
}

/// The list's third line: the model's reason in a filter view; elsewhere a
/// Keep-in-inbox filter shows as the first tag (`#Notifications`).
pub(super) fn row_preview(view: &MailView, item: &Mail) -> String {
    if matches!(view, MailView::Filter(_)) {
        let reason = item.filter.as_ref().map(|f| f.reason.trim()).unwrap_or_default();
        return if reason.is_empty() { one_line(&item.preview) } else { one_line(reason) };
    }
    let tag = item.filter.as_ref().filter(|f| f.action == FilterAction::Inbox).map(|f| f.name.clone());
    let mut labels: Vec<String> = tag.iter().cloned().collect();
    labels.extend(item.labels.iter().filter(|l| Some(*l) != tag.as_ref()).cloned());
    format!("{}{}", row_tags(&labels), one_line(&item.preview))
}

// MARK: Reader bar

struct Bar {
    root: gtk::Box,
    text: gtk::Label,
    inbox: gtk::Button,
    choices: gtk::Box,
    menu: gtk::MenuButton,
    ui: RefCell<std::rc::Weak<Ui>>,
    /// Thread and account shown.
    thread: RefCell<Option<(String, String)>>,
}

/// The slim bar above a filtered conversation: the filter and the model's
/// reason on the left, Move to inbox and Move to on the right.
pub(super) fn reader_bar() -> gtk::Box {
    let root = gtk::Box::new(Orientation::Horizontal, 12);
    root.add_css_class("filter-bar");
    root.set_visible(false);
    let text = gtk::Label::new(None);
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.add_css_class("filter-bar-text");
    root.append(&text);
    let inbox = gtk::Button::with_label("Move to inbox");
    inbox.add_css_class("agent-button");
    inbox.set_valign(gtk::Align::Center);
    inbox.set_tooltip_text(Some("Back to the inbox; later mail from this sender stays there"));
    root.append(&inbox);
    let menu = gtk::MenuButton::new();
    menu.set_label("Move to");
    menu.add_css_class("filter-move");
    menu.update_property(&[gtk::accessible::Property::Label("Move to another filter")]);
    menu.set_valign(gtk::Align::Center);
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    let choices = gtk::Box::new(Orientation::Vertical, 2);
    choices.add_css_class("filter-menu");
    popover.set_child(Some(&choices));
    menu.set_popover(Some(&popover));
    root.append(&menu);
    let bar = Rc::new(Bar {
        root: root.clone(),
        text,
        inbox: inbox.clone(),
        choices,
        menu,
        ui: RefCell::new(std::rc::Weak::new()),
        thread: RefCell::new(None),
    });
    let weak = Rc::downgrade(&bar);
    inbox.connect_clicked(move |_| {
        let Some(bar) = weak.upgrade() else { return };
        let (Some(ui), Some((thread, account))) = (bar.ui.borrow().upgrade(), bar.thread.borrow().clone()) else {
            return;
        };
        move_thread(&ui, &thread, &account, None);
    });
    BAR.with(|slot| *slot.borrow_mut() = Some(bar));
    root
}

pub(super) fn hide_bar() {
    if let Some(bar) = BAR.with(|slot| slot.borrow().clone()) {
        bar.root.set_visible(false);
        bar.thread.borrow_mut().take();
    }
}

pub(super) fn show_bar(ui: &Rc<Ui>, item: &Mail) {
    let Some(bar) = BAR.with(|slot| slot.borrow().clone()) else { return };
    let tag = item.filter.as_ref().filter(|_| {
        !matches!(*ui.view.borrow(), MailView::Drafts | MailView::Outbox | MailView::AgentRequests)
    });
    let Some(tag) = tag else {
        hide_bar();
        return;
    };
    *bar.ui.borrow_mut() = Rc::downgrade(ui);
    *bar.thread.borrow_mut() = Some((item.thread_id.clone(), item.account_id.clone()));
    let reason = tag.reason.trim();
    bar.text.set_markup(&if reason.is_empty() {
        format!("<b>{}</b>", glib::markup_escape_text(&tag.name))
    } else {
        format!(
            "<b>{}</b>  <span alpha=\"62%\">{}</span>",
            glib::markup_escape_text(&tag.name),
            glib::markup_escape_text(reason)
        )
    });
    bar.root.set_tooltip_text((!reason.is_empty()).then_some(reason));
    bar.inbox.set_visible(!item.is_inbox);
    while let Some(child) = bar.choices.first_child() {
        bar.choices.remove(&child);
    }
    let mut targets: Vec<(Option<String>, String)> = filters_of(&cached())
        .into_iter()
        .filter(|f| f["id"] != tag.id.as_str())
        .map(|f| (f["id"].as_str().map(str::to_string), f["name"].as_str().unwrap_or_default().to_string()))
        .collect();
    targets.push((None, if tag_action(tag) == "inbox" { "No filter" } else { "Inbox" }.to_string()));
    for (id, name) in targets {
        let button = gtk::Button::with_label(&name);
        if let Some(label) = button.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            label.set_xalign(0.0);
        }
        let (weak, menu, thread, account) =
            (Rc::downgrade(ui), bar.menu.clone(), item.thread_id.clone(), item.account_id.clone());
        button.connect_clicked(move |_| {
            menu.popdown();
            if let Some(ui) = weak.upgrade() {
                move_thread(&ui, &thread, &account, id.clone());
            }
        });
        bar.choices.append(&button);
    }
    bar.root.set_visible(true);
}

/// Move a conversation to a filter (None: the inbox), then show the next one.
fn move_thread(ui: &Rc<Ui>, thread_id: &str, account_id: &str, filter_id: Option<String>) {
    let index = ui.list.selected_row().map(|r| r.index()).unwrap_or(0);
    match ui.request(json!({"op":"move_to_filter","thread_id":thread_id,"filter_id":filter_id})) {
        Ok(_) => {
            let target = filter_id
                .as_deref()
                .and_then(filter)
                .and_then(|f| f["name"].as_str().map(str::to_string))
                .unwrap_or_else(|| "inbox".into());
            ui.refresh();
            let still_there = ui.items.borrow().iter().any(|m| m.thread_id == thread_id);
            if !still_there {
                let last = ui.items.borrow().len() as i32 - 1;
                if let Some(row) = ui.list.row_at_index(index.min(last).max(0)) {
                    ui.list.select_row(Some(&row));
                }
            }
            ui.status.set_text(&format!("Moved to {target}"));
            if !account_id.starts_with("demo:") {
                ui.background(json!({"op":"sync_account","account_id":account_id}));
            }
        }
        Err(error) => ui.status.set_text(&format!("Could not move: {error}")),
    }
}

// MARK: Palette

/// As the macOS palette: move the selected conversation, go to a filter,
/// set up or add filters, and Filter settings.
pub(super) fn commands(ui: &Ui) -> Vec<parity::Command> {
    let mut items = Vec::new();
    let status = cached();
    if status.is_null() {
        return items;
    }
    let filters = filters_of(&status);
    let item = ui
        .selected()
        .and_then(|i| ui.items.borrow().get(i).cloned())
        .filter(|_| !matches!(*ui.view.borrow(), MailView::Drafts | MailView::Outbox | MailView::AgentRequests));
    if let Some(item) = item.filter(|item| status["enabled"] == true || item.filter.is_some()) {
        if let Some(tag) = &item.filter {
            items.push(
                parity::Command::new(
                    "filter-move:",
                    &if item.is_inbox { format!("Remove from {}", tag.name) } else { "Move to inbox".into() },
                    "",
                    &format!("not {} filter unfilter inbox", tag.name),
                )
                .detail("Later mail from this sender stays in the inbox"),
            );
        }
        let current = item.filter.as_ref().map(|f| f.id.as_str()).unwrap_or_default();
        for filter in filters.iter().filter(|f| f["id"] != current) {
            items.push(
                parity::Command::new(
                    &format!("filter-move:{}", filter["id"].as_str().unwrap_or_default()),
                    &format!("Move to {}", filter["name"].as_str().unwrap_or_default()),
                    "",
                    "filter sort move to filter",
                )
                .detail(action_title(filter["action"].as_str().unwrap_or("inbox"))),
            );
        }
    }
    for filter in &filters {
        items.push(
            parity::Command::new(
                &format!("filter-go:{}", filter["id"].as_str().unwrap_or_default()),
                &format!("Go to {}", filter["name"].as_str().unwrap_or_default()),
                "",
                "filter",
            )
            .detail("Filter"),
        );
    }
    if status["enabled"] != true && filters.is_empty() {
        items.push(
            parity::Command::new(
                "filter setup",
                "Set up filters",
                "",
                "filters sort mail cold outreach promotions newsletters ai",
            )
            .detail("Sort mail automatically on this computer"),
        );
    } else if filters.len() < MAX_FILTERS {
        items.push(parity::Command::new("filter new", "New filter", "", "create filter sort mail"));
    }
    items.push(parity::Command::new("filter settings", "Filter settings", "", "filters sorting model ai sort mail"));
    items
}

/// Palette commands from [`commands`]; false for any other id.
pub(super) fn execute(ui: &Rc<Ui>, id: &str) -> bool {
    match id {
        "filter settings" => settings(ui),
        "filter setup" => setup(ui),
        "filter new" => editor(ui, None),
        _ => {
            if let Some(id) = id.strip_prefix("filter-go:") {
                ui.set_view(MailView::Filter(id.to_string()));
                ui.list.grab_focus();
                return true;
            }
        }
    }
    if matches!(id, "filter settings" | "filter setup" | "filter new") {
        return true;
    }
    let Some(target) = id.strip_prefix("filter-move:") else { return false };
    if let Some(item) = ui.selected().and_then(|i| ui.items.borrow().get(i).cloned()) {
        move_thread(ui, &item.thread_id, &item.account_id, (!target.is_empty()).then(|| target.to_string()));
    }
    true
}

// MARK: Sorting and the download

/// Hold new-mail notifications while sorting runs, or while a sync is
/// bringing in mail that sorting has not seen yet.
pub(super) fn holds_notifications(ui: &Ui) -> bool {
    SORTING.with(Cell::get) || (ACTIVE.with(Cell::get) && ui.inbox_sync_running.load(Ordering::Acquire))
}

/// Sort waiting mail in the background, repeating while some is left; then
/// refresh and let notifications through. Returns true while a sort runs,
/// so the caller leaves notifications to it.
pub(super) fn wake(ui: &Rc<Ui>) -> bool {
    if SORTING.with(Cell::get) {
        SORT_AGAIN.with(|again| again.set(true));
        return true;
    }
    let status = load_status(ui);
    if status["enabled"] != true || status["model"]["installed"] != true || downloading(&status) {
        return false;
    }
    SORTING.with(|sorting| sorting.set(true));
    sort_round(ui);
    true
}

fn sort_round(ui: &Rc<Ui>) {
    let weak = Rc::downgrade(ui);
    background(ui, json!({"op":"filters_wake","budget_ms":WAKE_BUDGET_MS}), move |result| {
        let Some(ui) = weak.upgrade() else { return };
        match result {
            Ok(value) => {
                if value["decisions"].as_array().is_some_and(|d| !d.is_empty()) {
                    ui.refresh();
                }
                let busy = value["busy"] == true;
                let more = value["remaining"].as_u64().unwrap_or(0) > 0 && value["error"].is_null();
                if busy || more {
                    let weak = Rc::downgrade(&ui);
                    glib::timeout_add_local_once(Duration::from_millis(if busy { 1000 } else { 0 }), move || {
                        if let Some(ui) = weak.upgrade() {
                            sort_round(&ui);
                        }
                    });
                    return;
                }
                if let Some(error) = value["error"].as_str() {
                    ui.status.set_text(error);
                }
            }
            Err(error) => ui.status.set_text(&format!("Sorting stopped: {error}")),
        }
        SORTING.with(|sorting| sorting.set(false));
        refresh_sidebar(&ui);
        ui.notify_new_mail();
        if SORT_AGAIN.with(|again| again.replace(false)) {
            wake(&ui);
        }
    });
}

fn start_download(ui: &Rc<Ui>) {
    if DOWNLOADING.with(|d| d.replace(true)) {
        return;
    }
    let weak = Rc::downgrade(ui);
    background(ui, json!({"op":"filters_download_model"}), move |result| {
        DOWNLOADING.with(|d| d.set(false));
        let Some(ui) = weak.upgrade() else { return };
        if let Err(error) = &result {
            ui.status.set_text(&format!("Model download failed: {error}"));
        }
        refresh_sidebar(&ui);
        render_settings();
        if result.is_ok_and(|v| v["installed"] == true) {
            wake(&ui);
        }
    });
    poll_download(ui);
}

/// Update the sidebar row and settings every half second while downloading.
fn poll_download(ui: &Rc<Ui>) {
    if POLLING.with(|p| p.replace(true)) {
        return;
    }
    refresh_sidebar(ui);
    render_settings();
    let weak = Rc::downgrade(ui);
    glib::timeout_add_local(Duration::from_millis(500), move || {
        let Some(ui) = weak.upgrade() else {
            POLLING.with(|p| p.set(false));
            return glib::ControlFlow::Break;
        };
        refresh_sidebar(&ui);
        render_settings();
        if downloading(&cached()) {
            glib::ControlFlow::Continue
        } else {
            POLLING.with(|p| p.set(false));
            glib::ControlFlow::Break
        }
    });
}

/// On launch: resume showing a download another window started, and sort.
pub(super) fn start(ui: &Rc<Ui>) {
    let status = load_status(ui);
    if downloading(&status) {
        poll_download(ui);
    } else {
        wake(ui);
    }
}

// MARK: Setup sheet

/// First run: the starters with a checkbox and an action each. Closes at
/// once; the download goes on in the sidebar and sorting starts after it.
pub(super) fn setup(ui: &Rc<Ui>) {
    let status = load_status(ui);
    let installed = status["model"]["installed"] == true;
    let dialog = sheet(ui, "Sort mail automatically", 460);
    let root = gtk::Box::new(Orientation::Vertical, 0);
    let column = gtk::Box::new(Orientation::Vertical, 8);
    column.set_margin_top(22);
    column.set_margin_start(24);
    column.set_margin_end(24);
    column.append(&label("Sort mail automatically", "agent-sheet-title"));
    column.append(&label(
        "A small AI model on this computer sorts first emails from people you haven’t written to. Your mail never leaves this computer.",
        "filter-lead",
    ));
    let card = list_card();
    card.set_margin_top(10);
    let mut choices: Vec<(Value, gtk::CheckButton, gtk::DropDown)> = Vec::new();
    for starter in status["starters"].as_array().into_iter().flatten() {
        let row = gtk::Box::new(Orientation::Horizontal, 12);
        row.add_css_class("agent-list-row");
        let check = gtk::CheckButton::with_label(starter["name"].as_str().unwrap_or_default());
        check.add_css_class("filter-check");
        check.set_active(true);
        check.set_hexpand(true);
        let action = gtk::DropDown::from_strings(&ACTION_TITLES);
        action.add_css_class("agent-dropdown");
        action.set_valign(gtk::Align::Center);
        action.set_size_request(132, -1);
        action.set_selected(action_index(starter["action"].as_str().unwrap_or("inbox")) as u32);
        action.update_property(&[gtk::accessible::Property::Label(&format!(
            "When {} arrives",
            starter["name"].as_str().unwrap_or_default()
        ))]);
        check.bind_property("active", &action, "sensitive").sync_create().build();
        row.append(&check);
        row.append(&action);
        card_row(&card, &row);
        choices.push((starter.clone(), check, action));
    }
    column.append(&card);
    let size = gigabytes(status["model"]["download_bytes"].as_u64().unwrap_or(0));
    let model_name = status["model"]["model_name"].as_str().unwrap_or("Email sorter");
    let note = if installed {
        "You can change filters any time.".to_string()
    } else {
        format!("Downloads {model_name} ({size}) once. You can change filters any time.")
    };
    let footer = label(&note, "agent-note");
    footer.set_margin_top(4);
    column.append(&footer);
    root.append(&column);
    let actions = gtk::Box::new(Orientation::Horizontal, 8);
    actions.set_margin_top(18);
    actions.set_margin_bottom(18);
    actions.set_margin_start(24);
    actions.set_margin_end(24);
    let spacer = gtk::Box::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = gtk::Button::with_label("Not now");
    cancel.add_css_class("agent-button");
    let confirm = gtk::Button::with_label(if installed { "Turn on" } else { "Download and turn on" });
    confirm.add_css_class("agent-approve");
    actions.append(&spacer);
    actions.append(&cancel);
    actions.append(&confirm);
    root.append(&actions);
    dialog.set_child(Some(&root));
    let choices = Rc::new(choices);
    {
        // At least one filter.
        let (all, confirm) = (choices.clone(), confirm.clone());
        let update = move || confirm.set_sensitive(all.iter().any(|(_, check, _)| check.is_active()));
        for (_, check, _) in choices.iter() {
            let update = update.clone();
            check.connect_toggled(move |_| update());
        }
    }
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let (d, weak) = (dialog.clone(), Rc::downgrade(ui));
    confirm.connect_clicked(move |_| {
        let Some(ui) = weak.upgrade() else { return };
        let keys: Vec<String> = choices
            .iter()
            .filter(|(_, check, _)| check.is_active())
            .filter_map(|(starter, _, _)| starter["key"].as_str().map(str::to_string))
            .collect();
        let status = match ui.request(json!({"op":"filters_setup","starter_keys":keys})) {
            Ok(status) => status,
            Err(error) => {
                ui.status.set_text(&format!("Could not turn on filters: {error}"));
                return;
            }
        };
        for (starter, check, dropdown) in choices.iter() {
            let chosen = ACTIONS[(dropdown.selected() as usize).min(2)];
            if !check.is_active() || starter["action"] == chosen {
                continue;
            }
            if let Some(created) = filters_of(&status).into_iter().find(|f| f["name"] == starter["name"]) {
                let _ = ui.request(json!({"op":"upsert_filter","id":created["id"],"name":created["name"],
                    "description":created["description"],"action":chosen}));
            }
        }
        d.close();
        ui.refresh();
        if status["model"]["installed"] == true {
            wake(&ui);
        } else {
            start_download(&ui);
        }
    });
    sheet_keys(&dialog, Some(confirm.clone()));
    dialog.set_default_widget(Some(&confirm));
    dialog.present();
    confirm.grab_focus();
}

// MARK: Editor

/// New filter (None) or Edit: name, what goes here, when it arrives.
pub(super) fn editor(ui: &Rc<Ui>, existing: Option<Value>) {
    let editing = existing.is_some();
    let existing = existing.unwrap_or(Value::Null);
    let dialog = sheet(ui, if editing { "Edit filter" } else { "New filter" }, 460);
    let root = gtk::Box::new(Orientation::Vertical, 0);
    let column = gtk::Box::new(Orientation::Vertical, 6);
    column.set_margin_top(22);
    column.set_margin_start(24);
    column.set_margin_end(24);
    column.append(&label(if editing { "Edit filter" } else { "New filter" }, "agent-sheet-title"));
    let name_label = label("Name", "filter-field-label");
    name_label.set_margin_top(10);
    column.append(&name_label);
    let name = gtk::Entry::new();
    name.add_css_class("filter-entry");
    name.set_text(existing["name"].as_str().unwrap_or_default());
    name.set_placeholder_text(Some("e.g. Receipts"));
    column.append(&name);
    let what_label = label("What goes here", "filter-field-label");
    what_label.set_margin_top(10);
    column.append(&what_label);
    let description = gtk::TextView::new();
    description.set_wrap_mode(gtk::WrapMode::WordChar);
    description.set_accepts_tab(false);
    description.add_css_class("filter-text");
    description.buffer().set_text(existing["description"].as_str().unwrap_or_default());
    let placeholder = label(
        "Describe it like you would to a friend: who sends it and what it’s about.",
        "filter-placeholder",
    );
    placeholder.set_valign(gtk::Align::Start);
    placeholder.set_can_target(false);
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("filter-text-frame");
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    // Three lines.
    scroll.set_min_content_height(51);
    scroll.set_max_content_height(51);
    scroll.set_child(Some(&description));
    overlay.set_child(Some(&scroll));
    overlay.add_overlay(&placeholder);
    column.append(&overlay);
    {
        let placeholder = placeholder.clone();
        let update = move |buffer: &gtk::TextBuffer| placeholder.set_visible(buffer.char_count() == 0);
        update(&description.buffer());
        description.buffer().connect_changed(update);
    }
    let when_label = label("When it arrives", "filter-field-label");
    when_label.set_margin_top(10);
    column.append(&when_label);
    let action = gtk::DropDown::from_strings(&ACTION_TITLES);
    action.add_css_class("agent-dropdown");
    action.set_halign(gtk::Align::Start);
    action.set_selected(action_index(existing["action"].as_str().unwrap_or("skip_inbox")) as u32);
    column.append(&action);
    let note = label("", "agent-note");
    column.append(&note);
    {
        let note = note.clone();
        let update = move |dropdown: &gtk::DropDown| note.set_text(action_note(ACTIONS[(dropdown.selected() as usize).min(2)]));
        update(&action);
        action.connect_selected_notify(update);
    }
    let error = label("", "agent-error");
    error.set_visible(false);
    error.set_margin_top(4);
    column.append(&error);
    root.append(&column);
    let actions = gtk::Box::new(Orientation::Horizontal, 8);
    actions.set_margin_top(18);
    actions.set_margin_bottom(18);
    actions.set_margin_start(24);
    actions.set_margin_end(24);
    if editing {
        let delete = gtk::Button::with_label("Delete filter");
        delete.add_css_class("agent-ghost");
        let (weak, d, filter) = (Rc::downgrade(ui), dialog.clone(), existing.clone());
        delete.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                confirm_delete(&ui, Some(&d), &filter);
            }
        });
        actions.append(&delete);
    }
    let spacer = gtk::Box::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    actions.append(&spacer);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("agent-button");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("agent-approve");
    actions.append(&cancel);
    actions.append(&save);
    root.append(&actions);
    dialog.set_child(Some(&root));
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let (d, weak, name_entry) = (dialog.clone(), Rc::downgrade(ui), name.clone());
    let id = existing["id"].clone();
    save.connect_clicked(move |_| {
        let Some(ui) = weak.upgrade() else { return };
        let buffer = description.buffer();
        let (start, end) = buffer.bounds();
        let text = buffer.text(&start, &end, false);
        let missing = if name.text().trim().is_empty() {
            name.grab_focus();
            Some("Give the filter a name.")
        } else if text.trim().is_empty() {
            description.grab_focus();
            Some("Say what goes here, so the model knows what to look for.")
        } else {
            None
        };
        if let Some(missing) = missing {
            error.set_text(missing);
            error.set_visible(true);
            return;
        }
        let request = json!({"op":"upsert_filter","id":id,"name":name.text().trim(),
            "description":text.trim(),
            "action":ACTIONS[(action.selected() as usize).min(2)]});
        match ui.request(request) {
            Ok(_) => {
                d.close();
                ui.refresh();
                render_settings();
            }
            Err(message) => {
                // The service's own sentence, without its error-kind prefix.
                error.set_text(message.strip_prefix("invalid action: ").unwrap_or(&message));
                error.set_visible(true);
            }
        }
    });
    sheet_keys(&dialog, Some(save.clone()));
    dialog.set_default_widget(Some(&save));
    dialog.present();
    name_entry.grab_focus_without_selecting();
    name_entry.set_position(-1);
}

/// Confirm, delete, and leave the filter's view if it was open.
fn confirm_delete(ui: &Rc<Ui>, editor: Option<&gtk::Window>, filter: &Value) {
    let name = filter["name"].as_str().unwrap_or_default().to_string();
    let id = filter["id"].as_str().unwrap_or_default().to_string();
    let editor = editor.cloned();
    let parent = editor.clone();
    confirm(
        ui,
        parent.as_ref(),
        &format!("Delete “{name}”?"),
        "Its conversations stay where they are. Nothing is deleted from your mail.",
        "Delete filter",
        move |ui| match ui.request(json!({"op":"delete_filter","id":id})) {
            Ok(_) => {
                if let Some(editor) = &editor {
                    editor.close();
                }
                if matches!(&*ui.view.borrow(), MailView::Filter(open) if *open == id) {
                    ui.set_view(MailView::Inbox);
                } else {
                    ui.refresh();
                }
                render_settings();
            }
            Err(error) => ui.status.set_text(&format!("Could not delete the filter: {error}")),
        },
    );
}

fn confirm(
    ui: &Rc<Ui>,
    parent: Option<&gtk::Window>,
    title: &str,
    body: &str,
    action: &str,
    confirmed: impl Fn(&Rc<Ui>) + 'static,
) {
    let dialog = gtk::Window::builder()
        .transient_for(parent.unwrap_or(ui.window.upcast_ref()))
        .modal(true)
        .title(title)
        .default_width(380)
        .resizable(false)
        .build();
    dialog.add_css_class("agent-window");
    dialog.add_css_class("agent-sheet");
    let root = gtk::Box::new(Orientation::Vertical, 6);
    root.set_margin_top(22);
    root.set_margin_bottom(18);
    root.set_margin_start(24);
    root.set_margin_end(24);
    root.append(&label(title, "agent-sheet-title"));
    root.append(&label(body, "agent-note"));
    let actions = gtk::Box::new(Orientation::Horizontal, 8);
    actions.set_margin_top(16);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("agent-button");
    let ok = gtk::Button::with_label(action);
    ok.add_css_class("agent-approve");
    actions.append(&cancel);
    actions.append(&ok);
    root.append(&actions);
    dialog.set_child(Some(&root));
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let (d, weak) = (dialog.clone(), Rc::downgrade(ui));
    ok.connect_clicked(move |_| {
        d.close();
        if let Some(ui) = weak.upgrade() {
            confirmed(&ui);
        }
    });
    // Enter activates the focused button; a destructive choice is never the default.
    sheet_keys(&dialog, None);
    dialog.present();
    cancel.grab_focus();
}

// MARK: Settings

struct Settings {
    ui: std::rc::Weak<Ui>,
    window: gtk::Window,
    content: gtk::Box,
    signature: RefCell<String>,
    progress: RefCell<Option<gtk::Label>>,
}

/// Filter settings: on/off, the model, the filters.
pub(super) fn settings(ui: &Rc<Ui>) {
    if let Some(open) = SETTINGS.with(|slot| slot.borrow().clone()) {
        open.window.present();
        return;
    }
    let window = sheet(ui, "Filter settings", 480);
    let root = gtk::Box::new(Orientation::Vertical, 0);
    let content = gtk::Box::new(Orientation::Vertical, 8);
    content.set_margin_top(22);
    content.set_margin_bottom(6);
    content.set_margin_start(24);
    content.set_margin_end(24);
    root.append(&content);
    let footer = gtk::Box::new(Orientation::Horizontal, 8);
    footer.set_margin_top(14);
    footer.set_margin_bottom(18);
    footer.set_margin_start(24);
    footer.set_margin_end(24);
    let spacer = gtk::Box::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    let done = gtk::Button::with_label("Done");
    done.add_css_class("agent-approve");
    let w = window.clone();
    done.connect_clicked(move |_| w.close());
    footer.append(&done);
    root.append(&footer);
    window.set_child(Some(&root));
    window.connect_close_request(|_| {
        SETTINGS.with(|slot| slot.borrow_mut().take());
        glib::Propagation::Proceed
    });
    sheet_keys(&window, Some(done.clone()));
    window.set_default_widget(Some(&done));
    let settings = Rc::new(Settings {
        ui: Rc::downgrade(ui),
        window: window.clone(),
        content,
        signature: RefCell::new(String::new()),
        progress: RefCell::new(None),
    });
    SETTINGS.with(|slot| *slot.borrow_mut() = Some(settings.clone()));
    load_status(ui);
    settings.render();
    window.present();
    done.grab_focus();
}

fn render_settings() {
    if let Some(settings) = SETTINGS.with(|slot| slot.borrow().clone()) {
        if let Some(ui) = settings.ui.upgrade() {
            load_status(&ui);
        }
        settings.render();
    }
}

impl Settings {
    fn render(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else { return };
        let status = cached();
        let model = &status["model"];
        let enabled = status["enabled"] == true;
        let installed = model["installed"] == true;
        let is_downloading = downloading(&status);
        let filters = filters_of(&status);
        let signature = format!(
            "{enabled}|{installed}|{is_downloading}|{}|{}",
            model["error"],
            filters
                .iter()
                .map(|f| format!("{}:{}:{}", f["id"], f["name"], f["action"]))
                .collect::<Vec<_>>()
                .join(",")
        );
        if *self.signature.borrow() == signature {
            if let Some(progress) = self.progress.borrow().as_ref() {
                progress.set_text(&settings_progress(&status));
            }
            return;
        }
        *self.signature.borrow_mut() = signature;
        *self.progress.borrow_mut() = None;
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
        self.content.append(&label("Filters", "agent-sheet-title"));
        let card = list_card();
        card.set_margin_top(10);
        // On/off.
        let row = gtk::Box::new(Orientation::Horizontal, 12);
        row.add_css_class("agent-list-row");
        let title = label("Sort mail automatically", "agent-row-title");
        title.set_hexpand(true);
        row.append(&title);
        let switch = gtk::Switch::new();
        switch.set_valign(gtk::Align::Center);
        switch.set_active(enabled);
        switch.update_property(&[gtk::accessible::Property::Label("Sort mail automatically")]);
        let (weak, settings, no_filters) = (Rc::downgrade(&ui), Rc::downgrade(self), filters.is_empty());
        switch.connect_state_set(move |_, on| {
            let Some(ui) = weak.upgrade() else { return glib::Propagation::Stop };
            if on && no_filters {
                // Nothing to sort into yet: choose filters first.
                if let Some(settings) = settings.upgrade() {
                    settings.window.close();
                }
                setup(&ui);
                return glib::Propagation::Stop;
            }
            match ui.request(json!({"op":"filters_set_enabled","enabled":on})) {
                Ok(status) => {
                    ui.refresh();
                    if on && status["model"]["installed"] != true {
                        start_download(&ui);
                    } else if on {
                        wake(&ui);
                    }
                }
                Err(error) => ui.status.set_text(&format!("Could not change filters: {error}")),
            }
            let weak = Rc::downgrade(&ui);
            glib::idle_add_local_once(move || {
                if weak.upgrade().is_some() {
                    render_settings();
                }
            });
            glib::Propagation::Proceed
        });
        row.append(&switch);
        card_row(&card, &row);
        // The model.
        let row = gtk::Box::new(Orientation::Horizontal, 12);
        row.add_css_class("agent-list-row");
        let names = gtk::Box::new(Orientation::Vertical, 1);
        names.set_hexpand(true);
        names.set_valign(gtk::Align::Center);
        names.append(&label(
            &format!(
                "{} · {} · runs on this computer",
                model["model_name"].as_str().unwrap_or("Email sorter"),
                gigabytes(model["download_bytes"].as_u64().unwrap_or(0))
            ),
            "agent-row-title",
        ));
        if is_downloading {
            let progress = label(&settings_progress(&status), "agent-row-subtitle");
            names.append(&progress);
            *self.progress.borrow_mut() = Some(progress);
        } else if let Some(error) = model["error"].as_str().filter(|_| !installed) {
            // The first clause ("Couldn't reach Hugging Face"); the
            // transport details go in the tooltip.
            let short = error.split(": ").next().unwrap_or(error);
            let line = label(&format!("Download failed: {short}"), "agent-error");
            line.set_tooltip_text(Some(error));
            names.append(&line);
        } else if !installed {
            names.append(&label("Not downloaded", "agent-row-subtitle"));
        }
        row.append(&names);
        let weak = Rc::downgrade(&ui);
        let trailing = if is_downloading {
            let cancel = gtk::Button::with_label("Cancel download");
            cancel.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    let _ = ui.request(json!({"op":"filters_cancel_download"}));
                }
            });
            cancel
        } else if installed {
            let remove = gtk::Button::with_label("Remove");
            remove.connect_clicked(move |_| {
                let Some(ui) = weak.upgrade() else { return };
                let parent = SETTINGS.with(|slot| slot.borrow().as_ref().map(|s| s.window.clone()));
                let size = gigabytes(cached()["model"]["download_bytes"].as_u64().unwrap_or(0));
                confirm(
                    &ui,
                    parent.as_ref(),
                    "Remove the sorting model?",
                    &format!("This frees {size}. Filters keep what they sorted; new mail stays in your inbox until you download the model again."),
                    "Remove",
                    |ui| {
                        if let Err(error) = ui.request(json!({"op":"filters_remove_model"})) {
                            ui.status.set_text(&format!("Could not remove the model: {error}"));
                        }
                        refresh_sidebar(ui);
                        render_settings();
                    },
                );
            });
            remove
        } else {
            let download = gtk::Button::with_label(if model["error"].is_string() { "Retry" } else { "Download" });
            download.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    start_download(&ui);
                }
            });
            download
        };
        trailing.add_css_class("agent-link");
        trailing.set_valign(gtk::Align::Center);
        row.append(&trailing);
        card_row(&card, &row);
        self.content.append(&card);
        // The filters.
        let heading = label("Your filters", "agent-group-label");
        heading.set_margin_top(14);
        self.content.append(&heading);
        let card = list_card();
        for filter in &filters {
            let row = gtk::Button::new();
            row.add_css_class("filter-settings-row");
            let line = gtk::Box::new(Orientation::Horizontal, 8);
            let action = filter["action"].as_str().unwrap_or("inbox");
            let icon = mail_icon(action_icon(action));
            icon.add_css_class("filter-settings-icon");
            line.append(&icon);
            let name = label(filter["name"].as_str().unwrap_or_default(), "agent-row-title");
            name.set_wrap(false);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            name.set_hexpand(true);
            line.append(&name);
            line.append(&label(action_title(action), "agent-row-subtitle"));
            let chevron = mail_icon("go-next-symbolic");
            chevron.add_css_class("filter-settings-icon");
            chevron.set_size_request(10, 10);
            line.append(&chevron);
            row.update_property(&[gtk::accessible::Property::Label(&format!(
                "Edit {}, {}",
                filter["name"].as_str().unwrap_or_default(),
                action_title(action)
            ))]);
            row.set_child(Some(&line));
            row.set_tooltip_text(Some("Edit filter"));
            let (weak, filter) = (Rc::downgrade(&ui), filter.clone());
            row.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    editor(&ui, Some(filter.clone()));
                }
            });
            card_row(&card, &row);
        }
        let new_filter = gtk::Button::with_label("New filter");
        new_filter.add_css_class("agent-link");
        new_filter.set_halign(gtk::Align::Start);
        new_filter.set_margin_top(6);
        let weak = Rc::downgrade(&ui);
        new_filter.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                editor(&ui, None);
            }
        });
        if card.first_child().is_some() {
            self.content.append(&card);
        }
        if filters.len() < MAX_FILTERS {
            self.content.append(&new_filter);
        }
    }
}

pub(super) const CSS: &str = r#"
.filter-lead { font-size: 13px; color: @secondary; }
.filter-check label { font-size: 13px; margin-left: 6px; }
.filter-check check { background: transparent; background-image: none; border: 1px solid @muted; border-radius: 3px; color: @canvas; min-width: 14px; min-height: 14px; box-shadow: none; }
.filter-check check:checked { background: @ink; border-color: @ink; color: @canvas; }
.filter-side-note { font-family: Inter, sans-serif; }
.filter-settings-icon { color: @muted; }
.side-item.filter-status, .side-item.filter-status image { color: @muted; }
.filter-field-label { font-size: 12px; font-weight: 600; color: @muted; }
.filter-entry { background: @input; border: 1px solid @line; border-radius: 6px; padding: 5px 8px; min-height: 22px; font-size: 13px; }
.filter-text-frame { background: @input; border: 1px solid @line; border-radius: 6px; padding: 5px 8px; }
.filter-text, .filter-text text { background: @input; color: @ink; font-size: 13px; }
.filter-placeholder { color: @muted; font-size: 13px; padding: 0 2px; }
.filter-settings-row { background: none; border-radius: 0; padding: 11px 0; min-height: 0; }
.filter-settings-row:hover { background: none; }
.filter-settings-row:hover .agent-row-title { text-decoration: underline; }
.filter-bar { background: @surface; border-bottom: 1px solid @line; padding: 8px 28px; min-height: 30px; }
.filter-bar-text { font-size: 13px; }
.filter-move > button { background: @selected; padding: 4px 10px 4px 12px; font-size: 12.5px; min-height: 26px; }
.filter-move > button:hover { background: @hover; }
.filter-menu button { background: transparent; padding: 5px 10px; font-size: 12px; min-height: 24px; }
.filter-menu button:hover { background: @hover; }
.filter-empty { color: @muted; font-size: 13px; margin: 28px 16px; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_matches_the_macos_filters() {
        assert_eq!(subtitle("skip_inbox", 12), "Skips your inbox · 12 this week");
        assert_eq!(subtitle("ignore", 4), "Ignored · 4 this week");
        assert_eq!(subtitle("inbox", 30), "Tagged in your inbox · 30 this week");
        assert_eq!(action_note("inbox"), "Stays in your inbox with a tag.");
        assert_eq!(action_note("skip_inbox"), "Archived. Read it here when you want.");
        assert_eq!(action_note("ignore"), "Archived and marked read.");
        assert_eq!(gigabytes(2_508_703_966), "2.5 GB");
        assert_eq!(gigabytes(156_603_310), "157 MB");
        assert_eq!(action_title("ignore"), "Ignore");
    }

    #[test]
    fn download_progress_is_a_bounded_percentage() {
        assert_eq!(percent(&json!({"download_bytes":200,"downloaded_bytes":84})), 42);
        assert_eq!(percent(&json!({"download_bytes":0,"downloaded_bytes":0})), 0);
        assert_eq!(percent(&json!({"download_bytes":10,"downloaded_bytes":20})), 100);
    }

    #[test]
    fn keep_in_inbox_filters_show_as_the_first_tag() {
        let mail = |action| Mail {
            thread_id: "t".into(),
            account_id: "a".into(),
            account: String::new(),
            sender: String::new(),
            subject: String::new(),
            preview: "Your receipt".into(),
            received: String::new(),
            unread: true,
            labels: vec!["INBOX".into(), "Work".into()],
            filter: Some(ThreadFilterTag {
                id: "f".into(),
                name: "Notifications".into(),
                action,
                reason: "a receipt".into(),
                source: "model".into(),
            }),
            is_inbox: true,
        };
        assert_eq!(row_preview(&MailView::Inbox, &mail(FilterAction::Inbox)), "#Notifications #Work · Your receipt");
        assert_eq!(row_preview(&MailView::Inbox, &mail(FilterAction::SkipInbox)), "#Work · Your receipt");
        assert_eq!(row_preview(&MailView::Filter("f".into()), &mail(FilterAction::Inbox)), "a receipt");
    }
}
