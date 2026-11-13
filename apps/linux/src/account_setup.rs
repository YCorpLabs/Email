//! Account setup, mirroring macOS `AccountSetupWindow.swift`: the same choices,
//! order, copy, preset servers, inline errors and keyboard behavior (Ctrl+digit
//! replaces ⌘+digit; Escape cancels; Enter connects).
use crate::Ui;
use gtk::{Orientation, gdk, glib};
use mail_service::MailService;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{OnceLock, mpsc},
    thread,
    time::Duration,
};
use webkit6::{gtk, prelude::*};

/// What this build offers, from the Rust service (`build_features`). macOS
/// reads the same request (`CoreBridge.buildFeatures`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BuildFeatures {
    pub channel: String,
    pub google_contact_photos: bool,
    pub microsoft_accounts: bool,
    /// "Google OAuth publicly available" (the profile's `GOOGLE_OAUTH_PUBLIC`).
    /// Off in official releases until Google's review passes: account setup then
    /// offers Gmail only through an app password. Connected accounts keep working.
    pub google_oauth_public: bool,
}

impl BuildFeatures {
    /// Used if the service cannot answer: withhold optional features.
    fn restricted() -> Self {
        Self {
            channel: "unknown".into(),
            google_contact_photos: false,
            microsoft_accounts: false,
            google_oauth_public: false,
        }
    }

    fn from_response(value: Option<Value>) -> Self {
        let Some(value) = value.filter(|v| v["channel"].is_string()) else {
            return Self::restricted();
        };
        Self {
            channel: value["channel"].as_str().unwrap_or_default().into(),
            google_contact_photos: value["google_contact_photos"].as_bool().unwrap_or(false),
            microsoft_accounts: value["microsoft_accounts"].as_bool().unwrap_or(false),
            google_oauth_public: value["google_oauth_public"].as_bool().unwrap_or(false),
        }
    }
}

/// Fixed when the Rust library is compiled. The first answer is cached for the
/// process, so the offered choices never change while the app runs; if the
/// service could not answer, optional features stay off.
pub(crate) fn build_features(service: &MailService) -> BuildFeatures {
    static FEATURES: OnceLock<BuildFeatures> = OnceLock::new();
    FEATURES
        .get_or_init(|| {
            BuildFeatures::from_response(service.request_json(r#"{"op":"build_features"}"#).ok())
        })
        .clone()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Choice {
    Gmail,
    GmailAppPassword,
    Icloud,
    Outlook,
    Other,
}

impl Choice {
    /// Without Google sign-in the app-password path is the only Gmail entry,
    /// so it is simply "Gmail".
    pub(crate) fn title(self, features: &BuildFeatures) -> &'static str {
        match self {
            Self::Gmail => "Gmail",
            Self::GmailAppPassword if features.google_oauth_public => "Gmail (app password)",
            Self::GmailAppPassword => "Gmail",
            Self::Icloud => "iCloud",
            Self::Outlook => "Outlook",
            Self::Other => "Other",
        }
    }
    fn uses_password(self) -> bool {
        matches!(self, Self::GmailAppPassword | Self::Icloud | Self::Other)
    }
}

/// Display order. Official releases do not offer Google sign-in until Google's
/// review passes (`google_oauth_public`); Gmail then goes straight to the
/// app-password path. Outlook/Hotmail stays out of distribution builds until
/// it has been tested with real accounts. Development builds keep both.
pub(crate) fn available(features: &BuildFeatures) -> Vec<Choice> {
    let mut choices = if features.google_oauth_public {
        vec![Choice::Gmail, Choice::GmailAppPassword]
    } else {
        vec![Choice::GmailAppPassword]
    };
    choices.push(Choice::Icloud);
    if features.microsoft_accounts {
        choices.push(Choice::Outlook);
    }
    choices.push(Choice::Other);
    choices
}

/// The choice selected when the window opens.
pub(crate) fn initial(features: &BuildFeatures, google_sign_in_configured: bool) -> Choice {
    if features.google_oauth_public && google_sign_in_configured {
        Choice::Gmail
    } else {
        Choice::GmailAppPassword
    }
}

// Gmail's servers are owned by the Rust preset (`ImapConfig::gmail`); the
// request names none.
pub(crate) const GMAIL_TWO_STEP_URL: &str = "https://myaccount.google.com/signinoptions/twosv";
pub(crate) const GMAIL_APP_PASSWORDS_URL: &str = "https://myaccount.google.com/apppasswords";
pub(crate) const GMAIL_HELP_URL: &str = "https://support.google.com/accounts/answer/185833";
const ICLOUD_HELP_URL: &str = "https://support.apple.com/en-us/102654";
/// The same three steps as macOS `GmailAppPasswordPreset.steps`.
pub(crate) const GMAIL_STEPS: [(&str, Option<(&str, &str)>); 3] = [
    (
        "Turn on 2-Step Verification for your Google Account.",
        Some(("Open 2-Step Verification ↗", GMAIL_TWO_STEP_URL)),
    ),
    (
        "Create an app password and name it “Email”.",
        Some(("Open App passwords ↗", GMAIL_APP_PASSWORDS_URL)),
    ),
    (
        "Paste the 16-letter password below. Spaces don't matter.",
        None,
    ),
];
/// macOS says "your Mac's Keychain"; the credential store is the only difference.
const GMAIL_ABOUT: &str = "An app password is a 16-letter code that lets Email sign in to Gmail's mail servers without your Google password. Email keeps it in your system keyring and sends it only to Google. Remove it on the App passwords page to revoke access; changing your Google password revokes it too.";
/// Shown when this build offers no Google sign-in (macOS `signInComing`).
pub(crate) const GMAIL_SIGN_IN_COMING: &str =
    "One-click Google sign-in is coming once Google finishes reviewing the app.";
fn gmail_unavailable(features: &BuildFeatures) -> String {
    let next = if features.google_oauth_public {
        "Use Gmail sign-in instead, or ask your administrator."
    } else {
        "Ask your administrator, or wait for one-click Google sign-in."
    };
    format!(
        "No App passwords option? Google doesn't offer them for accounts that use only security keys or Advanced Protection, or for some work and school accounts. {next}"
    )
}

/// The values typed into the password-based form.
#[derive(Clone, Debug, Default)]
pub(crate) struct Form {
    pub display_name: String,
    pub email: String,
    pub username: String,
    pub smtp_username: String,
    pub password: String,
    pub imap_host: String,
    pub imap_port: String,
    pub imap_tls: bool,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_tls: bool,
}

/// The `add_imap_account` request, identical to the macOS payload.
pub(crate) fn imap_payload(
    choice: Choice,
    form: &Form,
    google_sign_in: bool,
) -> Result<Value, &'static str> {
    let address = form.email.trim().to_string();
    if choice == Choice::GmailAppPassword {
        // The service owns Gmail's servers and usernames; send only the address
        // and the app password.
        if !address.contains('@') || form.password.trim().is_empty() {
            return Err(invalid_message(choice));
        }
        return Ok(json!({
            "op": "add_imap_account", "provider": "imap", "preset": "gmail",
            "email": address, "display_name": address, "username": address,
            "smtp_username": address, "password": form.password,
            "google_sign_in": google_sign_in,
        }));
    }
    let typed_login = form.username.trim().to_string();
    // iCloud's username field is hidden: never send one left over from Other.
    let login = match choice {
        Choice::Icloud => address.split('@').next().unwrap_or("").to_string(),
        _ if typed_login.is_empty() => address.clone(),
        _ => typed_login,
    };
    let port = |value: &str| value.trim().parse::<u16>().ok().filter(|p| *p > 0);
    let (Some(imap_port), Some(smtp_port)) = (port(&form.imap_port), port(&form.smtp_port)) else {
        return Err(invalid_message(choice));
    };
    if !address.contains('@')
        || login.is_empty()
        || form.password.trim().is_empty()
        || form.imap_host.trim().is_empty()
        || form.smtp_host.trim().is_empty()
    {
        return Err(invalid_message(choice));
    }
    let security = |tls: bool| if tls { "implicit_tls" } else { "start_tls" };
    let mut payload = json!({
        "op": "add_imap_account",
        "provider": if choice == Choice::Icloud { "icloud" } else { "imap" },
        "email": address,
        "display_name": if form.display_name.is_empty() { address.clone() } else { form.display_name.clone() },
        "username": login,
        "password": form.password,
        "imap_host": form.imap_host.trim(), "imap_port": imap_port, "imap_security": security(form.imap_tls),
        "smtp_host": form.smtp_host.trim(), "smtp_port": smtp_port, "smtp_security": security(form.smtp_tls),
    });
    let smtp_login = form.smtp_username.trim();
    if !smtp_login.is_empty() && choice == Choice::Other {
        payload["smtp_username"] = json!(smtp_login);
    } else if choice != Choice::Icloud {
        payload["smtp_username"] = payload["username"].clone();
    }
    Ok(payload)
}

fn invalid_message(choice: Choice) -> &'static str {
    match choice {
        Choice::GmailAppPassword => {
            "Enter your Gmail address and the 16-letter app password from step 2."
        }
        Choice::Icloud => "Enter your iCloud address and app-specific password.",
        _ => "Enter a valid email, provider-issued app password, server names, and ports.",
    }
}

fn intro(features: &BuildFeatures) -> &'static str {
    if !features.google_oauth_public && features.microsoft_accounts {
        "Outlook signs in through your browser. Gmail and iCloud use a password created for this app. Never enter your main account password here."
    } else if !features.google_oauth_public {
        "Gmail and iCloud connect with a password created for this app. Never enter your main account password here."
    } else if features.microsoft_accounts {
        "Gmail and Outlook sign in through your browser. Gmail (app password) and iCloud use a password created for this app. Never enter your main account password here."
    } else {
        "Gmail signs in through your browser. Gmail (app password) and iCloud use a password created for this app. Never enter your main account password here."
    }
}

fn status_text(choice: Choice, oauth_ready: bool) -> &'static str {
    match choice {
        Choice::Icloud => {
            "Use a password created for this app in your Apple Account. Never enter your Apple Account password."
        }
        Choice::GmailAppPassword => {
            "Email connects directly to imap.gmail.com and smtp.gmail.com. Your regular Google password won't work here."
        }
        Choice::Other => {
            "Use a provider-issued app password and the server settings from your provider."
        }
        Choice::Gmail if !oauth_ready => {
            "Google sign-in isn't configured in this build. Use Gmail (app password) instead."
        }
        Choice::Gmail | Choice::Outlook if oauth_ready => {
            "You'll sign in on your provider's website in your browser."
        }
        Choice::Gmail | Choice::Outlook => {
            "Sign-in isn't configured in this build yet. The app developer must register Email with this provider."
        }
    }
}

fn oauth_client_id(choice: Choice) -> Option<String> {
    match choice {
        Choice::Gmail => crate::oauth_client_id("gmail"),
        Choice::Outlook => crate::oauth_client_id("microsoft"),
        _ => None,
    }
    .map(|id| id.trim().to_string())
    .filter(|id| !id.is_empty())
}

fn google_client_secret() -> Option<String> {
    std::env::var("EMAIL_GOOGLE_CLIENT_SECRET")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            option_env!("EMAIL_GOOGLE_CLIENT_SECRET")
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
}

struct Setup {
    ui: std::rc::Weak<Ui>,
    window: gtk::Window,
    choices: Vec<Choice>,
    selected: Cell<Choice>,
    buttons: Vec<(Choice, gtk::Button)>,
    display_name: gtk::Entry,
    email: gtk::Entry,
    username: gtk::Entry,
    smtp_username: gtk::Entry,
    password: gtk::PasswordEntry,
    imap_host: gtk::Entry,
    imap_port: gtk::Entry,
    imap_security: gtk::DropDown,
    smtp_host: gtk::Entry,
    smtp_port: gtk::Entry,
    smtp_security: gtk::DropDown,
    display_name_row: gtk::Box,
    email_row: gtk::Box,
    password_rows: gtk::Box,
    advanced_rows: Vec<gtk::Box>,
    oauth_help: gtk::Label,
    gmail_steps: gtk::Box,
    gmail_about: gtk::Box,
    help: gtk::Button,
    status: gtk::Label,
    switch_to_app_password: gtk::Button,
    connect: gtk::Button,
    /// Choices with a request still running. Another choice can be used
    /// meanwhile; the same one cannot be submitted twice.
    in_flight: RefCell<HashSet<Choice>>,
    /// How a request ended while another choice was on screen: the message
    /// and whether it is a problem. Shown when that choice is selected again.
    outcomes: RefCell<HashMap<Choice, (String, bool)>>,
    sign_in_failed: Cell<bool>,
}

fn row(title: &str, field: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(Orientation::Horizontal, 12);
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.);
    label.set_size_request(110, -1);
    label.add_css_class("setup-label");
    field.as_ref().set_size_request(345, -1);
    row.append(&label);
    row.append(field);
    row
}

fn link(title: &str, url: &'static str) -> gtk::Button {
    let button = gtk::Button::with_label(title);
    button.add_css_class("setup-link");
    button.set_tooltip_text(Some(url));
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Start);
    button.connect_clicked(move |_| open_url(url));
    button
}

fn open_url(url: &str) {
    if let Some(display) = gdk::Display::default() {
        let _ = gtk::gio::AppInfo::launch_default_for_uri(url, Some(&display.app_launch_context()));
    }
}

fn wrapped(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.);
    label.set_max_width_chars(72);
    label.set_size_request(500, -1);
    label.add_css_class(class);
    label
}

/// Open the account window, transient for the mail window.
pub(crate) fn present(ui: &Rc<Ui>) {
    let features = &build_features(&ui.service);
    let choices = available(features);
    let window = gtk::Window::builder()
        .transient_for(&ui.window)
        .modal(true)
        .title("Connect an account")
        .default_width(560)
        .resizable(false)
        .build();
    window.add_css_class("account-setup");
    let content = gtk::Box::new(Orientation::Vertical, 12);
    content.set_margin_top(28);
    content.set_margin_bottom(24);
    content.set_margin_start(30);
    content.set_margin_end(30);
    let headline = gtk::Label::new(Some("Add an email account"));
    headline.set_xalign(0.);
    headline.add_css_class("setup-headline");
    content.append(&headline);
    content.append(&wrapped(intro(features), "setup-intro"));
    let chip_row = gtk::Box::new(Orientation::Horizontal, 8);
    let buttons: Vec<(Choice, gtk::Button)> = choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let button = gtk::Button::with_label(choice.title(features));
            button.add_css_class("provider-choice");
            button.set_tooltip_text(Some(&format!(
                "Choose {} (Ctrl+{})",
                choice.title(features),
                index + 1
            )));
            chip_row.append(&button);
            (*choice, button)
        })
        .collect();
    content.append(&chip_row);

    // Gmail (app password): three numbered steps with direct links.
    let gmail_steps = gtk::Box::new(Orientation::Vertical, 8);
    if !features.google_oauth_public {
        gmail_steps.append(&wrapped(GMAIL_SIGN_IN_COMING, "setup-coming"));
    }
    for (index, (text, target)) in GMAIL_STEPS.iter().enumerate() {
        let line = gtk::Box::new(Orientation::Horizontal, 8);
        let number = gtk::Label::new(Some(&(index + 1).to_string()));
        number.add_css_class("setup-step-number");
        number.set_size_request(14, -1);
        number.set_valign(gtk::Align::Start);
        let label = gtk::Label::new(Some(text));
        label.set_wrap(true);
        label.set_xalign(0.);
        label.set_valign(gtk::Align::Start);
        label.add_css_class("setup-step");
        // Natural width, so the link follows the text as on macOS.
        label.set_max_width_chars(if target.is_some() { 48 } else { 78 });
        line.append(&number);
        line.append(&label);
        if let Some((title, url)) = target {
            line.append(&link(title, url));
        }
        gmail_steps.append(&line);
    }
    content.append(&gmail_steps);

    let entry = |placeholder: &str| {
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(placeholder));
        entry.set_activates_default(true);
        entry
    };
    let display_name = entry("Optional");
    let email = entry("you@example.com");
    let username = entry("Usually your email address");
    let smtp_username = entry("Leave blank to use the mail username");
    let password = gtk::PasswordEntry::new();
    password.set_activates_default(true);
    let imap_host = entry("imap.example.com");
    let imap_port = entry("993");
    let smtp_host = entry("smtp.example.com");
    let smtp_port = entry("587");
    let imap_security = gtk::DropDown::from_strings(&["TLS", "STARTTLS"]);
    let smtp_security = gtk::DropDown::from_strings(&["STARTTLS", "TLS"]);
    let display_name_row = row("Display name", &display_name);
    let email_row = row("Email address", &email);
    content.append(&display_name_row);
    content.append(&email_row);
    let password_rows = gtk::Box::new(Orientation::Vertical, 10);
    let mut advanced_rows = Vec::new();
    for (title, widget) in [
        ("Mail username", username.clone().upcast::<gtk::Widget>()),
        ("SMTP username", smtp_username.clone().upcast()),
        ("App password", password.clone().upcast()),
        ("IMAP server", imap_host.clone().upcast()),
        ("IMAP port", imap_port.clone().upcast()),
        ("IMAP security", imap_security.clone().upcast()),
        ("SMTP server", smtp_host.clone().upcast()),
        ("SMTP port", smtp_port.clone().upcast()),
        ("SMTP security", smtp_security.clone().upcast()),
    ] {
        let field_row = row(title, &widget);
        password_rows.append(&field_row);
        if title != "App password" {
            advanced_rows.push(field_row);
        }
    }
    content.append(&password_rows);
    let oauth_help = wrapped(
        "Sign-in opens on your provider's website. Email receives a revocable access token, never your account password.",
        "setup-note",
    );
    content.append(&oauth_help);
    let gmail_about = gtk::Box::new(Orientation::Vertical, 6);
    gmail_about.append(&wrapped(GMAIL_ABOUT, "setup-note"));
    gmail_about.append(&wrapped(&gmail_unavailable(features), "setup-note"));
    content.append(&gmail_about);
    let help = gtk::Button::with_label("How to get an app password");
    help.add_css_class("setup-link");
    help.set_halign(gtk::Align::Start);
    content.append(&help);
    let status = wrapped("", "setup-status");
    status.set_selectable(true);
    content.append(&status);
    let switch_to_app_password = gtk::Button::with_label("Use Gmail (app password) instead →");
    switch_to_app_password.add_css_class("setup-link");
    switch_to_app_password.add_css_class("setup-switch");
    switch_to_app_password.set_halign(gtk::Align::Start);
    content.append(&switch_to_app_password);
    let actions = gtk::Box::new(Orientation::Horizontal, 10);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("setup-button");
    let connect = gtk::Button::with_label("Connect account");
    connect.add_css_class("setup-button");
    connect.add_css_class("setup-primary");
    actions.append(&cancel);
    actions.append(&connect);
    content.append(&actions);
    window.set_child(Some(&content));
    window.set_default_widget(Some(&connect));

    let setup = Rc::new(Setup {
        ui: Rc::downgrade(ui),
        window: window.clone(),
        choices,
        selected: Cell::new(Choice::Gmail),
        buttons,
        display_name,
        email,
        username,
        smtp_username,
        password,
        imap_host,
        imap_port,
        imap_security,
        smtp_host,
        smtp_port,
        smtp_security,
        display_name_row,
        email_row,
        password_rows,
        advanced_rows,
        oauth_help,
        gmail_steps,
        gmail_about,
        help,
        status,
        switch_to_app_password,
        connect,
        in_flight: RefCell::new(HashSet::new()),
        outcomes: RefCell::new(HashMap::new()),
        sign_in_failed: Cell::new(false),
    });
    for (choice, button) in &setup.buttons {
        let setup = setup.clone();
        let choice = *choice;
        button.connect_clicked(move |_| setup.select(choice));
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let setup = setup.clone();
        setup
            .connect
            .clone()
            .connect_clicked(move |_| setup.connect_clicked());
    }
    {
        let setup = setup.clone();
        setup.help.clone().connect_clicked(move |_| {
            open_url(if setup.selected.get() == Choice::GmailAppPassword {
                GMAIL_HELP_URL
            } else {
                ICLOUD_HELP_URL
            });
        });
    }
    {
        let setup = setup.clone();
        setup
            .switch_to_app_password
            .clone()
            .connect_clicked(move |_| {
                setup.select(Choice::GmailAppPassword);
                if !setup.email.text().is_empty() {
                    setup.password.grab_focus();
                }
            });
    }
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let setup = setup.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::Escape {
                // An open list (security menus) closes itself first, as on macOS.
                let in_popover = gtk::prelude::GtkWindowExt::focus(&setup.window)
                    .and_then(|focus| focus.ancestor(gtk::Popover::static_type()))
                    .is_some();
                if in_popover {
                    return glib::Propagation::Proceed;
                }
                setup.window.close();
                return glib::Propagation::Stop;
            }
            let digit = key
                .to_unicode()
                .and_then(|c| c.to_digit(10))
                .map(|d| d as usize);
            // Ctrl alone (Shift allowed for layouts that need it), like ⌘digit on macOS.
            let held = mods
                & (gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SUPER_MASK
                    | gdk::ModifierType::META_MASK);
            if held == gdk::ModifierType::CONTROL_MASK {
                if let Some(choice) = digit
                    .and_then(|d| d.checked_sub(1))
                    .and_then(|i| setup.choices.get(i))
                {
                    setup.select(*choice);
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
    }
    window.add_controller(keys);
    setup.select(initial(features, oauth_client_id(Choice::Gmail).is_some()));
    window.present();
}

impl Setup {
    fn select(&self, choice: Choice) {
        let choice = if self.choices.contains(&choice) {
            choice
        } else {
            *self.choices.first().unwrap_or(&Choice::Other)
        };
        self.selected.set(choice);
        self.sign_in_failed.set(false);
        self.password.set_text("");
        for (option, button) in &self.buttons {
            if *option == choice {
                button.add_css_class("active");
            } else {
                button.remove_css_class("active");
            }
        }
        let uses_password = choice.uses_password();
        self.password_rows.set_visible(uses_password);
        for advanced in &self.advanced_rows {
            advanced.set_visible(choice == Choice::Other);
        }
        self.oauth_help.set_visible(!uses_password);
        self.email_row.set_visible(uses_password);
        self.display_name_row.set_visible(choice == Choice::Other);
        self.gmail_steps
            .set_visible(choice == Choice::GmailAppPassword);
        self.gmail_about
            .set_visible(choice == Choice::GmailAppPassword);
        self.help
            .set_visible(matches!(choice, Choice::Icloud | Choice::GmailAppPassword));
        self.help.set_label(if choice == Choice::GmailAppPassword {
            "Learn more about app passwords ↗"
        } else {
            "Get an iCloud app password ↗"
        });
        let set_servers = |imap: &str,
                           imap_port: &str,
                           imap_tls: bool,
                           smtp: &str,
                           smtp_port: &str,
                           smtp_tls: bool| {
            self.imap_host.set_text(imap);
            self.imap_port.set_text(imap_port);
            self.imap_security
                .set_selected(if imap_tls { 0 } else { 1 });
            self.smtp_host.set_text(smtp);
            self.smtp_port.set_text(smtp_port);
            self.smtp_security
                .set_selected(if smtp_tls { 1 } else { 0 });
        };
        match choice {
            Choice::Icloud => {
                set_servers(
                    "imap.mail.me.com",
                    "993",
                    true,
                    "smtp.mail.me.com",
                    "587",
                    false,
                );
                self.email.set_placeholder_text(Some("you@icloud.com"));
                self.password
                    .set_placeholder_text(Some("iCloud app-specific password"));
            }
            Choice::GmailAppPassword => {
                self.email.set_placeholder_text(Some("you@gmail.com"));
                self.password
                    .set_placeholder_text(Some("abcd efgh ijkl mnop"));
            }
            Choice::Other => {
                set_servers("", "993", true, "", "587", false);
                self.email.set_placeholder_text(Some("you@example.com"));
                self.password
                    .set_placeholder_text(Some("Provider-issued app password"));
            }
            Choice::Gmail | Choice::Outlook => {}
        }
        self.connect.set_label(if uses_password {
            "Add account"
        } else {
            "Sign in with browser"
        });
        let oauth_ready = uses_password || oauth_client_id(choice).is_some();
        let running = self.in_flight.borrow().contains(&choice);
        self.connect.set_sensitive(oauth_ready && !running);
        self.status.remove_css_class("setup-problem");
        self.status.set_text(if running {
            "Still connecting…"
        } else {
            status_text(choice, oauth_ready)
        });
        self.update_switch(false);
        let outcome = self.outcomes.borrow_mut().remove(&choice);
        match outcome {
            Some((message, true)) => {
                self.sign_in_failed.set(choice == Choice::Gmail);
                self.problem(&message);
            }
            Some((message, false)) => self.status.set_text(&message),
            None => {}
        }
        if uses_password {
            self.email.grab_focus();
        }
    }

    /// Offer the app-password path wherever Google sign-in may not work for this person.
    fn update_switch(&self, waiting: bool) {
        let unconfigured = oauth_client_id(Choice::Gmail).is_none();
        self.switch_to_app_password.set_visible(
            self.selected.get() == Choice::Gmail
                && (unconfigured || waiting || self.sign_in_failed.get()),
        );
    }

    fn form(&self) -> Form {
        Form {
            display_name: self.display_name.text().to_string(),
            email: self.email.text().to_string(),
            username: self.username.text().to_string(),
            smtp_username: self.smtp_username.text().to_string(),
            password: self.password.text().to_string(),
            imap_host: self.imap_host.text().to_string(),
            imap_port: self.imap_port.text().to_string(),
            imap_tls: self.imap_security.selected() == 0,
            smtp_host: self.smtp_host.text().to_string(),
            smtp_port: self.smtp_port.text().to_string(),
            smtp_tls: self.smtp_security.selected() == 1,
        }
    }

    fn refresh_connect(&self) {
        let choice = self.selected.get();
        let ready = choice.uses_password() || oauth_client_id(choice).is_some();
        self.connect
            .set_sensitive(ready && !self.in_flight.borrow().contains(&choice));
    }

    fn problem(&self, message: &str) {
        self.status.set_text(message);
        self.status.add_css_class("setup-problem");
        self.update_switch(false);
    }

    fn connect_clicked(self: &Rc<Self>) {
        let choice = self.selected.get();
        if self.in_flight.borrow().contains(&choice) {
            return;
        }
        let payload = if choice.uses_password() {
            let google_sign_in =
                self.choices.contains(&Choice::Gmail) && oauth_client_id(Choice::Gmail).is_some();
            match imap_payload(choice, &self.form(), google_sign_in) {
                Ok(payload) => payload,
                Err(message) => return self.problem(message),
            }
        } else {
            let Some(client_id) = oauth_client_id(choice) else {
                return self.problem("Sign-in isn't configured in this build yet.");
            };
            let provider = if choice == Choice::Gmail {
                "gmail"
            } else {
                "microsoft"
            };
            let secret = if choice == Choice::Gmail {
                google_client_secret()
            } else {
                None
            };
            json!({"op": "start_oauth", "provider": provider, "client_id": client_id, "client_secret": secret})
        };
        let Some(ui) = self.ui.upgrade() else { return };
        self.in_flight.borrow_mut().insert(choice);
        self.connect.set_sensitive(false);
        self.status.remove_css_class("setup-problem");
        self.status.set_text(match choice {
            Choice::GmailAppPassword => "Connecting to Gmail…",
            Choice::Icloud | Choice::Other => "Checking your mail server…",
            Choice::Gmail => "Waiting for browser sign-in… If Google shows “Access blocked”, close that tab and use an app password.",
            Choice::Outlook => "Waiting for browser sign-in…",
        });
        self.update_switch(choice == Choice::Gmail);
        let (sender, receiver) = mpsc::channel();
        let path = ui.database_path.clone();
        thread::spawn(move || {
            let result = MailService::open(path)
                .and_then(|service| service.request_json(&payload.to_string()));
            let _ = sender.send(result);
        });
        let setup = self.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => Err("The mail engine stopped.".into()),
            };
            setup.in_flight.borrow_mut().remove(&choice);
            let elsewhere = setup.selected.get() != choice;
            match result {
                Ok(account) => {
                    if elsewhere {
                        // Keep the window and whatever is typed for the other choice.
                        let connected = account["email"].as_str().map_or_else(
                            || "Connected.".to_string(),
                            |a| format!("Connected {a}."),
                        );
                        setup
                            .outcomes
                            .borrow_mut()
                            .insert(choice, (connected, false));
                        setup.refresh_connect();
                    } else {
                        setup.password.set_text("");
                        setup.window.close();
                    }
                    // The main loop shows "Account connected", syncs and refreshes.
                    if let Some(ui) = setup.ui.upgrade() {
                        let _ = ui.completed.send(Ok(account));
                    }
                }
                // Another choice is on screen: keep the message for when it returns.
                Err(message) if elsewhere => {
                    setup.outcomes.borrow_mut().insert(choice, (message, true));
                    setup.refresh_connect();
                }
                Err(message) => {
                    setup.sign_in_failed.set(choice == Choice::Gmail);
                    setup.problem(&message);
                    setup.connect.set_sensitive(true);
                }
            }
            glib::ControlFlow::Break
        });
    }
}

/// Styles for the account window, in the app's palette (macOS `accentSoft`
/// equals `@selected`; `accent` matches `@ink`).
pub(crate) const CSS: &str = r#"
.account-setup { background: @canvas; color: @ink; }
.setup-headline { font-size: 20px; font-weight: 600; }
.setup-intro { font-size: 12px; color: @secondary; }
.setup-label { font-size: 12px; font-weight: 500; }
.setup-note { font-size: 11px; color: @secondary; }
.setup-coming { font-size: 12px; color: @secondary; }
.setup-status { font-size: 12px; color: @secondary; }
.setup-status.setup-problem { color: @problem; }
.provider-choice { background: alpha(@ink, 0.05); color: @ink; border-radius: 8px; min-height: 34px; min-width: 76px; padding: 0 12px; font-size: 12px; font-weight: 500; }
.provider-choice.active { background: @selected; font-weight: 600; box-shadow: inset 0 0 0 2px @ink; }
.setup-link { background: transparent; color: @ink; padding: 2px 0; min-height: 0; font-size: 12px; }
.setup-link:hover { text-decoration: underline; background: transparent; }
.setup-step { font-size: 12px; }
.setup-step-number { font-size: 12px; font-weight: 600; font-feature-settings: "tnum"; }
.setup-switch { font-weight: 600; }
.setup-button { border: 1px solid @line; padding: 4px 14px; }
.setup-primary { background: @selected; font-weight: 600; }
.setup-button:disabled { opacity: 0.45; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn development() -> BuildFeatures {
        BuildFeatures {
            channel: "development".into(),
            google_contact_photos: true,
            microsoft_accounts: true,
            google_oauth_public: true,
        }
    }

    fn distribution(google_oauth_public: bool) -> BuildFeatures {
        BuildFeatures {
            channel: "distribution".into(),
            google_contact_photos: false,
            microsoft_accounts: false,
            google_oauth_public,
        }
    }

    fn gmail_form(password: &str) -> Form {
        // Hidden server fields left over from another choice must not travel.
        Form {
            email: " person@gmail.example ".into(),
            password: password.into(),
            imap_host: "imap.example.com".into(),
            imap_port: "143".into(),
            smtp_host: "smtp.example.com".into(),
            smtp_port: "25".into(),
            ..Form::default()
        }
    }

    #[test]
    fn official_release_offers_gmail_only_through_app_passwords() {
        let titles = |features: &BuildFeatures| {
            available(features)
                .iter()
                .map(|c| c.title(features))
                .collect::<Vec<_>>()
        };
        // Development builds keep Google sign-in exactly as before.
        assert_eq!(
            titles(&development()),
            [
                "Gmail",
                "Gmail (app password)",
                "iCloud",
                "Outlook",
                "Other"
            ]
        );
        assert_eq!(initial(&development(), true), Choice::Gmail);
        assert_eq!(
            initial(&development(), false),
            Choice::GmailAppPassword,
            "no Google client configured"
        );
        // Official release before Google's review passes (the default).
        let before = distribution(false);
        assert_eq!(
            available(&before),
            [Choice::GmailAppPassword, Choice::Icloud, Choice::Other]
        );
        assert_eq!(titles(&before), ["Gmail", "iCloud", "Other"]);
        assert_eq!(
            initial(&before, true),
            Choice::GmailAppPassword,
            "even with a client configured"
        );
        assert_eq!(
            intro(&before),
            "Gmail and iCloud connect with a password created for this app. Never enter your main account password here."
        );
        assert!(gmail_unavailable(&before).ends_with("wait for one-click Google sign-in."));
        // After maintainers set GOOGLE_OAUTH_PUBLIC=true in the official profile.
        let after = distribution(true);
        assert_eq!(
            titles(&after),
            ["Gmail", "Gmail (app password)", "iCloud", "Other"]
        );
        assert!(!intro(&after).contains("Outlook"));
        assert!(
            gmail_unavailable(&after)
                .ends_with("Use Gmail sign-in instead, or ask your administrator.")
        );
        assert_eq!(
            BuildFeatures::from_response(None),
            BuildFeatures::restricted()
        );
        assert!(!BuildFeatures::restricted().google_oauth_public);
    }

    #[test]
    fn steps_link_directly_to_google_pages() {
        let links: Vec<_> = GMAIL_STEPS
            .iter()
            .filter_map(|(_, target)| target.map(|(_, url)| url))
            .collect();
        assert_eq!(
            links,
            [
                "https://myaccount.google.com/signinoptions/twosv",
                "https://myaccount.google.com/apppasswords"
            ]
        );
        assert!(
            GMAIL_ABOUT.contains("revoke")
                && gmail_unavailable(&development()).contains("security keys")
        );
        assert_eq!(
            GMAIL_SIGN_IN_COMING,
            "One-click Google sign-in is coming once Google finishes reviewing the app."
        );
    }

    #[test]
    fn gmail_app_password_payload_matches_the_macos_request() {
        let payload = imap_payload(
            Choice::GmailAppPassword,
            &gmail_form("abcd efgh ijkl mnop"),
            false,
        )
        .unwrap();
        assert_eq!(
            payload,
            json!({
                "op": "add_imap_account", "provider": "imap", "preset": "gmail",
                "email": "person@gmail.example", "display_name": "person@gmail.example",
                "username": "person@gmail.example", "smtp_username": "person@gmail.example",
                "password": "abcd efgh ijkl mnop", "google_sign_in": false,
            })
        );
        assert_eq!(
            imap_payload(Choice::GmailAppPassword, &gmail_form("  "), false).unwrap_err(),
            "Enter your Gmail address and the 16-letter app password from step 2."
        );
    }

    #[test]
    fn icloud_payload_keeps_its_existing_login_rules() {
        let form = Form {
            email: "person@icloud.com".into(),
            password: "app-specific".into(),
            imap_host: "imap.mail.me.com".into(),
            imap_port: "993".into(),
            imap_tls: true,
            smtp_host: "smtp.mail.me.com".into(),
            smtp_port: "587".into(),
            smtp_tls: false,
            ..Form::default()
        };
        let payload = imap_payload(Choice::Icloud, &form, true).unwrap();
        assert_eq!(payload["provider"], "icloud");
        assert_eq!(payload["username"], "person");
        // A username typed under Other (hidden for iCloud) never travels.
        let stale = Form {
            username: "bob@example.com".into(),
            ..form.clone()
        };
        assert_eq!(
            imap_payload(Choice::Icloud, &stale, true).unwrap()["username"],
            "person"
        );
        assert_eq!(payload["smtp_security"], "start_tls");
        assert!(payload.get("smtp_username").is_none());
        assert!(payload.get("preset").is_none());
    }

    #[test]
    fn other_payload_ignores_spaces_around_servers_and_ports() {
        // Same rule as macOS (`AccountSetupWindow.imapPayload`).
        let form = Form {
            email: "person@example.com".into(),
            password: "secret".into(),
            imap_host: " imap.example.com ".into(),
            imap_port: " 993 ".into(),
            imap_tls: true,
            smtp_host: "smtp.example.com".into(),
            smtp_port: "587 ".into(),
            ..Form::default()
        };
        let payload = imap_payload(Choice::Other, &form, false).unwrap();
        assert_eq!(payload["imap_host"], "imap.example.com");
        assert_eq!(payload["imap_port"], 993);
        assert_eq!(payload["smtp_port"], 587);
        let blank = Form {
            imap_host: "   ".into(),
            ..form
        };
        assert!(imap_payload(Choice::Other, &blank, false).is_err());
    }
}
