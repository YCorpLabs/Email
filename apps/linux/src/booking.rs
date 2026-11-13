//! Native scheduling surface backed by the same Rust service as macOS.
use super::*;
struct Panel {
    ui: Rc<Ui>,
    window: gtk::Window,
    rows: gtk::Box,
    notice: gtk::Label,
    refresh: gtk::Button,
    footer: gtk::Box,
    body: Option<gtk::TextView>,
    pending: Cell<bool>,
    generation: Cell<u64>,
    connected: Cell<bool>,
}
/// The distribution profile's Scheduling service. Unset hides Bookings,
/// booking links and open tracking, matching the macOS shell.
pub(super) fn configured() -> bool {
    mail_service::profile::SCHEDULING_ORIGIN.is_some()
}
fn scheduling_url(path: &str, fragment: Option<&str>) -> Option<Url> {
    let mut url = Url::parse(mail_service::profile::SCHEDULING_ORIGIN?).ok()?;
    url.set_path(path);
    url.set_fragment(fragment);
    Some(url)
}
/// Only HTTPS pages on the configured Scheduling host are trusted.
fn is_scheduling_url(url: &Url) -> bool {
    let Some(origin) = mail_service::profile::SCHEDULING_ORIGIN.and_then(|o| Url::parse(o).ok()) else {
        return false;
    };
    url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().map(str::to_ascii_lowercase) == origin.host_str().map(str::to_owned)
}
pub(super) fn open(ui: &Rc<Ui>, body: Option<gtk::TextView>) {
    if !configured() {
        return;
    }
    let window = gtk::Window::builder()
        .title(if body.is_some() {
            "Insert booking link"
        } else {
            "Scheduling"
        })
        .transient_for(&ui.window)
        .default_width(480)
        .default_height(330)
        .build();
    let root = gtk::Box::new(Orientation::Vertical, 18);
    for side in [
        gtk::PositionType::Top,
        gtk::PositionType::Bottom,
        gtk::PositionType::Left,
        gtk::PositionType::Right,
    ] {
        match side {
            gtk::PositionType::Top => root.set_margin_top(24),
            gtk::PositionType::Bottom => root.set_margin_bottom(24),
            gtk::PositionType::Left => root.set_margin_start(24),
            _ => root.set_margin_end(24),
        }
    }
    let header = gtk::Box::new(Orientation::Horizontal, 12);
    let title = label(
        if body.is_some() {
            "Booking links"
        } else {
            "Scheduling"
        },
        true,
    );
    title.add_css_class("title-2");
    title.set_hexpand(true);
    header.append(&title);
    let refresh = gtk::Button::with_label("Refresh");
    refresh.set_visible(false);
    header.append(&refresh);
    root.append(&header);
    let notice = label("Loading saved scheduling…", false);
    root.append(&notice);
    let rows = gtk::Box::new(Orientation::Vertical, 14);
    let scroll = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&rows)
        .build();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    root.append(&scroll);
    let footer = gtk::Box::new(Orientation::Horizontal, 12);
    let manage = gtk::Button::with_label("Manage online ↗");
    if let Some(studio) = scheduling_url("/studio", None) {
        manage.connect_clicked(move |_| open_url(studio.as_str()));
        footer.append(&manage);
    }
    let disconnect = gtk::Button::with_label("Disconnect this device");
    footer.append(&disconnect);
    footer.set_visible(false);
    root.append(&footer);
    window.set_child(Some(&root));
    let panel = Rc::new(Panel {
        ui: ui.clone(),
        window: window.clone(),
        rows,
        notice,
        refresh: refresh.clone(),
        footer: footer.clone(),
        body,
        pending: Cell::new(false),
        generation: Cell::new(0),
        connected: Cell::new(false),
    });
    {
        let p = panel.clone();
        refresh.connect_clicked(move |_| p.load(true));
    }
    {
        let p = panel.clone();
        disconnect.connect_clicked(move |_| {
            p.pending.set(false);
            let pp = p.clone();
            p.request(json!({"op":"booking_disconnect"}), move |_| pp.load(false));
        });
    }
    {
        let p = panel.clone();
        window.connect_close_request(move |_| {
            p.pending.set(false);
            p.generation.set(p.generation.get() + 1);
            glib::Propagation::Proceed
        });
    }
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
    panel.load(false);
    window.present();
}
fn label(value: &str, strong: bool) -> gtk::Label {
    let l = gtk::Label::new(Some(value));
    l.set_xalign(0.);
    l.set_wrap(true);
    l.set_selectable(false);
    if strong {
        l.add_css_class("heading")
    } else {
        l.add_css_class("dim-label")
    }
    l
}
fn open_url(value: &str) {
    if let Some(display) = gdk::Display::default() {
        let _ =
            gtk::gio::AppInfo::launch_default_for_uri(value, Some(&display.app_launch_context()));
    }
}
impl Panel {
    fn connected_layout(&self, connected: bool) {
        self.refresh.set_visible(connected);
        self.footer.set_visible(connected);
        self.rows.set_valign(if connected {
            gtk::Align::Start
        } else {
            gtk::Align::Center
        });
        if self.connected.replace(connected) != connected {
            self.window.set_default_size(
                if connected { 570 } else { 480 },
                if connected { 450 } else { 330 },
            );
        }
    }
    fn clear(&self) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
    }
    fn request(self: &Rc<Self>, payload: Value, done: impl FnOnce(Value) + 'static) {
        let weak = Rc::downgrade(self);
        let serial = self.generation.get();
        let polling = payload["op"].as_str() == Some("booking_connect_poll");
        let connecting = payload["op"].as_str() == Some("booking_connect_start");
        workflows::request_async(self.ui.database_path.clone(), payload, move |result| {
            let Some(p) = weak.upgrade() else { return };
            if p.generation.get() != serial {
                return;
            }
            match result {
                Ok(data) => done(data),
                Err(e) => {
                    if polling {
                        p.pending.set(false);
                    }
                    p.notice.set_text(&e);
                    if connecting {
                        let retry = gtk::Button::with_label("Try connecting again");
                        let panel = p.clone();
                        retry.connect_clicked(move |_| panel.connect());
                        p.rows.append(&retry);
                    }
                }
            }
        });
    }
    fn load(self: &Rc<Self>, refresh: bool) {
        self.pending.set(false);
        self.generation.set(self.generation.get() + 1);
        self.notice.set_text(if refresh {
            "Refreshing…"
        } else {
            "Loading saved scheduling…"
        });
        let p = self.clone();
        self.request(json!({"op":"booking_status"}), move |status| {
            if status["connected"].as_bool() != Some(true) {
                p.connected_layout(false);
                p.clear();
                p.notice.set_text("Your meeting links, right where you write.");
                p.rows.append(&label(
                    "Connect to copy meeting links, suggest times and see upcoming bookings. Bookings keep working when Email is closed.",
                    false,
                ));
                let connect = gtk::Button::with_label("Connect scheduling");
                let pp = p.clone();
                connect.connect_clicked(move |_| pp.connect());
                p.rows.append(&connect);
            } else {
                let pp = p.clone();
                p.request(
                    json!({"op":"booking_list","refresh":refresh}),
                    move |data| {
                        pp.render(data);
                        if !refresh {
                            pp.load(true);
                        }
                    },
                );
            }
        });
    }
    fn render(self: &Rc<Self>, data: Value) {
        self.connected_layout(true);
        self.clear();
        self.notice.set_text(&format!(
            "{} · {}",
            data["host"]["email"]
                .as_str()
                .unwrap_or("Scheduling connected"),
            if data["cached"].as_bool() == Some(true) {
                "Saved on this device"
            } else {
                "Up to date"
            }
        ));
        self.rows.append(&label("Meeting links", true));
        let mut count = 0;
        for item in data["meeting_types"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|x| x["published"].as_bool() == Some(true))
        {
            count += 1;
            let row = gtk::Box::new(Orientation::Horizontal, 12);
            let name = label(item["title"].as_str().unwrap_or("Meeting"), true);
            name.set_hexpand(true);
            row.append(&name);
            let b = gtk::Button::with_label(if self.body.is_some() {
                "Insert link"
            } else {
                "Copy link"
            });
            let id = item["id"].as_str().unwrap_or("").to_owned();
            let p = self.clone();
            b.connect_clicked(move |_| {
                let pp = p.clone();
                p.request(
                    json!({"op":"booking_link","meeting_type_id":id}),
                    move |result| {
                        let Some(url) = result["url"].as_str() else {
                            return;
                        };
                        if let Some(body) = &pp.body {
                            body.buffer().insert_at_cursor(url);
                            body.grab_focus();
                            pp.window.close();
                        } else {
                            pp.window.clipboard().set_text(url);
                            pp.notice.set_text("Booking link copied");
                        }
                    },
                );
            });
            row.append(&b);
            if self.body.is_some() {
                let suggest = gtk::Button::with_label("Suggest times");
                let id = item["id"].as_str().unwrap_or("").to_owned();
                let p = self.clone();
                suggest.connect_clicked(move |_| p.suggest(&id));
                row.append(&suggest);
            }
            self.rows.append(&row);
        }
        if count == 0 {
            self.rows.append(&label(
                "No active meeting links. Create one in Manage online.",
                false,
            ));
        }
        if self.body.is_some() {
            return;
        }
        self.rows.append(&label("Upcoming bookings", true));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let mut bookings: Vec<_> = data["bookings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| {
                b["status"].as_str() == Some("confirmed") && b["end"].as_i64().unwrap_or(0) > now
            })
            .collect();
        bookings.sort_by_key(|b| b["start"].as_i64().unwrap_or(0));
        if bookings.is_empty() {
            self.rows.append(&label("No upcoming bookings", false));
        }
        for b in bookings.iter().take(30) {
            self.rows
                .append(&label(b["title"].as_str().unwrap_or("Meeting"), true));
            let when = glib::DateTime::from_unix_local(b["start"].as_i64().unwrap_or(0) / 1000)
                .ok()
                .and_then(|d| d.format("%e %b %Y at %H:%M").ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            self.rows.append(&label(
                &format!("{} · {}", when, b["name"].as_str().unwrap_or("Guest")),
                false,
            ));
            let manage = gtk::Button::with_label("Manage booking ↗");
            let id = b["id"].as_str().unwrap_or("").to_owned();
            if let Some(url) = scheduling_url("/studio", Some(&format!("booking={id}"))) {
                manage.connect_clicked(move |_| open_url(url.as_str()));
                self.rows.append(&manage);
            }
            if let Some(email) = b["email"].as_str() {
                let b = gtk::Button::with_label("Email guest");
                let ui = self.ui.clone();
                let email = email.to_owned();
                let guest_email = email.clone();
                let host_email = data["host"]["email"].as_str().unwrap_or("").to_owned();
                let window = self.window.clone();
                b.connect_clicked(move |_| {
                    ui.compose(false);
                    if let Some(account) = ui
                        .accounts()
                        .iter()
                        .find(|account| account.email.eq_ignore_ascii_case(&host_email))
                    {
                        if let Some(from) =
                            ui.compose_fields.borrow().get("from").and_then(|widget| {
                                widget.clone().downcast::<gtk::ComboBoxText>().ok()
                            })
                        {
                            from.set_active_id(Some(&account.id));
                        }
                    }
                    if let Some(to) = ui.compose_to.borrow().as_ref() {
                        to.set_text(&email);
                        to.grab_focus();
                    }
                    window.close();
                });
                self.rows.append(&b);
                let view = gtk::Button::with_label("View conversation");
                let ui = self.ui.clone();
                let window = self.window.clone();
                view.connect_clicked(move |_| {
                    ui.search.set_text(&format!(
                        "from:{}",
                        parity::quote_search_value(&guest_email)
                    ));
                    ui.set_view(MailView::Inbox);
                    window.close();
                });
                self.rows.append(&view);
            }
        }
        if data["analytics"]
            .as_object()
            .is_some_and(|analytics| !analytics.is_empty())
        {
            self.rows.append(&label("Analytics · last 30 days", true));
            let totals = &data["analytics"]["totals"];
            self.rows.append(&label(
                &format!(
                    "{} page visits · {} bookings · {} cancellations",
                    totals["pageViews"].as_u64().unwrap_or(0),
                    totals["confirmed"].as_u64().unwrap_or(0),
                    totals["cancelled"].as_u64().unwrap_or(0)
                ),
                false,
            ));
            if let Some(url) = scheduling_url("/studio", Some("analytics")) {
                let b = gtk::Button::with_label("Open analytics ↗");
                b.connect_clicked(move |_| open_url(url.as_str()));
                self.rows.append(&b);
            }
        }
    }
    fn suggest(self: &Rc<Self>, id: &str) {
        self.generation.set(self.generation.get() + 1);
        self.notice.set_text("Loading live available times…");
        let p = self.clone();
        self.request(
            json!({"op":"booking_availability","meeting_type_id":id}),
            move |data| {
                p.clear();
                let zone = data["time_zone"].as_str().unwrap_or("UTC").to_owned();
                p.notice.set_text(
                    "Choose up to five times. The recipient confirms availability when booking.",
                );
                p.rows
                    .append(&label(data["title"].as_str().unwrap_or("Meeting"), true));
                p.rows
                    .append(&label(&format!("Times shown in {zone}"), false));
                let choices = Rc::new(RefCell::new(Vec::<(gtk::CheckButton, String)>::new()));
                for slot in data["slots"].as_array().into_iter().flatten().take(12) {
                    let Some(start) = slot["start"].as_i64() else {
                        continue;
                    };
                    let Some(url) = slot["url"].as_str() else {
                        continue;
                    };
                    let timezone = glib::TimeZone::from_identifier(Some(&zone))
                        .unwrap_or_else(glib::TimeZone::utc);
                    let when = glib::DateTime::from_unix_utc(start / 1000)
                        .ok()
                        .and_then(|date| date.to_timezone(&timezone).ok())
                        .and_then(|date| date.format("%a %e %b at %H:%M").ok())
                        .map(|value| value.to_string())
                        .unwrap_or_default();
                    let text = format!("{when} ({zone}) — {url}");
                    let check = gtk::CheckButton::with_label(&when);
                    choices.borrow_mut().push((check.clone(), text));
                    let weak = Rc::downgrade(&choices);
                    let notice = p.notice.clone();
                    check.connect_toggled(move |check| {
                        if let Some(choices) = weak.upgrade() {
                            if check.is_active()
                                && choices
                                    .borrow()
                                    .iter()
                                    .filter(|(button, _)| button.is_active())
                                    .count()
                                    > 5
                            {
                                check.set_active(false);
                                notice.set_text("Choose up to five times.");
                            }
                        }
                    });
                    p.rows.append(&check);
                }
                if choices.borrow().is_empty() {
                    p.rows.append(&label(
                        "No available times. Change availability in Manage online.",
                        false,
                    ));
                }
                let insert = gtk::Button::with_label("Insert selected times");
                let panel = p.clone();
                insert.connect_clicked(move |_| {
                    let lines = choices
                        .borrow()
                        .iter()
                        .filter(|(button, _)| button.is_active())
                        .map(|(_, text)| text.clone())
                        .collect::<Vec<_>>();
                    if lines.is_empty() {
                        panel.notice.set_text("Select at least one available time.");
                        return;
                    }
                    if let Some(body) = &panel.body {
                        body.buffer()
                            .insert_at_cursor(&format!("Available times:\n{}\n", lines.join("\n")));
                        body.grab_focus();
                        panel.window.close();
                    }
                });
                p.rows.append(&insert);
                let back = gtk::Button::with_label("Back to meeting links");
                let panel = p.clone();
                back.connect_clicked(move |_| panel.load(false));
                p.rows.append(&back);
            },
        );
    }
    fn connect(self: &Rc<Self>) {
        self.connected_layout(false);
        self.clear();
        self.notice.set_text("Opening your browser…");
        self.rows
            .append(&label("Preparing a secure connection", true));
        self.pending.set(false);
        self.generation.set(self.generation.get() + 1);
        let p = self.clone();
        self.request(
            json!({"op":"booking_connect_start","name":"Email on Linux"}),
            move |data| {
                let (Some(code), Some(raw)) = (
                    data["user_code"].as_str(),
                    data["verification_url"].as_str(),
                ) else {
                    return;
                };
                let Ok(url) = Url::parse(raw) else { return };
                if !is_scheduling_url(&url) {
                    return;
                }
                p.clear();
                p.rows.append(&label("Approve the connection in your browser",true));
                // The hosted service sets the grant scope, which still includes
                // optional email activity; say so in every build.
                p.rows.append(&label("Review scheduling and optional email-open tracking access. Return here after approving.",false));
                let b = gtk::Button::with_label("Open browser again");
                let raw = raw.to_owned();
                let raw2 = raw.clone();
                b.connect_clicked(move |_| open_url(&raw2));
                p.rows.append(&b);
                let fallback=gtk::Expander::builder().label("Need a manual code?").build();
                let code=label(code,true);code.set_xalign(0.5);fallback.set_child(Some(&code));p.rows.append(&fallback);
                let restart=gtk::Button::with_label("Start over");let panel=p.clone();restart.connect_clicked(move |_|panel.connect());p.rows.append(&restart);
                p.notice.set_text("Waiting for browser approval…");
                open_url(&raw);
                p.pending.set(true);
                let weak = Rc::downgrade(&p);
                glib::timeout_add_local(std::time::Duration::from_secs(5), move || {
                    let Some(p) = weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    if !p.pending.get() {
                        return glib::ControlFlow::Break;
                    }
                    let pp = p.clone();
                    p.request(json!({"op":"booking_connect_poll"}), move |data| {
                        if data["connected"].as_bool() == Some(true) {
                            pp.pending.set(false);
                            pp.load(true);
                        }
                    });
                    glib::ControlFlow::Continue
                });
            },
        );
    }
}
/// Email activity uses the shared cached service projection. Hosted image
/// requests are evidence of image loads, never verified people or reads.
pub(super) struct ActivityPanel {
    ui: std::rc::Weak<Ui>,
    rows: gtk::Box,
    scroll: gtk::ScrolledWindow,
    headline: gtk::Label,
    recipient: gtk::Label,
    detail: gtk::Label,
    filters: gtk::Box,
    filter_buttons: Vec<gtk::ToggleButton>,
    filter: Cell<u32>,
    heading: gtk::Label,
    back: gtk::Button,
    refresh: gtk::Button,
    spinner: gtk::Spinner,
    status: gtk::Label,
    freshness: gtk::Label,
    refreshed_at: Cell<Option<i64>>,
    message_id: RefCell<Option<String>>,
    feed: RefCell<Vec<Value>>,
    item: RefCell<Option<Value>>,
    generation: Cell<u64>,
    pending: Cell<bool>,
    status_after_load: RefCell<Option<String>>,
    closed: Cell<bool>,
    allows_back: bool,
}

pub(super) fn activity(ui: &Rc<Ui>, message_id: Option<String>) {
    if !configured() {
        return;
    }
    if let Some(panel) = ui.activity_panel.borrow_mut().take() { panel.closed.set(true); }
    while let Some(child) = ui.activity_host.first_child() { ui.activity_host.remove(&child); }
    *ui.activity_return_focus.borrow_mut() = gtk::prelude::RootExt::focus(&ui.window);
    ui.workspace.set_visible_child_name("mail"); ui.compact_navigation.set(false);
    ui.contact.root.set_visible(false); ui.activity_host.set_visible(true); ui.update_activity_layout();
    let message_id = message_id.filter(|id| !id.is_empty());
    let single = message_id.is_some();
    let root = gtk::Box::new(Orientation::Vertical, 10);
    root.set_margin_top(18); root.set_margin_bottom(16);
    root.set_margin_start(20); root.set_margin_end(20);
    let header = gtk::Box::new(Orientation::Horizontal, 8); header.add_css_class("activity-controls");
    let back = gtk::Button::from_icon_name("go-previous-symbolic"); back.set_widget_name("activity-back");
    back.set_tooltip_text(Some("Back to recent opens (Esc)")); header.append(&back);
    let title = label("Recent opens", true); title.set_hexpand(true);
    header.append(&title);
    let info = gtk::Button::from_icon_name("dialog-information-symbolic");
    info.add_css_class("flat"); info.set_tooltip_text(Some("Privacy protection and mail scanners can affect open tracking."));
    info.update_property(&[gtk::accessible::Property::Label("About open tracking")]);
    header.append(&info);
    let spinner = gtk::Spinner::new(); header.append(&spinner);
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.add_css_class("flat"); refresh.set_tooltip_text(Some("Check for new activity"));
    header.append(&refresh);
    let close = gtk::Button::from_icon_name("window-close-symbolic"); close.add_css_class("flat");
    close.set_tooltip_text(Some("Close recent opens (Esc)")); header.append(&close);
    root.append(&header);
    let filters = gtk::Box::new(Orientation::Horizontal, 0); filters.add_css_class("linked");
    let filter_buttons: Vec<_> = ["All", "Awaiting reply", "Has activity"].into_iter().map(gtk::ToggleButton::with_label).collect();
    for (index, button) in filter_buttons.iter().enumerate() {
        button.set_widget_name(&format!("activity-filter-{index}"));
        if index > 0 { button.set_group(Some(&filter_buttons[0])); }
        filters.append(button);
    }
    filter_buttons[0].set_active(true); root.append(&filters);
    let headline = label("Loading saved activity…", true);
    let recipient = label("", false);
    let detail = label("", true);
    let rows = gtk::Box::new(Orientation::Vertical, 8); rows.set_valign(gtk::Align::Start);
    let scroll = gtk::ScrolledWindow::builder().hexpand(true).vexpand(true)
        .min_content_height(150).max_content_height(320).propagate_natural_height(true)
        .hscrollbar_policy(gtk::PolicyType::Never).child(&rows).build();
    root.append(&scroll);
    let freshness = label("", false); freshness.add_css_class("caption"); root.append(&freshness);
    let status = label("", false); status.set_visible(false); root.append(&status);
    let panel = Rc::new(ActivityPanel {
        ui: Rc::downgrade(ui), rows, scroll, headline, recipient, detail,
        filters: filters.clone(), filter_buttons: filter_buttons.clone(), filter: Cell::new(0), heading: title,
        back: back.clone(), refresh: refresh.clone(), spinner,
        status, freshness, refreshed_at: Cell::new(None), message_id: RefCell::new(message_id), feed: RefCell::new(Vec::new()),
        item: RefCell::new(None), generation: Cell::new(0), pending: Cell::new(false), status_after_load: RefCell::new(None), closed: Cell::new(false),
        allows_back: !single,
    });
    let weak = Rc::downgrade(&panel);
    refresh.connect_clicked(move |_| if let Some(panel) = weak.upgrade() { panel.load(true); });
    for (index, button) in filter_buttons.iter().enumerate() {
        let weak = Rc::downgrade(&panel);
        button.connect_toggled(move |button| if button.is_active() { if let Some(panel) = weak.upgrade() {
            panel.filter.set(index as u32); panel.scroll.vadjustment().set_value(0.); panel.render();
        }});
    }
    let weak = Rc::downgrade(&panel);
    back.connect_clicked(move |_| if let Some(panel) = weak.upgrade() {
        panel.show_feed();
    });
    ui.activity_host.append(&root);
    *ui.activity_panel.borrow_mut() = Some(panel.clone());
    let weak = Rc::downgrade(&panel);
    close.connect_clicked(move |_| if let Some(panel) = weak.upgrade() { panel.dismiss(); });
    let keys = gtk::EventControllerKey::new(); let weak = Rc::downgrade(&panel);
    keys.connect_key_pressed(move |_, key, _, mods| {
        if mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) { return glib::Propagation::Proceed; }
        let Some(panel) = weak.upgrade() else { return glib::Propagation::Proceed; };
        if panel.handle_key(key) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
    });
    root.add_controller(keys);
    let weak = Rc::downgrade(&panel);
    glib::timeout_add_local(std::time::Duration::from_secs(30), move || {
        let Some(panel) = weak.upgrade() else { return glib::ControlFlow::Break; };
        if panel.closed.get() { return glib::ControlFlow::Break; }
        panel.update_freshness(); glib::ControlFlow::Continue
    });
    panel.render(); panel.refresh.grab_focus(); panel.load(false);
}

impl ActivityPanel {
    fn update_freshness(&self) {
        self.freshness.set_text(&self.refreshed_at.get().map(|at| format!("Last checked {}", relative_time(at)))
            .unwrap_or_else(|| "Saved activity · not yet checked".into()));
        self.freshness.set_tooltip_text(self.refreshed_at.get().map(exact_time).as_deref());
    }
    fn show_feed(self: &Rc<Self>) {
        self.generation.set(self.generation.get() + 1); self.spinner.stop(); self.refresh.set_sensitive(true);
        self.message_id.borrow_mut().take(); self.item.borrow_mut().take(); self.status.set_visible(false); self.scroll.vadjustment().set_value(0.);
        self.render(); self.filter_buttons[self.filter.get() as usize].grab_focus(); self.load(false);
    }
    pub(super) fn dismiss(&self) {
        self.closed.set(true); self.generation.set(self.generation.get() + 1);
        if let Some(ui) = self.ui.upgrade() {
            ui.activity_host.set_visible(false);
            while let Some(child) = ui.activity_host.first_child() { ui.activity_host.remove(&child); }
            ui.activity_panel.borrow_mut().take();
            ui.update_activity_layout();
            let focus = ui.activity_return_focus.borrow_mut().take();
            let weak = Rc::downgrade(&ui);
            // Finish dispatching Escape before restoring focus outside the removed pane.
            glib::idle_add_local_once(move || {
                let Some(ui) = weak.upgrade() else { return; };
                if ui.activity_host.get_visible() { return; }
                if focus.filter(|w| w.is_mapped()).is_some_and(|w| w.grab_focus()) { return; }
                if ui.list.is_mapped() && ui.list.grab_focus() { return; }
                ui.mail_reader_viewport.grab_focus();
            });
        }
    }
    fn handle_key(self: &Rc<Self>, key: gdk::Key) -> bool {
        if key == gdk::Key::Escape {
            if self.allows_back && self.message_id.borrow().is_some() { self.show_feed(); } else { self.dismiss(); }
            return true;
        }
        let mut buttons = Vec::new(); let mut child = self.rows.first_child();
        while let Some(widget) = child {
            if let Ok(button) = widget.clone().downcast::<gtk::Button>() { if button.is_sensitive() { buttons.push(button); } }
            child = widget.next_sibling();
        }
        if matches!(key, gdk::Key::j | gdk::Key::k | gdk::Key::Down | gdk::Key::Up) {
            if buttons.is_empty() { return true; }
            let forward = matches!(key, gdk::Key::j | gdk::Key::Down);
            let current = buttons.iter().position(|button| button.has_focus());
            let index = current.map(|index| if forward { (index + 1).min(buttons.len()-1) } else { index.saturating_sub(1) }).unwrap_or(if forward { 0 } else { buttons.len()-1 });
            buttons[index].grab_focus(); return true;
        }
        if key == gdk::Key::Return { if let Some(button) = buttons.iter().find(|b| b.has_focus()) { button.emit_clicked(); return true; } }
        false
    }
    fn load(self: &Rc<Self>, refresh: bool) {
        self.load_with(refresh, !refresh);
    }
    fn load_saved(self: &Rc<Self>) {
        self.load_with(false, false);
    }
    fn load_with(self: &Rc<Self>, refresh: bool, refresh_after_cache: bool) {
        if self.closed.get() { return; }
        let Some(ui) = self.ui.upgrade() else { return; };
        self.generation.set(self.generation.get() + 1); let serial = self.generation.get();
        self.spinner.start(); self.refresh.set_sensitive(false);
        let id = self.message_id.borrow().clone(); let weak = Rc::downgrade(self);
        workflows::request_async(ui.database_path.clone(), json!({"op":"tracking_activity","message_id":id,"refresh":refresh}), move |result| {
            let Some(panel) = weak.upgrade() else { return; };
            if panel.closed.get() || panel.generation.get() != serial { return; }
            panel.spinner.stop(); panel.refresh.set_sensitive(true);
            match result {
                Err(error) => {
                    panel.status.set_text(if refresh { "Couldn't check for new activity. Showing saved activity." } else { &error });
                    panel.status.set_visible(true);
                }
                Ok(data) => {
                    let items = data["items"].as_array().cloned().unwrap_or_default();
                    if panel.message_id.borrow().is_some() { *panel.item.borrow_mut() = items.into_iter().next(); }
                    else { *panel.feed.borrow_mut() = items; }
                    let checked = data["refreshed_at_ms"].as_i64();
                    panel.refreshed_at.set(checked); panel.update_freshness();
                    if let Some(error) = panel.status_after_load.borrow_mut().take() {
                        panel.status.set_text(&error); panel.status.set_visible(true);
                    } else { panel.status.set_visible(false); }
                    panel.render();
                    if refresh_after_cache { panel.load(true); }
                }
            }
        });
    }
    fn restore_position(self: &Rc<Self>, focus: Option<String>, scroll: f64) {
        let weak = Rc::downgrade(self); let generation = self.generation.get();
        glib::idle_add_local_once(move || {
            let Some(panel) = weak.upgrade() else { return; };
            if panel.closed.get() || panel.generation.get() != generation { return; }
            panel.scroll.vadjustment().set_value(scroll);
            if let Some(name) = focus { if let Some(target) = activity_widget(&panel.rows.clone().upcast(), &name) { target.grab_focus(); } }
        });
    }
    fn render(self: &Rc<Self>) {
        let focused = self.rows.root().and_then(|root| root.focus()).map(|widget| widget.widget_name().to_string()).filter(|name| name.starts_with("activity-"));
        let scroll = self.scroll.vadjustment().value();
        let single = self.message_id.borrow().is_some();
        self.back.set_visible(single && self.allows_back); self.filters.set_visible(!single);
        self.heading.set_text(if single { "Open activity" } else { "Recent opens" });
        self.recipient.set_visible(single); self.detail.set_visible(single);
        while let Some(child) = self.rows.first_child() { self.rows.remove(&child); }
        if single {
            let item = self.item.borrow().clone();
            let Some(item) = item else {
                self.headline.set_text("No opens recorded"); self.recipient.set_text("");
                self.detail.set_text("No saved activity for this message."); self.rows.append(&self.headline); self.rows.append(&self.detail); return;
            };
            self.headline.set_text(activity_subject(&item));
            self.recipient.set_text(item["recipient"].as_str().unwrap_or(""));
            let count = item["pixel_requests"].as_u64().unwrap_or(0);
            self.detail.set_text(&match count {
                0 => "No opens recorded".into(), 1 => "Opened once".into(), n => format!("Opened {n} times"),
            });
            self.detail.add_css_class("title-2"); self.rows.append(&self.headline);
            self.rows.append(&self.recipient); self.rows.append(&self.detail);
            self.rows.append(&label(if item["reply_received"] == true { "Reply received" } else { "No reply in downloaded mail" }, false));
            if item["tracking_status"] == "expired" {
                self.rows.append(&label("Tracking expired. Saved activity remains available.", false));
            }
            let followup = &item["followup"];
            let active = active_followup(followup);
            if active {
                let due = followup["due_at_ms"].as_i64();
                let text = if followup["status"] == "due" { "Follow-up due" } else { "Follow-up scheduled" };
                self.rows.append(&label(&due.map(|at| format!("{text} · {}", exact_time(at))).unwrap_or_else(|| text.into()), true));
            }
            let open = gtk::Button::with_label("Open conversation"); open.set_widget_name("activity-open"); open.set_halign(gtk::Align::Start);
            let weak = Rc::downgrade(self); let target = item.clone();
            open.connect_clicked(move |_| if let Some(panel) = weak.upgrade() { panel.open_conversation(&target); });
            self.rows.append(&open);
            if active || item["reply_received"].as_bool() != Some(true) {
                let action = gtk::Button::with_label(if active { "Cancel reminder" } else { "Remind in 3 days if no reply" });
                action.set_widget_name("activity-reminder");
                action.set_halign(gtk::Align::Start); action.set_sensitive(!self.pending.get());
                action.set_tooltip_text(Some(if active { "Cancel this conversation's reminder" } else { "Remind in three days only if no reply is downloaded" }));
                let weak = Rc::downgrade(self); let thread = item["thread_id"].as_str().map(str::to_owned);
                let sent = item["message_id"].as_str().map(str::to_owned);
                action.connect_clicked(move |button| if let (Some(panel), Some(thread)) = (weak.upgrade(), thread.as_ref()) {
                    if let Some(sent) = sent.as_ref() { panel.change_followup(thread, sent, active, button); }
                });
                self.rows.append(&action);
            }
            self.rows.append(&gtk::Separator::new(Orientation::Horizontal));
            let mut events = item["timeline"].as_array().cloned().unwrap_or_default();
            events.sort_by_key(|event| std::cmp::Reverse(event["at_ms"].as_i64().unwrap_or(0)));
            for event in &events {
                let Some(at) = event["at_ms"].as_i64() else { continue; };
                let Some(name) = timeline_label(event) else { continue; };
                let row = label(&format!("{name}\n{}", exact_time(at)), false);
                row.remove_css_class("dim-label");
                row.set_tooltip_text(Some(&exact_time(at))); self.rows.append(&row);
            }
            if item["timeline_complete"].as_bool() != Some(true) {
                self.rows.append(&label("Earlier opens may only have saved totals. The timeline can be incomplete.", false));
            }
            self.restore_position(focused, scroll);
            return;
        }
        let mut items = self.feed.borrow().clone();
        items.sort_by_key(|item| std::cmp::Reverse(item["last_activity_at_ms"].as_i64().or_else(|| item["last_requested_at"].as_i64()).unwrap_or(0)));
        let selected = self.filter.get(); items.retain(|item| activity_matches(item, selected));
        if items.is_empty() { self.rows.append(&label(if self.feed.borrow().is_empty() {
            if super::OPEN_TRACKING { "No tracked messages yet. Enable Track opens in a draft before sending." } else { "No tracked messages." }
        } else { "No messages match this filter." }, false)); }
        for (index, item) in items.iter().take(200).enumerate() {
            if index > 0 { self.rows.append(&gtk::Separator::new(Orientation::Horizontal)); }
            let content = gtk::Box::new(Orientation::Vertical, 3);
            let subject = label(activity_subject(item), true); subject.set_ellipsize(gtk::pango::EllipsizeMode::End); subject.set_max_width_chars(48);
            content.append(&subject);
            let recipient = label(item["recipient"].as_str().unwrap_or(""), false); recipient.set_ellipsize(gtk::pango::EllipsizeMode::End); recipient.set_max_width_chars(48);
            recipient.set_visible(!recipient.text().is_empty());
            let button = gtk::Button::new(); button.set_widget_name(&format!("activity-message-{}", item["message_id"].as_str().unwrap_or(""))); button.add_css_class("flat"); button.set_child(Some(&content));
            button.set_tooltip_text(Some("View message activity"));
            let target = item.clone(); let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| if let Some(panel) = weak.upgrade() {
                *panel.message_id.borrow_mut() = target["message_id"].as_str().map(str::to_owned);
                *panel.item.borrow_mut() = Some(target.clone()); panel.scroll.vadjustment().set_value(0.); panel.render(); panel.back.grab_focus(); panel.load(false);
            });
            self.rows.append(&button); self.rows.append(&recipient);
            self.rows.append(&label(&activity_summary(item), false));
        }
        self.restore_position(focused, scroll);
    }
    fn change_followup(self: &Rc<Self>, thread: &str, sent: &str, cancel: bool, button: &gtk::Button) {
        let Some(ui) = self.ui.upgrade() else { return; };
        if self.pending.replace(true) { return; } button.set_sensitive(false);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        let request = if cancel { json!({"op":"cancel_followup","thread_id":thread}) }
            else { json!({"op":"set_followup","thread_id":thread,"due_at_ms":now+3*86_400_000,"only_if_no_reply":true,"sent_message_id":sent}) };
        let weak = Rc::downgrade(self); let button = button.clone();
        workflows::request_async(ui.database_path.clone(), request, move |result| {
            let Some(panel) = weak.upgrade() else { return; }; panel.pending.set(false);
            if panel.closed.get() { return; }
            button.set_sensitive(true);
            match result {
                Ok(_) => { if let Some(ui) = panel.ui.upgrade() { ui.refresh(); } panel.load_saved(); }
                Err(error) => { *panel.status_after_load.borrow_mut() = Some(error); panel.load_saved(); }
            }
        });
    }
    fn open_conversation(&self, item: &Value) {
        let Some(ui) = self.ui.upgrade() else { return; };
        if ui.compose_pane.borrow().is_some() {
            self.status.set_text("Close your draft before opening another conversation.");
            self.status.set_visible(true); return;
        }
        let Some(id) = item["message_id"].as_str() else { return; };
        let message = ui.request(json!({"op":"get_message","id":id})).ok()
            .and_then(|v| serde_json::from_value::<Message>(v).ok());
        let Some(message) = message.filter(|message| activity_target_matches(item, message)) else {
            self.status.set_text("This sent message is no longer available in the local cache.");
            self.status.set_visible(true); return;
        };
        ui.search.set_text(""); ui.inbox_filter.set(0); ui.set_view(MailView::Sent);
        *ui.activity_navigation.borrow_mut() = Some(message.clone()); ui.refresh();
        let index = ui.items.borrow().iter().position(|row| row.thread_id == message.thread_id);
        if let Some(index) = index {
            ui.list.select_row(ui.list.row_at_index(index as i32).as_ref());
            ui.show_message(&message); ui.list.grab_focus(); if ui.window.width() < 1000 { self.dismiss(); }
        } else {
            self.status.set_text("Couldn't open this conversation from saved mail."); self.status.set_visible(true);
        }
    }
}

/// Compact selected-message details for the existing contact inspector.
pub(super) fn open_activity_summary(item: &Value) -> gtk::Box {
    let body = gtk::Box::new(Orientation::Vertical, 8);
    let count = item["pixel_requests"].as_u64().unwrap_or(0);
    body.append(&label(&match count { 0 => "No opens recorded".into(), 1 => "Opened once".into(), n => format!("Opened {n} times") }, true));
    if let Some(last) = item["last_requested_at"].as_i64() { body.append(&label(&format!("Last opened {}", relative_time(last)), false)); }
    body.append(&label(if item["reply_received"] == true { "Reply received" } else { "No reply in downloaded mail" }, false));
    if item["tracking_status"] == "expired" { body.append(&label("Tracking expired", false)); }
    for event in open_activity_events(item) {
        if let Some(at) = event["at_ms"].as_i64() {
            body.append(&label(&format!("{}\n{}", timeline_label(event).unwrap_or_default(), exact_time(at)), false));
        }
    }
    if item["timeline_complete"].as_bool() != Some(true) { body.append(&label("Earlier opens may only have saved totals.", false)); }
    body
}
fn open_activity_events(item: &Value) -> Vec<&Value> {
    let mut events: Vec<_> = item["timeline"].as_array().into_iter().flatten().filter(|event| event["kind"] == "image_request").collect();
    events.sort_by_key(|event| std::cmp::Reverse(event["at_ms"].as_i64().unwrap_or(0)));
    events
}

fn activity_target_matches(item: &Value, message: &Message) -> bool {
    message.is_sent && !message.is_draft
        && Some(message.id.as_str()) == item["message_id"].as_str()
        && Some(message.thread_id.as_str()) == item["thread_id"].as_str()
        && Some(message.account_id.as_str()) == item["account_id"].as_str()
}
fn activity_widget(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if root.widget_name() == name { return Some(root.clone()); }
    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(found) = activity_widget(&widget, name) { return Some(found); }
        child = widget.next_sibling();
    }
    None
}
fn activity_subject(item: &Value) -> &str {
    item["subject"].as_str().filter(|value| !value.trim().is_empty()).unwrap_or("(No subject)")
}
fn active_followup(value: &Value) -> bool { matches!(value["status"].as_str(), Some("waiting" | "due")) }
fn activity_matches(item: &Value, filter: u32) -> bool {
    match filter {
        1 => item["reply_received"].as_bool() != Some(true),
        2 => item["pixel_requests"].as_u64().unwrap_or(0) > 0 || item["reply_received"].as_bool() == Some(true),
        _ => true,
    }
}
fn activity_summary(item: &Value) -> String {
    let count = item["pixel_requests"].as_u64().unwrap_or(0);
    let mut parts = vec![match count { 0 => "No opens recorded".into(), 1 => "Opened once".into(), n => format!("Opened {n} times") }];
    if let Some(last) = item["last_requested_at"].as_i64() { parts.push(format!("Last opened {}", relative_time(last))); }
    if item["reply_received"].as_bool() == Some(true) { parts.push("Reply received".into()); }
    else { parts.push("Awaiting reply".into()); }
    if active_followup(&item["followup"]) { parts.push(if item["followup"]["status"] == "due" { "Follow-up due".into() } else { "Follow-up scheduled".into() }); }
    parts.join(" · ")
}
fn timeline_label(event: &Value) -> Option<String> {
    Some(match event["kind"].as_str()? {
        "sent" => "Sent".into(), "reply_received" => "Reply received".into(),
        "image_request" => match event["count"].as_u64().unwrap_or(1) { 1 => "Opened".into(), n => format!("Opened {n} times") },
        _ => return None,
    })
}
fn exact_time(ms: i64) -> String {
    glib::DateTime::from_unix_local(ms / 1000).ok()
        .and_then(|date| date.format("%e %b %Y at %H:%M:%S %Z").ok()).map(|value| value.to_string())
        .unwrap_or_else(|| "Time unavailable".into())
}

fn relative_time(millis: i64) -> String {
    let Ok(date) = glib::DateTime::from_unix_local(millis / 1000) else {
        return String::new();
    };
    let Ok(now) = glib::DateTime::now_local() else {
        return String::new();
    };
    let seconds = now.to_unix() - date.to_unix();
    if seconds < 60 {
        return "just now".into();
    }
    if seconds < 3600 {
        let m = seconds / 60;
        return format!("{m} minute{} ago", if m == 1 { "" } else { "s" });
    }
    if seconds < 6 * 3600 {
        let h = seconds / 3600;
        return format!("{h} hour{} ago", if h == 1 { "" } else { "s" });
    }
    let time = date.format("%H:%M").map(|t| t.to_string()).unwrap_or_default();
    let day = |d: &glib::DateTime| (d.year(), d.day_of_year());
    if day(&date) == day(&now) {
        return format!("today at {time}");
    }
    if now.add_days(-1).is_ok_and(|y| day(&y) == day(&date)) {
        return format!("yesterday at {time}");
    }
    format!("{} at {time}", date.format("%-e %b").map(|t| t.to_string()).unwrap_or_default())
}


#[cfg(test)]
mod activity_tests {
    use super::*;
    #[test]
    fn activity_navigation_requires_the_exact_cached_sent_message() {
        let target = json!({"message_id":"sent-a","thread_id":"thread-a","account_id":"account-a"});
        let mut message: Message = serde_json::from_value(json!({"id":"sent-a","thread_id":"thread-a","account_id":"account-a","sender":"me@example.test","to":[],"subject":"Subject","body_plain":"Body","body_html":null,"received_at":1,"is_read":true,"is_draft":false,"is_sent":true})).unwrap();
        assert!(activity_target_matches(&target, &message));
        for key in ["message_id", "thread_id", "account_id"] {
            let mut wrong = target.clone(); wrong[key] = json!("other");
            assert!(!activity_target_matches(&wrong, &message));
        }
        message.is_draft = true; assert!(!activity_target_matches(&target, &message));
        message.is_draft = false; message.is_sent = false; assert!(!activity_target_matches(&target, &message));
    }
    #[test]
    fn activity_filters_use_observed_requests_and_cached_replies() {
        let empty = json!({"pixel_requests":0,"reply_received":false});
        assert!(activity_matches(&empty, 0)); assert!(activity_matches(&empty, 1)); assert!(!activity_matches(&empty, 2));
        let requested = json!({"pixel_requests":4,"reply_received":false});
        assert!(activity_matches(&requested, 1)); assert!(activity_matches(&requested, 2));
        let replied = json!({"pixel_requests":0,"reply_received":true});
        assert!(!activity_matches(&replied, 1)); assert!(activity_matches(&replied, 2));
    }
    #[test]
    fn a_reply_is_activity_even_when_no_image_was_requested() {
        let summary = activity_summary(&json!({"pixel_requests":0,"reply_received":true}));
        assert!(summary.contains("Reply received")); assert!(summary.contains("No opens recorded"));
        assert!(!summary.contains("Awaiting reply"));
    }
    #[test]
    fn only_waiting_or_due_reminders_expose_cancellation() {
        for status in ["waiting", "due"] { assert!(active_followup(&json!({"status":status}))); }
        for status in ["replied", "cancelled", "unknown"] { assert!(!active_followup(&json!({"status":status}))); }
        assert!(!active_followup(&Value::Null));
    }
    #[test]
    fn contact_history_keeps_more_than_three_opens_in_time_order() {
        let item=json!({"timeline":[
            {"kind":"image_request","at_ms":1},{"kind":"sent","at_ms":99},
            {"kind":"image_request","at_ms":5},{"kind":"image_request","at_ms":3},
            {"kind":"image_request","at_ms":2},{"kind":"image_request","at_ms":4}
        ]});
        assert_eq!(open_activity_events(&item).iter().map(|e|e["at_ms"].as_i64().unwrap()).collect::<Vec<_>>(),vec![5,4,3,2,1]);
    }
    #[test]
    fn timeline_describes_grouped_opens() {
        assert_eq!(timeline_label(&json!({"kind":"image_request","count":3})).as_deref(), Some("Opened 3 times"));
        assert_eq!(timeline_label(&json!({"kind":"sent"})).as_deref(), Some("Sent"));
        assert_eq!(timeline_label(&json!({"kind":"human_read"})), None);
    }
}
