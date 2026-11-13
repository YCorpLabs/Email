//! Preview-first, durable Gmail sender cleanup, separate from local bulk triage.
use super::*;

struct Panel {
    ui: Rc<Ui>,
    window: gtk::Window,
    rows: gtk::Box,
    status: gtk::Label,
    jobs: RefCell<Vec<Value>>,
    expanded_samples: RefCell<std::collections::HashSet<String>>,
    busy: Cell<bool>,
    pending_controls: RefCell<std::collections::VecDeque<Value>>,
    confirming: Cell<bool>,
    retry_not_before: Cell<u64>,
}

thread_local! { static OPEN_PANEL: RefCell<std::rc::Weak<Panel>> = RefCell::new(std::rc::Weak::new()); }

pub(super) fn open(ui: &Rc<Ui>, sender: Option<(String, String)>) {
    if let Some(panel) = OPEN_PANEL.with(|slot| slot.borrow().upgrade()) {
        panel.window.present();
        if let Some((account_id, sender)) = sender {
            panel.request(Some(
                json!({"op":"create_sender_cleanup","account_id":account_id,"sender":sender}),
            ));
        }
        return;
    }
    let window = gtk::Window::builder()
        .title("Sender cleanup")
        .transient_for(&ui.window)
        .default_width(520)
        .default_height(300)
        .decorated(false)
        .build();
    window.add_css_class("cleanup-window");
    let content = gtk::Box::new(Orientation::Vertical, 12);
    for widget in [content.upcast_ref::<gtk::Widget>()] {
        widget.set_margin_top(24);
        widget.set_margin_bottom(24);
        widget.set_margin_start(24);
        widget.set_margin_end(24);
    }
    let header = gtk::Box::new(Orientation::Horizontal, 12);
    let titles = gtk::Box::new(Orientation::Vertical, 5);
    titles.set_hexpand(true);
    titles.append(&cleanup_label("Clean up", "cleanup-heading"));
    titles.append(&cleanup_label(
        "Mark unread messages from a sender as read",
        "cleanup-secondary",
    ));
    header.append(&titles);
    let dismiss = gtk::Button::with_label("✕");
    dismiss.add_css_class("flat");
    dismiss.set_tooltip_text(Some("Close (Esc)"));
    let w = window.clone();
    dismiss.connect_clicked(move |_| w.close());
    header.append(&dismiss);
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&header));
    content.append(&handle);
    content.append(&gtk::Separator::new(Orientation::Horizontal));
    let rows = gtk::Box::new(Orientation::Vertical, 12);
    rows.set_valign(gtk::Align::Start);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&rows));
    content.append(&scroll);
    let status = gtk::Label::new(None);
    status.set_wrap(true);
    status.set_xalign(0.0);
    content.append(&status);
    status.add_css_class("cleanup-secondary");
    let footer = gtk::Box::new(Orientation::Horizontal, 12);
    let hint = cleanup_label(
        "Changes start only after your confirmation",
        "cleanup-footnote",
    );
    hint.set_hexpand(true);
    footer.append(&hint);
    let close = gtk::Button::with_label("Done");
    close.add_css_class("cleanup-done");
    let w = window.clone();
    close.connect_clicked(move |_| w.close());
    footer.append(&close);
    content.append(&footer);
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
    window.set_child(Some(&content));
    let panel = Rc::new(Panel {
        ui: ui.clone(),
        window,
        rows,
        status,
        jobs: RefCell::new(Vec::new()),
        expanded_samples: RefCell::new(std::collections::HashSet::new()),
        busy: Cell::new(false),
        pending_controls: RefCell::new(std::collections::VecDeque::new()),
        confirming: Cell::new(false),
        retry_not_before: Cell::new(0),
    });
    OPEN_PANEL.with(|slot| *slot.borrow_mut() = Rc::downgrade(&panel));
    let weak = Rc::downgrade(&panel);
    panel.window.connect_hide(move |_| {
        if let Some(panel) = weak.upgrade() {
            while let Some(child) = panel.rows.first_child() {
                panel.rows.remove(&child);
            }
        }
    });
    panel.window.present();
    let initial = sender.map(|(account_id,sender)| json!({"op":"create_sender_cleanup","account_id":account_id,"sender":sender}));
    panel.request(initial);
    glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
        if !panel.window.is_visible() {
            return glib::ControlFlow::Break;
        }
        if !panel.busy.get() && !panel.confirming.get() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if now < panel.retry_not_before.get() {
                return glib::ControlFlow::Continue;
            }
            let job = panel
                .jobs
                .borrow()
                .iter()
                .find(|job| {
                    matches!(job["state"].as_str(), Some("discovering" | "running"))
                        && job["retry_at_ms"].as_u64().unwrap_or(0) <= now
                })
                .cloned();
            if let Some(job) = job {
                panel.request(Some(json!({"op":"step_sender_cleanup","job_id":job["id"]})));
            }
        }
        glib::ControlFlow::Continue
    });
}

impl Panel {
    fn request(self: &Rc<Self>, op: Option<Value>) {
        if self.busy.get() {
            if let Some(op) = op.filter(|op| op["op"] != "step_sender_cleanup") {
                let mut queued = self.pending_controls.borrow_mut();
                queued.retain(|control| {
                    control["job_id"] != op["job_id"] || control["op"] == "create_sender_cleanup"
                });
                queued.push_back(op);
                self.status.set_text(
                    "Request queued. The current batch may finish; your control runs next.",
                );
            }
            return;
        }
        self.busy.set(true);
        self.status
            .set_text("Working… · Pause and Cancel remain available");
        let path = self.ui.database_path.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| -> Result<Value, String> {
                let service = MailService::open(&path).map_err(|e| e.to_string())?;
                if let Some(op) = op {
                    service
                        .request_json(&op.to_string())
                        .map_err(|e| e.to_string())?;
                }
                service
                    .request_json(&json!({"op":"list_sender_cleanups"}).to_string())
                    .map_err(|e| e.to_string())
            })();
            let _ = tx.send(result);
        });
        let panel = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
            match rx.try_recv() {
                Ok(result) => {
                    panel.busy.set(false);

                    match result {
                        Ok(value) => {
                            *panel.jobs.borrow_mut() =
                                value.as_array().cloned().unwrap_or_default();
                            panel.render();
                            panel.ui.refresh();
                            panel.status.set_text("");
                        }
                        Err(error) => {
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64;
                            panel.retry_not_before.set(now + 5000);
                            panel.status.set_text(&error);
                        }
                    }
                    panel.run_next_control();
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(_) => {
                    panel.busy.set(false);
                    panel
                        .status
                        .set_text("Cleanup worker stopped. Reopen to retry.");
                    panel.run_next_control();
                    glib::ControlFlow::Break
                }
            }
        });
    }
    fn run_next_control(self: &Rc<Self>) {
        let next = self.pending_controls.borrow_mut().pop_front();
        if let Some(op) = next {
            self.request(Some(op));
        }
    }
    fn render(self: &Rc<Self>) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        self.window.set_default_widget(None::<&gtk::Widget>);
        if self.jobs.borrow().is_empty() {
            self.rows
                .append(&cleanup_label("No cleanup history yet", "cleanup-sender"));
            self.rows.append(&cleanup_label(
                "Choose a sender to preview all their unread messages before marking them read.",
                "cleanup-secondary",
            ));
            if let Some(item) = self
                .ui
                .selected()
                .and_then(|i| self.ui.items.borrow().get(i).cloned())
            {
                let start = gtk::Button::with_label("Clean up selected sender…");
                start.set_tooltip_text(Some(&format!(
                    "Preview unread mail from {} (Enter)",
                    item.sender
                )));
                start.add_css_class("suggested-action");
                start.set_halign(gtk::Align::Start);
                let panel = self.clone();
                start.connect_clicked(move |_| panel.ui.bulk_read_sender());
                self.rows.append(&start);
                self.window.set_default_widget(Some(&start));
            } else {
                self.rows.append(&cleanup_label(
                    "Select a message, then open Commands and choose Bulk mark sender as read.",
                    "cleanup-footnote",
                ));
            }
        }
        let accounts = self.ui.accounts();
        let mut ready_default_set = false;
        for job in self.jobs.borrow().iter() {
            let id = job["id"].as_str().unwrap_or_default().to_string();
            let state = job["state"].as_str().unwrap_or_default();
            let sender = job["sender"].as_str().unwrap_or_default();
            let account_id = job["account_id"].as_str().unwrap_or_default();
            let account = accounts
                .iter()
                .find(|a| a.id == account_id)
                .map(|a| a.email.as_str())
                .unwrap_or(account_id);
            let matched = job["matched"].as_u64().unwrap_or(0);
            let completed = job["completed"].as_u64().unwrap_or(0);
            let skipped = job["skipped"].as_u64().unwrap_or(0);
            let row = gtk::Box::new(Orientation::Vertical, 5);
            row.add_css_class("cleanup-card");
            let identity = gtk::Box::new(Orientation::Horizontal, 12);
            identity.append(&mail_icon("mail-unread-symbolic"));
            let names = gtk::Box::new(Orientation::Vertical, 4);
            names.set_hexpand(true);
            let title = cleanup_label(sender, "cleanup-sender");
            title.set_wrap(false);
            title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            names.append(&title);
            names.append(&cleanup_label(account, "cleanup-footnote"));
            identity.append(&names);
            let state_text = match state {
                "discovering" => "Finding matches…",
                "ready" if matched == 0 => "No unread messages",
                "ready" => "Ready",
                "running" => "Running",
                "paused" => "Paused",
                "completed" => "Completed",
                "cancelled" => "Cancelled",
                _ => state,
            };
            identity.append(&cleanup_label(state_text, "cleanup-footnote"));
            row.append(&identity);
            let summary = if state == "cancelled" && completed == 0 {
                "No messages changed".to_string()
            } else if state == "ready" && matched == 0 {
                "No unread messages found".to_string()
            } else if state == "ready" {
                "Ready to review".to_string()
            } else {
                format!("{completed} marked read · {skipped} skipped")
            };
            let count = if state == "discovering" {
                "Finding unread messages…".to_string()
            } else if state == "ready" && matched == 0 {
                summary
            } else {
                format!("{matched} matches  ·  {summary}")
            };
            row.append(&cleanup_label(&count, "cleanup-secondary"));
            if state == "running" && matched > 0 {
                let progress = gtk::ProgressBar::new();
                progress.set_fraction(((completed + skipped) as f64 / matched as f64).min(1.0));
                row.append(&progress);
            }
            if let Some(samples) = job["sample_subjects"].as_array().filter(|v| !v.is_empty()) {
                let expanded = self.expanded_samples.borrow().contains(&id);
                let toggle = gtk::Button::with_label(if expanded {
                    "⌄  Sample messages"
                } else {
                    "›  Sample messages"
                });
                toggle.add_css_class("flat");
                toggle.add_css_class("cleanup-disclosure");
                toggle.set_halign(gtk::Align::Start);
                let panel = self.clone();
                let sample_id = id.clone();
                toggle.connect_clicked(move |_| {
                    {
                        let mut expanded = panel.expanded_samples.borrow_mut();
                        if !expanded.insert(sample_id.clone()) {
                            expanded.remove(&sample_id);
                        }
                    }
                    panel.render();
                });
                row.append(&toggle);
                if expanded {
                    row.append(&cleanup_label("Downloaded examples", "cleanup-footnote"));
                    for subject in samples.iter().take(3).filter_map(Value::as_str) {
                        row.append(&cleanup_label(subject, "cleanup-secondary"));
                    }
                }
            }
            if let Some(error) = job["last_error"].as_str().filter(|s| !s.is_empty()) {
                let label = gtk::Label::new(Some(error));
                label.set_wrap(true);
                row.append(&label);
            }
            let buttons = gtk::Box::new(Orientation::Horizontal, 8);
            if state == "ready" && matched > 0 {
                let button = gtk::Button::with_label(&format!("Review and mark {matched} read…"));
                let panel = self.clone();
                let job_id = id.clone();
                let sender = sender.to_string();
                let account = account.to_string();
                button.connect_clicked(move |_| {
                    panel.confirming.set(true);
                    let dialog = gtk::Window::builder().title("Confirm sender cleanup").transient_for(&panel.window).modal(true).default_width(460).build();
                    let weak = Rc::downgrade(&panel);
                    dialog.connect_hide(move |_| { if let Some(panel) = weak.upgrade() { panel.confirming.set(false); } });
                    let content = gtk::Box::new(Orientation::Vertical, 16); content.set_margin_top(24); content.set_margin_bottom(24); content.set_margin_start(24); content.set_margin_end(24);
                    let label = gtk::Label::new(Some(&format!("Mark {matched} Gmail messages as read?\n\nAccount: {account}\nExact sender: {sender}\nOnly messages found during preview. New arrivals are excluded. Completed changes have no automatic Undo."))); label.set_wrap(true); label.set_xalign(0.0); content.append(&label);
                    let confirm = gtk::Button::with_label("Mark read"); let p = panel.clone(); let id = job_id.clone(); let d = dialog.clone(); confirm.connect_clicked(move |_| { p.request(Some(json!({"op":"confirm_sender_cleanup","job_id":id}))); d.close(); }); content.append(&confirm);
                    let cancel = gtk::Button::with_label("Cancel"); let d = dialog.clone(); cancel.connect_clicked(move |_| d.close()); content.append(&cancel); dialog.set_child(Some(&content)); dialog.present();
                });
                buttons.append(&button);
                if !ready_default_set {
                    button.set_tooltip_text(Some("Review the exact sender and count (Enter)"));
                    self.window.set_default_widget(Some(&button));
                    ready_default_set = true;
                }
            }
            let mut actions = Vec::new();
            if state == "discovering" || state == "running" {
                actions.push(("Pause", "pause_sender_cleanup"));
            }
            if state == "paused" {
                actions.push(("Resume", "resume_sender_cleanup"));
            }
            if state != "completed" && state != "cancelled" {
                actions.push(("Cancel job", "cancel_sender_cleanup"));
            }
            for (title, op) in actions {
                let button = gtk::Button::with_label(title);
                let panel = self.clone();
                let id = id.clone();
                button.connect_clicked(move |_| panel.request(Some(json!({"op":op,"job_id":id}))));
                buttons.append(&button);
            }
            if buttons.first_child().is_some() {
                row.append(&buttons);
            }
            if self.rows.first_child().is_some() {
                self.rows
                    .append(&gtk::Separator::new(Orientation::Horizontal));
            }
            self.rows.append(&row);
        }
    }
}

fn cleanup_label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.add_css_class(class);
    label
}
