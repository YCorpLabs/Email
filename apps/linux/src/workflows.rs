use super::*;

pub(super) fn request_async(
    path: PathBuf,
    request: Value,
    done: impl FnOnce(Result<Value, String>) + 'static,
) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(
            MailService::open(path).and_then(|service| service.request_json(&request.to_string())),
        );
    });
    let mut done = Some(done);
    glib::timeout_add_local(std::time::Duration::from_millis(35), move || {
        match rx.try_recv() {
            Ok(result) => {
                if let Some(done) = done.take() {
                    done(result);
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(_) => {
                if let Some(done) = done.take() {
                    done(Err("Mail worker stopped. Please retry.".into()));
                }
                glib::ControlFlow::Break
            }
        }
    });
}
fn label(text: &str, strong: bool) -> gtk::Label {
    let item = gtk::Label::new(Some(text));
    item.set_xalign(0.);
    item.set_wrap(true);
    item.set_selectable(true);
    if strong {
        item.add_css_class("heading");
    } else {
        item.add_css_class("dim-label");
    }
    item
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn local_date(ms: i64) -> String {
    glib::DateTime::from_unix_local(ms / 1000)
        .ok()
        .and_then(|date| date.format("%e %b %Y at %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Time unavailable".into())
}
fn load_rows(
    ui: &Rc<Ui>,
    rows: &gtk::Box,
    notice: &gtk::Label,
    calendar: bool,
    thread_id: Option<String>,
) {
    while let Some(child) = rows.first_child() {
        rows.remove(&child);
    }
    let rows = rows.clone();
    let notice = notice.clone();
    let weak = Rc::downgrade(ui);
    request_async(
        ui.database_path.clone(),
        if calendar {
            json!({"op":"list_calendar_invitations","limit":100})
        } else {
            json!({"op":"list_followups"})
        },
        move |result| {
            let Some(ui) = weak.upgrade() else { return };
            match result {
                Err(error) => notice.set_text(&error),
                Ok(value) => {
                    let items = value.as_array().cloned().unwrap_or_default();
                    let mut found = false;
                    for item in items {
                        if calendar {
                            found = true;
                            let event = &item["event"];
                            let start = &event["start"];
                            rows.append(&label(
                                event["summary"].as_str().unwrap_or("Untitled meeting"),
                                true,
                            ));
                            let zone = if start["all_day"].as_bool() == Some(true) {
                                "All day"
                            } else {
                                start["timezone"]
                                    .as_str()
                                    .unwrap_or("Floating time · timezone not specified")
                            };
                            rows.append(&label(
                                &format!(
                                    "{} · {}",
                                    start["display"].as_str().unwrap_or("Time unavailable"),
                                    zone
                                ),
                                false,
                            ));
                            let method = event["method"].as_str().unwrap_or("");
                            let state = event["status"].as_str().unwrap_or("");
                            let location = event["location"].as_str().unwrap_or("");
                            let detail = [
                                if method == "CANCEL" {
                                    "Cancelled invitation"
                                } else {
                                    method
                                },
                                state,
                                location,
                            ]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(" · ");
                            if !detail.is_empty() {
                                rows.append(&label(&detail, false));
                            }
                            for warning in event["warnings"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                            {
                                rows.append(&label(warning, false));
                            }
                        } else if item["thread_id"].as_str() == thread_id.as_deref() {
                            found = true;
                            let state = item["status"].as_str().unwrap_or("waiting");
                            rows.append(&label(
                                &format!(
                                    "{} · {}",
                                    state,
                                    local_date(item["due_at_ms"].as_i64().unwrap_or(0))
                                ),
                                true,
                            ));
                            if state == "waiting" || state == "due" {
                                let cancel = gtk::Button::with_label("Cancel reminder");
                                rows.append(&cancel);
                                let id = thread_id.clone();
                                let ui = ui.clone();
                                let notice = notice.clone();
                                cancel.connect_clicked(move |button| {
                                    button.set_sensitive(false);
                                    let notice = notice.clone();
                                    let ui = ui.clone();
                                    request_async(
                                        ui.database_path.clone(),
                                        json!({"op":"cancel_followup","thread_id":id}),
                                        move |result| match result {
                                            Ok(_) => {
                                                notice.set_text("Reminder cancelled");
                                                ui.refresh();
                                            }
                                            Err(error) => notice.set_text(&error),
                                        },
                                    );
                                });
                            }
                        }
                    }
                    if !found {
                        rows.append(&label(if calendar {"No invitations saved yet. Select a meeting email and check its attachments."} else {"No reminder set for this conversation."},false));
                    }
                }
            }
        },
    );
}
pub(super) fn open(ui: &Rc<Ui>, calendar: bool) {
    let selected = ui
        .selected()
        .and_then(|i| ui.items.borrow().get(i).cloned());
    let thread_id = selected.as_ref().map(|item| item.thread_id.clone());
    if !calendar
        && (thread_id.is_none() || matches!(*ui.view.borrow(), MailView::Drafts | MailView::Outbox))
    {
        ui.status
            .set_text("Select a conversation to set a follow-up.");
        return;
    }
    let message_id = ui
        .current_message
        .borrow()
        .as_ref()
        .filter(|m| Some(&m.thread_id) == thread_id.as_ref())
        .map(|m| m.id.clone());
    let window = gtk::Window::builder()
        .transient_for(&ui.window)
        .modal(true)
        .title(if calendar {
            "Calendar invitations"
        } else {
            "Follow up"
        })
        .default_width(520)
        .resizable(true)
        .build();
    let content = gtk::Box::new(Orientation::Vertical, 12);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(20);
    content.set_margin_end(20);
    content.append(&label(
        if calendar {
            "Calendar invitations"
        } else {
            "Follow up"
        },
        true,
    ));
    content.append(&label(if calendar {"Meeting details saved from your emails. This is an invitation list, not your live calendar or availability."}else{"Keep this conversation in view until it needs your attention. A new incoming reply can cancel the reminder."},false));
    let notice = label("", false);
    let rows = gtk::Box::new(Orientation::Vertical, 12);
    let duration = gtk::Entry::new();
    duration.set_text("3d");
    duration.set_placeholder_text(Some("5m, 2h, 3d, 1w"));
    duration.set_tooltip_text(Some("Follow-up duration · Return to save"));
    if calendar {
        let agenda = gtk::Button::with_label("Open connected calendar…");
        {
            let ui = ui.clone();
            let invitations = window.clone();
            agenda.connect_clicked(move |_| {
                invitations.close();
                super::agenda::open(&ui);
            });
        }
        content.append(&agenda);
        let scan = gtk::Button::with_label("Check selected email for invitations");
        scan.set_sensitive(message_id.is_some());
        content.append(&scan);
        let ui = ui.clone();
        let rows = rows.clone();
        let notice = notice.clone();
        let tid = thread_id.clone();
        scan.connect_clicked(move |button| {
            button.set_sensitive(false);
            notice.set_text("Checking calendar attachments…");
            let button = button.clone();
            let ui = ui.clone();
            let rows = rows.clone();
            let notice = notice.clone();
            let tid = tid.clone();
            request_async(
                ui.database_path.clone(),
                json!({"op":"refresh_calendar_invitation","message_id":message_id}),
                move |result| {
                    button.set_sensitive(true);
                    match result {
                        Ok(value) => {
                            notice.set_text(if value.as_array().is_some_and(|v| v.is_empty()) {
                                "This email contains no supported calendar invitation."
                            } else {
                                "Invitation details saved · no RSVP sent"
                            });
                            load_rows(&ui, &rows, &notice, true, tid);
                        }
                        Err(error) => {
                            notice.set_text(&format!("Could not check invitations: {error}"))
                        }
                    }
                },
            );
        });
    } else {
        let line = gtk::Box::new(Orientation::Horizontal, 10);
        line.append(&label("Remind me in", false));
        line.append(&duration);
        content.append(&line);
        let conditional = gtk::CheckButton::with_label("Only if nobody replies");
        conditional.set_active(true);
        content.append(&conditional);
        let preview = label("", false);
        content.append(&preview);
        let save = gtk::Button::with_label("Set follow-up");
        save.add_css_class("suggested-action");
        content.append(&save);
        let update = {
            let save = save.clone();
            let preview = preview.clone();
            move |entry: &gtk::Entry| {
                let parsed = parity::duration_query(entry.text().as_str());
                save.set_sensitive(parsed.is_some());
                preview.set_text(
                    &parsed
                        .map(|(seconds, title)| {
                            format!(
                                "{title} · {}",
                                local_date(now_ms() + (seconds as i64) * 1000)
                            )
                        })
                        .unwrap_or_else(|| "Use 5m, 2h, 3d or 1w (up to one year).".into()),
                );
            }
        };
        update(&duration);
        duration.connect_changed(update);
        {
            let save = save.clone();
            duration.connect_activate(move |_| {
                save.emit_clicked();
            });
        }
        let ui = ui.clone();
        let duration = duration.clone();
        let notice = notice.clone();
        let rows = rows.clone();
        let tid = thread_id.clone();
        save.connect_clicked(move |button|{let Some((seconds,_))=parity::duration_query(duration.text().as_str())else{return};button.set_sensitive(false);let button=button.clone();let ui=ui.clone();let notice=notice.clone();let rows=rows.clone();let tid=tid.clone();request_async(ui.database_path.clone(),json!({"op":"set_followup","thread_id":tid,"due_at_ms":now_ms()+(seconds as i64)*1000,"only_if_no_reply":conditional.is_active()}),move |result|{button.set_sensitive(true);match result {Ok(_)=>{notice.set_text("Saved · find this conversation in Follow-ups (G then W)");ui.refresh();load_rows(&ui,&rows,&notice,false,tid);},Err(error)=>notice.set_text(&error)}});});
    }
    content.append(&notice);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_min_content_height(if calendar { 180 } else { 90 });
    scroll.set_max_content_height(if calendar { 180 } else { 90 });
    scroll.set_propagate_natural_height(false);
    scroll.set_child(Some(&rows));
    content.append(&scroll);
    content.append(&label(
        if calendar {
            "Recurring meetings are not expanded. Times retain the invitation’s timezone."
        } else {
            "Local to this device · checked while Email runs or when reopened."
        },
        false,
    ));
    let done = gtk::Button::with_label("Done");
    done.set_halign(gtk::Align::End);
    {
        let window = window.clone();
        done.connect_clicked(move |_| window.close());
    }
    content.append(&done);
    window.set_child(Some(&content));
    let keys = gtk::EventControllerKey::new();
    {
        let window = window.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                window.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    window.add_controller(keys);
    window.present();
    if !calendar {
        duration.grab_focus();
    }
    load_rows(ui, &rows, &notice, calendar, thread_id);
}
