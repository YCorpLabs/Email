//! Account setup, mirroring macOS `AccountSetupWindow.swift`: email first, the
//! same choices, order, copy, one-line errors with a fix link and Details, and
//! keyboard behavior (Ctrl+digit replaces ⌘+digit; Escape cancels; Enter
//! continues or connects).
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
    /// Off in official releases: account setup then offers Gmail only through
    /// an app password. Connected accounts keep working.
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

/// Display order. Official releases do not offer Google sign-in
/// (`google_oauth_public` is off); Gmail then goes straight to the
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

/// The Gmail choice for a Google address.
pub(crate) fn initial(features: &BuildFeatures, google_sign_in_configured: bool) -> Choice {
    if features.google_oauth_public && google_sign_in_configured {
        Choice::Gmail
    } else {
        Choice::GmailAppPassword
    }
}

/// The choice for the engine's `setup_provider` answer (macOS
/// `AccountSetupChoice.forProvider`). Everything not clearly Google or Apple
/// goes to Other.
pub(crate) fn for_provider(
    provider: &str,
    features: &BuildFeatures,
    google_sign_in_configured: bool,
) -> Choice {
    match provider {
        "gmail" => initial(features, google_sign_in_configured),
        "icloud" => Choice::Icloud,
        "outlook" if features.microsoft_accounts => Choice::Outlook,
        _ => Choice::Other,
    }
}

/// The steps, field and short explanation for a provider that needs a
/// password made for Email (macOS `AppPasswordGuide`).
pub(crate) struct Guide {
    pub intro: &'static str,
    pub steps: [(&'static str, Option<&'static str>); 3],
    pub field_label: &'static str,
    pub placeholder: &'static str,
    pub disclosure: &'static str,
    pub about: [&'static str; 4],
}

pub(crate) const TITLE: &str = "Add an account";
const CHOOSE_PROVIDER: &str = "Choose provider";
const GUIDE_LINK: &str = "Need help? Step-by-step guide ↗";
const ENTER_EMAIL: &str = "Enter your email address.";
/// The first screen's Google sign-in button and the divider under it (macOS
/// `GoogleSignInButton`, `AccountSetupCopy.or`).
pub(crate) const CONTINUE_WITH_GOOGLE: &str = "Continue with Google";
const OR: &str = "or";

/// macOS `AccountSetupCopy.gmail`; the credential store is the only difference.
pub(crate) const GMAIL: Guide = Guide {
    intro: "Gmail needs an app password.",
    steps: [
        (
            "Turn on 2-Step Verification ↗",
            Some("https://myaccount.google.com/signinoptions/twosv"),
        ),
        (
            "Create an app password ↗",
            Some("https://myaccount.google.com/apppasswords"),
        ),
        ("Paste it below", None),
    ],
    field_label: "App password",
    placeholder: "16 letters, spaces are fine",
    disclosure: "What's an app password?",
    about: [
        "It's a 16-letter code just for Email, stored in your system keyring (Secret Service).",
        "You can revoke it in your Google Account. Changing your Google password revokes it too.",
        "No App passwords option? Google hides it for accounts that use only security keys, Advanced Protection, and some work or school accounts.",
        "Email connects to imap.gmail.com and smtp.gmail.com.",
    ],
};

/// macOS `AccountSetupCopy.icloud`.
pub(crate) const ICLOUD: Guide = Guide {
    intro: "iCloud needs an app-specific password.",
    steps: [
        (
            "Sign in to your Apple Account ↗",
            Some("https://account.apple.com/"),
        ),
        ("Create an app-specific password", None),
        ("Paste it below", None),
    ],
    field_label: "App password",
    placeholder: "abcd-efgh-ijkl-mnop",
    disclosure: "What's an app-specific password?",
    about: [
        "It's a password Apple makes for one app, here Email, stored in your system keyring (Secret Service).",
        "Create it under Sign-In and Security. Your Apple Account needs two-factor authentication.",
        "You can revoke it at account.apple.com. Changing your Apple Account password revokes it too.",
        "Email connects to imap.mail.me.com and smtp.mail.me.com.",
    ],
};

/// Other: no steps, one line; the engine finds the servers.
pub(crate) const OTHER_INTRO: &str = "Use your email password or an app password.";

fn guide(choice: Option<Choice>) -> Option<&'static Guide> {
    match choice {
        Some(Choice::GmailAppPassword) => Some(&GMAIL),
        Some(Choice::Icloud) => Some(&ICLOUD),
        _ => None,
    }
}

fn guide_section(choice: Option<Choice>) -> Option<&'static str> {
    match choice {
        Some(Choice::GmailAppPassword | Choice::Gmail) => Some("gmail"),
        Some(Choice::Icloud) => Some("icloud"),
        Some(Choice::Other) => Some("other"),
        _ => None,
    }
}

/// The website guide for a section, only in builds whose support channel is
/// Y Corp's own (`mail_service::profile::setup_guide_url`).
pub(crate) fn guide_url(section: &str) -> Option<String> {
    mail_service::profile::setup_guide_url().map(|base| format!("{base}#{section}"))
}

fn oauth_intro(choice: Choice) -> &'static str {
    if choice == Choice::Outlook {
        "You'll sign in to Microsoft in your browser."
    } else {
        "You'll sign in to Google in your browser."
    }
}

/// Server settings the engine guessed (`servers` in `setup_error`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Servers {
    pub imap_host: String,
    pub imap_port: String,
    pub imap_tls: bool,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_tls: bool,
}

/// An account-setup failure as the engine explains it (macOS `SetupProblem`):
/// one line, an optional fix link, the server's words for Details.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Problem {
    pub code: String,
    pub message: String,
    pub action: Option<(String, String)>,
    pub detail: String,
    pub servers: Option<Servers>,
}

impl Problem {
    fn plain(message: &str) -> Self {
        Self {
            code: "invalid".into(),
            message: message.into(),
            ..Self::default()
        }
    }

    /// From a failed envelope (`request_envelope`).
    pub(crate) fn from_envelope(envelope: &Value) -> Self {
        let message = envelope["error"]
            .as_str()
            .unwrap_or("The mail action failed.")
            .to_string();
        let setup = &envelope["setup_error"];
        if !setup.is_object() {
            return Self {
                code: "other".into(),
                message,
                ..Self::default()
            };
        }
        let action = setup["action"]["url"]
            .as_str()
            // Only web pages: a fix link never opens anything else.
            .filter(|url| url.starts_with("https://"))
            .zip(setup["action"]["label"].as_str())
            .map(|(url, label)| (label.to_string(), url.to_string()));
        let servers = &setup["servers"];
        let servers = servers["imap_host"].as_str().zip(servers["smtp_host"].as_str()).map(
            |(imap, smtp)| Servers {
                imap_host: imap.into(),
                imap_port: servers["imap_port"].as_u64().unwrap_or(993).to_string(),
                imap_tls: servers["imap_security"] != "start_tls",
                smtp_host: smtp.into(),
                smtp_port: servers["smtp_port"].as_u64().unwrap_or(587).to_string(),
                smtp_tls: servers["smtp_security"] == "implicit_tls",
            },
        );
        Self {
            code: setup["code"].as_str().unwrap_or("other").into(),
            message: setup["message"].as_str().map_or(message, str::to_string),
            action,
            detail: setup["detail"].as_str().unwrap_or_default().into(),
            servers,
        }
    }
}

/// The values typed into the password-based form.
#[derive(Clone, Debug, Default)]
pub(crate) struct Form {
    pub display_name: String,
    pub email: String,
    pub username: String,
    pub smtp_username: String,
    pub password: String,
    /// Other only: the server rows are shown, so their values are sent.
    pub servers_shown: bool,
    pub imap_host: String,
    pub imap_port: String,
    pub imap_tls: bool,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_tls: bool,
}

/// The `add_imap_account` request, identical to the macOS payload
/// (`AccountSetupWindow.imapPayload`). Gmail and iCloud name no servers; Other
/// names none until its server rows are shown, and the engine guesses them.
pub(crate) fn imap_payload(choice: Choice, form: &Form) -> Result<Value, &'static str> {
    let address = form.email.trim().to_string();
    let has_password = !form.password.trim().is_empty();
    let display_name = if form.display_name.is_empty() {
        address.clone()
    } else {
        form.display_name.clone()
    };
    match choice {
        Choice::GmailAppPassword => {
            if !address.contains('@') || !has_password {
                return Err(invalid_message(choice));
            }
            return Ok(json!({
                "op": "add_imap_account", "provider": "imap", "preset": "gmail",
                "email": address, "display_name": address, "username": address,
                "smtp_username": address, "password": form.password,
            }));
        }
        Choice::Icloud => {
            if !address.contains('@') || !has_password {
                return Err(invalid_message(choice));
            }
            // The engine's iCloud preset owns the servers and username.
            return Ok(json!({
                "op": "add_imap_account", "provider": "icloud",
                "email": address, "display_name": address, "password": form.password,
            }));
        }
        _ if !form.servers_shown => {
            if !address.contains('@') || !has_password {
                return Err("Enter your email address and password.");
            }
            return Ok(json!({
                "op": "add_imap_account", "provider": "imap",
                "email": address, "display_name": display_name, "password": form.password,
            }));
        }
        _ => {}
    }
    let typed_login = form.username.trim().to_string();
    let login = if typed_login.is_empty() {
        address.clone()
    } else {
        typed_login
    };
    let port = |value: &str| value.trim().parse::<u16>().ok().filter(|p| *p > 0);
    let (Some(imap_port), Some(smtp_port)) = (port(&form.imap_port), port(&form.smtp_port)) else {
        return Err(invalid_message(choice));
    };
    if !address.contains('@')
        || !has_password
        || form.imap_host.trim().is_empty()
        || form.smtp_host.trim().is_empty()
    {
        return Err(invalid_message(choice));
    }
    let security = |tls: bool| if tls { "implicit_tls" } else { "start_tls" };
    let smtp_login = form.smtp_username.trim();
    Ok(json!({
        "op": "add_imap_account", "provider": "imap",
        "email": address, "display_name": display_name,
        "username": login,
        "smtp_username": if smtp_login.is_empty() { login.as_str() } else { smtp_login },
        "password": form.password,
        "imap_host": form.imap_host.trim(), "imap_port": imap_port, "imap_security": security(form.imap_tls),
        "smtp_host": form.smtp_host.trim(), "smtp_port": smtp_port, "smtp_security": security(form.smtp_tls),
    }))
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
    features: BuildFeatures,
    choices: Vec<Choice>,
    /// `None` until the address is entered or a provider chosen.
    selected: Cell<Option<Choice>>,
    picked_by_hand: Cell<bool>,
    email: gtk::Entry,
    /// The Email row and provider menu, hidden while a browser sign-in waits.
    email_group: gtk::Box,
    provider_menu: gtk::MenuButton,
    provider_items: Vec<(Choice, gtk::Button)>,
    intro: gtk::Label,
    steps: gtk::Box,
    password_row: gtk::Box,
    password_label: gtk::Label,
    password: gtk::PasswordEntry,
    about: gtk::Expander,
    about_lines: gtk::Box,
    advanced: gtk::ToggleButton,
    advanced_icon: gtk::Image,
    advanced_rows: gtk::Box,
    display_name: gtk::Entry,
    username: gtk::Entry,
    smtp_username: gtk::Entry,
    imap_host: gtk::Entry,
    imap_port: gtk::Entry,
    imap_security: gtk::DropDown,
    smtp_host: gtk::Entry,
    smtp_port: gtk::Entry,
    smtp_security: gtk::DropDown,
    guide: gtk::Button,
    /// Status and errors, under the password field.
    problem_row: gtk::Box,
    status: gtk::Label,
    fix: gtk::Button,
    fix_url: RefCell<Option<String>>,
    details: gtk::Expander,
    details_text: gtk::Label,
    switch_to_app_password: gtk::Button,
    connect: gtk::Button,
    /// Other: Advanced is offered once a first attempt failed.
    advanced_offered: Cell<bool>,
    /// Choices with a request still running. Another choice can be used
    /// meanwhile; the same one cannot be submitted twice.
    in_flight: RefCell<HashSet<Choice>>,
    /// How a request ended while another choice was on screen. Shown when
    /// that choice is selected again.
    outcomes: RefCell<HashMap<Choice, (String, Option<Problem>)>>,
    sign_in_failed: Cell<bool>,
    /// "Continue with Google" and the "or" divider, on the first screen only.
    google_rows: Vec<gtk::Widget>,
}

const LABEL_WIDTH: i32 = 104;
const FIELD_WIDTH: i32 = 340;

fn row(title: &str, field: &impl IsA<gtk::Widget>) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(Orientation::Horizontal, 12);
    let label = gtk::Label::new(Some(title));
    label.set_xalign(1.);
    label.set_size_request(LABEL_WIDTH, -1);
    label.add_css_class("setup-label");
    field.as_ref().set_size_request(FIELD_WIDTH, -1);
    row.append(&label);
    row.append(field);
    (row, label)
}

/// Lines a widget up with the fields.
fn indented(widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(Orientation::Horizontal, 0);
    row.set_margin_start(LABEL_WIDTH + 12);
    row.append(widget);
    row
}

fn link(title: &str, url: &'static str) -> gtk::Button {
    let button = gtk::Button::with_label(title);
    button.add_css_class("setup-link");
    button.set_tooltip_text(Some(url));
    button.set_halign(gtk::Align::Start);
    button.connect_clicked(move |_| open_url(url));
    button
}

fn open_url(url: &str) {
    if let Some(display) = gdk::Display::default() {
        let _ = gtk::gio::AppInfo::launch_default_for_uri(url, Some(&display.app_launch_context()));
    }
}

fn wrapped(text: &str, class: &str, width: i32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.);
    label.set_max_width_chars(60);
    label.set_size_request(width, -1);
    label.add_css_class(class);
    label
}

/// Google's standard "G" mark from the sign-in branding kit (48×48 artboard),
/// the same paths macOS draws (`GoogleLogo`).
const GOOGLE_LOGO: [(&str, (f64, f64, f64)); 4] = [
    ("M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z", (234., 67., 53.)),
    ("M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z", (66., 133., 244.)),
    ("M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z", (251., 188., 5.)),
    ("M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z", (52., 168., 83.)),
];

/// One drawing step of an SVG path.
#[derive(Debug, PartialEq)]
enum PathStep {
    Move(f64, f64),
    Line(f64, f64),
    Curve(f64, f64, f64, f64, f64, f64),
    Close,
}

/// The SVG path commands the mark uses (M L H V C S Z, absolute and
/// relative), as absolute steps. Stops at anything else.
fn svg_path(data: &str) -> Vec<PathStep> {
    let bytes = data.as_bytes();
    let mut at = 0;
    let skip = |at: &mut usize| {
        while *at < bytes.len() && matches!(bytes[*at], b' ' | b',') {
            *at += 1;
        }
    };
    let number = |at: &mut usize| -> Option<f64> {
        skip(at);
        let start = *at;
        if *at < bytes.len() && matches!(bytes[*at], b'-' | b'+') {
            *at += 1;
        }
        let mut dot = false;
        while *at < bytes.len() && (bytes[*at].is_ascii_digit() || (bytes[*at] == b'.' && !dot)) {
            dot |= bytes[*at] == b'.';
            *at += 1;
        }
        data[start..*at].parse().ok()
    };
    let (mut steps, mut command) = (Vec::new(), b'M');
    let (mut x, mut y, mut start, mut control) = (0., 0., (0., 0.), None::<(f64, f64)>);
    loop {
        skip(&mut at);
        if at >= bytes.len() {
            return steps;
        }
        let before = at;
        if bytes[at].is_ascii_alphabetic() {
            command = bytes[at];
            at += 1;
        }
        let (ox, oy) = if command.is_ascii_lowercase() { (x, y) } else { (0., 0.) };
        let step = match command.to_ascii_uppercase() {
            b'M' => number(&mut at).zip(number(&mut at)).map(|(a, b)| {
                (x, y) = (ox + a, oy + b);
                start = (x, y);
                command = if command == b'm' { b'l' } else { b'L' };
                control = None;
                PathStep::Move(x, y)
            }),
            b'L' => number(&mut at).zip(number(&mut at)).map(|(a, b)| {
                (x, y) = (ox + a, oy + b);
                control = None;
                PathStep::Line(x, y)
            }),
            b'H' => number(&mut at).map(|a| {
                x = ox + a;
                control = None;
                PathStep::Line(x, y)
            }),
            b'V' => number(&mut at).map(|b| {
                y = oy + b;
                control = None;
                PathStep::Line(x, y)
            }),
            b'C' => (0..6).map(|_| number(&mut at)).collect::<Option<Vec<_>>>().map(|n| {
                let step = PathStep::Curve(ox + n[0], oy + n[1], ox + n[2], oy + n[3], ox + n[4], oy + n[5]);
                control = Some((ox + n[2], oy + n[3]));
                (x, y) = (ox + n[4], oy + n[5]);
                step
            }),
            b'S' => (0..4).map(|_| number(&mut at)).collect::<Option<Vec<_>>>().map(|n| {
                let (cx, cy) = control.map_or((x, y), |(cx, cy)| (2. * x - cx, 2. * y - cy));
                let step = PathStep::Curve(cx, cy, ox + n[0], oy + n[1], ox + n[2], oy + n[3]);
                control = Some((ox + n[0], oy + n[1]));
                (x, y) = (ox + n[2], oy + n[3]);
                step
            }),
            b'Z' => {
                (x, y) = start;
                control = None;
                Some(PathStep::Close)
            }
            _ => None,
        };
        match step {
            Some(step) if at > before => steps.push(step),
            _ => return steps,
        }
    }
}

fn draw_google_logo(cr: &gtk::cairo::Context, size: f64) {
    let scale = size / 48.;
    for (data, (r, g, b)) in GOOGLE_LOGO {
        cr.new_path();
        for step in svg_path(data) {
            match step {
                PathStep::Move(x, y) => cr.move_to(x * scale, y * scale),
                PathStep::Line(x, y) => cr.line_to(x * scale, y * scale),
                PathStep::Curve(a, b, c, d, x, y) => {
                    cr.curve_to(a * scale, b * scale, c * scale, d * scale, x * scale, y * scale)
                }
                PathStep::Close => cr.close_path(),
            }
        }
        cr.set_source_rgb(r / 255., g / 255., b / 255.);
        let _ = cr.fill();
    }
}

/// "Continue with Google" per Google's sign-in button branding guidelines:
/// the standard "G" at 18 px, 12 px before it, the text centred, 40 px tall,
/// a 4 px radius, and Google's light or dark colours from the app theme
/// (`@google_fill`, `@google_stroke`, `@google_ink` in `appearance_css`).
fn google_button() -> gtk::Button {
    let logo = gtk::DrawingArea::new();
    logo.set_content_width(18);
    logo.set_content_height(18);
    logo.set_valign(gtk::Align::Center);
    logo.set_draw_func(|_, cr, width, _| draw_google_logo(cr, f64::from(width)));
    let label = gtk::Label::new(Some(CONTINUE_WITH_GOOGLE));
    // The text is centred in the button, as on macOS, never under the logo.
    let balance = gtk::Box::new(Orientation::Horizontal, 0);
    balance.set_size_request(18, -1);
    let content = gtk::CenterBox::new();
    content.set_start_widget(Some(&logo));
    content.set_center_widget(Some(&label));
    content.set_end_widget(Some(&balance));
    let button = gtk::Button::new();
    button.set_child(Some(&content));
    button.add_css_class("setup-google");
    button.set_size_request(FIELD_WIDTH, -1);
    button.set_tooltip_text(Some(CONTINUE_WITH_GOOGLE));
    button
}

/// A hairline, "or", a hairline, as wide as the fields.
fn or_divider() -> gtk::Box {
    let divider = gtk::Box::new(Orientation::Horizontal, 10);
    divider.set_size_request(FIELD_WIDTH, -1);
    for index in 0..3 {
        if index == 1 {
            let or = gtk::Label::new(Some(OR));
            or.add_css_class("setup-or");
            divider.append(&or);
        } else {
            let line = gtk::Separator::new(Orientation::Horizontal);
            line.add_css_class("setup-or-line");
            line.set_hexpand(true);
            line.set_valign(gtk::Align::Center);
            divider.append(&line);
        }
    }
    divider
}

/// Open the account window, transient for the mail window.
pub(crate) fn present(ui: &Rc<Ui>) {
    let features = build_features(&ui.service);
    let choices = available(&features);
    let window = gtk::Window::builder()
        .transient_for(&ui.window)
        .modal(true)
        .title(TITLE)
        .default_width(520)
        .resizable(false)
        .build();
    window.add_css_class("account-setup");
    // One title: the heading inside the window (macOS hides its title text).
    let header = gtk::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::new(None)));
    header.add_css_class("setup-header");
    window.set_titlebar(Some(&header));
    let content = gtk::Box::new(Orientation::Vertical, 12);
    content.set_margin_top(8);
    content.set_margin_bottom(24);
    content.set_margin_start(30);
    content.set_margin_end(30);
    let headline = gtk::Label::new(Some(TITLE));
    headline.set_xalign(0.);
    headline.add_css_class("setup-headline");
    headline.set_margin_bottom(6);
    content.append(&headline);

    // Google sign-in first where the build offers it, then email for the rest.
    let google_offered = choices.contains(&Choice::Gmail) && oauth_client_id(Choice::Gmail).is_some();
    let google = google_button();
    let mut google_rows: Vec<gtk::Widget> = Vec::new();
    if google_offered {
        let button_row = indented(&google);
        button_row.set_margin_top(4);
        let divider_row = indented(&or_divider());
        divider_row.set_margin_top(2);
        divider_row.set_margin_bottom(2);
        content.append(&button_row);
        content.append(&divider_row);
        google_rows = vec![button_row.upcast(), divider_row.upcast()];
    }

    let entry = |placeholder: &str| {
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(placeholder));
        entry.set_activates_default(true);
        entry
    };
    let email = entry("you@example.com");
    email.set_input_purpose(gtk::InputPurpose::Email);
    // The email field and the small provider menu under it sit close together.
    let email_group = gtk::Box::new(Orientation::Vertical, 2);
    email_group.append(&row("Email", &email).0);
    content.append(&email_group);

    // Small "Choose provider" control for custom domains on Google or Apple.
    let provider_menu = gtk::MenuButton::new();
    provider_menu.set_label(CHOOSE_PROVIDER);
    provider_menu.add_css_class("setup-provider-menu");
    provider_menu.set_tooltip_text(Some(&format!(
        "Choose your provider by hand (Ctrl+1–Ctrl+{})",
        choices.len()
    )));
    let menu_box = gtk::Box::new(Orientation::Vertical, 2);
    let provider_items: Vec<(Choice, gtk::Button)> = choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let item = gtk::Box::new(Orientation::Horizontal, 24);
            let name = gtk::Label::new(Some(choice.title(&features)));
            name.set_hexpand(true);
            name.set_xalign(0.);
            let key = gtk::Label::new(Some(&format!("Ctrl+{}", index + 1)));
            key.add_css_class("setup-menu-key");
            item.append(&name);
            item.append(&key);
            let button = gtk::Button::new();
            button.set_child(Some(&item));
            button.add_css_class("setup-menu-item");
            menu_box.append(&button);
            (*choice, button)
        })
        .collect();
    let popover = gtk::Popover::new();
    popover.set_child(Some(&menu_box));
    popover.set_has_arrow(false);
    provider_menu.set_popover(Some(&popover));
    email_group.append(&indented(&provider_menu));

    let intro = wrapped("", "setup-intro", 460);
    intro.set_margin_top(4);
    content.append(&intro);
    let steps = gtk::Box::new(Orientation::Vertical, 2);
    content.append(&steps);

    let password = gtk::PasswordEntry::new();
    password.set_activates_default(true);
    let (password_row, password_label) = row("App password", &password);
    content.append(&password_row);

    // Status and errors sit right under the field they are about (macOS
    // `problemRow`): one line, the fix link, the server's words behind Details.
    let status = wrapped("", "setup-status", FIELD_WIDTH);
    status.set_selectable(true);
    let fix = gtk::Button::with_label("");
    fix.add_css_class("setup-link");
    fix.add_css_class("setup-fix");
    fix.set_halign(gtk::Align::Start);
    let details_text = wrapped("", "setup-details", FIELD_WIDTH);
    details_text.set_selectable(true);
    let details = gtk::Expander::new(Some("Details"));
    details.add_css_class("setup-disclosure");
    details.set_child(Some(&details_text));
    let problem_box = gtk::Box::new(Orientation::Vertical, 3);
    problem_box.append(&status);
    problem_box.append(&fix);
    problem_box.append(&details);
    let problem_row = indented(&problem_box);
    content.append(&problem_row);

    let about_lines = gtk::Box::new(Orientation::Vertical, 4);
    let about = gtk::Expander::new(None);
    about.add_css_class("setup-disclosure");
    about.set_child(Some(&about_lines));
    content.append(&indented(&about));

    let display_name = entry("Optional");
    let username = entry("Usually your email address");
    let smtp_username = entry("Same as the mail username");
    let imap_host = entry("imap.example.com");
    let imap_port = entry("993");
    let smtp_host = entry("smtp.example.com");
    let smtp_port = entry("587");
    let imap_security = gtk::DropDown::from_strings(&["TLS", "STARTTLS"]);
    let smtp_security = gtk::DropDown::from_strings(&["STARTTLS", "TLS"]);
    let advanced_rows = gtk::Box::new(Orientation::Vertical, 8);
    for (title, widget) in [
        ("IMAP server", imap_host.clone().upcast::<gtk::Widget>()),
        ("IMAP port", imap_port.clone().upcast()),
        ("IMAP security", imap_security.clone().upcast()),
        ("SMTP server", smtp_host.clone().upcast()),
        ("SMTP port", smtp_port.clone().upcast()),
        ("SMTP security", smtp_security.clone().upcast()),
        ("Mail username", username.clone().upcast()),
        ("SMTP username", smtp_username.clone().upcast()),
        ("Display name", display_name.clone().upcast()),
    ] {
        advanced_rows.append(&row(title, &widget).0);
    }
    // Advanced: a disclosure above rows that line up with the other fields.
    let advanced_icon = gtk::Image::from_icon_name("pan-end-symbolic");
    let advanced_title = gtk::Box::new(Orientation::Horizontal, 4);
    advanced_title.append(&advanced_icon);
    advanced_title.append(&gtk::Label::new(Some("Advanced")));
    let advanced = gtk::ToggleButton::new();
    advanced.set_child(Some(&advanced_title));
    advanced.add_css_class("setup-toggle");
    content.append(&indented(&advanced));
    content.append(&advanced_rows);

    let guide_button = gtk::Button::with_label(GUIDE_LINK);
    guide_button.add_css_class("setup-link");
    guide_button.add_css_class("setup-guide");
    content.append(&indented(&guide_button));

    let switch_to_app_password = gtk::Button::with_label("Use Gmail (app password) instead →");
    switch_to_app_password.add_css_class("setup-link");
    switch_to_app_password.add_css_class("setup-switch");
    switch_to_app_password.set_halign(gtk::Align::Start);
    content.append(&switch_to_app_password);
    // Dialog buttons at the bottom right; Enter on the default button.
    let actions = gtk::Box::new(Orientation::Horizontal, 10);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(8);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("setup-button");
    let connect = gtk::Button::with_label("Continue");
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
        features,
        choices,
        selected: Cell::new(None),
        picked_by_hand: Cell::new(false),
        email,
        email_group,
        provider_menu,
        provider_items,
        intro,
        steps,
        password_row,
        password_label,
        password,
        about,
        about_lines,
        advanced,
        advanced_icon,
        advanced_rows,
        display_name,
        username,
        smtp_username,
        imap_host,
        imap_port,
        imap_security,
        smtp_host,
        smtp_port,
        smtp_security,
        guide: guide_button,
        problem_row,
        status,
        fix,
        fix_url: RefCell::new(None),
        details,
        details_text,
        switch_to_app_password,
        connect,
        advanced_offered: Cell::new(false),
        in_flight: RefCell::new(HashSet::new()),
        outcomes: RefCell::new(HashMap::new()),
        sign_in_failed: Cell::new(false),
        google_rows,
    });
    {
        // Straight to the browser: no address needed, Google supplies it.
        let setup = setup.clone();
        google.connect_clicked(move |_| {
            setup.choose(Choice::Gmail, true);
            setup.connect_clicked();
        });
    }
    for (choice, button) in &setup.provider_items {
        let setup = setup.clone();
        let choice = *choice;
        button.connect_clicked(move |_| {
            if let Some(popover) = setup.provider_menu.popover() {
                popover.popdown();
            }
            setup.choose(choice, true);
        });
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
        setup.guide.clone().connect_clicked(move |_| {
            if let Some(url) = guide_section(setup.selected.get()).and_then(guide_url) {
                open_url(&url);
            }
        });
    }
    {
        let setup = setup.clone();
        setup.fix.clone().connect_clicked(move |_| {
            if let Some(url) = setup.fix_url.borrow().as_deref() {
                open_url(url);
            }
        });
    }
    {
        let setup = setup.clone();
        setup
            .switch_to_app_password
            .clone()
            .connect_clicked(move |_| setup.choose(Choice::GmailAppPassword, true));
    }
    {
        let setup = setup.clone();
        setup.advanced.clone().connect_toggled(move |_| setup.show_servers());
    }
    {
        // Follow the address while the person hasn't chosen a provider by hand.
        let setup = setup.clone();
        setup.email.clone().connect_changed(move |entry| {
            if setup.selected.get().is_none() || setup.picked_by_hand.get() {
                return;
            }
            let address = entry.text().trim().to_string();
            if !address.contains('@') {
                return;
            }
            let next = setup.provider(&address);
            if Some(next) != setup.selected.get() {
                setup.selected.set(Some(next));
                setup.advanced_offered.set(false);
                setup.show_choice();
            }
        });
    }
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let setup = setup.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::Escape {
                // An open list (security menus, provider menu) closes itself first, as on macOS.
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
                    setup.choose(*choice, true);
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
    }
    window.add_controller(keys);
    setup.show_choice();
    window.present();
    setup.email.grab_focus();
}

impl Setup {
    /// The provider for the typed address, asked of the engine (no network).
    fn provider(&self, address: &str) -> Choice {
        let answer = self.ui.upgrade().and_then(|ui| {
            ui.service
                .request_json(&json!({"op": "setup_provider", "email": address}).to_string())
                .ok()
        });
        let provider = answer
            .as_ref()
            .and_then(|value| value["provider"].as_str())
            .unwrap_or("other");
        for_provider(provider, &self.features, oauth_client_id(Choice::Gmail).is_some())
    }

    fn choose(&self, choice: Choice, by_hand: bool) {
        let next = if self.choices.contains(&choice) {
            choice
        } else {
            *self.choices.last().unwrap_or(&Choice::Other)
        };
        if by_hand {
            self.picked_by_hand.set(true);
        }
        if Some(next) != self.selected.get() {
            self.selected.set(Some(next));
            self.password.set_text("");
            self.sign_in_failed.set(false);
            self.advanced_offered.set(false);
            self.advanced.set_active(false);
            self.show_choice();
        }
        let outcome = self.outcomes.borrow_mut().remove(&next);
        match outcome {
            Some((_, Some(problem))) => {
                self.sign_in_failed.set(next == Choice::Gmail);
                self.show_problem(&problem);
            }
            Some((message, None)) => self.show_status(&message),
            None => {}
        }
        if next.uses_password() {
            if self.email.text().contains('@') {
                self.password.grab_focus();
            } else {
                self.email.grab_focus();
            }
        } else {
            self.connect.grab_focus();
        }
    }

    /// Show what the current choice needs; nothing else (macOS `showChoice`).
    fn show_choice(&self) {
        let choice = self.selected.get();
        let guide = guide(choice);
        for row in &self.google_rows {
            row.set_visible(choice.is_none());
        }
        for (option, button) in &self.provider_items {
            if Some(*option) == choice {
                button.add_css_class("active");
            } else {
                button.remove_css_class("active");
            }
        }
        self.intro.set_visible(choice.is_some());
        self.intro.set_text(match (guide, choice) {
            (Some(guide), _) => guide.intro,
            (None, Some(Choice::Other)) => OTHER_INTRO,
            (None, Some(choice)) => oauth_intro(choice),
            (None, None) => "",
        });
        while let Some(child) = self.steps.first_child() {
            self.steps.remove(&child);
        }
        for (index, (text, url)) in guide.map(|g| g.steps).unwrap_or_default().iter().enumerate() {
            let line = gtk::Box::new(Orientation::Horizontal, 6);
            let number = gtk::Label::new(Some(&format!("{}.", index + 1)));
            number.add_css_class("setup-step-number");
            number.set_size_request(18, -1);
            number.set_xalign(1.);
            line.append(&number);
            match url {
                Some(url) => line.append(&link(text, url)),
                None => {
                    let label = gtk::Label::new(Some(text));
                    label.add_css_class("setup-step");
                    line.append(&label);
                }
            }
            self.steps.append(&line);
        }
        self.steps.set_visible(guide.is_some());
        let uses_password = choice.is_some_and(Choice::uses_password);
        self.password_row.set_visible(uses_password);
        self.password_label
            .set_text(guide.map_or("Password", |g| g.field_label));
        self.password
            .set_placeholder_text(Some(guide.map_or("", |g| g.placeholder)));
        if let Some(parent) = self.about.parent() {
            parent.set_visible(guide.is_some());
        }
        self.about.set_label(Some(guide.map_or("", |g| g.disclosure)));
        self.about.set_expanded(false);
        while let Some(child) = self.about_lines.first_child() {
            self.about_lines.remove(&child);
        }
        for line in guide.map(|g| g.about).unwrap_or_default() {
            self.about_lines
                .append(&wrapped(line, "setup-note", FIELD_WIDTH));
        }
        let guide_link = guide_section(choice).and_then(guide_url);
        if let Some(parent) = self.guide.parent() {
            parent.set_visible(guide_link.is_some());
        }
        self.guide.set_tooltip_text(guide_link.as_deref());
        self.show_servers();
        match choice {
            None => {
                self.connect.set_label("Continue");
                self.connect.set_sensitive(true);
            }
            Some(choice) => {
                self.connect.set_label(if uses_password {
                    "Add account"
                } else {
                    "Sign in with browser"
                });
                let ready = uses_password || oauth_client_id(choice).is_some();
                self.connect
                    .set_sensitive(ready && !self.in_flight.borrow().contains(&choice));
            }
        }
        self.clear_problem();
        if let Some(choice) = choice {
            if self.in_flight.borrow().contains(&choice) {
                self.show_status("Still connecting…");
            } else if !choice.uses_password() && oauth_client_id(choice).is_none() {
                self.show_status(if choice == Choice::Gmail {
                    "Google sign-in isn't configured in this build. Use Gmail (app password) instead."
                } else {
                    "Sign-in isn't configured in this build yet."
                });
            }
        }
        self.update_switch(false);
        self.update_waiting();
    }

    /// While the browser sign-in waits, only the wait message, the app-password
    /// link and Cancel apply (macOS `updateWaiting`). The address, provider
    /// menu, intro and the sign-in button come back when it fails or is
    /// cancelled in the browser.
    fn update_waiting(&self) {
        let selected = self.selected.get();
        let waiting = selected
            .is_some_and(|choice| !choice.uses_password() && self.in_flight.borrow().contains(&choice));
        self.email_group.set_visible(!waiting);
        self.intro.set_visible(selected.is_some() && !waiting);
        self.connect.set_visible(!waiting);
        // With no fields left, the wait message lines up with the title and link.
        self.problem_row
            .set_margin_start(if waiting { 0 } else { LABEL_WIDTH + 12 });
        self.status
            .set_size_request(if waiting { 460 } else { FIELD_WIDTH }, -1);
    }

    fn clear_problem(&self) {
        self.problem_row.set_visible(false);
        self.status.set_text("");
        self.status.remove_css_class("setup-problem");
        self.status.set_visible(false);
        self.fix.set_visible(false);
        self.fix_url.borrow_mut().take();
        self.details.set_visible(false);
        self.details.set_expanded(false);
        self.details_text.set_text("");
    }

    fn show_status(&self, message: &str) {
        self.clear_problem();
        self.status.set_text(message);
        self.status.set_visible(!message.is_empty());
        self.problem_row.set_visible(!message.is_empty());
    }

    /// One line, a fix link if there is one, and the server's words behind Details.
    fn show_problem(&self, problem: &Problem) {
        self.clear_problem();
        self.problem_row.set_visible(true);
        // The explanation stays closed so the error sits right under the field.
        self.about.set_expanded(false);
        self.status.set_text(&problem.message);
        self.status.add_css_class("setup-problem");
        self.status.set_visible(true);
        if let Some((label, url)) = &problem.action {
            self.fix.set_label(&format!("{label} ↗"));
            self.fix.set_tooltip_text(Some(url));
            *self.fix_url.borrow_mut() = Some(url.clone());
            self.fix.set_visible(true);
        }
        self.details_text.set_text(&problem.detail);
        self.details.set_visible(!problem.detail.is_empty());
        if self.selected.get() == Some(Choice::Other) {
            self.advanced_offered.set(true);
            if let Some(servers) = &problem.servers {
                self.imap_host.set_text(&servers.imap_host);
                self.imap_port.set_text(&servers.imap_port);
                self.imap_security
                    .set_selected(if servers.imap_tls { 0 } else { 1 });
                self.smtp_host.set_text(&servers.smtp_host);
                self.smtp_port.set_text(&servers.smtp_port);
                self.smtp_security
                    .set_selected(if servers.smtp_tls { 1 } else { 0 });
                self.advanced.set_active(true);
            }
            self.show_servers();
        }
        self.update_switch(false);
    }

    /// Other's server rows: offered after a failed attempt, open when the
    /// guess failed or the person opens Advanced.
    fn servers_shown(&self) -> bool {
        self.selected.get() == Some(Choice::Other)
            && self.advanced_offered.get()
            && self.advanced.is_active()
    }

    fn show_servers(&self) {
        let offered = self.selected.get() == Some(Choice::Other) && self.advanced_offered.get();
        if let Some(parent) = self.advanced.parent() {
            parent.set_visible(offered);
        }
        self.advanced_rows.set_visible(self.servers_shown());
        self.advanced_icon.set_icon_name(Some(if self.advanced.is_active() {
            "pan-down-symbolic"
        } else {
            "pan-end-symbolic"
        }));
    }

    /// Offer the app-password path wherever Google sign-in may not work for this person.
    fn update_switch(&self, waiting: bool) {
        let unconfigured = oauth_client_id(Choice::Gmail).is_none();
        self.switch_to_app_password.set_visible(
            self.selected.get() == Some(Choice::Gmail)
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
            servers_shown: self.servers_shown(),
            imap_host: self.imap_host.text().to_string(),
            imap_port: self.imap_port.text().to_string(),
            imap_tls: self.imap_security.selected() == 0,
            smtp_host: self.smtp_host.text().to_string(),
            smtp_port: self.smtp_port.text().to_string(),
            smtp_tls: self.smtp_security.selected() == 1,
        }
    }

    /// A typed gmail.com or googlemail.com address, so Google skips its
    /// account chooser (macOS `googleLoginHint`). Anything else could be
    /// someone else's account.
    fn google_login_hint(&self) -> Option<String> {
        let address = self.email.text().trim().to_string();
        if !address.contains('@') {
            return None;
        }
        let answer = self.ui.upgrade().and_then(|ui| {
            ui.service
                .request_json(&json!({"op": "setup_provider", "email": address}).to_string())
                .ok()
        })?;
        (answer["provider"].as_str() == Some("gmail")).then_some(address)
    }

    fn refresh_connect(&self) {
        let Some(choice) = self.selected.get() else { return };
        let ready = choice.uses_password() || oauth_client_id(choice).is_some();
        self.connect
            .set_sensitive(ready && !self.in_flight.borrow().contains(&choice));
    }

    fn connect_clicked(self: &Rc<Self>) {
        let Some(choice) = self.selected.get() else {
            // Email first: the address decides which steps to show.
            let address = self.email.text().trim().to_string();
            if !address.contains('@') {
                return self.show_problem(&Problem::plain(ENTER_EMAIL));
            }
            let choice = self.provider(&address);
            return self.choose(choice, false);
        };
        if self.in_flight.borrow().contains(&choice) {
            return;
        }
        let payload = if choice.uses_password() {
            match imap_payload(choice, &self.form()) {
                Ok(payload) => payload,
                Err(message) => return self.show_problem(&Problem::plain(message)),
            }
        } else {
            let Some(client_id) = oauth_client_id(choice) else {
                return self.show_problem(&Problem::plain("Sign-in isn't configured in this build yet."));
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
            let login_hint = (choice == Choice::Gmail)
                .then(|| self.google_login_hint())
                .flatten();
            json!({"op": "start_oauth", "provider": provider, "client_id": client_id, "client_secret": secret, "login_hint": login_hint})
        };
        let Some(ui) = self.ui.upgrade() else { return };
        self.in_flight.borrow_mut().insert(choice);
        self.connect.set_sensitive(false);
        self.show_status(match choice {
            Choice::GmailAppPassword => "Connecting to Gmail…",
            Choice::Icloud => "Connecting to iCloud…",
            Choice::Other => "Checking your mail server…",
            Choice::Gmail => "Waiting for browser sign-in… If Google shows “Access blocked”, close that tab and use an app password.",
            Choice::Outlook => "Waiting for browser sign-in…",
        });
        self.update_switch(choice == Choice::Gmail);
        self.update_waiting();
        let (sender, receiver) = mpsc::channel();
        let path = ui.database_path.clone();
        thread::spawn(move || {
            let result = match MailService::open(path) {
                Ok(service) => {
                    let envelope = service.request_envelope(&payload.to_string());
                    if envelope["ok"] == true {
                        Ok(envelope["data"].clone())
                    } else {
                        Err(Problem::from_envelope(&envelope))
                    }
                }
                Err(error) => Err(Problem {
                    code: "other".into(),
                    message: error,
                    ..Problem::default()
                }),
            };
            let _ = sender.send(result);
        });
        let setup = self.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(Problem::plain("The mail engine stopped."))
                }
            };
            setup.in_flight.borrow_mut().remove(&choice);
            let elsewhere = setup.selected.get() != Some(choice);
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
                            .insert(choice, (connected, None));
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
                Err(problem) if elsewhere => {
                    setup
                        .outcomes
                        .borrow_mut()
                        .insert(choice, (problem.message.clone(), Some(problem)));
                    setup.refresh_connect();
                }
                Err(problem) => {
                    setup.sign_in_failed.set(choice == Choice::Gmail);
                    setup.update_waiting();
                    setup.show_problem(&problem);
                    setup.connect.set_sensitive(true);
                    if !choice.uses_password() {
                        setup.connect.grab_focus();
                    }
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
/* The header bar makes this window client-side decorated. GTK frames such
   windows in the base theme's light colors: a white 4px band without a
   compositor (solid-csd), a light outline with one. Use the app's line
   color instead, like the macOS window's hairline. */
window.account-setup.solid-csd { padding: 0; border: 1px solid @line; border-radius: 0; box-shadow: none; outline: none; }
window.account-setup.csd:not(.solid-csd) { outline: none; box-shadow: 0 3px 12px 1px rgba(0, 0, 0, 0.3), 0 0 0 1px @line; }
.setup-header { background: @canvas; box-shadow: none; border: none; min-height: 30px; }
.setup-headline { font-size: 20px; font-weight: 600; }
.setup-intro { font-size: 13px; font-weight: 500; }
.setup-label { font-size: 13px; }
.setup-note { font-size: 12px; color: @secondary; }
.setup-status { font-size: 13px; color: @secondary; }
.setup-status.setup-problem { color: @problem; }
.setup-details { font-size: 11px; font-family: monospace; color: @secondary; }
.setup-provider-menu > button { background: transparent; color: @muted; font-size: 11px; padding: 0 2px; min-height: 0; box-shadow: none; border: none; }
.setup-menu-item { background: transparent; border: none; border-radius: 5px; padding: 4px 8px; }
.setup-menu-item:hover, .setup-menu-item.active { background: @selected; }
.setup-menu-key { color: @muted; font-size: 11px; }
.setup-link { background: transparent; color: @link; padding: 1px 0; min-height: 0; font-size: 13px; border: none; box-shadow: none; }
.setup-link:hover { text-decoration: underline; background: transparent; }
.setup-guide { font-size: 12px; }
.setup-fix { font-weight: 500; }
.setup-disclosure title, .setup-toggle { color: @secondary; font-size: 12px; }
.setup-toggle { background: transparent; border: none; box-shadow: none; padding: 0; min-height: 0; }
.setup-toggle:checked { background: transparent; }
.setup-step { font-size: 13px; }
.setup-step-number { font-size: 13px; color: @secondary; font-feature-settings: "tnum"; }
.setup-switch { font-weight: 600; font-size: 12px; }
.setup-button { border: 1px solid @line; padding: 4px 14px; min-height: 26px; }
.setup-primary { background: @ink; color: @canvas; border-color: @ink; font-weight: 600; }
.setup-primary label { color: @canvas; }
.setup-button:disabled { opacity: 0.45; }
button.setup-google { background: @google_fill; color: @google_ink; border: 1px solid @google_stroke; border-radius: 4px; min-height: 38px; padding: 0 12px; box-shadow: none; font-family: Roboto, Inter, sans-serif; font-size: 14px; font-weight: 500; }
button.setup-google label { color: @google_ink; }
button.setup-google:hover { background-image: linear-gradient(alpha(@google_ink, 0.08), alpha(@google_ink, 0.08)); }
button.setup-google:active { background-image: linear-gradient(alpha(@google_ink, 0.12), alpha(@google_ink, 0.12)); }
.setup-or { color: @muted; font-size: 12px; }
separator.setup-or-line { background: @line; min-height: 1px; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_mark_paths_parse_completely() {
        // Implicit repeats, relative commands, H/v/h and numbers like "3.14.76".
        assert_eq!(
            svg_path("M1 2h3v4H0l1-1c0 1 1 1 1 0s1-1 1 0z"),
            vec![
                PathStep::Move(1., 2.),
                PathStep::Line(4., 2.),
                PathStep::Line(4., 6.),
                PathStep::Line(0., 6.),
                PathStep::Line(1., 5.),
                PathStep::Curve(1., 6., 2., 6., 2., 5.),
                PathStep::Curve(2., 4., 3., 4., 3., 5.),
                PathStep::Close,
            ]
        );
        assert_eq!(svg_path("M0 0C.27-3.14.76-4.59 1 1").last(), Some(&PathStep::Curve(0.27, -3.14, 0.76, -4.59, 1., 1.)));
        for (data, _) in GOOGLE_LOGO {
            let steps = svg_path(data);
            assert!(matches!(steps.first(), Some(PathStep::Move(..))), "{data}");
            assert_eq!(steps.last(), Some(&PathStep::Close), "{data}");
            // Every point stays on the 48×48 artboard.
            for step in &steps {
                if let PathStep::Move(x, y) | PathStep::Line(x, y) | PathStep::Curve(_, _, _, _, x, y) = step {
                    assert!((-0.01..=48.01).contains(x) && (-0.01..=48.01).contains(y), "{data}: {step:?}");
                }
            }
        }
        // Bad data stops instead of looping.
        assert_eq!(svg_path("M1 2 ?"), vec![PathStep::Move(1., 2.)]);
    }

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

    fn form(email: &str, password: &str) -> Form {
        // Hidden server fields left over from another choice must not travel.
        Form {
            email: email.into(),
            password: password.into(),
            imap_host: "imap.example.com".into(),
            imap_port: "143".into(),
            smtp_host: "smtp.example.com".into(),
            smtp_port: "25".into(),
            username: "someone.else@example.com".into(),
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
            ["Gmail", "Gmail (app password)", "iCloud", "Outlook", "Other"]
        );
        let before = distribution(false);
        assert_eq!(
            available(&before),
            [Choice::GmailAppPassword, Choice::Icloud, Choice::Other]
        );
        assert_eq!(titles(&before), ["Gmail", "iCloud", "Other"]);
        let after = distribution(true);
        assert_eq!(titles(&after), ["Gmail", "Gmail (app password)", "iCloud", "Other"]);
        assert_eq!(BuildFeatures::from_response(None), BuildFeatures::restricted());
        assert!(!BuildFeatures::restricted().google_oauth_public);
    }

    #[test]
    fn the_address_chooses_the_provider_as_on_macos() {
        let official = distribution(false);
        assert_eq!(for_provider("gmail", &official, true), Choice::GmailAppPassword);
        assert_eq!(for_provider("gmail", &development(), true), Choice::Gmail);
        assert_eq!(
            for_provider("gmail", &development(), false),
            Choice::GmailAppPassword,
            "no Google client configured"
        );
        assert_eq!(for_provider("icloud", &official, false), Choice::Icloud);
        assert_eq!(for_provider("outlook", &official, false), Choice::Other);
        assert_eq!(for_provider("outlook", &development(), false), Choice::Outlook);
        assert_eq!(for_provider("other", &official, false), Choice::Other);
    }

    #[test]
    fn copy_matches_macos_and_never_mentions_a_google_review() {
        assert_eq!(GMAIL.intro, "Gmail needs an app password.");
        assert_eq!(
            GMAIL.steps.map(|(text, _)| text),
            ["Turn on 2-Step Verification ↗", "Create an app password ↗", "Paste it below"]
        );
        assert_eq!(
            GMAIL.steps.iter().filter_map(|(_, url)| *url).collect::<Vec<_>>(),
            [
                "https://myaccount.google.com/signinoptions/twosv",
                "https://myaccount.google.com/apppasswords"
            ]
        );
        assert_eq!(GMAIL.placeholder, "16 letters, spaces are fine");
        assert_eq!(ICLOUD.steps[0].1, Some("https://account.apple.com/"));
        for guide in [&GMAIL, &ICLOUD] {
            assert!(guide.about[0].contains("Secret Service"));
            for text in guide.about.iter().chain([&guide.intro]) {
                assert!(!text.contains("review") && !text.contains("Never enter"), "{text}");
            }
        }
        // Visible before any disclosure: about 40 words for Gmail.
        let visible = [TITLE, "Email", CHOOSE_PROVIDER, GMAIL.intro, GMAIL.field_label, GMAIL.placeholder, GMAIL.disclosure, GUIDE_LINK, "Cancel", "Add account"]
            .into_iter()
            .chain(GMAIL.steps.map(|(text, _)| text))
            .map(|text| text.split_whitespace().count())
            .sum::<usize>();
        assert!(visible <= 45, "{visible} words");
    }

    #[test]
    fn guide_links_only_in_builds_with_our_support_channel() {
        // Tests build the neutral profile, which has no support channel.
        assert_eq!(
            guide_url("gmail").is_some(),
            mail_service::profile::setup_guide_url().is_some()
        );
        if let Some(url) = guide_url("icloud") {
            assert_eq!(url, "https://ycorplabs.com/email/guide/#icloud");
        }
    }

    #[test]
    fn gmail_app_password_payload_matches_the_macos_request() {
        let payload =
            imap_payload(Choice::GmailAppPassword, &form(" person@gmail.example ", "abcd efgh ijkl mnop")).unwrap();
        assert_eq!(
            payload,
            json!({
                "op": "add_imap_account", "provider": "imap", "preset": "gmail",
                "email": "person@gmail.example", "display_name": "person@gmail.example",
                "username": "person@gmail.example", "smtp_username": "person@gmail.example",
                "password": "abcd efgh ijkl mnop",
            })
        );
        assert_eq!(
            imap_payload(Choice::GmailAppPassword, &form("person@gmail.example", "  ")).unwrap_err(),
            "Enter your Gmail address and the 16-letter app password from step 2."
        );
    }

    #[test]
    fn icloud_payload_leaves_servers_and_username_to_the_engine() {
        let payload = imap_payload(Choice::Icloud, &form("person@icloud.com", "abcd-efgh-ijkl-mnop")).unwrap();
        assert_eq!(
            payload,
            json!({
                "op": "add_imap_account", "provider": "icloud", "email": "person@icloud.com",
                "display_name": "person@icloud.com", "password": "abcd-efgh-ijkl-mnop",
            })
        );
        assert_eq!(
            imap_payload(Choice::Icloud, &form("person", "x")).unwrap_err(),
            "Enter your iCloud address and app-specific password."
        );
    }

    #[test]
    fn other_sends_servers_only_once_they_are_shown() {
        let hidden = form("person@example.com", "secret");
        assert_eq!(
            imap_payload(Choice::Other, &hidden).unwrap(),
            json!({
                "op": "add_imap_account", "provider": "imap", "email": "person@example.com",
                "display_name": "person@example.com", "password": "secret",
            }),
            "the engine guesses the servers"
        );
        // Same rule as macOS (`AccountSetupWindow.imapPayload`).
        let shown = Form {
            servers_shown: true,
            username: String::new(),
            imap_host: " imap.example.com ".into(),
            imap_port: " 993 ".into(),
            imap_tls: true,
            smtp_host: "smtp.example.com".into(),
            smtp_port: "587 ".into(),
            ..hidden
        };
        let payload = imap_payload(Choice::Other, &shown).unwrap();
        assert_eq!(payload["imap_host"], "imap.example.com");
        assert_eq!(payload["imap_port"], 993);
        assert_eq!(payload["smtp_port"], 587);
        assert_eq!(payload["username"], "person@example.com");
        assert_eq!(payload["smtp_username"], "person@example.com");
        let blank = Form {
            imap_host: "   ".into(),
            ..shown
        };
        assert!(imap_payload(Choice::Other, &blank).is_err());
    }

    #[test]
    fn problems_come_from_the_envelope_with_only_web_links() {
        let envelope = json!({"ok": false, "error": "That looks like your Google password.", "setup_error": {
            "code": "google_password", "message": "That looks like your Google password.",
            "action": {"label": "Create an app password instead", "url": "https://myaccount.google.com/apppasswords"},
            "detail": "imap.gmail.com:993 said: …", "servers": null}});
        let problem = Problem::from_envelope(&envelope);
        assert_eq!(problem.code, "google_password");
        assert_eq!(
            problem.action,
            Some((
                "Create an app password instead".into(),
                "https://myaccount.google.com/apppasswords".into()
            ))
        );
        assert_eq!(problem.detail, "imap.gmail.com:993 said: …");
        let unsafe_link = json!({"ok": false, "error": "x", "setup_error": {"code": "x", "message": "x",
            "action": {"label": "Open", "url": "file:///etc/passwd"}, "detail": ""}});
        assert_eq!(Problem::from_envelope(&unsafe_link).action, None);
        let servers = json!({"ok": false, "error": "x", "setup_error": {"code": "servers_not_found", "message": "x",
            "detail": "d", "servers": {"imap_host": "imap.example.org", "imap_port": 993, "imap_security": "implicit_tls",
            "smtp_host": "smtp.example.org", "smtp_port": 587, "smtp_security": "start_tls", "known": false}}});
        assert_eq!(
            Problem::from_envelope(&servers).servers,
            Some(Servers {
                imap_host: "imap.example.org".into(),
                imap_port: "993".into(),
                imap_tls: true,
                smtp_host: "smtp.example.org".into(),
                smtp_port: "587".into(),
                smtp_tls: false,
            })
        );
        let plain = Problem::from_envelope(&json!({"ok": false, "error": "valid email required"}));
        assert_eq!((plain.code.as_str(), plain.message.as_str()), ("other", "valid email required"));
    }
}
