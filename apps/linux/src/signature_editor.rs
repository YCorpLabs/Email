//! Per-identity local signatures. Provider From validation remains independent.
use super::*;

#[derive(Clone)]
pub(super) struct SignatureHandle {
    managed: Rc<RefCell<Option<String>>>,
}
impl SignatureHandle {
    pub(super) fn has_user_content(&self, body: &gtk::TextView) -> bool {
        let text = body_text(body);
        !text.is_empty() && self.managed.borrow().as_deref() != Some(text.as_str())
    }
}

fn body_text(body: &gtk::TextView) -> String {
    let buffer = body.buffer();
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string()
}

fn identity(combo: &gtk::ComboBoxText, accounts: &[Account]) -> Option<(String, String)> {
    let id = combo.active_id()?;
    if let Some((account, email)) = id.split_once('\t') {
        return Some((account.into(), email.into()));
    }
    accounts
        .iter()
        .find(|account| account.id == id)
        .map(|account| (account.id.clone(), account.email.clone()))
}

fn request_async(
    ui: &Rc<Ui>,
    request: Value,
    callback: impl FnOnce(Result<Value, String>) + 'static,
) {
    let path = ui.database_path.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = MailService::open(&path)
            .map_err(|e| e.to_string())
            .and_then(|service| service.request_json(&request.to_string()));
        let _ = tx.send(result);
    });
    let mut callback = Some(callback);
    glib::timeout_add_local(std::time::Duration::from_millis(30), move || {
        match rx.try_recv() {
            Ok(result) => {
                if let Some(callback) = callback.take() {
                    callback(result);
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                if let Some(callback) = callback.take() {
                    callback(Err("Signature service stopped".into()));
                }
                glib::ControlFlow::Break
            }
        }
    });
}

fn load_default(
    ui: &Rc<Ui>,
    combo: &gtk::ComboBoxText,
    body: &gtk::TextView,
    accounts: &[Account],
    handle: &SignatureHandle,
    generation: &Rc<Cell<u64>>,
    reply: bool,
    allow_auto: bool,
) {
    generation.set(generation.get().wrapping_add(1));
    let current = generation.get();
    let Some(selected) = identity(combo, accounts) else {
        return;
    };
    if !allow_auto || ui.active_draft.borrow().is_some() {
        return;
    }
    let original = body_text(body);
    if !original.is_empty() && handle.managed.borrow().as_deref() != Some(original.as_str()) {
        return;
    }
    let request = json!({"op":"get_signature","account_id":selected.0,"email":selected.1});
    let combo = combo.downgrade();
    let body = body.downgrade();
    let accounts = accounts.to_vec();
    let handle = handle.clone();
    let generation = generation.clone();
    let weak_ui = Rc::downgrade(ui);
    request_async(ui, request, move |result| {
        let (Some(ui), Some(combo), Some(body)) =
            (weak_ui.upgrade(), combo.upgrade(), body.upgrade())
        else {
            return;
        };
        if body.root().is_none()
            || generation.get() != current
            || identity(&combo, &accounts) != Some(selected)
            || body_text(&body) != original
            || ui.active_draft.borrow().is_some()
        {
            return;
        }
        let Ok(value) = result else {
            return;
        };
        let enabled = value[if reply { "for_reply" } else { "for_new" }]
            .as_bool()
            .unwrap_or(false);
        let rendered = if enabled {
            value["body"].as_str().map(|s| format!("\n\n-- \n{s}"))
        } else {
            None
        };
        body.buffer().set_text(rendered.as_deref().unwrap_or(""));
        let start = body.buffer().start_iter();
        body.buffer().place_cursor(&start);
        *handle.managed.borrow_mut() = rendered;
    });
}

pub(super) fn install(
    ui: &Rc<Ui>,
    combo: &gtk::ComboBoxText,
    body: &gtk::TextView,
    button: &gtk::Button,
    accounts: &[Account],
    reply: bool,
    allow_auto: bool,
) -> SignatureHandle {
    let handle = SignatureHandle {
        managed: Rc::new(RefCell::new(None)),
    };
    let generation = Rc::new(Cell::new(0u64));
    load_default(
        ui,
        combo,
        body,
        accounts,
        &handle,
        &generation,
        reply,
        allow_auto,
    );
    {
        let weak_ui = Rc::downgrade(ui);
        let body = body.clone();
        let accounts = accounts.to_vec();
        let handle = handle.clone();
        let generation = generation.clone();
        combo.connect_changed(move |combo| {
            if let Some(ui) = weak_ui.upgrade() {
                load_default(
                    &ui,
                    combo,
                    &body,
                    &accounts,
                    &handle,
                    &generation,
                    reply,
                    allow_auto,
                );
            }
        });
    }
    {
        let weak_ui = Rc::downgrade(ui);
        let combo = combo.clone();
        let body = body.clone();
        let accounts = accounts.to_vec();
        let handle = handle.clone();
        let generation = generation.clone();
        button.connect_clicked(move |button| {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            let Some(selected) = identity(&combo, &accounts) else {
                return;
            };
            button.set_sensitive(false);
            let button = button.downgrade();
            let combo = combo.clone();
            let body = body.clone();
            let accounts = accounts.clone();
            let handle = handle.clone();
            let generation = generation.clone();
            let next_ui = ui.clone();
            request_async(
                &ui,
                json!({"op":"get_signature","account_id":selected.0,"email":selected.1}),
                move |result| {
                    if let Some(button) = button.upgrade() {
                        button.set_sensitive(true);
                    }
                    if body.root().is_none()
                        || identity(&combo, &accounts) != Some(selected.clone())
                    {
                        return;
                    }
                    match result {
                        Ok(value) => open_editor(
                            &next_ui,
                            &combo,
                            &body,
                            &accounts,
                            &handle,
                            &generation,
                            selected,
                            value,
                            reply,
                            allow_auto,
                        ),
                        Err(error) => next_ui
                            .status
                            .set_text(&format!("Could not open signature: {error}")),
                    }
                },
            );
        });
    }
    handle
}

fn open_editor(
    ui: &Rc<Ui>,
    combo: &gtk::ComboBoxText,
    body: &gtk::TextView,
    accounts: &[Account],
    handle: &SignatureHandle,
    generation: &Rc<Cell<u64>>,
    selected: (String, String),
    value: Value,
    reply: bool,
    allow_auto: bool,
) {
    let window = gtk::Window::builder()
        .title("Signature")
        .transient_for(&ui.window)
        .modal(true)
        .default_width(440)
        .default_height(360)
        .build();
    let content = gtk::Box::new(Orientation::Vertical, 12);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(20);
    content.set_margin_end(20);
    let title = gtk::Label::new(Some("Signature"));
    title.add_css_class("composer-title");
    title.set_xalign(0.0);
    content.append(&title);
    let address = gtk::Label::new(Some(&selected.1));
    address.set_xalign(0.0);
    address.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    address.add_css_class("muted");
    content.append(&address);
    let editor = gtk::TextView::new();
    editor.set_wrap_mode(gtk::WrapMode::WordChar);
    editor.set_accepts_tab(false);
    editor.set_top_margin(10);
    editor.set_bottom_margin(10);
    editor.set_left_margin(10);
    editor.set_right_margin(10);
    editor
        .buffer()
        .set_text(value["body"].as_str().unwrap_or(""));
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_min_content_height(150);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&editor));
    content.append(&scroll);
    let options = gtk::Box::new(Orientation::Horizontal, 18);
    let new_messages = gtk::CheckButton::with_label("New messages");
    let replies = gtk::CheckButton::with_label("Replies");
    new_messages.set_active(value["for_new"].as_bool().unwrap_or(true));
    replies.set_active(value["for_reply"].as_bool().unwrap_or(true));
    options.append(&new_messages);
    options.append(&replies);
    content.append(&options);
    let hint = gtk::Label::new(Some("Tab next · Ctrl+Return save · Esc close"));
    hint.set_xalign(0.0);
    hint.add_css_class("muted");
    content.append(&hint);
    let feedback = gtk::Label::new(None);
    feedback.set_xalign(0.0);
    feedback.set_wrap(true);
    content.append(&feedback);
    let buttons = gtk::Box::new(Orientation::Horizontal, 8);
    let remove = gtk::Button::with_label("Remove");
    remove.set_sensitive(!value.is_null());
    let cancel = gtk::Button::with_label("Cancel");
    let insert = gtk::Button::with_label("Insert");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    let spacer = gtk::Box::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    buttons.append(&remove);
    buttons.append(&spacer);
    buttons.append(&cancel);
    buttons.append(&insert);
    buttons.append(&save);
    content.append(&buttons);
    window.set_child(Some(&content));
    {
        let save = save.clone();
        let insert = insert.clone();
        let feedback = feedback.clone();
        editor.buffer().connect_changed(move |buffer| {
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            let valid = !text.trim().is_empty() && text.len() <= 16 * 1024;
            save.set_sensitive(valid);
            insert.set_sensitive(valid);
            feedback.set_text(if text.len() > 16 * 1024 {
                "Keep signatures under 16 KiB."
            } else {
                ""
            });
        });
    }
    let valid = !body_text(&editor).trim().is_empty();
    save.set_sensitive(valid);
    insert.set_sensitive(valid);
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let window = window.clone();
        let body = body.clone();
        let editor = editor.clone();
        let handle = handle.clone();
        insert.connect_clicked(move |_| {
            let text = body_text(&editor);
            if text.trim().is_empty() || text.len() > 16 * 1024 {
                return;
            }
            let buffer = body.buffer();
            buffer.begin_user_action();
            buffer.delete_selection(true, true);
            buffer.insert_at_cursor(&format!("\n\n-- \n{text}"));
            buffer.end_user_action();
            *handle.managed.borrow_mut() = None;
            window.close();
            body.grab_focus();
        });
    }
    for (button, deleting) in [(save.clone(), false), (remove.clone(), true)] {
        let ui = ui.clone();
        let window = window.clone();
        let body = body.clone();
        let combo = combo.clone();
        let accounts = accounts.to_vec();
        let handle = handle.clone();
        let generation = generation.clone();
        let selected = selected.clone();
        let editor = editor.clone();
        let new_messages = new_messages.clone();
        let replies = replies.clone();
        let feedback = feedback.clone();
        let save = save.clone();
        let remove = remove.clone();
        let insert = insert.clone();
        button.connect_clicked(move |_| {
            let request = if deleting { json!({"op":"delete_signature","account_id":selected.0,"email":selected.1}) } else { json!({"op":"upsert_signature","account_id":selected.0,"email":selected.1,"body":body_text(&editor),"for_new":new_messages.is_active(),"for_reply":replies.is_active()}) };
            save.set_sensitive(false); remove.set_sensitive(false); insert.set_sensitive(false); editor.set_editable(false);
            let ui_next = ui.clone(); let window = window.clone(); let body = body.clone(); let combo = combo.clone(); let accounts = accounts.clone(); let handle = handle.clone(); let generation = generation.clone(); let feedback = feedback.clone(); let save = save.clone(); let remove = remove.clone(); let insert = insert.clone(); let editor = editor.clone();
            request_async(&ui, request, move |result| { match result {
                Ok(_) => { load_default(&ui_next, &combo, &body, &accounts, &handle, &generation, reply, allow_auto); window.close(); body.grab_focus(); },
                Err(error) => { editor.set_editable(true); save.set_sensitive(true); remove.set_sensitive(true); insert.set_sensitive(true); feedback.set_text(&error); }
            }});
        });
    }
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let window = window.clone();
        let save = save.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::Escape {
                window.close();
                return glib::Propagation::Stop;
            }
            if mods.contains(gdk::ModifierType::CONTROL_MASK)
                && (key == gdk::Key::Return || key == gdk::Key::KP_Enter)
            {
                if save.is_sensitive() {
                    save.emit_clicked();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    window.add_controller(keys);
    {
        let body = body.clone();
        window.connect_close_request(move |_| {
            body.grab_focus();
            glib::Propagation::Proceed
        });
    }
    window.present();
    editor.grab_focus();
}
