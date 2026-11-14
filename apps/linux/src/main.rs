//! GTK 4 shell: native inbox, live account service, and isolated WebKitGTK reader.
mod search_filter_rules;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gtk::{Orientation, gdk, glib};
use mail_core::{
    Account, DraftAttachmentMeta, LabelSummary, MailAction, Message, OutboxItem, Provider,
    SplitInbox, ThreadSummary,
};
use mail_service::MailService;
use scraper::{Html, Selector};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use url::Url;
use webkit6::{gtk, prelude::*};

mod account_setup;
mod agenda;
mod agent;
mod booking;
mod cleanup;
mod parity;
mod reader_links;
mod recipient_completion;
mod signature_editor;
mod support;
mod workflows;

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

fn oauth_client_id(provider: &str) -> Option<String> {
    match provider {
        // The build's own OAuth application, injected at compile time from the
        // environment or config/oauth-desktop.local.env. No built-in fallback.
        "gmail" => std::env::var("EMAIL_GOOGLE_CLIENT_ID")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                option_env!("EMAIL_GOOGLE_CLIENT_ID")
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            }),
        "microsoft" => std::env::var("EMAIL_MICROSOFT_CLIENT_ID")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                option_env!("EMAIL_MICROSOFT_CLIENT_ID")
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            }),
        _ => None,
    }
}

/// Per-draft open tracking (Track opens and its command), from the distribution
/// profile's `OPEN_TRACKING` key compiled into the mail service: `development`
/// only, refused in any distribution profile and never without a Scheduling
/// origin. Saved activity for mail already sent with tracking stays readable.
pub(crate) const OPEN_TRACKING: bool = mail_service::profile::OPEN_TRACKING;

const CSS: &str = r#"
window { background: @surface; color: @ink; font-family: Inter, sans-serif; font-size: 13px; }
window.email-main, .app { background: transparent; }
/* Only the sidebar may show the compositor backdrop. Calendar owns an opaque workspace. */
.calendar-workspace, .calendar-workspace > viewport { background: @surface; color: @ink; }
.compact-bar { background: @surface; padding: 8px 12px; border-bottom: 1px solid @line; }
paned > separator { background: @line; min-width: 1px; }
headerbar { background: @surface; color: @ink; box-shadow: none; border-bottom: 1px solid @line; min-height: 32px; }
button { background-image: none; box-shadow: none; text-shadow: none; color: @ink; border: none; border-radius: 6px; min-height: 26px; padding: 4px 8px; }
button:hover { background: @hover; }
button:focus-visible { outline: 1px solid @muted; outline-offset: -1px; }
/* A strong material tint keeps sharp background windows from bleeding through.
   Backdrop blur is compositor-owned; GNOME does not provide it to this surface. */
.sidebar { background: alpha(@sidebar, 0.97); border-right: 1px solid @line; padding: 18px 10px 10px; }
.brand { color: @ink; font-size: 17px; font-weight: 650; margin: 0 8px 18px; }
.side-group { min-height: 26px; margin: 16px 8px 5px; }
.side-group label { color: @muted; font-size: 11px; font-weight: 500; letter-spacing: 0.4px; padding: 0; margin: 0; }
.side-item { background: transparent; color: @secondary; font-size: 13px; padding: 4px 8px; min-height: 23px; border-radius: 6px; margin: 2px 0; }
.side-item:hover { background: @hover; color: @ink; }
.side-item.active { background: @selected; color: @ink; font-weight: 600; }
.side-item label, .action label { color: inherit; }
.side-item image { color: @secondary; -gtk-icon-size: 16px; }
.key-hint { color: @muted; font-size: 11px; font-family: monospace; }
.compose-button { background: @selected; color: @ink; margin-bottom: 8px; }
.inbox { background: @surface; border-right: 1px solid @line; }
.inbox-header { padding: 18px 18px 12px; border-bottom: 1px solid @line; }
.section-title { color: @ink; font-size: 19px; font-weight: 650; }
.hint { color: @muted; font-size: 11px; margin-top: 6px; }
.search { background: @input; color: @ink; border: 1px solid @line; border-radius: 7px; margin-top: 12px; min-height: 23px; box-shadow: none; }
.search text { color: @ink; background: transparent; }
.mail-list { background: transparent; }
.mail-list row, .mail-list row:selected { color: @ink; }
.mail-list row { background: transparent; border: none; padding: 9px 16px; }
.mail-list row:selected { background: @selected; box-shadow: inset 2px 0 alpha(@ink,0.55); }
.mail-selection check { background: transparent; background-image: none; border: 1px solid @muted; border-radius: 3px; color: @canvas; min-width: 13px; min-height: 13px; box-shadow: none; }
.mail-selection check:checked { background: @ink; border-color: @ink; color: @canvas; }
.mail-selection { padding: 0; margin: 0; }
.row-star { background: transparent; color: @muted; padding: 0; min-width: 16px; min-height: 16px; opacity: 0.6; }
.row-star:hover { color: @ink; opacity: 1; }
.mail-list row:hover:not(:selected) { background: @hover; }
.sender { color: @ink; font-size: 13px; font-weight: 600; }
.subject { color: @ink; font-size: 12px; font-weight: 500; margin-top: 3px; }
.preview { color: @muted; font-size: 12px; margin-top: 3px; }
.account, .mail-time { color: @muted; font-size: 10px; font-weight: 400; }
.unread-dot { color: @ink; font-size: 8px; margin-right: 5px; }
.read .sender { color: @secondary; font-weight: 400; }
.read .subject { color: @secondary; font-weight: 400; }
.reader-header { background: @canvas; padding: 22px 28px 14px; }
.reader-title { color: @ink; font-size: 18px; font-weight: 650; margin-bottom: 10px; }
.reader-sender { color: @ink; font-size: 13px; font-weight: 600; margin-bottom: 3px; }
.reader-meta { color: @secondary; font-size: 12px; }
.recipient-summary > button { padding: 0; background: transparent; min-height: 22px; }
.recipient-summary > button:hover { background: @hover; }
.action { background: transparent; color: @secondary; border: none; border-radius: 6px; padding: 5px 7px; min-width: 24px; min-height: 24px; }
.action image { -gtk-icon-size: 17px; }
.action:hover { background: @hover; color: @ink; }
.action.active { color: @ink; background: @selected; }
.remote-images-notice { background: @surface; color: @muted; border: 1px solid @line; border-radius: 8px; padding: 0 8px 0 10px; min-height: 30px; }
.remote-images-text { color: @muted; font-size: 12px; }
.remote-images-load { background: transparent; color: @ink; font-size: 12px; font-weight: 600; padding: 2px 4px; min-height: 22px; }
.remote-images-load:hover { background: @hover; }
.remote-images-menu button { background: transparent; padding: 5px 10px; font-size: 12px; }
.remote-images-menu button:hover { background: @hover; }
.conversation-item { background: transparent; color: @secondary; border-bottom: 1px solid @line; border-radius: 0; padding: 14px 10px; margin: 0; font-size: 11px; }
.conversation-item:hover { background: @hover; }
.reading { background: @canvas; }
/* First run: one welcome with the app's primary button (macOS paintEmptyAction). */
.welcome-page { background: @canvas; }
.welcome-title { font-size: 16px; font-weight: 600; color: @ink; }
.welcome-detail { font-size: 13px; color: @muted; }
.welcome-action { background: @ink; color: @canvas; border: none; border-radius: 9px; min-height: 34px; min-width: 144px; padding: 0 16px; font-size: 12px; font-weight: 600; margin-top: 11px; }
.welcome-action:hover { background: alpha(@ink, 0.88); }
.welcome-action label { color: @canvas; }
.plain, .plain text { background: @canvas; color: @ink; font-family: Inter, sans-serif; font-size: 14px; }
.plain { padding: 28px; }
.palette, popover contents { background: @surface; color: @ink; border: 1px solid @line; border-radius: 10px; padding: 10px; }
.palette-help { color: @muted; font-size: 11px; margin-top: 12px; }
entry { background: @input; color: @ink; border: 1px solid @line; box-shadow: none; padding: 6px 8px; }
.filter-tab { background: transparent; color: @muted; font-size: 12px; padding: 3px 16px; min-height: 22px; }
.selection-action { padding: 4px 5px; font-size: 11px; min-width: 0; }
.filter-tab.active { background: @selected; color: @ink; font-weight: 600; }
.shortcut-bar { color: @muted; font-size: 10px; padding: 8px 20px; border-top: 1px solid @line; }
entry:focus-within, .search:focus-within { border-color: @muted; outline: none; box-shadow: none; }
textview, textview text { color: @ink; background: @canvas; }
.separator { background: @line; min-height: 1px; margin: 12px 0; }
.composer { background: @input; border: 1px solid @line; border-radius: 12px; padding: 14px 16px; }
.composer-title { color: @ink; font-size: 14px; font-weight: 600; min-height: 24px; }
.composer .compose-details-button, .composer .compose-tool { color: @muted; background: transparent; font-size: 11px; padding: 3px 6px; min-height: 24px; }
.composer .compose-details-button:hover, .composer .compose-tool:hover { color: @ink; background: @hover; }
.composer .compose-close { color: @muted; background: transparent; font-size: 19px; padding: 0; min-width: 24px; min-height: 24px; }
.composer .compose-close:hover { background: @hover; color: @ink; }
.compose-field { border-bottom: 1px solid @line; padding: 2px 0; }
.compose-field-label { color: @muted; font-size: 12px; }
.composer entry, .composer combobox button { background: transparent; background-image: none; border: none; border-radius: 0; box-shadow: none; padding: 4px 6px; min-height: 24px; font-size: 12px; }
.composer entry:focus-within { outline: none; box-shadow: none; }
.composer textview, .composer textview text { background: @input; color: @ink; font-family: Inter, sans-serif; font-size: 14px; }
.compose-placeholder { color: @muted; font-size: 14px; }
.composer .compose-send { background: @ink; color: @canvas; border-radius: 7px; padding: 6px 16px; min-height: 26px; font-size: 12px; font-weight: 600; }
.composer .compose-send:hover { opacity: 0.88; }

.command-panel { background: @surface; border: 1px solid @line; border-radius: 14px; padding: 22px; }
.command-heading { color: @muted; min-height: 30px; padding: 0; }
.command-heading label, .contact-heading { color: @muted; font-size: 10px; font-weight: 600; letter-spacing: 0.4px; padding: 0; }
.contact-heading { margin: 3px 0; }
.command-search { background: @input; border: 1px solid @line; border-radius: 8px; padding: 12px; font-size: 16px; margin: 14px 0; }
.command-list { background: transparent; }
.command-list row { padding: 11px 12px; border-radius: 8px; margin: 2px 0; }
.command-list row:selected { background: @selected; color: @ink; }
.command-title { font-size: 13px; font-weight: 600; color: @ink; }
.command-detail { font-size: 11px; color: @muted; }
.keycap { font-family: monospace; font-size: 10px; color: @muted; }
.feedback-thanks { background: @surface; color: @ink; border: 1px solid @line; border-radius: 12px; padding: 14px 18px; min-width: 282px; }
.feedback-heart { color: #ac67e8; font-size: 20px; min-width: 22px; }
.feedback-title { font-size: 13px; font-weight: 600; }
.settings-card { padding: 24px; background: @surface; }
.activity-controls button { min-width: 20px; min-height: 20px; padding: 3px; }
.contact-pane { background: @canvas; border-left: 1px solid @line; }
.contact-name { color: @ink; font-size: 14px; font-weight: 600; }
.contact-email { color: @muted; font-size: 11px; }
.contact-action { color: @muted; background: transparent; font-size: 11px; padding: 3px 4px; }
.contact-close { color: @muted; background: transparent; font-size: 18px; padding: 0; min-width: 24px; }
.contact-recent-row { padding: 10px 0; background: transparent; }
.contact-recent { color: @secondary; font-size: 12px; }
.avatar { background: #39362f; color: #e6dccb; border: 1px solid alpha(#e6dccb,0.16); border-radius: 13px; font-size: 16px; font-weight: 600; }
.avatar-photo { border-radius: 13px; }

"#;

fn appearance_css(dark: bool) -> String {
    let colors = if dark {
        [
            "#1f2021", "#242526", "#1b1c1d", "#262728", "#303134", "#292a2c", "#353637", "#ecebe8",
            "#d0cfcb", "#a3a39f",
        ]
    } else {
        [
            "#fafaf9", "#f2f1ef", "#ffffff", "#ffffff", "#eaeaec", "#f0efed", "#e5e4e1", "#262626",
            "#555550", "#777875",
        ]
    };
    let names = [
        "surface",
        "sidebar",
        "canvas",
        "input",
        "selected",
        "hover",
        "line",
        "ink",
        "secondary",
        "muted",
    ];
    let mut css = String::new();
    for (name, value) in names.iter().zip(colors) {
        css.push_str(&format!("@define-color {name} {value};\n"));
    }
    css.push_str(&format!("@define-color problem {};\n", if dark { "#ff6961" } else { "#c4161c" }));
    // Links in account setup (macOS linkColor).
    css.push_str(&format!("@define-color link {};\n", if dark { "#419cff" } else { "#0068da" }));
    // Google's sign-in button themes (branding guidelines), as on macOS.
    let google = if dark { ["#131314", "#8e918f", "#e3e3e3"] } else { ["#ffffff", "#747775", "#1f1f1f"] };
    for (name, value) in ["google_fill", "google_stroke", "google_ink"].iter().zip(google) {
        css.push_str(&format!("@define-color {name} {value};\n"));
    }
    css.push_str(CSS);
    css.push_str(account_setup::CSS);
    css.push_str(r#"
.recipient-popover contents { background: @input; color: @ink; border: 1px solid @line; border-radius: 9px; padding: 4px; }
.recipient-suggestion { background: transparent; border: 0; border-radius: 5px; padding: 6px 10px; font-size: 12px; }
.recipient-suggestion:hover, .recipient-selected { background: @selected; }
.recipient-suggestion .dim-label { font-size: 10px; color: @muted; }
.recipient-hint { font-size: 10px; color: @muted; padding: 6px; }
"#);
    css.push_str(agent::CSS);
    css.push_str(
        r#"
.cleanup-window { background: @canvas; color: @ink; }
.cleanup-heading { font-size: 17px; font-weight: 600; }
.cleanup-card { background: @canvas; padding: 0; }
.cleanup-sender { font-size: 13px; font-weight: 600; }
.cleanup-count { font-size: 14px; font-weight: 500; }
.cleanup-secondary { font-size: 12px; color: @muted; }
.cleanup-footnote { font-size: 11px; color: @muted; }
.cleanup-disclosure { padding: 2px 0; font-size: 12px; }
.cleanup-done { background: @ink; color: @canvas; border: 0; border-radius: 7px; padding: 7px 18px; font-size: 12px; font-weight: 600; }
"#,
    );
    css
}

fn mail_icon(name: &str) -> gtk::DrawingArea {
    let icon = gtk::DrawingArea::new();
    icon.set_content_width(16);
    icon.set_content_height(16);
    icon.set_size_request(16, 16);
    icon.set_hexpand(false);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    let name = name.to_string();
    icon.set_draw_func(move |widget, cr, w, h| {
        let color = widget.color();
        cr.set_source_rgba(
            color.red() as f64,
            color.green() as f64,
            color.blue() as f64,
            color.alpha() as f64,
        );
        let size = f64::from(w.min(h));
        cr.translate((f64::from(w) - size) / 2.0, (f64::from(h) - size) / 2.0);
        cr.scale(size / 24.0, size / 24.0);
        cr.set_line_width(1.65);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        let path = |points: &[(f64, f64)]| {
            cr.move_to(points[0].0, points[0].1);
            for &(x, y) in &points[1..] {
                cr.line_to(x, y);
            }
            let _ = cr.stroke();
        };
        let circle = |x: f64, y: f64, r: f64| {
            cr.arc(x, y, r, 0.0, std::f64::consts::TAU);
            let _ = cr.stroke();
        };
        match name.as_str() {
            "mail-inbox-symbolic" => {
                // Layered tray, with the same optical footprint as the Mac sidebar.
                path(&[(5., 7.), (7., 4.), (17., 4.), (19., 7.)]);
                path(&[(4., 10.), (6., 7.), (18., 7.), (20., 10.)]);
                cr.move_to(3., 11.);
                cr.line_to(8., 11.);
                cr.curve_to(9., 11., 9., 14., 11., 14.);
                cr.line_to(13., 14.);
                cr.curve_to(15., 14., 15., 11., 16., 11.);
                cr.line_to(21., 11.);
                cr.line_to(21., 18.);
                cr.curve_to(21., 19.2, 20.2, 20., 19., 20.);
                cr.line_to(5., 20.);
                cr.curve_to(3.8, 20., 3., 19.2, 3., 18.);
                cr.close_path();
                let _ = cr.stroke();
            }
            "document-edit-symbolic" => {
                cr.move_to(12., 4.);
                cr.line_to(6., 4.);
                cr.curve_to(4., 4., 3., 5., 3., 7.);
                cr.line_to(3., 18.);
                cr.curve_to(3., 20., 4., 21., 6., 21.);
                cr.line_to(17., 21.);
                cr.curve_to(19., 21., 20., 20., 20., 18.);
                cr.line_to(20., 12.);
                let _ = cr.stroke();
                path(&[
                    (10., 14.),
                    (11., 10.),
                    (18., 3.),
                    (21., 6.),
                    (14., 13.),
                    (10., 14.),
                ]);
                path(&[(16.5, 4.5), (19.5, 7.5)]);
            }
            "chart-symbolic" => {
                path(&[(4.,20.),(4.,12.),(8.,12.),(8.,20.),(4.,20.)]);
                path(&[(10.,20.),(10.,5.),(14.,5.),(14.,20.),(10.,20.)]);
                path(&[(16.,20.),(16.,9.),(20.,9.),(20.,20.),(16.,20.)]);
            }
            "alarm-symbolic" => {
                circle(12., 12., 9.);
                path(&[(12., 6.), (12., 12.), (16., 14.)]);
            }
            "mail-send-symbolic" => {
                path(&[(3., 10.), (22., 3.), (15., 22.), (11., 14.), (3., 10.)]);
                path(&[(11., 14.), (22., 3.)]);
            }
            "avatar-default-symbolic" => {
                circle(12., 12., 9.);
                circle(12., 9., 3.);
                cr.move_to(5.5, 18.);
                cr.curve_to(7., 13., 17., 13., 18.5, 18.);
                let _ = cr.stroke();
            }
            "view-filter-symbolic" => {
                path(&[(4., 6.), (20., 6.)]);
                path(&[(7., 12.), (17., 12.)]);
                path(&[(10., 18.), (14., 18.)]);
            }
            "list-add-symbolic" => {
                path(&[(12., 6.), (12., 18.)]);
                path(&[(6., 12.), (18., 12.)]);
            }
            "system-search-symbolic" => {
                circle(10., 10., 6.5);
                path(&[(15., 15.), (21., 21.)]);
            }
            "view-refresh-symbolic" => {
                cr.arc(12., 12., 8., 0.5, 5.7);
                let _ = cr.stroke();
                path(&[(20., 4.), (20., 10.), (14., 9.)]);
            }
            "tag-symbolic" | "folder-saved-search-symbolic" => {
                path(&[
                    (3., 4.),
                    (12., 4.),
                    (21., 13.),
                    (13., 21.),
                    (3., 11.),
                    (3., 4.),
                ]);
                circle(8., 8., 1.);
            }
            "mail-restore-symbolic" => {
                path(&[(4., 13.), (4., 20.), (20., 20.), (20., 13.)]);
                path(&[(8., 8.), (12., 4.), (16., 8.)]);
                path(&[(12., 4.), (12., 15.)]);
            }
            "mail-archive-symbolic" => {
                path(&[(4., 8.), (4., 21.), (20., 21.), (20., 8.)]);
                path(&[(3., 3.), (21., 3.), (21., 8.), (3., 8.), (3., 3.)]);
                path(&[(9., 12.), (15., 12.)]);
            }
            "weather-clear-night-symbolic" => {
                cr.arc(12., 12., 9., -1.0, 4.0);
                let _ = cr.stroke();
                cr.arc(17., 7., 8., 1.4, 3.6);
                let _ = cr.stroke();
            }
            "mail-reply-all-symbolic" => {
                path(&[(8., 5.), (2., 11.), (8., 17.)]);
                path(&[(14., 5.), (8., 11.), (14., 17.)]);
                path(&[(8., 11.), (16., 11.), (20., 14.), (22., 19.)]);
            }
            "mail-reply-sender-symbolic" => {
                path(&[(10., 5.), (3., 11.), (10., 17.)]);
                path(&[(3., 11.), (13., 11.), (18., 13.), (21., 19.)]);
            }
            "star-symbolic" | "star-filled" => {
                let mut pts = Vec::new();
                for i in 0..11 {
                    let a = -std::f64::consts::FRAC_PI_2 + i as f64 * std::f64::consts::PI / 5.;
                    let r = if i % 2 == 0 { 9. } else { 4.1 };
                    pts.push((12. + r * a.cos(), 12. + r * a.sin()));
                }
                if name == "star-filled" {
                    cr.move_to(pts[0].0, pts[0].1);
                    for (x, y) in &pts[1..] {
                        cr.line_to(*x, *y);
                    }
                    let _ = cr.fill_preserve();
                    let _ = cr.stroke();
                } else {
                    path(&pts);
                }
            }
            "image-x-generic-symbolic" => {
                path(&[(3., 4.), (21., 4.), (21., 20.), (3., 20.), (3., 4.)]);
                circle(8., 9., 1.5);
                path(&[(4., 18.), (10., 12.), (14., 16.), (17., 13.), (21., 17.)]);
            }
            "emblem-system-symbolic" => {
                let mut points = Vec::new();
                for i in 0..33 {
                    let angle = i as f64 * std::f64::consts::TAU / 32.0;
                    let radius = if i % 4 < 2 { 9.0 } else { 7.0 };
                    points.push((12.0 + radius * angle.cos(), 12.0 + radius * angle.sin()));
                }
                path(&points);
                circle(12., 12., 3.);
            }
            "mail-attachment-symbolic" => {
                cr.save().ok();
                cr.translate(12., 12.);
                cr.rotate(-0.7);
                cr.arc(0., -3., 4., std::f64::consts::PI, 0.);
                cr.line_to(4., 5.);
                cr.arc(0., 5., 4., 0., std::f64::consts::PI);
                cr.line_to(-4., -3.);
                let _ = cr.stroke();
                cr.move_to(0., -3.);
                cr.line_to(0., 5.);
                let _ = cr.stroke();
                cr.restore().ok();
            }
            "security-high-symbolic" => {
                path(&[(12., 3.), (19., 6.), (19., 11.), (17.5, 16.), (12., 21.), (6.5, 16.), (5., 11.), (5., 6.), (12., 3.)]);
                path(&[(8.5, 12.), (11., 14.5), (15.5, 9.5)]);
            }
            "view-more-symbolic" => {
                for y in [5., 12., 19.] {
                    circle(12., y, 0.8);
                }
            }
            "x-office-calendar-symbolic" => {
                path(&[(4., 5.), (20., 5.), (20., 21.), (4., 21.), (4., 5.)]);
                path(&[(4., 10.), (20., 10.)]);
                path(&[(8., 3.), (8., 7.)]);
                path(&[(16., 3.), (16., 7.)]);
                for (x, y) in [(8., 14.), (12., 14.), (16., 14.), (8., 18.), (12., 18.)] {
                    circle(x, y, 0.6);
                }
            }
            _ => {
                path(&[
                    (3., 5.),
                    (21., 5.),
                    (21., 19.),
                    (3., 19.),
                    (3., 5.),
                    (12., 12.),
                    (21., 5.),
                ]);
            }
        }
    });
    icon
}

fn navigation_button(icon: &str, name: &str, shortcut: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("side-item");
    let content = gtk::Box::new(Orientation::Horizontal, 10);
    content.append(&mail_icon(icon));
    let title = gtk::Label::new(Some(name));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&title);
    let hint = gtk::Label::new(Some(shortcut));
    hint.add_css_class("key-hint");
    content.append(&hint);
    button.set_child(Some(&content));
    button
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_child(Some(&mail_icon(icon)));
    button.add_css_class("action");
    button.set_tooltip_text(Some(tooltip));
    button
}

fn sender_name(sender: &str) -> &str {
    sender
        .split('<')
        .next()
        .unwrap_or(sender)
        .trim()
        .trim_matches('"')
}

fn message_time(timestamp: i64) -> String {
    glib::DateTime::from_unix_local(timestamp / 1000)
        .and_then(|date| date.format("%b %e"))
        .map(|text| text.to_string())
        .unwrap_or_default()
}

#[derive(Clone)]
struct Mail {
    thread_id: String,
    account_id: String,
    account: String,
    sender: String,
    subject: String,
    preview: String,
    received: String,
    unread: bool,
    labels: Vec<String>,
}

#[derive(Clone, Debug)]
enum MailView {
    Inbox,
    Account(String),
    Split(String),
    Label(String),
    Snoozed,
    Followups,
    Archive,
    Sent,
    Drafts,
    Outbox,
}
enum AttachmentEvent {
    Portrait {
        account_id: String,
        address: String,
        result: Result<Value, String>,
    },
    Headers {
        thread_id: String,
        result: Result<Value, String>,
    },
    InlineImages {
        message_id: String,
        result: Result<Value, String>,
    },
    List {
        message_id: String,
        result: Result<Value, String>,
    },
    Preview {
        message_id: String,
        part_id: String,
        filename: String,
        mime_type: String,
        result: Result<Value, String>,
    },
}

/// The store and whether it is isolated, resolved once per process so every
/// caller sees the same checked path and the same isolation decision.
fn resolved_store() -> &'static Result<(PathBuf, bool), String> {
    static RESOLVED: std::sync::OnceLock<Result<(PathBuf, bool), String>> =
        std::sync::OnceLock::new();
    RESOLVED.get_or_init(|| resolve_database_path().map_err(|error| error.to_string()))
}

fn database_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    resolved_store()
        .clone()
        .map(|(path, _)| path)
        .map_err(Into::into)
}

/// The mail-service request for an EMAIL_DEMO value, as on macOS. Launch dates
/// follow the local time zone, so the inbox reads today, yesterday and this week.
fn demo_request(value: &str) -> Option<String> {
    match value {
        "1" | "small" => Some(r#"{"op":"seed_demo"}"#.into()),
        "launch" => {
            let offset_minutes = glib::DateTime::now_local()
                .map(|now| now.utc_offset().as_seconds() / 60)
                .unwrap_or(0);
            Some(
                json!({"op":"seed_demo","set":"launch","utc_offset_minutes":offset_minutes})
                    .to_string(),
            )
        }
        _ => None,
    }
}

fn store_is_isolated() -> bool {
    matches!(resolved_store(), Ok((_, true)))
}

fn resolve_database_path() -> Result<(PathBuf, bool), Box<dyn std::error::Error>> {
    if let Some(path) = store_path_override(isolated_store_value())? {
        // A path in the default store's directory shares its settings, however it
        // is spelled: it gets the normal app ID and never receives demo data.
        let isolated = !mail_service::is_default_store(&path);
        return Ok((path, isolated));
    }
    let directory = default_store_directory()?;
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::symlink_metadata(&directory)?
            .file_type()
            .is_symlink()
        {
            return Err("mail data directory must not be a symlink".into());
        }
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok((directory.join("mail.sqlite"), false))
}

fn default_store_directory() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(mail_service::default_store_directory().ok_or("no HOME or XDG_DATA_HOME configured")?)
}

/// The one rule for an isolated store: an `EMAIL_STORE_PATH`, as on macOS, that
/// does not name the default store (`mail_service::is_default_store`). An empty
/// value is refused by `store_path_override`.
fn isolated_store_value() -> Option<std::ffi::OsString> {
    std::env::var_os("EMAIL_STORE_PATH")
}

/// `EMAIL_STORE_PATH` selects a disposable store, as on macOS; relative paths
/// resolve against the working directory. Account settings and preference
/// files beside the database follow it, so a demo run never reads or changes
/// the default mail data directory. As on macOS, a missing directory is created
/// private (0700) and a symlinked database is refused.
fn store_path_override(
    value: Option<std::ffi::OsString>,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let Some(path) = resolve_store_path(value)? else {
        return Ok(None);
    };
    let parent = path.parent().ok_or("EMAIL_STORE_PATH must name a database file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        // A symlink could point an "isolated" run at the real mail cache.
        if std::fs::symlink_metadata(&path).is_ok_and(|file| file.file_type().is_symlink()) {
            return Err("EMAIL_STORE_PATH must not be a symlink".into());
        }
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(parent)?;
    // One spelling per store, matching how mail-service keys its credentials.
    Ok(Some(mail_service::canonical_store_path(&path)))
}

/// An isolated store runs as its own GTK application, one per store: it never
/// hands off to, or takes over, the installed app's window, notifications or
/// actions, while relaunching the same store still reuses its window.
fn application_id(isolated_store: Option<&std::path::Path>) -> String {
    let base = mail_service::profile::LINUX_APP_ID;
    match isolated_store {
        None => base.into(),
        Some(path) => {
            let digest = glib::compute_checksum_for_string(
                glib::ChecksumType::Sha256,
                path.to_string_lossy().as_ref(),
            )
            .map(|digest| digest.to_string())
            .unwrap_or_default();
            format!("{base}.Isolated.s{}", digest.get(..12).unwrap_or("store"))
        }
    }
}

fn resolve_store_path(
    value: Option<std::ffi::OsString>,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let Some(value) = value else {
        return Ok(None);
    };
    // An empty value is a mistake, never a request for the real store.
    if value.is_empty() {
        return Err("EMAIL_STORE_PATH is empty. Set a database path or unset it.".into());
    }
    let path = std::path::absolute(PathBuf::from(value))?;
    if path.file_name().is_none() || path.parent().is_none() {
        return Err("EMAIL_STORE_PATH must name a database file".into());
    }
    Ok(Some(path))
}

/// Remote images are blocked unless the user chose to load them by default.
/// Only that explicit choice (`I` or the image menu) writes this file.
fn remote_images_enabled(database_path: &std::path::Path) -> bool {
    std::fs::read_to_string(database_path.with_file_name("remote-images-enabled"))
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

/// Tooltip of the notice's Load images button, identical to macOS
/// (`RemoteImagePreference.loadButtonTooltip`).
fn load_images_tooltip(load_by_default: bool) -> String {
    let action = if load_by_default { "block" } else { "always load" };
    format!("Load remote images in this email. I to {action} remote images by default")
}

/// Image-control tooltip, identical to macOS
/// (`RemoteImagePreference.controlTooltip`). `I` always changes the remembered
/// default, never just the current email.
fn remote_images_tooltip(allowed_here: bool, load_by_default: bool) -> String {
    let state = match (allowed_here, load_by_default) {
        (true, true) => "Remote images load automatically",
        (true, false) => "Remote images are loaded in this email",
        (false, _) => "Remote images are blocked",
    };
    let action = if load_by_default { "block" } else { "always load" };
    format!("{state}. Image options · I to {action} remote images by default")
}

/// Mirrors the macOS reader check (`RemoteImagePreference.containsRemoteImages`):
/// an `<img>` with an HTTPS source, or an HTTPS URL anywhere in a `style`
/// attribute (`url()`, `image-set()`, escaped forms). The sanitizer drops
/// `<style>` elements and the reader allows only `https:` (the service upgrades
/// `<img>` http sources). Same-message `cid:` images arrive as `data:` URLs.
fn has_remote_images(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let remote_after = |rest: &str, quotes: &[&str]| {
        let rest = rest.trim_start();
        let rest = quotes
            .iter()
            .find_map(|quote| rest.strip_prefix(quote))
            .unwrap_or(rest)
            .trim_start();
        rest.starts_with("https://")
    };
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let image_source = lower.match_indices("<img").any(|(start, _)| {
        let tag = &lower[start + 4..];
        if tag.bytes().next().is_some_and(word) {
            return false;
        }
        // Sanitized attribute values may contain '>': end at one outside quotes.
        let mut quote = None;
        let end = tag
            .char_indices()
            .find(|&(_, c)| match quote {
                Some(open) if c == open => {
                    quote = None;
                    false
                }
                Some(_) => false,
                None if c == '"' || c == '\'' => {
                    quote = Some(c);
                    false
                }
                None => c == '>',
            })
            .map_or(tag.len(), |(at, _)| at);
        let tag = &tag[..end];
        // Attribute names only: skip text inside quoted values.
        let mut quote = None;
        tag.char_indices().any(|(at, c)| {
            if let Some(open) = quote {
                if c == open {
                    quote = None;
                }
                return false;
            }
            if c == '"' || c == '\'' {
                quote = Some(c);
                return false;
            }
            tag[at..].starts_with("src")
                && (at == 0 || !word(tag.as_bytes()[at - 1]))
                && tag[at + 3..]
                    .trim_start()
                    .strip_prefix('=')
                    .is_some_and(|value| remote_after(value, &["\"", "'"]))
        })
    });
    let style_source = lower.match_indices("style").any(|(at, _)| {
        if at > 0 && word(lower.as_bytes()[at - 1]) {
            return false;
        }
        let Some(value) = lower[at + 5..].trim_start().strip_prefix('=') else {
            return false;
        };
        let value = value.trim_start();
        let Some(quote) = value.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            return false;
        };
        let value = &value[1..];
        value[..value.find(quote).unwrap_or(value.len())].contains("https://")
    });
    image_source || style_source
}

fn snooze_duration(input: &str) -> Option<(u64, String)> {
    let input = input.trim().to_ascii_lowercase();
    let digits = input.bytes().take_while(u8::is_ascii_digit).count();
    let amount = input.get(..digits)?.parse::<u64>().ok()?;
    if amount == 0 {
        return None;
    }
    let (seconds, unit) = match input.get(digits..)?.trim() {
        "" | "m" | "min" | "mins" | "minute" | "minutes" => (60, "minute"),
        "h" | "hr" | "hour" | "hours" => (60 * 60, "hour"),
        "d" | "day" | "days" => (24 * 60 * 60, "day"),
        "w" | "week" | "weeks" => (7 * 24 * 60 * 60, "week"),
        _ => return None,
    };
    let duration = amount.checked_mul(seconds)?;
    if duration > 365 * 24 * 60 * 60 {
        return None;
    }
    let suffix = if amount == 1 { "" } else { "s" };
    Some((duration, format!("In {amount} {unit}{suffix}")))
}

fn operation_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("linux-ui:{now}")
}

fn dispatch_due_in_background(
    database_path: PathBuf,
    in_flight: Arc<AtomicBool>,
    completed: Sender<bool>,
) {
    if in_flight.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::spawn(move || {
        let changed = MailService::open(database_path)
            .ok()
            .and_then(|service| service.request_json(r#"{"op":"dispatch_due"}"#).ok())
            .and_then(|value| value.as_array().map(|items| !items.is_empty()))
            .unwrap_or(false);
        let _ = completed.send(changed);
        in_flight.store(false, Ordering::Release);
    });
}

struct Ui {
    window: gtk::ApplicationWindow,
    workspace: gtk::Stack,
    /// The mail page: the list and reader, or the first-run welcome.
    mail_stack: gtk::Stack,
    /// Sidebar rows that do nothing until an account exists.
    account_only: Vec<gtk::Widget>,
    calendar_host: gtk::Box,
    calendar: RefCell<Option<Rc<agenda::Agenda>>>,
    service: MailService,
    database_path: PathBuf,
    completed: Sender<Result<Value, String>>,
    attachment_completed: Sender<AttachmentEvent>,
    items: RefCell<Vec<Mail>>,
    activity_navigation: RefCell<Option<Message>>,
    list: gtk::ListBox,
    list_scroll: gtk::ScrolledWindow,
    loaded_limit: Cell<u32>,
    loading_older: Cell<bool>,
    can_load_older: Cell<bool>,
    older_count_before: Cell<usize>,
    search: gtk::SearchEntry,
    inbox_filter: Cell<u8>,
    search_generation: Cell<u64>,
    search_preview: RefCell<Option<(String, Result<Vec<ThreadSummary>, String>)>>,
    goto_pending: Cell<bool>,
    archive_browse_started: Cell<bool>,
    filter_buttons: Vec<gtk::Button>,
    selection_bar: gtk::Box,
    selection_count: gtk::Label,
    selection_buttons: Vec<gtk::Button>,
    marked: RefCell<std::collections::HashSet<String>>,
    marked_scope: RefCell<String>,
    row_checks: RefCell<std::collections::HashMap<String, gtk::CheckButton>>,
    updating_checks: Cell<bool>,
    last_triage: RefCell<Option<(String, Vec<String>)>>,
    /// Most recently discarded draft (message ID, subject), restorable with Z in Drafts.
    last_discarded_draft: RefCell<Option<(String, String)>>,
    dark: Rc<Cell<bool>>,
    title: gtk::Label,
    meta: gtk::Label,
    sender_label: gtk::Label,
    unsubscribe_button: gtk::Button,
    favorite_button: gtk::Button,
    recipient_details: gtk::Label,
    recipient_button: gtk::MenuButton,
    reading_content: gtk::Box,
    reader_header: gtk::Box,
    shortcut_bar: parity::ShortcutStrip,
    compose_pane: RefCell<Option<gtk::Box>>,
    plain: gtk::TextView,
    plain_scroll: gtk::ScrolledWindow,
    reader: RefCell<Option<webkit6::WebView>>,
    remote_images: gtk::Button,
    remote_images_menu: gtk::Popover,
    tracking_activity: gtk::Button,
    remote_images_notice: gtk::Box,
    load_remote_images: gtk::Button,
    current_html: RefCell<Option<String>>,
    allow_remote_images: Cell<bool>,
    remote_images_default: Cell<bool>,
    /// Message whose remote images the user loaded or blocked by hand. Linux
    /// shows one message at a time, so a newly arrived reply starts blocked.
    remote_images_message: RefCell<Option<String>>,
    /// A new-message composer fills the reader area (a reply does not).
    compose_covers_reader: Cell<bool>,
    rendered_remote_images: Cell<bool>,
    /// Public builds offer Recent opens only for mail already sent with tracking.
    offers_open_activity: Cell<bool>,
    palette: gtk::Window,
    palette_page: RefCell<String>,
    palette_rows: gtk::ListBox,
    palette_heading: parity::PaletteHeading,
    palette_empty: gtk::Label,
    palette_items: RefCell<Vec<parity::Command>>,
    contact: parity::ContactPane,
    activity_host: gtk::Box,
    mail_inbox_viewport: gtk::ScrolledWindow,
    mail_reader_viewport: gtk::ScrolledWindow,
    mail_reading_host: gtk::Box,
    activity_panel: RefCell<Option<Rc<booking::ActivityPanel>>>,
    activity_return_focus: RefCell<Option<gtk::Widget>>,
    portraits: RefCell<std::collections::HashMap<String, Option<Vec<u8>>>>,
    compose_to: RefCell<Option<gtk::Entry>>,
    history: RefCell<Vec<Message>>,
    reader_mode: Cell<bool>,
    compact_message: Cell<bool>,
    compact_navigation: Cell<bool>,
    contact_requested: Cell<bool>,
    reader_top_pending: Cell<bool>,
    notifications: Cell<bool>,
    notified: RefCell<std::collections::HashMap<String, i64>>,
    notified_followups: RefCell<std::collections::HashSet<String>>,
    notification_previews: Cell<bool>,
    open_check_in_flight: Cell<bool>,
    last_open_check: Cell<i64>,
    notification_generation: Cell<u64>,
    notification_since: Cell<i64>,
    started_at: i64,
    theme_mode: RefCell<String>,
    css: gtk::CssProvider,
    compose_actions: RefCell<std::collections::HashMap<String, gtk::Button>>,
    compose_fields: RefCell<std::collections::HashMap<String, gtk::Widget>>,
    command: gtk::Entry,
    status: gtk::Label,
    accounts_box: gtk::Box,
    views_box: gtk::Box,
    splits_box: gtk::Box,
    labels_box: gtk::Box,
    labels_heading: gtk::Box,
    view: RefCell<MailView>,
    inbox_label: gtk::Label,
    thread_actions: gtk::Box,
    conversation: gtk::Box,
    attachments: gtk::Box,
    attachments_scroll: gtk::ScrolledWindow,
    current_message: RefCell<Option<Message>>,
    active_draft: RefCell<Option<String>>,
    reply_thread: RefCell<Option<String>>,
    last_queued_draft: RefCell<Option<String>>,
    undo_button: gtk::Button,
    inbox_sync_running: Arc<AtomicBool>,
    queue_retry_at: Cell<u64>,
}
// Release even if opening the service fails or the worker unwinds.
struct InboxSyncGuard(Arc<AtomicBool>);
impl Drop for InboxSyncGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl Ui {
    fn request(&self, op: Value) -> Result<Value, String> {
        self.service.request_json(&op.to_string())
    }
    fn background(&self, op: Value) {
        let sync_guard = if op["op"].as_str() == Some("sync_all") {
            if self
                .inbox_sync_running
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }
            Some(InboxSyncGuard(self.inbox_sync_running.clone()))
        } else {
            None
        };
        let path = self.database_path.clone();
        let completed = self.completed.clone();
        thread::spawn(move || {
            let _sync_guard = sync_guard;
            let result = MailService::open(path)
                .map_err(|error| error.to_string())
                .and_then(|service| service.request_json(&op.to_string()));
            let _ = completed.send(result);
        });
    }
    // Optional network enrichment is only useful after navigation settles.
    // Cached bodies still render synchronously, before any provider requests.
    fn settled_attachment(self: &Rc<Self>, message_id: &str, event: AttachmentEvent) {
        let weak = Rc::downgrade(self);
        let id = message_id.to_owned();
        glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
            if let Some(ui) = weak.upgrade() {
                if ui
                    .current_message
                    .borrow()
                    .as_ref()
                    .is_some_and(|m| m.id == id)
                {
                    ui.background_attachment(event);
                }
            }
        });
    }
    fn background_attachment(&self, event: AttachmentEvent) {
        let path = self.database_path.clone();
        let completed = self.attachment_completed.clone();
        thread::spawn(move || {
            let (op, make_event): (
                Value,
                Box<dyn FnOnce(Result<Value, String>) -> AttachmentEvent>,
            ) = match event {
                AttachmentEvent::Portrait {
                    account_id,
                    address,
                    result,
                } => {
                    let authorize = result.ok().is_some_and(|v| v["authorize"] == true);
                    let result = MailService::open(&path).and_then(|service| {
                        if authorize { service.request_json(&json!({"op":"enable_google_photos","account_id":account_id}).to_string())?; }
                        service.request_json(&json!({"op":"google_contact_photo","account_id":account_id,"address":address}).to_string())
                    });
                    let _ = completed.send(AttachmentEvent::Portrait {
                        account_id,
                        address,
                        result,
                    });
                    return;
                }
                AttachmentEvent::Headers { thread_id, .. } => (
                    json!({"op":"refresh_recipient_headers","thread_id":thread_id}),
                    Box::new(move |result| AttachmentEvent::Headers { thread_id, result }),
                ),
                AttachmentEvent::InlineImages { message_id, .. } => {
                    let op = json!({"op":"render_message","message_id":message_id,"include_inline_images":true});
                    (
                        op,
                        Box::new(move |result| AttachmentEvent::InlineImages {
                            message_id,
                            result,
                        }),
                    )
                }
                AttachmentEvent::List { message_id, .. } => {
                    let op = json!({"op":"list_attachments","message_id":message_id});
                    (
                        op,
                        Box::new(move |result| AttachmentEvent::List { message_id, result }),
                    )
                }
                AttachmentEvent::Preview {
                    message_id,
                    part_id,
                    filename,
                    mime_type,
                    result: _,
                } => {
                    let op =
                        json!({"op":"fetch_attachment","message_id":message_id,"part_id":part_id});
                    (
                        op,
                        Box::new(move |result| AttachmentEvent::Preview {
                            message_id,
                            part_id,
                            filename,
                            mime_type,
                            result,
                        }),
                    )
                }
            };
            let result = MailService::open(path)
                .map_err(|e| e.to_string())
                .and_then(|service| service.request_json(&op.to_string()));
            let _ = completed.send(make_event(result));
        });
    }
    fn accounts(&self) -> Vec<Account> {
        self.request(json!({"op":"list_accounts"}))
            .and_then(|value| serde_json::from_value(value).map_err(|e| e.to_string()))
            .unwrap_or_default()
    }
    fn selected(&self) -> Option<usize> {
        self.list.selected_row()?.widget_name().parse().ok()
    }
    fn thread_summaries(&self, view: &MailView) -> Result<Vec<ThreadSummary>, String> {
        let key = self.search_scope_key();
        if let Some((cached_key, result)) = self.search_preview.borrow_mut().take() {
            if cached_key == key {
                return result;
            }
        }
        let request = parity::thread_search_request(
            view,
            self.search.text().as_str(),
            self.inbox_filter.get(),
            500,
            0,
        );
        parity::search_thread_pages(&self.service, request, self.loaded_limit.get())
    }
    fn search_scope_key(&self) -> String {
        format!(
            "{:?}:{}:{}:{}",
            self.view.borrow(),
            self.inbox_filter.get(),
            self.loaded_limit.get(),
            self.search.text()
        )
    }
    fn set_view(self: &Rc<Self>, view: MailView) {
        self.activity_navigation.borrow_mut().take();
        self.workspace.set_visible_child_name("mail");
        if matches!(view, MailView::Followups) {
            self.inbox_filter.set(0);
        }
        self.search_generation
            .set(self.search_generation.get().wrapping_add(1));
        self.search_preview.borrow_mut().take();
        self.compact_navigation.set(false);
        self.compact_message.set(false);
        *self.view.borrow_mut() = view;
        self.loaded_limit.set(500);
        self.can_load_older.set(true);
        self.refresh();
        if matches!(*self.view.borrow(), MailView::Archive) {
            self.loading_older.set(true);
            self.older_count_before.set(self.items.borrow().len());
            self.status.set_text("Loading archived mail…");
            self.background(
                json!({"op":"sync_archives","restart": !self.archive_browse_started.replace(true)}),
            );
        }
    }
    /// Calendar keeps its own account list and saved events: update it as soon
    /// as an account is added or removed (macOS `accountsChanged`).
    fn refresh_calendar_accounts(&self) {
        let calendar = self.calendar.borrow().clone();
        if let Some(calendar) = calendar {
            calendar.accounts_changed();
        }
    }
    /// After removing an account: back to the unified inbox, but stay in
    /// Calendar, now without that account, while one can still be shown.
    fn account_removed(self: &Rc<Self>) {
        let in_calendar = self.workspace.visible_child_name().as_deref() == Some("calendar");
        self.refresh_calendar_accounts();
        self.set_view(MailView::Inbox);
        if in_calendar && self.calendar_available(&self.accounts()) {
            agenda::open(self);
        }
    }
    /// A calendar connects only to an account that signed in with Google
    /// (`connect_calendar`), which this build may not offer (macOS
    /// `calendarAvailable`).
    fn calendar_available(&self, accounts: &[Account]) -> bool {
        !accounts.is_empty()
            && (account_setup::build_features(&self.service).google_oauth_public
                || accounts.iter().any(|account| account.provider == Provider::Gmail))
    }
    /// Show the mail chrome only once there is an account (macOS
    /// `updateAccountChrome`): until then the window is a welcome.
    fn update_account_chrome(&self, accounts: &[Account]) {
        let empty = accounts.is_empty();
        for widget in &self.account_only {
            widget.set_visible(!empty);
        }
        if empty {
            self.labels_heading.set_visible(false);
            if let Some(button) = self.filter_buttons.get(1) {
                button.set_label("Unread");
            }
        }
        self.mail_stack
            .set_visible_child_name(if empty { "welcome" } else { "columns" });
    }
    fn refresh_navigation(self: &Rc<Self>, accounts: &[Account]) {
        let counts = self.request(json!({"op":"unread_counts"})).unwrap_or_default();
        let (unread, followup_due, followup_active) = parity::navigation_counts(&counts);
        while let Some(child) = self.accounts_box.first_child() {
            self.accounts_box.remove(&child);
        }
        for account in accounts {
            let count = counts["accounts"][&account.id].as_u64().unwrap_or(0);
            let button = navigation_button(
                "avatar-default-symbolic",
                &account.email,
                &if count > 0 {
                    count.to_string()
                } else {
                    String::new()
                },
            );
            button.add_css_class("side-item");
            button.set_tooltip_text(Some(&account.email));
            let ui = self.clone();
            let id = account.id.clone();
            button.connect_clicked(move |_| ui.set_view(MailView::Account(id.clone())));
            self.accounts_box.append(&button);
        }
        while let Some(child) = self.views_box.first_child() {
            self.views_box.remove(&child);
        }
        let fixed = [
            ("mail-inbox-symbolic", "All Inboxes", "i", MailView::Inbox),
            ("mail-archive-symbolic", "Archive", "a", MailView::Archive),
            ("alarm-symbolic", "Snoozed", "s", MailView::Snoozed),
            (
                "mail-reply-symbolic",
                "Follow-ups",
                "w",
                MailView::Followups,
            ),
            ("document-edit-symbolic", "Drafts", "d", MailView::Drafts),
            ("mail-send-symbolic", "Sent", "t", MailView::Sent),
            ("mail-send-symbolic", "Outbox", "o", MailView::Outbox),
        ];
        for (icon, name, shortcut, view) in fixed {
            let count = match &view { MailView::Inbox => Some(unread), MailView::Followups => Some(followup_due), _ => None };
            let badge = if self.goto_pending.get() { shortcut.to_string() } else { count.map(|n| n.to_string()).unwrap_or_default() };
            let button = navigation_button(icon, name, &badge);
            let tooltip = match &view {
                MailView::Inbox => format!("{unread} unread conversations in downloaded inboxes · G then I"),
                MailView::Followups => format!("{followup_due} due · {followup_active} active follow-ups · G then W"),
                _ => format!("{name} · G then {}", shortcut.to_uppercase()),
            };
            button.set_tooltip_text(Some(&tooltip));
            if self.workspace.visible_child_name().as_deref() == Some("mail")
                && std::mem::discriminant(&view) == std::mem::discriminant(&*self.view.borrow())
            {
                button.add_css_class("active");
            }
            button.add_css_class("side-item");
            let ui = self.clone();
            button.connect_clicked(move |_| ui.set_view(view.clone()));
            self.views_box.append(&button);
        }
        let invitations = navigation_button(
            "x-office-calendar-symbolic",
            "Calendar",
            if self.goto_pending.get() { "c" } else { "" },
        );
        if self.workspace.visible_child_name().as_deref() == Some("calendar") {
            invitations.add_css_class("active");
        }
        invitations.set_tooltip_text(Some("Calendar · G then C"));
        invitations.set_visible(self.calendar_available(accounts));
        {
            let ui = self.clone();
            invitations.connect_clicked(move |_| agenda::open(&ui));
        }
        self.views_box.append(&invitations);
        while let Some(child) = self.splits_box.first_child() {
            self.splits_box.remove(&child);
        }
        if let Ok(value) = self.request(json!({"op":"list_splits"})) {
            for split in serde_json::from_value::<Vec<SplitInbox>>(value).unwrap_or_default() {
                let button = navigation_button("folder-saved-search-symbolic", &split.name, "");
                button.add_css_class("side-item");
                button.set_tooltip_text(Some(&format!("Filter: {}", split.query)));
                let ui = self.clone();
                let id = split.id;
                button.connect_clicked(move |_| ui.set_view(MailView::Split(id.clone())));
                self.splits_box.append(&button);
            }
        }
        while let Some(child) = self.labels_box.first_child() {
            self.labels_box.remove(&child);
        }
        if let Ok(value) = self.request(json!({"op":"list_labels"})) {
            for label in serde_json::from_value::<Vec<LabelSummary>>(value).unwrap_or_default() {
                if [
                    "STARRED", "INBOX", "UNREAD", "SENT", "DRAFT", "TRASH", "SPAM",
                ]
                .iter()
                .any(|system| label.name.eq_ignore_ascii_case(system))
                {
                    continue;
                }
                let button =
                    navigation_button("tag-symbolic", &label.name, &label.count.to_string());
                button.add_css_class("side-item");
                let ui = self.clone();
                let name = label.name;
                button.connect_clicked(move |_| {
                    ui.search.set_text("");
                    ui.set_view(MailView::Label(name.clone()));
                });
                self.labels_box.append(&button);
            }
        }
        self.labels_heading
            .set_visible(self.labels_box.first_child().is_some());
    }
    fn refresh(self: &Rc<Self>) {
        // A local action or sync refresh invalidates older search snapshots too.
        self.search_generation
            .set(self.search_generation.get().wrapping_add(1));
        let scope = self.search_scope_key();
        if *self.marked_scope.borrow() != scope {
            *self.marked_scope.borrow_mut() = scope;
            self.marked.borrow_mut().clear();
        }
        let previous_id = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).map(|m| m.thread_id.clone()));
        let accounts = self.accounts();
        let counts = self
            .request(json!({"op":"unread_counts"}))
            .unwrap_or_default();
        if let Some(button) = self.filter_buttons.get(1) {
            button.set_label(&format!("Unread {}", counts["total"].as_u64().unwrap_or(0)));
            button.set_tooltip_text(Some("Unread conversations in downloaded inboxes"));
        }
        self.refresh_navigation(&accounts);
        self.update_account_chrome(&accounts);
        for (i, button) in self.filter_buttons.iter().enumerate() {
            if i == self.inbox_filter.get() as usize {
                button.add_css_class("active");
            } else {
                button.remove_css_class("active");
            }
        }
        let view = self.view.borrow().clone();
        let scope_name = match &view {
            MailView::Label(name) => Some(name.clone()),
            MailView::Account(id) => accounts
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.email.clone()),
            MailView::Split(id) => self
                .request(json!({"op":"list_splits"}))
                .ok()
                .and_then(|v| serde_json::from_value::<Vec<SplitInbox>>(v).ok())
                .and_then(|rows| rows.into_iter().find(|r| &r.id == id))
                .map(|r| r.name),
            _ => None,
        };
        self.inbox_label
            .set_text(scope_name.as_deref().unwrap_or(match &view {
                MailView::Inbox => "Inbox",
                MailView::Account(_) => "Account inbox",
                MailView::Split(_) => "Split inbox",
                MailView::Label(_) => "Label",
                MailView::Snoozed => "Snoozed",
                MailView::Followups => "Follow-ups",
                MailView::Archive => "Archive",
                MailView::Drafts => "Drafts",
                MailView::Outbox => "Outbox",
                MailView::Sent => "Sent",
            }));
        if let Some(button) = self
            .thread_actions
            .first_child()
            .and_then(|v| v.next_sibling())
            .and_then(|v| v.downcast::<gtk::Button>().ok())
        {
            button.set_tooltip_text(Some(if matches!(view, MailView::Archive) {
                "Move to Inbox · Shift+E"
            } else {
                "Archive · E"
            }));
            button.set_child(Some(&mail_icon(if matches!(view, MailView::Archive) {
                "mail-restore-symbolic"
            } else {
                "mail-archive-symbolic"
            })));
        }
        self.undo_button.set_visible(
            matches!(view, MailView::Outbox) || self.last_queued_draft.borrow().is_some(),
        );
        self.undo_button
            .set_label(if matches!(view, MailView::Outbox) {
                "Cancel scheduled send"
            } else {
                "Undo send  Ctrl+Z"
            });
        let op = match &view {
            MailView::Inbox
            | MailView::Account(_)
            | MailView::Split(_)
            | MailView::Label(_)
            | MailView::Snoozed
            | MailView::Followups
            | MailView::Archive
            | MailView::Sent => Value::Null,
            MailView::Drafts => json!({"op":"list_drafts"}),
            MailView::Outbox => json!({"op":"list_outbox"}),
        };
        let mut fresh = if matches!(view, MailView::Drafts) {
            self.request(op)
                .ok()
                .and_then(|v| serde_json::from_value::<Vec<Message>>(v).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|message| Mail {
                    thread_id: message.id,
                    account_id: message.account_id,
                    account: "Draft".into(),
                    sender: parity::draft_recipient_summary(
                        &message.to.join(", "),
                        &message
                            .recipient_headers
                            .as_ref()
                            .map(|h| h.cc.join(", "))
                            .unwrap_or_default(),
                        &message
                            .recipient_headers
                            .as_ref()
                            .map(|h| h.bcc.join(", "))
                            .unwrap_or_default(),
                    ),
                    subject: message.subject,
                    preview: message.body_plain,
                    received: "Draft".into(),
                    unread: false,
                    labels: Vec::new(),
                })
                .collect::<Vec<_>>()
        } else if matches!(view, MailView::Outbox) {
            self.request(op)
                .ok()
                .and_then(|v| serde_json::from_value::<Vec<OutboxItem>>(v).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|item| Mail {
                    thread_id: item.message_id,
                    account_id: item.account_id,
                    account: item.status.clone(),
                    sender: item.to.join(", "),
                    subject: item.subject,
                    preview: item.body_preview,
                    received: item.status,
                    unread: false,
                    labels: Vec::new(),
                })
                .collect::<Vec<_>>()
        } else {
            let summaries = match self.thread_summaries(&view) {
                Ok(rows) => rows,
                Err(error) => {
                    self.status.set_text(&format!("Search: {error}"));
                    self.search
                        .set_tooltip_text(Some(&format!("{error}\n{}", parity::SEARCH_HELP)));
                    return;
                }
            };
            if !self.search.text().trim().is_empty() {
                self.status.set_text(&format!(
                    "{} downloaded conversations · Esc clears search",
                    summaries.len()
                ));
            } else if self.status.text().starts_with("Searching")
                || self.status.text().starts_with("Search:")
            {
                self.status.set_text("Downloaded mail");
            }
            self.search.set_tooltip_text(Some(parity::SEARCH_HELP));
            summaries
                .into_iter()
                .map(|thread| {
                    let account = accounts
                        .iter()
                        .find(|a| a.id == thread.account_id)
                        .map(|a| a.display_name.clone())
                        .unwrap_or_else(|| thread.account_id.clone());
                    Mail {
                        thread_id: thread.id,
                        account_id: thread.account_id,
                        account,
                        sender: thread.sender,
                        subject: thread.subject,
                        preview: thread.snippet,
                        received: message_time(thread.last_message_at),
                        unread: thread.unread_count > 0,
                        labels: thread.labels,
                    }
                })
                .collect::<Vec<_>>()
        };
        // Explicit activity navigation may target a sent conversation older than
        // the currently loaded page. Keep that exact cached target selectable.
        if matches!(view, MailView::Sent) && self.search.text().trim().is_empty() && self.inbox_filter.get() == 0 {
            if let Some(message) = self.activity_navigation.borrow().as_ref() {
                if !fresh.iter().any(|row| row.thread_id == message.thread_id) {
                    fresh.push(Mail {
                        thread_id: message.thread_id.clone(), account_id: message.account_id.clone(),
                        account: accounts.iter().find(|a| a.id == message.account_id).map(|a| a.display_name.clone()).unwrap_or_else(|| message.account_id.clone()),
                        sender: message.sender.clone(), subject: message.subject.clone(), preview: message.body_plain.chars().take(240).collect(),
                        received: message_time(message.received_at), unread: false, labels: Vec::new(),
                    });
                }
            }
        }
        *self.items.borrow_mut() = fresh;
        let visible: std::collections::HashSet<String> = self
            .items
            .borrow()
            .iter()
            .map(|m| m.thread_id.clone())
            .collect();
        self.marked.borrow_mut().retain(|id| visible.contains(id));
        self.row_checks.borrow_mut().clear();
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        for (i, item) in self.items.borrow().iter().enumerate() {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&i.to_string());
            let content = gtk::Box::new(Orientation::Vertical, 0);
            if !item.unread {
                content.add_css_class("read");
            }
            let top = gtk::Box::new(Orientation::Horizontal, 7);
            let check = gtk::CheckButton::new();
            check.add_css_class("mail-selection");
            check.set_active(self.marked.borrow().contains(&item.thread_id));
            check.set_tooltip_text(Some("Select conversation · X · Shift+J/K extends"));
            let id = item.thread_id.clone();
            self.row_checks
                .borrow_mut()
                .insert(id.clone(), check.clone());
            let weak = Rc::downgrade(self);
            check.connect_toggled(move |button| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                if ui.updating_checks.get() {
                    return;
                }
                if button.is_active() {
                    ui.marked.borrow_mut().insert(id.clone());
                } else {
                    ui.marked.borrow_mut().remove(&id);
                }
                ui.update_selection_bar();
            });
            top.append(&check);
            let dot = gtk::Label::new(Some(if item.unread { "●" } else { "" }));
            dot.add_css_class("unread-dot");
            dot.set_width_chars(1);
            top.append(&dot);
            let sender = gtk::Label::new(Some(sender_name(&item.sender)));
            sender.set_ellipsize(gtk::pango::EllipsizeMode::End);
            sender.set_tooltip_text(Some(&item.sender));
            sender.add_css_class("sender");
            sender.set_xalign(0.0);
            sender.set_hexpand(true);
            let account = gtk::Label::new(Some(&item.received));
            account.add_css_class("account");
            top.append(&sender);
            let star = gtk::Button::new();
            star.add_css_class("row-star");
            let starred = item
                .labels
                .iter()
                .any(|label| label.eq_ignore_ascii_case("STARRED"));
            star.set_child(Some(&mail_icon(if starred {
                "star-filled"
            } else {
                "star-symbolic"
            })));
            star.set_tooltip_text(Some("Toggle favorite · F"));
            let weak = Rc::downgrade(self);
            star.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.list.select_row(ui.list.row_at_index(i as i32).as_ref());
                    ui.favorite();
                }
            });
            top.append(&star);
            top.append(&account);
            let subject = gtk::Label::new(Some(&item.subject));
            subject.add_css_class("subject");
            subject.set_xalign(0.0);
            subject.set_margin_start(14);
            subject.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let preview = gtk::Label::new(Some(&item.preview));
            preview.add_css_class("preview");
            preview.set_xalign(0.0);
            preview.set_margin_start(14);
            preview.set_ellipsize(gtk::pango::EllipsizeMode::End);
            content.append(&top);
            content.append(&subject);
            content.append(&preview);
            row.set_child(Some(&content));
            self.list.append(&row);
            let menu = gtk::Popover::new();
            menu.set_parent(&row);
            let choices = gtk::Box::new(Orientation::Vertical, 2);
            menu.set_child(Some(&choices));
            let gesture = gtk::GestureClick::new();
            gesture.set_button(3);
            let weak = Rc::downgrade(self);
            let r = row.clone();
            let clicked_id = item.thread_id.clone();
            gesture.connect_pressed(move |_, _, x, y| {
                if let Some(ui) = weak.upgrade() {
                    if !ui.marked.borrow().is_empty() && !ui.marked.borrow().contains(&clicked_id) {
                        ui.clear_marked();
                    }
                    ui.list.select_row(Some(&r));
                    while let Some(child) = choices.first_child() {
                        choices.remove(&child);
                    }
                    let count = ui.marked.borrow().len();
                    let actions = if matches!(*ui.view.borrow(), MailView::Drafts) {
                        parity::draft_context_menu_actions()
                    } else {
                        parity::context_menu_actions(
                            count,
                            matches!(*ui.view.borrow(), MailView::Archive),
                        )
                    };
                    for (id, title) in actions {
                        let button = gtk::Button::with_label(&title);
                        let weak = Rc::downgrade(&ui);
                        let menu = menu.clone();
                        button.connect_clicked(move |_| {
                            menu.popdown();
                            if let Some(ui) = weak.upgrade() {
                                ui.execute_command(id);
                            }
                        });
                        choices.append(&button);
                    }
                    menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                    menu.popup();
                }
            });
            row.add_controller(gesture);
            if previous_id.as_ref() == Some(&item.thread_id) {
                self.list.select_row(Some(&row));
            }
        }
        if self.list.selected_row().is_none() {
            if let Some(row) = self.list.row_at_index(0) {
                self.list.select_row(Some(&row));
            } else {
                *self.current_message.borrow_mut() = None;
                self.contact.show_open_activity(None);
                while let Some(child) = self.attachments.first_child() {
                    self.attachments.remove(&child);
                }
                self.attachments_scroll.set_visible(false);
                while let Some(child) = self.conversation.first_child() {
                    self.conversation.remove(&child);
                }
                self.sender_label.set_text("");
                self.thread_actions.set_visible(false);
                self.recipient_button.set_visible(false);
                self.recipient_details.set_text("");
                self.contact.root.set_visible(false);
                self.title.set_text(if matches!(view, MailView::Sent) { "No sent mail cached yet" } else { "Inbox zero" });
                self.meta.set_text("Nothing matches your search.");
                self.show_plain(if matches!(view, MailView::Sent) {
                    "Messages sent from Email appear here after delivery. Only locally available sent mail is shown."
                } else {
                    "Your inbox is clear."
                });
            }
        }
        self.update_selection_bar();
    }
    fn show_selected(self: &Rc<Self>) {
        if self.reader_covered() {
            return;
        }
        let Some(i) = self.selected() else {
            return;
        };
        let items = self.items.borrow();
        let Some(item) = items.get(i) else {
            return;
        };
        self.reader_mode.set(false);
        self.set_shortcuts(false);
        self.title.set_text(&item.subject);
        self.meta.set_text(&format!(
            "{}  ·  {}  ·  {}",
            item.sender, item.account, item.received
        ));
        while let Some(child) = self.conversation.first_child() {
            self.conversation.remove(&child);
        }
        let view = self.view.borrow().clone();
        let messages = if matches!(view, MailView::Drafts | MailView::Outbox) {
            self.request(json!({"op":"get_message","id":item.thread_id}))
                .ok()
                .and_then(|v| serde_json::from_value::<Option<Message>>(v).ok())
                .flatten()
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            self.request(json!({"op":"list_thread_messages","thread_id":item.thread_id}))
                .ok()
                .and_then(|v| serde_json::from_value::<Vec<Message>>(v).ok())
                .unwrap_or_default()
        };
        self.favorite_button.set_child(Some(&mail_icon(
            if item.labels.iter().any(|l| l == "STARRED") {
                "star-filled"
            } else {
                "star-symbolic"
            },
        )));
        self.thread_actions
            .set_visible(!matches!(view, MailView::Drafts | MailView::Outbox));
        *self.history.borrow_mut() = messages.clone();
        self.conversation.set_visible(messages.len() > 1);
        for message in &messages {
            let button = gtk::Button::with_label(&format!(
                "{}  ·  {}",
                sender_name(&message.sender),
                message_time(message.received_at)
            ));
            button.add_css_class("conversation-item");
            let ui = self.clone();
            let message = message.clone();
            button.connect_clicked(move |_| ui.show_message(&message));
            self.conversation.append(&button);
        }
        if let Some(message) = messages.last() {
            self.show_message(message);
        } else {
            self.show_plain("Message body is not cached yet.");
        }
    }
    fn show_recipient_headers(&self, message: &Message) {
        self.recipient_button.set_visible(true);
        let to = if message.to.is_empty() {
            "Not specified".into()
        } else {
            message.to.join(", ")
        };
        let cc = message
            .recipient_headers
            .as_ref()
            .filter(|h| !h.cc.is_empty())
            .map(|h| format!("  ·  Cc {}", h.cc.join(", ")))
            .unwrap_or_default();
        self.sender_label.set_text(&message.sender);
        self.meta.set_text(&format!(
            "To {}{}  ·  {}",
            to,
            cc,
            message_time(message.received_at)
        ));
        let mut details = format!("From: {}\nTo: {}", message.sender, to);
        if let Some(headers) = &message.recipient_headers {
            for (label, values) in [
                ("Cc", &headers.cc),
                ("Bcc", &headers.bcc),
                ("Reply-To", &headers.reply_to),
            ] {
                if !values.is_empty() {
                    details.push_str(&format!("\n{label}: {}", values.join(", ")));
                }
            }
        }
        let date = glib::DateTime::from_unix_local(message.received_at / 1000)
            .and_then(|date| date.format("%A, %e %B %Y at %H:%M:%S %Z"))
            .map(|text| text.to_string())
            .unwrap_or_default();
        details.push_str(&format!("\nDate: {date}"));
        self.recipient_details.set_text(&details);
    }
    fn show_message(self: &Rc<Self>, message: &Message) {
        self.recipient_button.popdown();
        *self.current_message.borrow_mut() = Some(message.clone());
        // Loading images "in this email" lasts while that message is shown, also
        // across a refresh that re-selects it; any other message starts over.
        if self.remote_images_message.borrow().as_deref() != Some(message.id.as_str()) {
            self.allow_remote_images
                .set(self.remote_images_default.get());
            *self.remote_images_message.borrow_mut() = Some(message.id.clone());
        }
        self.title.set_text(&message.subject);
        self.show_recipient_headers(message);
        let can_unsubscribe = message
            .unsubscribe_header
            .as_ref()
            .is_some_and(|h| !h.is_empty())
            || message
                .body_html
                .as_deref()
                .and_then(unsubscribe_url)
                .is_some();
        self.unsubscribe_button.set_visible(can_unsubscribe);
        self.refresh_contact();
        if message.recipient_headers.is_none()
            && !message.is_draft
            && !message.account_id.starts_with("demo:")
        {
            self.settled_attachment(
                &message.id,
                AttachmentEvent::Headers {
                    thread_id: message.thread_id.clone(),
                    result: Ok(Value::Null),
                },
            );
        }
        let rendered = self.request(json!({"op":"render_message","message_id":message.id}));
        if let Some(html) = rendered
            .ok()
            .and_then(|v| v["body_html_safe"].as_str().map(ToString::to_string))
        {
            self.show_html(&html);
            if message
                .body_html
                .as_ref()
                .is_some_and(|body| body.to_ascii_lowercase().contains("cid:"))
            {
                self.settled_attachment(
                    &message.id,
                    AttachmentEvent::InlineImages {
                        message_id: message.id.clone(),
                        result: Ok(Value::Null),
                    },
                );
            }
        } else {
            // Plain mail uses the same reader typography and left gutter as HTML,
            // while preserving sender-authored whitespace and escaping all markup.
            self.show_html(&format!("<div class=\"plain-text\">{}</div>", glib::markup_escape_text(&message.body_plain)));
        }
        while let Some(child) = self.attachments.first_child() {
            self.attachments.remove(&child);
        }
        self.attachments_scroll
            .set_visible(message.has_attachments == Some(true));
        if message.has_attachments != Some(false) {
            if message.has_attachments == Some(true) {
                self.attachments
                    .append(&gtk::Label::new(Some("Checking attachments…")));
            }
            self.settled_attachment(
                &message.id,
                AttachmentEvent::List {
                    message_id: message.id.clone(),
                    result: Ok(Value::Null),
                },
            );
        }
    }
    fn show_attachments(self: &Rc<Self>, message_id: &str, result: Result<Value, String>) {
        match result {
            Ok(value) => {
                while let Some(child) = self.attachments.first_child() {
                    self.attachments.remove(&child);
                }
                let files = value.as_array().cloned().unwrap_or_default();
                self.attachments_scroll.set_visible(!files.is_empty());
                if files.is_empty() {
                    return;
                }
                for file in files {
                    let Some(part_id) = file["part_id"].as_str() else {
                        continue;
                    };
                    let name = file["filename"]
                        .as_str()
                        .unwrap_or("Attachment")
                        .to_string();
                    let mime = file["mime_type"]
                        .as_str()
                        .unwrap_or("application/octet-stream")
                        .to_string();
                    let size = file["size"].as_u64().unwrap_or(0);
                    let button = gtk::Button::with_label(&format!(
                        "📎  {name}  ·  {} KB",
                        size.div_ceil(1024)
                    ));
                    button.add_css_class("action");
                    let ui = self.clone();
                    let id = message_id.to_string();
                    let part = part_id.to_string();
                    button
                        .connect_clicked(move |_| ui.preview_attachment(&id, &part, &name, &mime));
                    self.attachments.append(&button);
                }
            }
            Err(error) => {
                self.attachments_scroll.set_visible(false);
                self.status
                    .set_text(&format!("Could not list attachments: {error}"));
            }
        }
    }
    fn preview_attachment(
        self: &Rc<Self>,
        message_id: &str,
        part_id: &str,
        filename: &str,
        mime: &str,
    ) {
        self.status.set_text(&format!("Loading {filename}…"));
        self.background_attachment(AttachmentEvent::Preview {
            message_id: message_id.to_string(),
            part_id: part_id.to_string(),
            filename: filename.to_string(),
            mime_type: mime.to_string(),
            result: Ok(Value::Null),
        });
    }
    fn present_attachment(
        self: &Rc<Self>,
        filename: &str,
        mime: &str,
        result: Result<Value, String>,
    ) {
        let Ok(value) = result else {
            self.status.set_text("Could not fetch attachment.");
            return;
        };
        let Some(encoded) = value["data_base64"].as_str() else {
            self.status.set_text("Attachment data unavailable.");
            return;
        };
        let Ok(bytes) = STANDARD.decode(encoded) else {
            self.status.set_text("Attachment encoding invalid.");
            return;
        };
        if bytes.len() > 20 * 1024 * 1024 {
            self.status.set_text("Attachment too large for preview.");
            return;
        }
        let dialog = gtk::Window::builder()
            .transient_for(&self.window)
            .title(filename)
            .default_width(720)
            .default_height(560)
            .modal(true)
            .build();
        let layout = gtk::Box::new(Orientation::Vertical, 8);
        layout.set_margin_top(12);
        layout.set_margin_bottom(12);
        layout.set_margin_start(12);
        layout.set_margin_end(12);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);
        if mime.starts_with("image/") {
            match gdk::Texture::from_bytes(&glib::Bytes::from(&bytes)) {
                Ok(texture) => {
                    let image = gtk::Picture::for_paintable(&texture);
                    image.set_can_shrink(true);
                    scroll.set_child(Some(&image));
                }
                Err(_) => {
                    scroll.set_child(Some(&gtk::Label::new(Some("Image preview unavailable."))));
                }
            }
        } else if mime.starts_with("text/") && bytes.len() <= 1024 * 1024 {
            let text = gtk::TextView::new();
            text.set_editable(false);
            text.set_wrap_mode(gtk::WrapMode::WordChar);
            text.buffer().set_text(&String::from_utf8_lossy(&bytes));
            scroll.set_child(Some(&text));
        } else {
            scroll.set_child(Some(&gtk::Label::new(Some(
                "Preview unavailable for this file type. Save it explicitly to inspect it.",
            ))));
        }
        let save = gtk::Button::with_label("Save attachment…");
        save.add_css_class("action");
        let name = filename.to_string();
        let dialog_for_click = dialog.clone();
        save.connect_clicked(move |_| {
            let chooser = gtk::FileDialog::new();
            chooser.set_initial_name(Some(&name));
            let bytes = bytes.clone();
            let dialog_for_save = dialog_for_click.clone();
            chooser.save(
                Some(&dialog_for_save),
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            let _ = std::fs::write(path, &bytes);
                        }
                    }
                },
            );
        });
        layout.append(&scroll);
        layout.append(&save);
        dialog.set_child(Some(&layout));
        dialog.present();
        self.status.set_text("");
    }
    /// A new-message composer covers the reader (a reply leaves it visible).
    /// Reader updates keep their state but stay hidden; close_compose shows them.
    fn reader_covered(&self) -> bool {
        self.compose_pane.borrow().is_some() && self.compose_covers_reader.get()
    }
    fn show_plain(&self, text: &str) {
        *self.current_html.borrow_mut() = None;
        self.remote_images.set_visible(false);
        self.remote_images_notice.set_visible(false);
        // Nothing is shown: the next message starts from the default again.
        *self.remote_images_message.borrow_mut() = None;
        self.plain.buffer().set_text(text);
        if self.reader_covered() {
            return;
        }
        if let Some(reader) = self.reader.borrow().as_ref() {
            reader.set_visible(false);
        }
        self.plain_scroll.set_visible(true);
    }
    fn show_html(&self, html: &str) {
        if self.reader_covered() {
            // Keep the latest body for when the composer closes.
            *self.current_html.borrow_mut() = Some(html.to_owned());
            return;
        }
        if self.current_html.borrow().as_deref() == Some(html)
            && self.rendered_remote_images.get() == self.allow_remote_images.get()
            && self
                .reader
                .borrow()
                .as_ref()
                .is_some_and(|r| r.is_visible())
        {
            return;
        }
        *self.current_html.borrow_mut() = Some(html.to_owned());
        self.render_html(html);
    }
    fn render_html(&self, html: &str) {
        if self.reader_covered() {
            return;
        }
        let has_remote_images = has_remote_images(html);
        self.remote_images.set_visible(has_remote_images);
        self.remote_images_notice
            .set_visible(has_remote_images && !self.allow_remote_images.get());
        self.load_remote_images
            .set_tooltip_text(Some(&load_images_tooltip(self.remote_images_default.get())));
        self.rendered_remote_images.set(self.allow_remote_images.get());
        self.remote_images
            .set_child(Some(&mail_icon("image-x-generic-symbolic")));
        self.remote_images
            .set_tooltip_text(Some(&remote_images_tooltip(
                self.allow_remote_images.get(),
                self.remote_images_default.get(),
            )));
        if self.reader.borrow().is_none() {
            let reader = new_html_reader();
            self.reading_content
                .insert_child_after(&reader, Some(&self.plain_scroll));
            *self.reader.borrow_mut() = Some(reader);
        }
        self.plain_scroll.set_visible(false);
        if let Some(reader) = self.reader.borrow().as_ref() {
            reader.set_visible(true);
            reader.set_background_color(
                &gdk::RGBA::parse(if self.dark.get() {
                    "#1c1c1e"
                } else {
                    "#ffffff"
                })
                .expect("reader color"),
            );
            reader.load_html(
                &document(html, self.allow_remote_images.get(), self.dark.get()),
                None,
            );
        }
    }
    /// Public builds offer activity only for tracked sent mail already in the
    /// cache. Checked at launch and when the palette opens until found.
    fn check_saved_open_activity(&self) {
        // Public builds cannot create tracked mail, so once offered it stays.
        if OPEN_TRACKING || self.offers_open_activity.get() {
            return;
        }
        // One indexed EXISTS query on the shell's own service connection.
        let offered = self
            .request(json!({"op":"tracking_capability"}))
            .ok()
            .and_then(|value| value["saved_activity"].as_bool())
            .unwrap_or(false);
        // An open palette keeps its rows (and selection); the next one lists it.
        self.offers_open_activity.set(offered);
        self.tracking_activity.set_visible(offered);
    }
    fn load_remote_images_for_message(&self) {
        if self.allow_remote_images.get() {
            return;
        }
        // Keyboard activation leaves focus on the button that is about to hide;
        // give it to an open reply, else the message list, as macOS does.
        let return_focus = self.remote_images_notice.focus_child().is_some();
        self.toggle_remote_images_for_message();
        if return_focus {
            let body = self.compose_fields.borrow().get("body").cloned();
            match body {
                Some(body) => {
                    body.grab_focus();
                }
                None => {
                    self.list.grab_focus();
                }
            }
        }
    }
    fn toggle_remote_images_for_message(&self) {
        self.allow_remote_images.set(!self.allow_remote_images.get());
        if let Some(html) = self.current_html.borrow().clone() {
            self.render_html(&html);
        }
    }
    /// Same choices as the macOS image menu: this email, or the remembered default.
    fn show_remote_images_menu(self: &Rc<Self>) {
        let choices = gtk::Box::new(Orientation::Vertical, 2);
        choices.add_css_class("remote-images-menu");
        let message_title = if self.allow_remote_images.get() {
            "Block images in this email"
        } else {
            "Load images in this email"
        };
        let default_title = if self.remote_images_default.get() {
            "Block remote images by default"
        } else {
            "Load remote images by default"
        };
        for (for_message, title) in [(true, message_title), (false, default_title)] {
            let button = gtk::Button::with_label(title);
            if let Some(label) = button.child().and_downcast::<gtk::Label>() {
                label.set_xalign(0.0);
            }
            let weak = Rc::downgrade(self);
            let menu = self.remote_images_menu.clone();
            button.connect_clicked(move |_| {
                menu.popdown();
                if let Some(ui) = weak.upgrade() {
                    if for_message {
                        ui.toggle_remote_images_for_message();
                    } else {
                        ui.toggle_remote_images();
                    }
                }
            });
            choices.append(&button);
        }
        self.remote_images_menu.set_child(Some(&choices));
        self.remote_images_menu.popup();
    }
    fn toggle_remote_images(&self) {
        let enabled = !self.remote_images_default.get();
        self.allow_remote_images.set(enabled);
        self.remote_images_default.set(enabled);
        if let Err(error) = std::fs::write(
            self.database_path.with_file_name("remote-images-enabled"),
            if enabled { "1\n" } else { "0\n" },
        ) {
            self.status
                .set_text(&format!("Could not save image preference: {error}"));
        }
        if let Some(html) = self.current_html.borrow().clone() {
            self.render_html(&html);
        }
    }
    fn unsubscribe(self: &Rc<Self>) {
        let header_destination = self
            .current_message
            .borrow()
            .as_ref()
            .and_then(|m| m.unsubscribe_header.as_ref())
            .and_then(|header| {
                header.split(',').find_map(|s| {
                    Url::parse(s.trim().trim_matches(['<', '>']))
                        .ok()
                        .filter(|u| {
                            ["https", "mailto"].contains(&u.scheme())
                                && reader_links::external_uri(u.as_str()).is_some()
                        })
                })
            });
        let destination = header_destination.or_else(|| {
            self.current_html
                .borrow()
                .as_deref()
                .and_then(unsubscribe_url)
        });
        // mailto links go through the same sanitizer as reader link clicks.
        let Some(destination) =
            destination.and_then(|url| reader_links::external_uri(url.as_str()))
        else {
            self.status
                .set_text("No unsubscribe link was found in this message.");
            return;
        };
        let dialog = gtk::Window::builder()
            .transient_for(&self.window)
            .title("Open unsubscribe link?")
            .modal(true)
            .default_width(460)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 12);
        content.set_margin_top(20);
        content.set_margin_bottom(20);
        content.set_margin_start(20);
        content.set_margin_end(20);
        let explanation = gtk::Label::new(Some(
            "The link will open in your browser. Some senders ask you to confirm there.",
        ));
        explanation.set_wrap(true);
        explanation.set_xalign(0.0);
        let target = gtk::Label::new(Some(destination.as_str()));
        target.set_wrap(true);
        target.set_selectable(true);
        target.set_xalign(0.0);
        let buttons = gtk::Box::new(Orientation::Horizontal, 8);
        let cancel = gtk::Button::with_label("Cancel");
        let open = gtk::Button::with_label("Open unsubscribe link");
        open.add_css_class("action");
        let dialog_for_cancel = dialog.clone();
        cancel.connect_clicked(move |_| dialog_for_cancel.close());
        let dialog_for_open = dialog.clone();
        let status = self.status.clone();
        let uri = destination.to_string();
        open.connect_clicked(move |_| {
            if let Err(error) =
                gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>)
            {
                status.set_text(&format!("Could not open unsubscribe link: {error}"));
            }
            dialog_for_open.close();
        });
        buttons.append(&cancel);
        buttons.append(&open);
        content.append(&explanation);
        content.append(&target);
        content.append(&buttons);
        dialog.set_child(Some(&content));
        dialog.present();
    }
    fn restore_triage_undo(&self) {
        *self.last_triage.borrow_mut() = self
            .request(json!({"op":"latest_triage_undo"}))
            .ok()
            .and_then(|v| {
                let id = v["operation_id"].as_str()?.to_string();
                let accounts =
                    serde_json::from_value::<Vec<String>>(v["account_ids"].clone()).ok()?;
                Some((id, accounts))
            });
    }
    fn update_selection_bar(&self) {
        if matches!(*self.view.borrow(), MailView::Drafts) {
            let discarded = self.last_discarded_draft.borrow().is_some();
            self.selection_bar.set_visible(discarded);
            self.selection_count.set_text("Draft discarded");
            for (i, button) in self.selection_buttons.iter().enumerate() {
                button.set_visible(i == 3);
            }
            self.selection_buttons[3].set_tooltip_text(Some("Restore the discarded draft · Z"));
            return;
        }
        self.selection_buttons[3].set_tooltip_text(None);
        self.selection_buttons[4].set_visible(true);
        let active = !self.marked.borrow().is_empty();
        let visible = !matches!(*self.view.borrow(), MailView::Drafts | MailView::Outbox)
            && (active || self.last_triage.borrow().is_some());
        self.selection_bar.set_visible(visible);
        self.selection_count.set_text(&if active {
            format!("{} selected", self.marked.borrow().len())
        } else {
            "Saved locally".into()
        });
        for i in [0, 1, 2] {
            self.selection_buttons[i].set_visible(
                active && (i != 2 || !matches!(*self.view.borrow(), MailView::Archive)),
            );
        }
        self.selection_buttons[3].set_visible(self.last_triage.borrow().is_some());
        self.updating_checks.set(true);
        for (id, check) in self.row_checks.borrow().iter() {
            check.set_active(self.marked.borrow().contains(id));
        }
        self.updating_checks.set(false);
        if active {
            self.shortcut_bar.set_text("X  Select     Shift J K  Extend     M U  Read / unread     E  Archive     Esc  Clear");
        }
    }
    fn toggle_marked(self: &Rc<Self>) {
        let Some(item) = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned())
        else {
            return;
        };
        if self.marked.borrow().contains(&item.thread_id) {
            self.marked.borrow_mut().remove(&item.thread_id);
        } else {
            self.marked.borrow_mut().insert(item.thread_id);
        }
        self.update_selection_bar();
    }
    fn extend_marked(self: &Rc<Self>, delta: i32) {
        if let Some(item) = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned())
        {
            self.marked.borrow_mut().insert(item.thread_id);
        }
        self.move_by(delta);
        if let Some(item) = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned())
        {
            self.marked.borrow_mut().insert(item.thread_id);
        }
        self.update_selection_bar();
    }
    fn select_loaded(self: &Rc<Self>) {
        if matches!(*self.view.borrow(), MailView::Drafts | MailView::Outbox) {
            return;
        }
        *self.marked.borrow_mut() = self
            .items
            .borrow()
            .iter()
            .map(|m| m.thread_id.clone())
            .collect();
        self.update_selection_bar();
        self.status.set_text(&format!(
            "{} loaded conversations selected",
            self.marked.borrow().len()
        ));
    }
    fn clear_marked(&self) {
        if self.marked.borrow().is_empty() {
            self.last_triage.borrow_mut().take();
        }
        self.marked.borrow_mut().clear();
        self.update_selection_bar();
        self.set_shortcuts(self.reader_mode.get());
        self.list.grab_focus();
    }
    fn selection_triage(self: &Rc<Self>, kind: &str) {
        if matches!(*self.view.borrow(), MailView::Drafts | MailView::Outbox) {
            return;
        }
        let targets: Vec<Mail> = if self.marked.borrow().is_empty() {
            self.selected()
                .and_then(|i| self.items.borrow().get(i).cloned())
                .into_iter()
                .collect()
        } else {
            self.items
                .borrow()
                .iter()
                .filter(|m| self.marked.borrow().contains(&m.thread_id))
                .cloned()
                .collect()
        };
        if targets.is_empty() {
            return;
        }
        let ids: Vec<String> = targets.iter().map(|m| m.thread_id.clone()).collect();
        let accounts: Vec<String> = targets
            .iter()
            .map(|m| m.account_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let op = operation_id();
        match self
            .request(json!({"op":"batch_triage","operation_id":op,"kind":kind,"thread_ids":ids}))
        {
            Ok(result) => {
                if result["undoable"] == true {
                    *self.last_triage.borrow_mut() = Some((op, accounts.clone()));
                } else {
                    self.restore_triage_undo();
                }
                self.marked.borrow_mut().clear();
                self.refresh();
                self.status.set_text(&format!(
                    "{} conversations updated · saved locally",
                    targets.len()
                ));
                for account in accounts.into_iter().filter(|id| !id.starts_with("demo:")) {
                    self.background(json!({"op":"sync_account","account_id":account}));
                }
            }
            Err(e) => self.status.set_text(&format!("Selection unchanged: {e}")),
        }
    }
    fn undo_triage(self: &Rc<Self>) {
        if matches!(*self.view.borrow(), MailView::Drafts) {
            self.undo_discard_draft();
            return;
        }
        let Some((op, accounts)) = self.last_triage.borrow().clone() else {
            return;
        };
        match self.request(json!({"op":"undo_triage","operation_id":op})) {
            Ok(_) => {
                self.restore_triage_undo();
                self.refresh();
                self.status.set_text("Action undone · saved locally");
                for account in accounts.into_iter().filter(|id| !id.starts_with("demo:")) {
                    self.background(json!({"op":"sync_account","account_id":account}));
                }
            }
            Err(e) => self.status.set_text(&format!("Could not undo: {e}")),
        }
    }
    /// Discard the selected draft without confirmation; Z restores it.
    fn discard_selected_draft(self: &Rc<Self>) {
        if !matches!(*self.view.borrow(), MailView::Drafts) || self.compose_pane.borrow().is_some() {
            return;
        }
        let Some(index) = self.selected() else { return };
        let Some(item) = self.items.borrow().get(index).cloned() else { return };
        match self.request(json!({"op":"discard_draft","message_id":item.thread_id})) {
            Ok(_) => self.draft_discarded(item.thread_id, item.subject, Some(index)),
            Err(error) => self
                .status
                .set_text(&format!("Draft kept · could not discard: {error}")),
        }
    }
    /// Shared by the list and the composer. Keeps the selection on the draft
    /// that took the discarded one's place.
    fn draft_discarded(self: &Rc<Self>, id: String, subject: String, index: Option<usize>) {
        *self.last_discarded_draft.borrow_mut() = Some((id, subject));
        if !matches!(*self.view.borrow(), MailView::Drafts) {
            self.status
                .set_text("Draft discarded · Undo from Drafts (G D, then Z)");
            self.refresh();
            return;
        }
        let index = index.or_else(|| self.selected()).unwrap_or(0);
        self.refresh();
        let count = self.items.borrow().len();
        if count > 0 {
            if let Some(row) = self.list.row_at_index(index.min(count - 1) as i32) {
                self.list.select_row(Some(&row));
            }
        }
        self.update_selection_bar();
        self.set_shortcuts(false);
        self.status.set_text("Draft discarded · Z to undo");
        self.list.grab_focus();
    }
    fn undo_discard_draft(self: &Rc<Self>) {
        let Some((id, _)) = self.last_discarded_draft.borrow().clone() else {
            return;
        };
        let result = self.request(json!({"op":"restore_discarded_draft","message_id":id}));
        self.last_discarded_draft.borrow_mut().take();
        match result {
            Ok(_) => {
                self.search.set_text("");
                self.refresh();
                let position = self.items.borrow().iter().position(|m| m.thread_id == id);
                if let Some(row) = position.and_then(|i| self.list.row_at_index(i as i32)) {
                    self.list.select_row(Some(&row));
                }
                self.status.set_text("Draft restored");
            }
            Err(error) => self
                .status
                .set_text(&format!("Could not restore the draft: {error}")),
        }
        self.update_selection_bar();
        self.set_shortcuts(false);
        self.list.grab_focus();
    }
    fn archive(self: &Rc<Self>) {
        self.selection_triage(if matches!(*self.view.borrow(), MailView::Archive) {
            "restore"
        } else {
            "archive"
        });
    }
    fn favorite(self: &Rc<Self>) {
        let Some(item) = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned())
        else {
            return;
        };
        let action = if item
            .labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case("STARRED"))
        {
            MailAction::RemoveLabel {
                thread_id: item.thread_id,
                label: "STARRED".into(),
            }
        } else {
            MailAction::AddLabel {
                thread_id: item.thread_id,
                label: "STARRED".into(),
            }
        };
        self.apply_action(action, &item.account_id);
    }
    fn bulk_read_sender(self: &Rc<Self>) {
        let Some(item) = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned())
        else {
            return;
        };
        let gmail = self.accounts().iter().any(|a| {
            a.id == item.account_id && format!("{:?}", a.provider).to_lowercase().contains("gmail")
        });
        if !gmail || item.account_id.starts_with("demo:") {
            self.bulk_read_sender_downloaded();
            return;
        }
        // Open the provider-wide preview directly. Scanning changes no mail;
        // the cleanup panel still requires an explicit count confirmation.
        cleanup::open(self, Some((item.account_id, item.sender)));
    }

    fn bulk_read_sender_downloaded(self: &Rc<Self>) {
        let Some(item) = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned())
        else {
            return;
        };
        let ids = match self.request(
            json!({"op":"unread_from_sender","account_id":item.account_id,"sender":item.sender}),
        ) {
            Ok(value) => value.as_array().cloned().unwrap_or_default(),
            Err(e) => {
                self.status.set_text(&e);
                return;
            }
        };
        if ids.is_empty() {
            self.status
                .set_text("No unread downloaded conversations from this sender.");
            return;
        }
        let dialog = gtk::Window::builder()
            .title("Mark sender as read")
            .transient_for(&self.window)
            .modal(true)
            .default_width(440)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 16);
        for set in [content.upcast_ref::<gtk::Widget>()] {
            set.set_margin_top(24);
            set.set_margin_bottom(24);
            set.set_margin_start(24);
            set.set_margin_end(24);
        }
        let title = gtk::Label::new(Some(&format!("Mark {} conversations as read?", ids.len())));
        title.add_css_class("title-3");
        let detail = gtk::Label::new(Some(&format!(
            "From {} in this account. Includes all downloaded matching conversations, beyond the visible page. Mail not downloaded yet is excluded. Changes sync to your provider in the background.",
            item.sender
        )));
        detail.set_wrap(true);
        detail.set_max_width_chars(54);
        content.append(&title);
        content.append(&detail);
        let buttons = gtk::Box::new(Orientation::Horizontal, 12);
        let cancel = gtk::Button::with_label("Cancel");
        let close = dialog.clone();
        cancel.connect_clicked(move |_| close.close());
        let confirm = gtk::Button::with_label("Mark all as read");
        confirm.add_css_class("suggested-action");
        let ui = self.clone();
        let close = dialog.clone();
        confirm.connect_clicked(move |_| {
            match ui.request(json!({"op":"bulk_mark_read","operation_id":format!("bulk:{}",operation_id()),"account_id":item.account_id,"thread_ids":ids})) {
                Ok(_)=>{ui.refresh();ui.status.set_text("Marked read; changes queued for sync.");if !item.account_id.starts_with("demo:"){ui.background(json!({"op":"sync_account","account_id":item.account_id}));}},
                Err(e)=>ui.status.set_text(&e)
            }
            close.close();
        });
        buttons.append(&cancel);
        buttons.append(&confirm);
        content.append(&buttons);
        dialog.set_child(Some(&content));
        let keys = gtk::EventControllerKey::new();
        let close = dialog.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                close.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.add_controller(keys);
        dialog.present();
        cancel.grab_focus();
    }
    fn mark(self: &Rc<Self>, unread: bool) {
        if !self.marked.borrow().is_empty() {
            self.selection_triage(if unread { "unread" } else { "read" });
            return;
        }

        let selected = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned());
        if let Some(item) = selected {
            self.apply_action(
                MailAction::MarkRead {
                    thread_id: item.thread_id,
                    read: !unread,
                },
                &item.account_id,
            );
        }
    }
    fn label_thread(self: &Rc<Self>, remove: bool) {
        let Some(item) = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned())
        else {
            return;
        };
        let dialog = gtk::Window::builder()
            .transient_for(&self.window)
            .title(if remove { "Remove label" } else { "Add label" })
            .modal(true)
            .default_width(360)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 10);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("Label name"));
        if remove {
            if let Some(label) = item.labels.first() {
                entry.set_text(label);
            }
        }
        let button = gtk::Button::with_label(if remove { "Remove" } else { "Add" });
        button.add_css_class("action");
        let ui = self.clone();
        let dialog_for_click = dialog.clone();
        let entry_for_click = entry.clone();
        button.connect_clicked(move |_| {
            let label = entry_for_click.text().trim().to_string();
            if label.is_empty() {
                return;
            }
            let action = if remove {
                MailAction::RemoveLabel {
                    thread_id: item.thread_id.clone(),
                    label,
                }
            } else {
                MailAction::AddLabel {
                    thread_id: item.thread_id.clone(),
                    label,
                }
            };
            ui.apply_action(action, &item.account_id);
            dialog_for_click.close();
        });
        content.append(&entry);
        content.append(&button);
        dialog.set_child(Some(&content));
        dialog.present();
    }
    fn snooze(self: &Rc<Self>) {
        let Some(item) = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned())
        else {
            return;
        };
        let dialog = gtk::Window::builder()
            .transient_for(&self.window)
            .title("Snooze email")
            .modal(true)
            .default_width(390)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 10);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        let heading = gtk::Label::new(Some("When should this email return?"));
        heading.set_xalign(0.0);
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("5m, 3h, 1d, or 1w"));
        entry.set_text("1d");
        let summary = gtk::Label::new(Some("In 1 day"));
        summary.set_xalign(0.0);
        summary.add_css_class("hint");
        let summary_for_change = summary.clone();
        entry.connect_changed(move |entry| {
            let label = snooze_duration(entry.text().as_str())
                .map(|(_, label)| label)
                .unwrap_or_else(|| "Enter a duration up to one year".to_string());
            summary_for_change.set_text(&label);
        });
        let presets = gtk::Box::new(Orientation::Horizontal, 6);
        for (label, duration) in [
            ("5 min", "5m"),
            ("3 hours", "3h"),
            ("24 hours", "1d"),
            ("Next week", "1w"),
        ] {
            let button = gtk::Button::with_label(label);
            let entry_for_button = entry.clone();
            button.connect_clicked(move |_| entry_for_button.set_text(duration));
            presets.append(&button);
        }
        let confirm = gtk::Button::with_label("Snooze");
        confirm.add_css_class("action");
        let ui = self.clone();
        let dialog_for_confirm = dialog.clone();
        let entry_for_confirm = entry.clone();
        let item_for_confirm = item.clone();
        confirm.connect_clicked(move |_| {
            let Some((seconds, label)) = snooze_duration(entry_for_confirm.text().as_str()) else {
                ui.status
                    .set_text("Enter a duration such as 5m, 3h, 1d, or 1w.");
                return;
            };
            ui.snooze_for(&item_for_confirm, seconds, &label);
            dialog_for_confirm.close();
        });
        let confirm_for_entry = confirm.clone();
        entry.connect_activate(move |_| confirm_for_entry.emit_clicked());
        content.append(&heading);
        content.append(&entry);
        content.append(&summary);
        content.append(&presets);
        content.append(&confirm);
        dialog.set_child(Some(&content));
        dialog.present();
        entry.grab_focus();
    }
    fn snooze_for(self: &Rc<Self>, item: &Mail, seconds: u64, label: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let wake_at_ms = now + (seconds * 1000) as i64;
        match self.request(
            json!({"op":"snooze_thread","thread_id":item.thread_id,"wake_at_ms":wake_at_ms}),
        ) {
            Ok(_) => {
                self.status.set_text(&format!("Snoozed. Back {label}."));
                self.refresh();
            }
            Err(error) => self.status.set_text(&format!("Snooze failed: {error}")),
        }
    }
    fn unsnooze(self: &Rc<Self>) {
        let Some(item) = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned())
        else {
            return;
        };
        match self.request(json!({"op":"unsnooze_thread","thread_id":item.thread_id})) {
            Ok(_) => {
                self.status.set_text("Moved back to inbox.");
                self.refresh();
            }
            Err(error) => self.status.set_text(&format!("Unsnooze failed: {error}")),
        }
    }
    fn create_split(self: &Rc<Self>) {
        let dialog = gtk::Window::builder()
            .transient_for(&self.window)
            .title("Split inboxes")
            .modal(true)
            .default_width(480)
            .build();
        let layout = gtk::Box::new(Orientation::Vertical, 12);
        for side in [
            gtk::PositionType::Top,
            gtk::PositionType::Bottom,
            gtk::PositionType::Left,
            gtk::PositionType::Right,
        ] {
            match side {
                gtk::PositionType::Top => layout.set_margin_top(24),
                gtk::PositionType::Bottom => layout.set_margin_bottom(24),
                gtk::PositionType::Left => layout.set_margin_start(24),
                _ => layout.set_margin_end(24),
            }
        }
        let explanation = gtk::Label::new(Some(
            "Focus on matching mail automatically. Splits are saved views; messages stay in your inbox.",
        ));
        explanation.set_wrap(true);
        explanation.set_max_width_chars(54);
        explanation.set_xalign(0.0);
        layout.append(&explanation);
        let existing = self
            .request(json!({"op":"list_splits"}))
            .ok()
            .and_then(|v| serde_json::from_value::<Vec<SplitInbox>>(v).ok())
            .unwrap_or_default();
        let editing = Rc::new(RefCell::new(None::<String>));
        let choose = gtk::ComboBoxText::new();
        choose.append_text("Create a new split");
        for split in &existing {
            choose.append_text(&format!("Edit {} · {}", split.name, split.query));
        }
        choose.set_active(Some(0));
        layout.append(&choose);
        let name = gtk::Entry::new();
        name.set_placeholder_text(Some("Name · e.g. Work"));
        let query = gtk::Entry::new();
        query.set_placeholder_text(Some("*@example.com, *@example.org"));
        layout.append(&name);
        layout.append(&query);
        let syntax = gtk::Label::new(Some(
            "Separate alternatives with commas. * matches any text. Addresses match senders and recipients. Also supports from:, to:, subject: and label:.",
        ));
        syntax.set_wrap(true);
        syntax.set_max_width_chars(54);
        syntax.set_xalign(0.0);
        syntax.add_css_class("hint");
        layout.append(&syntax);
        if let Some(mail) = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned())
        {
            let address = mail
                .sender
                .split('<')
                .nth(1)
                .and_then(|v| v.split('>').next())
                .unwrap_or(&mail.sender)
                .trim()
                .to_string();
            let shortcuts = gtk::Box::new(Orientation::Horizontal, 8);
            let sender = gtk::Button::with_label("This sender");
            let entry = query.clone();
            let rule = format!("from:{address}");
            sender.connect_clicked(move |_| entry.set_text(&rule));
            shortcuts.append(&sender);
            if let Some(domain) = address.split('@').nth(1) {
                let button = gtk::Button::with_label(&format!("Anyone at {domain}"));
                let entry = query.clone();
                let rule = format!("*@{domain}");
                button.connect_clicked(move |_| entry.set_text(&rule));
                shortcuts.append(&button);
            }
            layout.append(&shortcuts);
        }
        let preview = gtk::Label::new(Some("Enter rules to preview cached matches."));
        preview.set_xalign(0.0);
        preview.set_wrap(true);
        preview.set_max_width_chars(54);
        preview.set_height_request(90);
        layout.append(&preview);
        let save = gtk::Button::with_label("Create split");
        save.set_sensitive(false);
        let ui = self.clone();
        let result = preview.clone();
        let button = save.clone();
        query.connect_changed(move |entry| {
            let text = entry.text();
            if text.trim().is_empty() {
                result.set_text("Enter rules to preview cached matches.");
                button.set_sensitive(false);
                return;
            }
            match ui.request(json!({"op":"preview_split", "query":text.as_str()})) {
                Ok(value) => {
                    let rows = value.as_array().cloned().unwrap_or_default();
                    let subjects = rows
                        .iter()
                        .take(3)
                        .filter_map(|r| r["subject"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    result.set_text(&format!(
                        "{}{} matching conversations in cached mail\n{}",
                        rows.len(),
                        if rows.len() == 500 { "+" } else { "" },
                        subjects
                    ));
                    button.set_sensitive(true);
                }
                Err(error) => {
                    result.set_text(&error);
                    button.set_sensitive(false);
                }
            }
        });
        let entry_name = name.clone();
        let entry_query = query.clone();
        let selected_id = editing.clone();
        let button = save.clone();
        choose.connect_changed(move |combo| {
            let split = combo
                .active()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| existing.get(i as usize));
            *selected_id.borrow_mut() = split.map(|s| s.id.clone());
            entry_name.set_text(split.map(|s| s.name.as_str()).unwrap_or(""));
            entry_query.set_text(split.map(|s| s.query.as_str()).unwrap_or(""));
            button.set_label(if split.is_some() {
                "Save changes"
            } else {
                "Create split"
            });
        });
        let selected_id_for_delete = editing.clone();
        let ui = self.clone();
        let close = dialog.clone();
        save.connect_clicked(move |_| {
            let query = query.text().trim().to_string();
            let entered = name.text().trim().to_string();
            let chosen = if entered.is_empty() {
                query.chars().take(80).collect::<String>()
            } else {
                entered
            };
            match ui.request(
                json!({"op":"upsert_split", "id":*editing.borrow(), "name":chosen, "query":query}),
            ) {
                Ok(value) => {
                    if let Some(id) = value["id"].as_str() {
                        ui.set_view(MailView::Split(id.into()));
                    } else {
                        ui.refresh();
                    }
                    close.close();
                }
                Err(error) => preview.set_text(&error),
            }
        });
        layout.append(&save);
        let delete = gtk::Button::with_label("Delete selected split");
        delete.add_css_class("destructive-action");
        delete.set_sensitive(false);
        let delete_button = delete.clone();
        choose.connect_changed(move |combo| {
            delete_button.set_sensitive(combo.active().is_some_and(|i| i > 0))
        });
        let ui = self.clone();
        let close = dialog.clone();
        let selected = selected_id_for_delete;
        delete.connect_clicked(move |_| {
            if let Some(id) = selected.borrow().clone() {
                match ui.request(json!({"op":"delete_split","id":id})) {
                    Ok(_) => {
                        ui.set_view(MailView::Inbox);
                        close.close();
                    }
                    Err(error) => ui.status.set_text(&error),
                }
            }
        });
        layout.append(&delete);
        dialog.set_child(Some(&layout));
        dialog.present();
    }
    fn apply_action(self: &Rc<Self>, action: MailAction, account_id: &str) {
        match self
            .request(json!({"op":"apply_action","operation_id":operation_id(),"action":action}))
        {
            Ok(_) => {
                self.refresh();
                if !account_id.starts_with("demo:") {
                    self.background(json!({"op":"sync_account","account_id":account_id}));
                }
            }
            Err(error) => self.status.set_text(&format!("Action failed: {error}")),
        }
    }
    fn sync_inbox_if_configured(&self) {
        if self
            .accounts()
            .iter()
            .any(|account| !account.id.starts_with("demo:"))
        {
            self.sync_all();
        }
    }
    fn sync_all(&self) {
        self.status.set_text("Syncing accounts…");
        self.background(json!({"op":"sync_all"}));
    }
    fn load_older(self: &Rc<Self>) {
        if self.loading_older.get()
            || !self.can_load_older.get()
            || self.items.borrow().is_empty()
            || matches!(
                *self.view.borrow(),
                MailView::Snoozed | MailView::Followups | MailView::Drafts | MailView::Outbox
            )
        {
            return;
        }
        self.loading_older.set(true);
        let previous = self.items.borrow().len();
        self.older_count_before.set(previous);
        self.loaded_limit
            .set(self.loaded_limit.get().saturating_add(100));
        self.refresh();
        if self.items.borrow().len() > previous {
            self.loading_older.set(false);
        } else {
            self.status.set_text("Loading older mail…");
            self.background(json!({"op":if matches!(*self.view.borrow(),MailView::Archive) { "sync_archives" } else { "sync_all" }}));
        }
    }
    fn add_account(self: &Rc<Self>) {
        account_setup::present(self);
    }
    fn move_by(&self, delta: i32) {
        let now = self.list.selected_row().map(|r| r.index()).unwrap_or(-1);
        if let Some(row) = self.list.row_at_index((now + delta).max(0)) {
            self.list.select_row(Some(&row));
        }
    }
    fn compose(self: &Rc<Self>, reply: bool) {
        self.compose_with_mode(reply, false);
    }

    fn compose_with_mode(self: &Rc<Self>, reply: bool, reply_all: bool) {
        self.compose_with_content(reply, reply_all, None);
    }

    fn compose_with_content(
        self: &Rc<Self>,
        reply: bool,
        reply_all: bool,
        support_message: Option<(&str, &str, &str)>,
    ) {
        self.workspace.set_visible_child_name("mail");
        let selected = self
            .selected()
            .and_then(|i| self.items.borrow().get(i).cloned());
        let reply_message = if reply {
            self.current_message.borrow().clone().filter(|message| {
                selected
                    .as_ref()
                    .is_some_and(|mail| mail.thread_id == message.thread_id)
            })
        } else {
            None
        };
        let mut accounts = self.accounts();
        if accounts.is_empty() {
            self.status.set_text("Add an account before composing.");
            return;
        }
        self.close_compose();
        *self.reply_thread.borrow_mut() = reply_message.as_ref().map(|m| m.thread_id.clone());
        self.compact_navigation.set(false);
        self.compact_message.set(true);
        let existing = if matches!(*self.view.borrow(), MailView::Drafts) && !reply && support_message.is_none() {
            selected
                .as_ref()
                .and_then(|mail| {
                    self.request(json!({"op":"get_message","id":mail.thread_id}))
                        .ok()
                })
                .and_then(|v| serde_json::from_value::<Option<Message>>(v).ok())
                .flatten()
        } else {
            None
        };
        if let Some(draft) = &existing {
            accounts.retain(|account| account.id == draft.account_id);
        }
        *self.active_draft.borrow_mut() = existing.as_ref().map(|m| m.id.clone());
        if !reply {
            self.title.set_text("New message");
            self.meta.set_text("Compose");
            self.remote_images_notice.set_visible(false);
            self.plain_scroll.set_visible(false);
            if let Some(reader) = self.reader.borrow().as_ref() {
                reader.set_visible(false);
            }
        }
        let layout = gtk::Box::new(Orientation::Vertical, 8);
        layout.add_css_class("composer");
        layout.set_vexpand(!reply);
        layout.set_margin_top(if reply { 10 } else { 24 });
        layout.set_margin_bottom(16);
        layout.set_margin_start(24);
        layout.set_margin_end(24);
        self.compose_covers_reader.set(!reply);
        if !reply {
            self.reader_header.set_visible(false);
        }
        self.shortcut_bar.set_text(
            "Tab  Next field     Shift Tab  Previous     Ctrl ↵  Send     Esc  Save and close",
        );
        let account_picker = gtk::ComboBoxText::new();
        for account in &accounts {
            account_picker.append(
                Some(&account.id),
                &format!("{}  ·  {}", account.display_name, account.email),
            );
        }
        account_picker.set_active_id(
            reply_message
                .as_ref()
                .map(|m| m.account_id.as_str())
                .or_else(|| selected.as_ref().map(|m| m.account_id.as_str()))
                .or_else(|| accounts.first().map(|a| a.id.as_str())),
        );
        let own_addresses = parity::own_addresses(self, &accounts);
        let preferred = existing
            .as_ref()
            .map(|draft| vec![draft.sender.clone()])
            .unwrap_or_else(|| {
                reply_message
                    .as_ref()
                    .map(|m| {
                        let mut addresses = if own_addresses
                            .iter()
                            .any(|a| a.eq_ignore_ascii_case(&parity::address(&m.sender)))
                        {
                            vec![m.sender.clone()]
                        } else {
                            m.to.clone()
                        };
                        if let Some(headers) = &m.recipient_headers {
                            addresses.extend(headers.cc.clone());
                        }
                        addresses
                    })
                    .unwrap_or_default()
            });
        if let Some(draft) = &existing {
            let identity = format!("{}\t{}", draft.account_id, parity::address(&draft.sender));
            account_picker.append(Some(&identity), &draft.sender);
            account_picker.set_active_id(Some(&identity));
        }
        parity::populate_identities(self, &account_picker, preferred, accounts.clone());
        let to = gtk::Entry::new();
        to.set_placeholder_text(Some("To"));
        let cc = gtk::Entry::new();
        cc.set_placeholder_text(Some("Cc"));
        let bcc = gtk::Entry::new();
        bcc.set_placeholder_text(Some("Bcc · hidden from other recipients"));
        let subject = gtk::Entry::new();
        subject.set_placeholder_text(Some("Subject"));
        if reply {
            if let Some(message) = &reply_message {
                let headers = message.recipient_headers.as_ref();
                let (targets, copies) = parity::reply_recipients(
                    &message.sender,
                    headers.map(|h| h.reply_to.as_slice()).unwrap_or(&[]),
                    &message.to,
                    headers.map(|h| h.cc.as_slice()).unwrap_or(&[]),
                    &own_addresses,
                    reply_all,
                );
                to.set_text(&targets.join(", "));
                cc.set_text(&copies.join(", "));
                subject.set_text(
                    &if message.subject.to_ascii_lowercase().starts_with("re:") {
                        message.subject.clone()
                    } else {
                        format!("Re: {}", message.subject)
                    },
                );
            } else if let Some(mail) = &selected {
                to.set_text(&mail.sender);
                subject.set_text(&format!("Re: {}", mail.subject));
            }
        }
        if let Some(draft) = &existing {
            to.set_text(&draft.to.join(", "));
            if let Some(headers) = &draft.recipient_headers {
                cc.set_text(&headers.cc.join(", "));
                bcc.set_text(&headers.bcc.join(", "));
            }
            subject.set_text(&draft.subject);
        }
        let body = gtk::TextView::new();
        body.set_wrap_mode(gtk::WrapMode::WordChar);
        body.set_vexpand(true);
        if let Some(draft) = &existing {
            body.buffer().set_text(&draft.body_plain);
        }
        if let Some((recipient, title, text)) = support_message {
            to.set_text(recipient);
            subject.set_text(title);
            body.buffer().set_text(text);
        }
        body.set_top_margin(8);
        body.set_bottom_margin(8);
        body.set_left_margin(6);
        body.set_right_margin(6);
        body.set_accepts_tab(false);
        let close = gtk::Button::with_label("×");
        close.add_css_class("compose-close");
        close.set_tooltip_text(Some("Save and close · Esc"));
        close.set_focusable(true);
        let discard = gtk::Button::new();
        discard.set_child(Some(&mail_icon("user-trash-symbolic")));
        discard.add_css_class("compose-tool");
        discard.set_tooltip_text(Some("Discard draft · undo from Drafts with Z"));
        discard.update_property(&[gtk::accessible::Property::Label("Discard draft")]);
        let save = gtk::Button::with_label("Save draft");
        save.add_css_class("compose-tool");
        save.set_tooltip_text(Some("Save draft · Ctrl+S"));
        let send = gtk::Button::with_label("Send");
        send.add_css_class("compose-send");
        send.set_tooltip_text(Some("Send · Ctrl+Enter"));
        let send_later = gtk::Button::with_label("Schedule…");
        send_later.add_css_class("compose-tool");
        let booking_link = gtk::Button::with_label("Booking link");
        booking_link.add_css_class("compose-tool");
        booking_link.set_tooltip_text(Some("Insert a booking link · also in Commands"));
        { let ui = self.clone(); let body = body.clone(); booking_link.connect_clicked(move |_|booking::open(&ui,Some(body.clone()))); }
        // Booking links and open tracking need the profile's Scheduling service.
        booking_link.set_visible(booking::configured());
        let signature = gtk::Button::with_label("Signature…");
        signature.add_css_class("compose-tool");
        signature.set_tooltip_text(Some("Edit or insert a signature for this From address"));
        let attach = gtk::Button::with_label("Attach file");
        attach.add_css_class("compose-tool");
        let attach_content = gtk::Box::new(Orientation::Horizontal, 5);
        attach_content.append(&mail_icon("mail-attachment-symbolic"));
        attach_content.append(&gtk::Label::new(Some("Attach file")));
        attach.set_child(Some(&attach_content));
        let attachment_chips = gtk::Box::new(Orientation::Vertical, 5);
        let heading = gtk::Box::new(Orientation::Horizontal, 4);
        let recipient_name = reply_message
            .as_ref()
            .map(|m| sender_name(&m.sender).to_string())
            .or_else(|| {
                selected
                    .as_ref()
                    .map(|m| sender_name(&m.sender).to_string())
            })
            .unwrap_or_default();
        let compose_title = gtk::Label::new(Some(&if reply_all {
            "Reply all".into()
        } else if reply {
            format!("Reply to {recipient_name}")
        } else {
            "New message".into()
        }));
        compose_title.add_css_class("composer-title");
        compose_title.set_xalign(0.0);
        compose_title.set_hexpand(true);
        compose_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        heading.append(&compose_title);
        let details_button =
            gtk::ToggleButton::with_label(if reply { "Details" } else { "Cc/Bcc" });
        details_button.add_css_class("compose-details-button");
        details_button.set_tooltip_text(Some("Edit sender, recipient and subject"));
        details_button.set_tooltip_text(Some(
            "Edit From, To, Cc, Bcc and subject · also available in Commands",
        ));
        heading.append(&details_button);
        heading.append(&close);
        let details = gtk::Box::new(Orientation::Vertical, 0);
        let details_panel: gtk::Widget = if reply {
            let scroll = gtk::ScrolledWindow::new();
            scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            scroll.set_propagate_natural_height(true);
            scroll.set_min_content_height(0);
            scroll.set_max_content_height(160);
            scroll.set_vexpand(false);
            scroll.set_child(Some(&details));
            scroll.set_visible(false);
            scroll.upcast()
        } else {
            details.clone().upcast()
        };
        let mut copy_rows = Vec::new();
        for (name, field) in [
            ("From", account_picker.clone().upcast::<gtk::Widget>()),
            ("To", to.clone().upcast()),
            ("Cc", cc.clone().upcast()),
            ("Bcc", bcc.clone().upcast()),
            ("Subject", subject.clone().upcast()),
        ] {
            let row = gtk::Box::new(Orientation::Horizontal, 10);
            row.add_css_class("compose-field");
            let label = gtk::Label::new(Some(name));
            label.add_css_class("compose-field-label");
            label.set_xalign(0.0);
            label.set_width_chars(7);
            label.set_mnemonic_widget(Some(&field));
            field.set_hexpand(true);
            row.append(&label);
            row.append(&field);
            if name == "Cc" || name == "Bcc" {
                row.set_visible(reply || !cc.text().is_empty() || !bcc.text().is_empty());
                copy_rows.push(row.clone());
            }
            details.append(&row);
        }
        details_button.set_active(!reply && (!cc.text().is_empty() || !bcc.text().is_empty()));
        if !reply && details_button.is_active() {
            details_button.set_label("Hide Cc/Bcc");
        }
        {
            let cc = cc.clone();
            let details_panel = details_panel.clone();
            let to = to.clone();
            let body = body.clone();
            let copy_rows = copy_rows.clone();
            details_button.connect_toggled(move |button| {
                if reply {
                    details_panel.set_visible(button.is_active());
                    button.set_label(if button.is_active() {
                        "Hide details"
                    } else {
                        "Details"
                    });
                } else {
                    button.set_label(if button.is_active() {
                        "Hide Cc/Bcc"
                    } else {
                        "Cc/Bcc"
                    });
                    for row in &copy_rows {
                        row.set_visible(button.is_active());
                    }
                }
                if button.is_active() {
                    if reply {
                        to.grab_focus();
                    } else {
                        cc.grab_focus();
                    }
                } else {
                    body.grab_focus();
                }
            });
        }
        let body_scroll = gtk::ScrolledWindow::new();
        body_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        body_scroll.set_propagate_natural_height(false);
        body_scroll.set_min_content_height(if reply { 112 } else { 180 });
        body_scroll.set_vexpand(true);
        body_scroll.set_child(Some(&body));
        let editor = gtk::Overlay::new();
        editor.set_child(Some(&body_scroll));
        editor.set_vexpand(!reply);
        let placeholder = gtk::Label::new(Some("Write your message…"));
        placeholder.add_css_class("compose-placeholder");
        placeholder.set_halign(gtk::Align::Start);
        placeholder.set_valign(gtk::Align::Start);
        placeholder.set_margin_start(6);
        placeholder.set_margin_top(8);
        placeholder.set_can_target(false);
        placeholder.set_visible(body.buffer().char_count() == 0);
        editor.add_overlay(&placeholder);
        body.buffer()
            .connect_changed(move |buffer| placeholder.set_visible(buffer.char_count() == 0));
        let tracking = gtk::CheckButton::with_label("Track opens");
        tracking.set_tooltip_text(Some("Privacy protection and mail scanners can affect open tracking."));
        let tracking_state = existing.as_ref().filter(|_| OPEN_TRACKING).and_then(|draft|self.request(json!({"op":"tracking_get","message_id":draft.id})).ok());
        // OPEN_TRACKING is the linked mail service's own profile value.
        tracking.set_active(tracking_state.as_ref().and_then(|value|value["enabled"].as_bool()).unwrap_or(false));
        {
            let weak=Rc::downgrade(self);
            tracking.connect_toggled(move |button|{
                if !button.is_active(){return;}
                let Some(ui)=weak.upgrade() else{return};
                let button=button.clone();let weak=Rc::downgrade(&ui);
                workflows::request_async(ui.database_path.clone(),json!({"op":"booking_status"}),move |result|{
                    let Some(ui)=weak.upgrade() else{return};
                    if !button.is_active(){return;}
                    match result {
                        Ok(value) if value["connected"].as_bool()==Some(true)=>{},
                        Ok(_)=>{button.set_active(false);ui.status.set_text("Connect Scheduling, then enable Track opens.");booking::open(&ui,None);},
                        Err(error)=>{button.set_active(false);ui.status.set_text(&format!("Could not check tracking access: {error}"));}
                    }
                });
            });
        }
        let buttons = gtk::Box::new(Orientation::Horizontal, 8);
        buttons.set_margin_top(6);
        buttons.append(&attach);
        buttons.append(&send_later);
        buttons.append(&signature);
        buttons.append(&booking_link);
        let footer_space = gtk::Box::new(Orientation::Horizontal, 0);
        footer_space.set_hexpand(true);
        buttons.append(&footer_space);
        buttons.append(&discard);
        buttons.append(&save);
        buttons.append(&send);
        layout.append(&heading);
        layout.append(&details_panel);
        layout.append(&editor);
        layout.append(&attachment_chips);
        // Public builds leave open tracking out; see OPEN_TRACKING.
        if OPEN_TRACKING {
            layout.append(&tracking);
        }
        layout.append(&buttons);
        self.reading_content.append(&layout);
        *self.compose_pane.borrow_mut() = Some(layout.clone());
        *self.compose_actions.borrow_mut() = [
            ("send", send.clone()),
            ("save", save.clone()),
            ("attach", attach.clone()),
            ("schedule", send_later.clone()),
            ("signature", signature.clone()),
            ("booking", booking_link.clone()),
            ("close", close.clone()),
            ("discard", discard.clone()),
        ]
        .into_iter()
        .map(|(id, b)| (id.into(), b))
        .collect();
        *self.compose_fields.borrow_mut() = [
            ("from", account_picker.clone().upcast::<gtk::Widget>()),
            ("to", to.clone().upcast()),
            ("cc", cc.clone().upcast()),
            ("bcc", bcc.clone().upcast()),
            ("subject", subject.clone().upcast()),
            ("body", body.clone().upcast()),
            ("details-panel", details_panel.clone()),
            ("details-toggle", details_button.clone().upcast()),
        ]
        .into_iter()
        .map(|(id, w)| (id.into(), w))
        .collect();
        if OPEN_TRACKING {
            self.compose_fields.borrow_mut().insert("tracking".into(), tracking.clone().upcast());
        }
        *self.compose_to.borrow_mut() = Some(to.clone());
        self.list.set_sensitive(false);
        let recipient_completions = recipient_completion::install(
            self,
            &account_picker,
            [to.clone(), cc.clone(), bcc.clone()],
        );
        let signatures = signature_editor::install(
            self,
            &account_picker,
            &body,
            &signature,
            &accounts,
            reply,
            existing.is_none(),
        );
        let composer_keys = gtk::EventControllerKey::new();
        composer_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let close = close.clone();
            let send = send.clone();
            let save = save.clone();
            composer_keys.connect_key_pressed(move |_, key, _, mods| {
                let key = parity::shortcut_key(key, mods);
                if key == gdk::Key::Escape {
                    if recipient_completions.dismiss() {
                        return glib::Propagation::Stop;
                    }
                    close.emit_clicked();
                    return glib::Propagation::Stop;
                }
                if mods.contains(gdk::ModifierType::CONTROL_MASK) {
                    if key == gdk::Key::Return || key == gdk::Key::KP_Enter {
                        send.emit_clicked();
                        return glib::Propagation::Stop;
                    }
                    if key == gdk::Key::s {
                        save.emit_clicked();
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            });
        }
        layout.add_controller(composer_keys);
        {
            // Discard without confirmation. Unsaved edits are saved first so
            // Undo in Drafts restores exactly what was on screen; an untouched
            // new message has nothing worth restoring and simply closes.
            let ui = self.clone();
            let account = account_picker.clone();
            let to = to.clone();
            let cc = cc.clone();
            let bcc = bcc.clone();
            let subject = subject.clone();
            let body = body.clone();
            let signatures = signatures.clone();
            discard.connect_clicked(move |_| {
                let has_content = ui.active_draft.borrow().is_some()
                    || signatures.has_user_content(&body)
                    || (!reply
                        && (!to.text().is_empty()
                            || !subject.text().is_empty()
                            || !cc.text().is_empty()
                            || !bcc.text().is_empty()));
                if !has_content {
                    ui.close_compose();
                    return;
                }
                let Some(id) = ui.save_compose_draft(&account, &to, &subject, &body) else {
                    return;
                };
                match ui.request(json!({"op":"discard_draft","message_id":id})) {
                    Ok(_) => {
                        let title = subject.text().to_string();
                        ui.close_compose();
                        ui.draft_discarded(id, title, None);
                    }
                    Err(error) => ui
                        .status
                        .set_text(&format!("Could not discard draft: {error}")),
                }
            });
        }
        {
            let ui = self.clone();
            let account = account_picker.clone();
            let to = to.clone();
            let subject = subject.clone();
            let body = body.clone();
            let cc = cc.clone();
            let bcc = bcc.clone();
            let initial_to = to.text().to_string();
            let initial_cc = cc.text().to_string();
            let initial_bcc = bcc.text().to_string();
            let copy_rows = copy_rows.clone();
            close.connect_clicked(move |_| {
                let has_changes = cc.text().as_str() != initial_cc
                    || bcc.text().as_str() != initial_bcc
                    || (!reply && (!cc.text().is_empty() || !bcc.text().is_empty()));
                let has_changes = has_changes
                    || signatures.has_user_content(&body)
                    || ui.active_draft.borrow().is_some()
                    || (!reply && (!to.text().is_empty() || !subject.text().is_empty()));
                if !has_changes {
                    ui.close_compose();
                    return;
                }
                // Closing saves silently, so a stray key that wiped every
                // address would otherwise be saved without the user noticing.
                if parity::recipients_cleared(
                    [initial_to.as_str(), initial_cc.as_str(), initial_bcc.as_str()],
                    [to.text().as_str(), cc.text().as_str(), bcc.text().as_str()],
                ) {
                    let confirm = gtk::AlertDialog::builder()
                        .message("Save this draft without recipients?")
                        .detail("To, Cc and Bcc are now empty. Restore the recipients it had when you opened it, or save it without any.")
                        .buttons(["Restore Recipients", "Save Without Recipients"])
                        .cancel_button(0)
                        .default_button(0)
                        .build();
                    let (ui, account, to, cc, bcc, subject, body, copy_rows) = (ui.clone(), account.clone(), to.clone(), cc.clone(), bcc.clone(), subject.clone(), body.clone(), copy_rows.clone());
                    let (initial_to, initial_cc, initial_bcc) = (initial_to.clone(), initial_cc.clone(), initial_bcc.clone());
                    let window = ui.window.clone();
                    confirm.choose(Some(&window), None::<&gtk::gio::Cancellable>, move |result| {
                        if result == Ok(1) {
                            if ui.save_compose_draft(&account, &to, &subject, &body).is_some() {
                                ui.close_compose();
                            }
                            return;
                        }
                        to.set_text(&initial_to);
                        cc.set_text(&initial_cc);
                        bcc.set_text(&initial_bcc);
                        if !initial_cc.is_empty() || !initial_bcc.is_empty() {
                            for row in &copy_rows { row.set_visible(true); }
                        }
                        body.grab_focus();
                    });
                    return;
                }
                if ui.save_compose_draft(&account, &to, &subject, &body).is_some() {
                    ui.close_compose();
                }
            });
        }
        if let Some(id) = self.active_draft.borrow().clone() {
            self.update_attachment_chips(&id, &attachment_chips);
        }
        {
            let ui = self.clone();
            let account = account_picker.clone();
            let to = to.clone();
            let subject = subject.clone();
            let body = body.clone();
            let chips = attachment_chips.clone();
            attach.connect_clicked(move |_| {
                let chooser = gtk::FileDialog::new();
                let ui = ui.clone(); let account = account.clone(); let to = to.clone(); let subject = subject.clone(); let body = body.clone(); let chips = chips.clone();
                let window = ui.window.clone();
                chooser.open(Some(&window), None::<&gtk::gio::Cancellable>, move |result| {
                    let Ok(file) = result else { return; }; let Some(path) = file.path() else { return; };
                    let Ok(metadata) = std::fs::metadata(&path) else { ui.status.set_text("Cannot read selected file."); return; };
                    if metadata.len() > 20 * 1024 * 1024 { ui.status.set_text("File exceeds 20 MB attachment limit."); return; }
                    let Some(id) = ui.save_compose_draft(&account, &to, &subject, &body) else { return; };
                    let Ok(bytes) = std::fs::read(&path) else { ui.status.set_text("Cannot read selected file."); return; };
                    let filename = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Attachment".into());
                    let mime_type = attachment_mime(&filename);
                    match ui.request(json!({"op":"add_draft_attachment","message_id":id,"filename":filename,"mime_type":mime_type,"content_base64":STANDARD.encode(bytes)})) {
                        Ok(_) => ui.update_attachment_chips(&id, &chips),
                        Err(error) => ui.status.set_text(&format!("Could not attach file: {error}")),
                    }
                });
            });
        }
        let ui = self.clone();
        let account_for_save = account_picker.clone();
        let to_for_save = to.clone();
        let subject_for_save = subject.clone();
        let body_for_save = body.clone();
        save.connect_clicked(move |_| {
            if ui
                .save_compose_draft(
                    &account_for_save,
                    &to_for_save,
                    &subject_for_save,
                    &body_for_save,
                )
                .is_some()
            {
                ui.status.set_text("Draft saved.");
                ui.close_compose();
            }
        });
        let ui = self.clone();
        let body_for_send = body.clone();
        let account_for_send = account_picker.clone();
        let to_for_send = to.clone();
        let subject_for_send = subject.clone();
        send.connect_clicked(move |_| {
            let Some(id) = ui.save_compose_draft(
                &account_for_send,
                &to_for_send,
                &subject_for_send,
                &body_for_send,
            ) else {
                return;
            };
            match ui.request(json!({"op":"queue_send_draft","message_id":id,"undo_seconds":10})) {
                Ok(_) => {
                    *ui.last_queued_draft.borrow_mut() = Some(id);
                    ui.undo_button.set_visible(true);
                    ui.status
                        .set_text("Queued to send. Undo within 10 seconds.");
                    ui.close_compose();
                    ui.refresh();
                    if support::is_feedback(to_for_send.text().as_str()) { ui.show_feedback_thanks(); }
                }
                Err(error) => ui
                    .status
                    .set_text(&format!("Could not queue send: {error}")),
            }
        });
        {
            let ui = self.clone();
            let account = account_picker.clone();
            let to = to.clone();
            let subject = subject.clone();
            let body = body.clone();
            send_later.connect_clicked(move |_| {
                let dialog = gtk::Window::builder()
                    .transient_for(&ui.window)
                    .title("Send later")
                    .modal(true)
                    .default_width(360)
                    .build();
                let layout = gtk::Box::new(Orientation::Vertical, 9);
                layout.set_margin_top(18);
                layout.set_margin_bottom(18);
                layout.set_margin_start(18);
                layout.set_margin_end(18);
                let hint = gtk::Label::new(Some("Send in how many minutes?"));
                hint.set_xalign(0.0);
                let minutes = gtk::SpinButton::with_range(1.0, 10_080.0, 1.0);
                minutes.set_value(60.0);
                let schedule = gtk::Button::with_label("Schedule send");
                schedule.add_css_class("action");
                let ui_for_click = ui.clone();
                let account = account.clone();
                let to = to.clone();
                let subject = subject.clone();
                let body = body.clone();
                let dialog_for_click = dialog.clone();
                let minutes_for_click = minutes.clone();
                schedule.connect_clicked(move |_| {
                    let Some(id) = ui_for_click.save_compose_draft(&account, &to, &subject, &body)
                    else {
                        return;
                    };
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    let delay = minutes_for_click.value_as_int() as i64 * 60_000;
                    match ui_for_click.request(
                        json!({"op":"schedule_draft","message_id":id,"send_at_ms":now + delay}),
                    ) {
                        Ok(_) => {
                            ui_for_click.status.set_text("Message scheduled.");
                            dialog_for_click.close();
                            ui_for_click.close_compose();
                            ui_for_click.refresh();
                            if support::is_feedback(to.text().as_str()) { ui_for_click.show_feedback_thanks(); }
                        }
                        Err(error) => ui_for_click
                            .status
                            .set_text(&format!("Could not schedule: {error}")),
                    }
                });
                layout.append(&hint);
                layout.append(&minutes);
                layout.append(&schedule);
                dialog.set_child(Some(&layout));
                dialog.present();
            });
        }
        // Start in the message once anyone is addressed (an existing draft,
        // a reply). Otherwise To, with the caret at the end: GTK selects an
        // entry's whole value on focus, so the next key would replace it.
        if reply
            || support_message.is_some()
            || [&to, &cc, &bcc].iter().any(|field| !field.text().trim().is_empty())
        {
            body.grab_focus();
        } else {
            to.grab_focus_without_selecting();
            to.set_position(-1);
        }
    }
    fn save_compose_draft(
        &self,
        account: &gtk::ComboBoxText,
        to: &gtk::Entry,
        subject: &gtk::Entry,
        body: &gtk::TextView,
    ) -> Option<String> {
        let identity = account.active_id()?.to_string();
        let (account_id, from) = identity
            .split_once('\t')
            .map(|(a, f)| (a.to_string(), Some(f.to_string())))
            .unwrap_or((identity, None));
        let buffer = body.buffer();
        let body_plain = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let fields = self.compose_fields.borrow();
        let field_text = |id: &str| {
            fields
                .get(id)
                .and_then(|w| w.clone().downcast::<gtk::Entry>().ok())
                .map(|e| e.text().to_string())
                .unwrap_or_default()
        };
        let cc = field_text("cc");
        let bcc = field_text("bcc");
        let existing = self.active_draft.borrow().clone();
        let op = if let Some(id) = existing {
            json!({"op":"update_draft","message_id":id,"from":from,"to":to.text().to_string(),"cc":cc,"bcc":bcc,"subject":subject.text().to_string(),"body_plain":body_plain})
        } else {
            json!({"op":"compose_draft","reply_thread_id":self.reply_thread.borrow().clone(),"account_id":account_id,"from":from,"to":to.text().to_string(),"cc":cc,"bcc":bcc,"subject":subject.text().to_string(),"body_plain":body_plain})
        };
        match self.request(op) {
            Ok(value) => {
                let id = value["id"].as_str()?.to_string();
                *self.active_draft.borrow_mut() = Some(id.clone());
                let enabled = fields.get("tracking").and_then(|widget|widget.clone().downcast::<gtk::CheckButton>().ok()).is_some_and(|button|button.is_active());
                // Public builds make no tracking calls: the service sends any draft
                // without a pixel and drops tracking once the provider accepts it.
                let previous = OPEN_TRACKING && self.request(json!({"op":"tracking_get","message_id":id})).ok().and_then(|value|value["enabled"].as_bool()).unwrap_or(false);
                // Keeping an ordinary draft untracked performs no remote tracking request.
                if OPEN_TRACKING && (enabled || previous) {
                    if let Err(error)=self.request(json!({"op":"tracking_set","message_id":id,"enabled":enabled})) {
                        self.status.set_text(&format!("Draft saved, but open tracking could not be updated: {error}"));
                        return None;
                    }
                }
                Some(id)
            }
            Err(error) => {
                self.status
                    .set_text(&format!("Could not save draft: {error}"));
                None
            }
        }
    }
    fn update_attachment_chips(self: &Rc<Self>, message_id: &str, chips: &gtk::Box) {
        while let Some(child) = chips.first_child() {
            chips.remove(&child);
        }
        let files = self
            .request(json!({"op":"list_draft_attachments","message_id":message_id}))
            .ok()
            .and_then(|v| serde_json::from_value::<Vec<DraftAttachmentMeta>>(v).ok())
            .unwrap_or_default();
        for file in files {
            let button = gtk::Button::with_label(&format!("📎 {}  ·  Remove", file.filename));
            button.add_css_class("action");
            let ui = self.clone();
            let id = message_id.to_string();
            let chips_for_click = chips.clone();
            button.connect_clicked(move |_| {
                match ui.request(
                    json!({"op":"remove_draft_attachment","message_id":id,"attachment_id":file.id}),
                ) {
                    Ok(_) => ui.update_attachment_chips(&id, &chips_for_click),
                    Err(error) => ui
                        .status
                        .set_text(&format!("Could not remove attachment: {error}")),
                }
            });
            chips.append(&button);
        }
    }
    fn undo_send(self: &Rc<Self>) {
        let id = self.last_queued_draft.borrow_mut().take().or_else(|| {
            self.selected()
                .and_then(|i| self.items.borrow().get(i).map(|m| m.thread_id.clone()))
        });
        let Some(id) = id else {
            return;
        };
        match self.request(json!({"op":"cancel_scheduled_draft","message_id":id})) {
            Ok(_) => {
                self.undo_button.set_visible(false);
                self.status.set_text("Send cancelled. Draft restored.");
                self.refresh();
            }
            Err(error) => self
                .status
                .set_text(&format!("Could not cancel send: {error}")),
        }
    }
    fn close_compose(self: &Rc<Self>) {
        let pane = self.compose_pane.borrow_mut().take();
        if let Some(pane) = pane {
            self.compose_actions.borrow_mut().clear();
            self.compose_fields.borrow_mut().clear();
            *self.compose_to.borrow_mut() = None;
            self.reader_header.set_visible(true);
            self.set_shortcuts(false);
            self.reading_content.remove(&pane);
            self.list.set_sensitive(true);
            self.compose_covers_reader.set(false);
            if self.selected().is_some() {
                self.show_selected();
            } else {
                // Nothing is selected (the list may have emptied while composing):
                // rebuild the list's empty state, header included.
                self.refresh();
            }
        }
        *self.active_draft.borrow_mut() = None;
        *self.reply_thread.borrow_mut() = None;
    }
    fn run_command(self: &Rc<Self>) {
        let input = self.command.text().to_lowercase();
        let input = input.trim();
        match input {
            "archive" => self.archive(),
            "compose" => self.compose(false),
            "reply" => self.compose(true),
            "mark read" => self.mark(false),
            "mark unread" => self.mark(true),
            "search" => {
                self.search.grab_focus();
            }
            "sync" => self.sync_all(),
            "add account" => self.add_account(),
            "label" => self.label_thread(false),
            "remove label" => self.label_thread(true),
            "snooze" => self.snooze(),
            "unsnooze" => self.unsnooze(),
            "unsubscribe" => self.unsubscribe(),
            "new split" => self.create_split(),
            "sent" => self.set_view(MailView::Sent),
            "outbox" => self.set_view(MailView::Outbox),
            "drafts" => self.set_view(MailView::Drafts),
            _ => {
                if let Some(q) = input.strip_prefix("search ") {
                    self.search.set_text(q);
                    self.search.grab_focus();
                }
            }
        }
        self.command.set_text("");
        self.palette.hide();
    }
}
fn unsubscribe_url(html: &str) -> Option<Url> {
    let fragment = Html::parse_fragment(html);
    let selector = Selector::parse("a[href]").expect("fixed anchor selector");
    fragment.select(&selector).find_map(|link| {
        let label = link.text().collect::<Vec<_>>().join(" ");
        let label = if label.trim().is_empty() {
            link.value()
                .attr("aria-label")
                .or_else(|| link.value().attr("title"))
                .unwrap_or("")
        } else {
            label.as_str()
        }
        .to_ascii_lowercase();
        if !["unsubscribe", "opt out", "opt-out", "stop receiving"]
            .iter()
            .any(|term| label.contains(term))
        {
            return None;
        }
        let raw = link.value().attr("href")?;
        if raw.chars().any(char::is_control) {
            return None;
        }
        let url = Url::parse(raw).ok()?;
        match url.scheme() {
            "http" | "https"
                if url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none() =>
            {
                Some(url)
            }
            "mailto"
                if !url.path().is_empty()
                    && reader_links::sanitized_mailto(url.as_str()).is_some() =>
            {
                Some(url)
            }
            _ => None,
        }
    })
}
fn document(body: &str, allow_remote_images: bool, dark: bool) -> String {
    let image_sources = if allow_remote_images {
        "data: https:"
    } else {
        "data:"
    };
    let theme = if dark {
        "html,body{background:#1c1c1e!important;color:#e2e2e5!important}body :where(div,span,p,td,th,table,section,article,h1,h2,h3,h4,h5,h6,li,blockquote,strong,b,em,font){background-color:transparent!important;color:#e2e2e5!important;border-color:#454549!important}a,a *{color:#d7cfc2!important}"
    } else {
        "html,body{background:#fff;color:#262629}"
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src {image_sources}; style-src 'unsafe-inline'; font-src data:; base-uri 'none'; form-action 'none'; frame-src 'none'; referrer 'no-referrer'"><meta name="color-scheme" content="light dark"><style>html{{font:14px/1.65 Inter,system-ui,sans-serif}}body{{margin:0;padding:24px;overflow-wrap:break-word}}img{{max-width:100%;height:auto}}p{{margin:0 0 1em}}.plain-text{{white-space:pre-wrap;max-width:72ch;font:15px/1.62 Inter,system-ui,sans-serif;text-align:left;tab-size:4}}{theme}</style></head><body>{body}</body></html>"#
    )
}

fn attachment_mime(filename: &str) -> &'static str {
    match filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "ics" => "text/calendar",
        _ => "application/octet-stream",
    }
}

fn new_html_reader() -> webkit6::WebView {
    let reader = webkit6::WebView::new();
    reader.set_vexpand(true);
    reader.set_hexpand(true);
    if let Some(settings) = webkit6::prelude::WebViewExt::settings(&reader) {
        settings.set_enable_javascript(false);
        settings.set_enable_html5_local_storage(false);
        settings.set_enable_webgl(false);
        // Explicit compatibility option for VMs without a working virtual GPU.
        // Keep hardware acceleration enabled on normal desktops.
        if std::env::var("EMAIL_SOFTWARE_RENDERING").as_deref() == Ok("1") {
            settings.set_hardware_acceleration_policy(webkit6::HardwareAccelerationPolicy::Never);
        }
    }
    reader.connect_decide_policy(|_, decision, kind| {
        if kind == webkit6::PolicyDecisionType::NavigationAction {
            if let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() {
                let uri = nav
                    .navigation_action()
                    .and_then(|action| action.request())
                    .and_then(|request| request.uri());
                if uri.as_ref().is_none_or(|value| value.starts_with("about:")) {
                    return false;
                }
                let user_gesture = nav
                    .navigation_action()
                    .is_some_and(|action| action.is_user_gesture());
                if let Some(uri) = uri
                    .filter(|_| user_gesture)
                    .as_deref()
                    .and_then(reader_links::external_uri)
                {
                    let _ = gtk::gio::AppInfo::launch_default_for_uri(
                        &uri,
                        None::<&gtk::gio::AppLaunchContext>,
                    );
                }
            }
            decision.ignore();
            return true;
        }
        false
    });
    reader
}

fn process_completed(ui: &Rc<Ui>, receiver: &Receiver<Result<Value, String>>) {
    while let Ok(result) = receiver.try_recv() {
        let completed_older_sync = ui.loading_older.get();
        let succeeded = result.is_ok();
        let archive_has_page = result
            .as_ref()
            .ok()
            .and_then(Value::as_array)
            .is_some_and(|rows| {
                rows.iter()
                    .any(|row| row["data"]["messages_added"].as_u64().unwrap_or(0) > 0)
            });
        match result {
            Ok(value) => {
                if let Some(delay) = parity::pending_retry_delay(&value) {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    ui.queue_retry_at.set(now.saturating_add(delay));
                }
                if let Some(account_id) = value["id"]
                    .as_str()
                    .filter(|_| value["provider"].is_string())
                {
                    ui.status.set_text("Account connected. Loading inbox…");
                    ui.refresh_calendar_accounts();
                    ui.background(json!({"op":"sync_account","account_id":account_id}));
                } else if let Some(results) = value.as_array() {
                    let failures = results
                        .iter()
                        .filter(|item| {
                            item.get("error").is_some() || item["data"].get("sync_error").is_some()
                        })
                        .count();
                    ui.status.set_text(if failures == 0 {
                        "Sync complete."
                    } else {
                        "Some accounts could not sync. Check account settings."
                    });
                } else {
                    ui.status.set_text("Done.");
                }
                ui.refresh();
                ui.notify_new_mail();
                ui.notify_followups();
            }
            Err(error) => ui
                .status
                .set_text(&format!("Could not complete operation: {error}")),
        }
        if completed_older_sync {
            if succeeded
                && ui.items.borrow().len() <= ui.older_count_before.get()
                && (!matches!(*ui.view.borrow(), MailView::Archive) || !archive_has_page)
            {
                ui.can_load_older.set(false);
                ui.status.set_text("All older mail loaded.");
            }
            ui.loading_older.set(false);
        }
    }
}
fn process_attachments(ui: &Rc<Ui>, receiver: &Receiver<AttachmentEvent>) {
    while let Ok(event) = receiver.try_recv() {
        match event {
            AttachmentEvent::Portrait {
                address, result, ..
            } => {
                if let Ok(value) = result {
                    if let Some(bytes) = value["data_base64"]
                        .as_str()
                        .and_then(|s| STANDARD.decode(s).ok())
                    {
                        ui.portraits
                            .borrow_mut()
                            .insert(address.clone(), Some(bytes.clone()));
                        if ui
                            .current_message
                            .borrow()
                            .as_ref()
                            .is_some_and(|m| parity::address(&m.sender) == address)
                        {
                            ui.contact.set_photo(&bytes);
                        }
                    }
                }
            }
            AttachmentEvent::Headers { thread_id, result } => {
                if let Ok(value) = result {
                    if let Ok(messages) = serde_json::from_value::<Vec<Message>>(value) {
                        let selected = ui.current_message.borrow().clone();
                        if let Some(selected) = selected.filter(|m| m.thread_id == thread_id) {
                            if let Some(updated) =
                                messages.into_iter().find(|m| m.id == selected.id)
                            {
                                ui.show_recipient_headers(&updated);
                                *ui.current_message.borrow_mut() = Some(updated);
                            }
                        }
                    }
                }
            }
            AttachmentEvent::InlineImages { message_id, result } => {
                if ui
                    .current_message
                    .borrow()
                    .as_ref()
                    .is_some_and(|message| message.id == message_id)
                {
                    if let Ok(value) = result {
                        if let Some(html) = value["body_html_safe"].as_str() {
                            ui.show_html(html);
                        }
                    }
                }
            }
            AttachmentEvent::List { message_id, result } => {
                if ui
                    .current_message
                    .borrow()
                    .as_ref()
                    .is_some_and(|message| message.id == message_id)
                {
                    ui.show_attachments(&message_id, result);
                }
            }
            AttachmentEvent::Preview {
                message_id,
                filename,
                mime_type,
                result,
                ..
            } => {
                if ui
                    .current_message
                    .borrow()
                    .as_ref()
                    .is_some_and(|message| message.id == message_id)
                {
                    ui.present_attachment(&filename, &mime_type, result);
                }
            }
        }
    }
}

impl Ui {
    fn update_activity_layout(&self) {
        let width = self.window.width();
        let single = width < 1100;
        let message = self.compact_message.get() || self.compose_pane.borrow().is_some();
        let activity = self.activity_host.get_visible();
        if let Some(viewport) = self.activity_host.parent() { viewport.set_hexpand(width < 1000); }
        self.mail_reading_host.set_visible(activity || !single || message);
        self.mail_inbox_viewport.set_visible((!activity && (!single || !message)) || (activity && width >= 1360));
        self.mail_reader_viewport.set_visible(!activity || width >= 1000);
        self.contact.root.set_visible(!activity && width >= 1400 && self.contact_requested.get());
    }
}

// GTK Entry/SearchEntry delegate keyboard focus to an internal GtkText.
// Treat the whole editor subtree as typing, so mail shortcuts cannot eat text.
fn focus_within(widget: &impl IsA<gtk::Widget>) -> bool {
    widget.is_mapped()
        && (widget.has_focus() || widget.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN))
}

fn sidebar_heading(text: &str) -> gtk::Box {
    // Reserve space on a container, not on an ellipsized single-line label.
    // GtkLabel's single-line font metrics can clip cap tops on fractional-scale
    // Wayland renderers. Let Pango measure the complete glyph ink naturally.
    let row = gtk::Box::new(Orientation::Horizontal, 0);
    row.add_css_class("side-group");
    row.set_size_request(-1, 26);
    row.set_vexpand(false);
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_valign(gtk::Align::Center);
    label.set_wrap(false);
    label.set_single_line_mode(false);
    row.append(&label);
    row
}

fn build(app: &gtk::Application) {
    let css = gtk::CssProvider::new();
    let theme_path = database_path()
        .expect("mail data directory")
        .with_file_name("appearance");
    let dark = Rc::new(Cell::new(
        std::fs::read_to_string(&theme_path)
            .map(|value| value.trim() != "light")
            .unwrap_or(true),
    ));
    css.load_from_data(&appearance_css(dark.get()));
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Email")
        .resizable(true)
        .default_width(1320)
        .default_height(840)
        .build();
    window.add_css_class("email-main");
    let root = gtk::Paned::new(Orientation::Horizontal);
    root.set_position(190);
    root.set_resize_start_child(false);
    root.set_shrink_start_child(false);
    root.set_shrink_end_child(true);
    root.add_css_class("app");
    let sidebar = gtk::Box::new(Orientation::Vertical, 0);
    sidebar.add_css_class("sidebar");
    sidebar.set_size_request(150, -1);
    // Keep narrow windows usable without letting sidebar content resize the shell.
    let sidebar_scroll = gtk::ScrolledWindow::new();
    sidebar_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
    sidebar_scroll.set_propagate_natural_width(false);
    sidebar_scroll.set_min_content_width(150);
    sidebar_scroll.set_child(Some(&sidebar));
    let brand = gtk::Label::new(Some("Email"));
    brand.add_css_class("brand");
    brand.set_xalign(0.0);
    sidebar.append(&brand);
    let compose_button = navigation_button("document-edit-symbolic", "Compose", "C");
    compose_button.add_css_class("compose-button");
    sidebar.append(&compose_button);
    let workspace_heading = sidebar_heading("WORKSPACE");
    sidebar.append(&workspace_heading);
    let views_box = gtk::Box::new(Orientation::Vertical, 0);
    sidebar.append(&views_box);
    let accounts_heading = sidebar_heading("ACCOUNTS");
    sidebar.append(&accounts_heading);
    let accounts_box = gtk::Box::new(Orientation::Vertical, 0);
    sidebar.append(&accounts_box);
    let labels_heading = sidebar_heading("LABELS");
    sidebar.append(&labels_heading);
    let labels_box = gtk::Box::new(Orientation::Vertical, 0);
    sidebar.append(&labels_box);
    let new_split = navigation_button("list-add-symbolic", "New split", "");
    new_split.add_css_class("side-item");
    sidebar.append(&new_split);
    let splits_heading = sidebar_heading("SPLITS");
    sidebar.append(&splits_heading);
    sidebar.reorder_child_after(&splits_heading, Some(&views_box));
    let splits_box = gtk::Box::new(Orientation::Vertical, 0);
    sidebar.insert_child_after(&splits_box, Some(&splits_heading));
    sidebar.reorder_child_after(&new_split, Some(&splits_box));
    let add_account = navigation_button("list-add-symbolic", "Add account", "");
    add_account.add_css_class("action");
    let sync = icon_button("view-refresh-symbolic", "Refresh inbox · S");
    sync.add_css_class("action");
    let spacer = gtk::Box::new(Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    sidebar.append(&add_account);
    let manage = navigation_button("emblem-system-symbolic", "Manage accounts", "");
    sidebar.append(&manage);
    let agent_access = agent::sidebar_button();
    sidebar.append(&agent_access);
    sidebar.append(&spacer);
    let commands_footer = navigation_button("view-more-symbolic", "Commands", "Ctrl K");
    sidebar.append(&commands_footer);
    let support_button = navigation_button("dialog-question-symbolic", "Support & feedback", "");
    sidebar.append(&support_button);
    let theme_button = navigation_button("weather-clear-night-symbolic", "Appearance", "");
    theme_button.set_tooltip_text(Some("Switch between light and dark appearance"));
    sidebar.append(&theme_button);
    let status = gtk::Label::new(None);
    status.set_wrap(true);
    status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status.set_max_width_chars(18);
    status.set_width_chars(1);
    status.set_xalign(0.0);
    status.add_css_class("hint");
    sidebar.append(&status);
    let inbox = gtk::Box::new(Orientation::Vertical, 0);
    inbox.add_css_class("inbox");
    inbox.set_size_request(320, -1);
    let header = gtk::Box::new(Orientation::Vertical, 0);
    header.add_css_class("inbox-header");
    let label = gtk::Label::new(Some("Inbox"));
    label.add_css_class("section-title");
    label.set_xalign(0.0);
    let hint = gtk::Label::new(Some("Your mail, up to date"));
    hint.add_css_class("hint");
    hint.set_xalign(0.0);
    let search = gtk::SearchEntry::new();
    search.add_css_class("search");
    search.set_search_delay(0);
    search.set_placeholder_text(Some("Search downloaded mail   /"));
    search.set_tooltip_text(Some(parity::SEARCH_HELP));
    let inbox_heading = gtk::Box::new(Orientation::Horizontal, 8);
    label.set_hexpand(true);
    inbox_heading.append(&label);
    inbox_heading.append(&sync);
    header.append(&inbox_heading);
    header.append(&hint);
    let search_row = gtk::Box::new(Orientation::Horizontal, 6);
    search.set_hexpand(true);
    search_row.append(&search);
    let search_filters = gtk::Button::new();
    search_filters.set_child(Some(&mail_icon("view-filter-symbolic")));
    search_filters.add_css_class("flat");
    search_filters.set_tooltip_text(Some("Refine search (Ctrl+Shift+F)"));
    search_row.append(&search_filters);
    header.append(&search_row);
    let filters = gtk::Box::new(Orientation::Horizontal, 6);
    filters.set_margin_top(12);
    let filter_buttons: Vec<gtk::Button> = ["All", "Unread", "Favorites"]
        .iter()
        .map(|name| {
            let button = gtk::Button::with_label(name);
            button.add_css_class("filter-tab");
            filters.append(&button);
            button
        })
        .collect();
    filter_buttons[0].add_css_class("active");
    header.append(&filters);
    let selection_bar = gtk::Box::new(Orientation::Vertical, 6);
    selection_bar.set_margin_top(12);
    let selection_count = gtk::Label::new(None);
    selection_count.add_css_class("hint");
    selection_count.set_xalign(0.0);
    selection_bar.append(&selection_count);
    let selection_actions = gtk::Box::new(Orientation::Horizontal, 6);
    selection_bar.append(&selection_actions);
    let selection_buttons: Vec<gtk::Button> = ["All", "Read", "Archive", "Undo Z", "Clear"]
        .iter()
        .map(|title| {
            let b = gtk::Button::with_label(title);
            b.add_css_class("selection-action");
            selection_actions.append(&b);
            b
        })
        .collect();
    selection_bar.set_visible(false);
    header.append(&selection_bar);
    inbox.append(&header);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    let list = gtk::ListBox::new();
    list.add_css_class("mail-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    scroll.set_child(Some(&list));
    inbox.append(&scroll);
    let reading = gtk::Box::new(Orientation::Vertical, 0);
    reading.set_hexpand(true);
    reading.add_css_class("reading");
    let reading_header = gtk::Box::new(Orientation::Vertical, 0);
    reading_header.add_css_class("reader-header");
    let actions = gtk::Box::new(Orientation::Horizontal, 8);
    let title = gtk::Label::new(None);
    title.add_css_class("reader-title");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let archive = icon_button("mail-archive-symbolic", "Archive · E");
    archive.add_css_class("action");
    let label_button = icon_button("tag-symbolic", "Labels · L");
    label_button.add_css_class("action");
    let snooze_button = icon_button("alarm-symbolic", "Snooze · Z");
    snooze_button.add_css_class("action");
    let unsubscribe_button = gtk::Button::with_label("Unsubscribe");
    unsubscribe_button.set_tooltip_text(Some("Unsubscribe · Ctrl+Shift+U"));
    unsubscribe_button.add_css_class("action");
    let undo_button = gtk::Button::with_label("Undo send  Ctrl+Z");
    undo_button.add_css_class("action");
    undo_button.set_visible(false);
    let thread_actions = gtk::Box::new(Orientation::Horizontal, 8);
    let reply = icon_button("mail-reply-sender-symbolic", "Reply · R");
    let reply_all = icon_button("mail-reply-all-symbolic", "Reply all · Shift+R");
    let favorite = icon_button("star-symbolic", "Toggle favorite · F");
    let contact_toggle = icon_button("avatar-default-symbolic", "Show contact · 0");
    let tracking_activity=icon_button("chart-symbolic", "Recent opens");
    // Development profiles offer activity; others once tracked mail exists
    // (check_saved_open_activity).
    tracking_activity.set_visible(OPEN_TRACKING);
    thread_actions.append(&tracking_activity);
    thread_actions.append(&contact_toggle);
    thread_actions.append(&archive);
    thread_actions.append(&reply);
    thread_actions.append(&reply_all);
    thread_actions.append(&favorite);
    thread_actions.append(&label_button);
    thread_actions.append(&snooze_button);
    thread_actions.append(&unsubscribe_button);
    let command_button = gtk::Button::new();
    command_button.set_child(Some(&mail_icon("view-more-symbolic")));
    command_button.set_tooltip_text(Some("Commands · Ctrl+K"));
    command_button.add_css_class("action");
    reading_header.append(&title);
    let sender_label = gtk::Label::new(None);
    sender_label.add_css_class("reader-sender");
    sender_label.set_xalign(0.0);
    sender_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    reading_header.append(&sender_label);
    let header_spacer = gtk::Box::new(Orientation::Horizontal, 0);
    header_spacer.set_hexpand(true);
    actions.append(&header_spacer);
    actions.append(&thread_actions);
    actions.append(&undo_button);
    actions.append(&command_button);
    let meta = gtk::Label::new(None);
    meta.add_css_class("reader-meta");
    meta.set_xalign(0.0);
    meta.set_hexpand(true);
    meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let recipient_button = gtk::MenuButton::new();
    recipient_button.set_child(Some(&meta));
    recipient_button.set_always_show_arrow(true);
    recipient_button.set_has_frame(false);
    recipient_button.set_halign(gtk::Align::Start);
    recipient_button.set_tooltip_text(Some("Full recipients, Cc, Reply-To, and date"));
    recipient_button.add_css_class("recipient-summary");
    reading_header.append(&recipient_button);
    reading_header.append(&actions);
    let recipient_details = gtk::Label::new(None);
    recipient_details.set_xalign(0.0);
    recipient_details.set_wrap(true);
    recipient_details.set_selectable(true);
    recipient_details.add_css_class("reader-meta");
    recipient_details.set_max_width_chars(48);
    recipient_details.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    recipient_details.set_margin_top(10);
    recipient_details.set_margin_bottom(10);
    recipient_details.set_margin_start(12);
    recipient_details.set_margin_end(12);
    let recipient_popover = gtk::Popover::new();
    recipient_popover.set_child(Some(&recipient_details));
    recipient_button.set_popover(Some(&recipient_popover));
    let remote_images = icon_button("image-x-generic-symbolic", "Remote images · I");
    remote_images.add_css_class("action");
    remote_images.set_halign(gtk::Align::Start);
    remote_images.set_tooltip_text(Some(
        "Press I to change the image mode. Remote images can tell a sender when you opened their email.",
    ));
    remote_images.set_visible(false);
    actions.append(&remote_images);
    let remote_images_menu = gtk::Popover::new();
    remote_images_menu.set_parent(&remote_images);
    remote_images_menu.set_position(gtk::PositionType::Bottom);
    {
        // A plain button does not unparent extra children when it is destroyed.
        let menu = remote_images_menu.clone();
        remote_images.connect_destroy(move |_| menu.unparent());
    }
    reading.append(&reading_header);
    let reading_content = gtk::Box::new(Orientation::Vertical, 0);
    reading_content.set_vexpand(true);
    let conversation = gtk::Box::new(Orientation::Vertical, 4);
    conversation.set_margin_start(18);
    conversation.set_margin_end(18);
    reading_content.append(&conversation);
    // Shown above the message while its remote images are blocked.
    let remote_images_notice = gtk::Box::new(Orientation::Horizontal, 6);
    remote_images_notice.add_css_class("remote-images-notice");
    remote_images_notice.set_margin_start(28);
    remote_images_notice.set_margin_end(28);
    remote_images_notice.set_margin_top(4);
    remote_images_notice.set_margin_bottom(10);
    remote_images_notice.set_tooltip_text(Some(
        "Remote images can tell the sender when and where you opened this email.",
    ));
    remote_images_notice.update_property(&[gtk::accessible::Property::Label(
        "Remote images blocked",
    )]);
    let notice_icon = mail_icon("image-x-generic-symbolic");
    notice_icon.set_margin_end(2);
    let notice_text = gtk::Label::new(Some("Remote images are blocked."));
    notice_text.add_css_class("remote-images-text");
    notice_text.set_xalign(0.0);
    notice_text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let load_remote_images = gtk::Button::with_label("Load images");
    load_remote_images.add_css_class("remote-images-load");
    load_remote_images.set_valign(gtk::Align::Center);
    load_remote_images.set_focus_on_click(false);
    load_remote_images.update_property(&[gtk::accessible::Property::Label(
        "Load images in this email",
    )]);
    remote_images_notice.append(&notice_icon);
    remote_images_notice.append(&notice_text);
    remote_images_notice.append(&load_remote_images);
    remote_images_notice.set_visible(false);
    reading_content.append(&remote_images_notice);
    let attachments = gtk::Box::new(Orientation::Horizontal, 7);
    attachments.set_margin_start(18);
    attachments.set_margin_end(18);
    let attachments_scroll = gtk::ScrolledWindow::new();
    attachments_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    attachments_scroll.set_min_content_height(48);
    attachments_scroll.set_child(Some(&attachments));
    attachments_scroll.set_visible(false);
    let plain = gtk::TextView::new();
    plain.add_css_class("plain");
    plain.set_editable(false);
    plain.set_cursor_visible(false);
    plain.set_wrap_mode(gtk::WrapMode::WordChar);
    plain.set_vexpand(true);
    let plain_scroll = gtk::ScrolledWindow::new();
    plain_scroll.set_vexpand(true);
    plain_scroll.set_child(Some(&plain));
    reading_content.append(&plain_scroll);
    reading_content.append(&attachments_scroll);
    reading.append(&reading_content);
    let palette = gtk::Window::builder()
        .transient_for(&window)
        .modal(true)
        .decorated(false)
        .resizable(false)
        .default_width(540)
        .default_height(460)
        .title("Commands")
        .build();
    let palette_content = gtk::Box::new(Orientation::Vertical, 0);
    palette_content.add_css_class("command-panel");
    let palette_heading = parity::PaletteHeading::new();
    palette_content.append(&palette_heading.root);
    let command = gtk::Entry::new();
    command.set_placeholder_text(Some("Type a command…"));
    command.add_css_class("command-search");
    palette_content.append(&command);
    let palette_rows = gtk::ListBox::new();
    palette_rows.add_css_class("command-list");
    palette_rows.set_selection_mode(gtk::SelectionMode::Single);
    let results = gtk::ScrolledWindow::new();
    results.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    results.set_min_content_height(270);
    results.set_vexpand(true);
    results.set_child(Some(&palette_rows));
    palette_content.append(&results);
    let palette_empty = gtk::Label::new(Some("No matching commands"));
    palette_empty.add_css_class("command-detail");
    palette_content.append(&palette_empty);
    let help = gtk::Label::new(Some("↑ ↓  Navigate       ↵  Run       Esc  Back / close"));
    help.add_css_class("palette-help");
    palette_content.append(&help);
    palette.set_child(Some(&palette_content));
    let columns = gtk::Paned::new(Orientation::Horizontal);
    columns.set_position(350);
    columns.set_resize_start_child(false);
    columns.set_shrink_start_child(false);
    columns.set_shrink_end_child(true);
    // External policy isolates child minimum sizes without drawing nested scrollbars.
    // Never policy propagates child minima, which can prevent outer-window resizing.
    let inbox_viewport = gtk::ScrolledWindow::new();
    inbox_viewport.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
    inbox_viewport.set_propagate_natural_width(false);
    inbox_viewport.set_min_content_width(280);
    inbox_viewport.set_child(Some(&inbox));
    columns.set_start_child(Some(&inbox_viewport));
    root.set_start_child(Some(&sidebar_scroll));
    let workspace = gtk::Stack::new();
    workspace.set_transition_type(gtk::StackTransitionType::None);
    workspace.set_hhomogeneous(false);
    workspace.set_vhomogeneous(false);
    workspace.set_hexpand(true);
    workspace.set_vexpand(true);
    // Without an account the mail page is one welcome with one action
    // (macOS hides the list and shows the welcome in the reader).
    let welcome = gtk::Box::new(Orientation::Vertical, 14);
    welcome.set_halign(gtk::Align::Center);
    welcome.set_valign(gtk::Align::Center);
    let welcome_title = gtk::Label::new(Some("Welcome to Email"));
    welcome_title.add_css_class("welcome-title");
    // The text depends on the build's features; set once the service is open.
    let welcome_detail = gtk::Label::new(None);
    welcome_detail.add_css_class("welcome-detail");
    welcome_detail.set_wrap(true);
    welcome_detail.set_justify(gtk::Justification::Center);
    welcome_detail.set_max_width_chars(52);
    let welcome_action = gtk::Button::with_label("Add an account");
    welcome_action.add_css_class("welcome-action");
    welcome_action.set_halign(gtk::Align::Center);
    welcome.append(&welcome_title);
    welcome.append(&welcome_detail);
    welcome.append(&welcome_action);
    let welcome_page = gtk::Box::new(Orientation::Vertical, 0);
    welcome_page.add_css_class("welcome-page");
    welcome_page.set_hexpand(true);
    welcome_page.set_vexpand(true);
    welcome_page.append(&welcome);
    welcome.set_vexpand(true);
    let mail_stack = gtk::Stack::new();
    mail_stack.set_transition_type(gtk::StackTransitionType::None);
    mail_stack.set_hhomogeneous(false);
    mail_stack.set_vhomogeneous(false);
    mail_stack.add_named(&columns, Some("columns"));
    mail_stack.add_named(&welcome_page, Some("welcome"));
    workspace.add_named(&mail_stack, Some("mail"));
    let calendar_host = gtk::Box::new(Orientation::Vertical, 0);
    calendar_host.set_focusable(true);
    calendar_host.add_css_class("calendar-workspace");
    let calendar_viewport = gtk::ScrolledWindow::new();
    calendar_viewport.add_css_class("calendar-workspace");
    calendar_viewport.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
    calendar_viewport.set_propagate_natural_width(false);
    calendar_viewport.set_min_content_width(280);
    calendar_viewport.set_child(Some(&calendar_host));
    workspace.add_named(&calendar_viewport, Some("calendar"));
    workspace.set_visible_child_name("mail");
    root.set_end_child(Some(&workspace));
    let shortcut_bar = parity::ShortcutStrip::new();
    shortcut_bar.set_text(
        "J K  Next / previous     ↵  Read     R  Reply     E  Archive     Ctrl K  Commands",
    );
    reading.append(&shortcut_bar.root);
    let reader_viewport = gtk::ScrolledWindow::new();
    reader_viewport.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
    reader_viewport.set_propagate_natural_width(false);
    reader_viewport.set_min_content_width(280);
    reader_viewport.set_min_content_height(200);
    reader_viewport.set_child(Some(&reading));
    reader_viewport.set_focusable(true);
    let reading_and_contact = gtk::Box::new(Orientation::Horizontal, 0);
    reader_viewport.set_hexpand(true);
    reading_and_contact.append(&reader_viewport);
    let contact = parity::ContactPane::new();
    contact.root.set_visible(
        std::fs::read_to_string(database_path().unwrap().with_extension("contact-pane"))
            .is_ok_and(|s| s == "on"),
    );
    reading_and_contact.append(&contact.root);
    let activity_host = gtk::Box::new(Orientation::Vertical, 0);
    activity_host.add_css_class("contact-pane"); activity_host.set_visible(false);
    let activity_viewport = gtk::ScrolledWindow::new();
    activity_viewport.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
    activity_viewport.set_min_content_width(360); activity_viewport.set_max_content_width(360);
    activity_viewport.set_propagate_natural_width(true); activity_viewport.set_hexpand(false);
    activity_viewport.set_child(Some(&activity_host));
    activity_host.bind_property("visible", &activity_viewport, "visible").sync_create().build();
    reading_and_contact.append(&activity_viewport);
    columns.set_end_child(Some(&reading_and_contact));
    // The tiling compositor owns the outer size. Adapt the panes to its allocation.
    let shell = gtk::Box::new(Orientation::Vertical, 0);
    let compact_bar = gtk::Box::new(Orientation::Horizontal, 8);
    compact_bar.add_css_class("compact-bar");
    let navigation_toggle = gtk::Button::with_label("Mailboxes · G M");
    let back_to_inbox = gtk::Button::with_label("← Inbox · Esc");
    compact_bar.append(&navigation_toggle);
    compact_bar.append(&back_to_inbox);
    compact_bar.set_visible(false);
    shell.append(&compact_bar);
    root.set_vexpand(true);
    shell.append(&root);
    let celebration_host = gtk::Overlay::new();
    celebration_host.set_child(Some(&shell));
    window.set_child(Some(&celebration_host));
    let database_path = database_path().expect("mail data directory");
    let image_preference = remote_images_enabled(&database_path);
    let service = MailService::open(&database_path).expect("open mail service");
    welcome_detail.set_text(if account_setup::build_features(&service).microsoft_accounts {
        "Add your Gmail, iCloud, Outlook or other email account to get started."
    } else {
        "Add your Gmail, iCloud or other email account to get started."
    });
    // Matches macOS: EMAIL_DEMO=1 (or small) adds three labelled example
    // messages and EMAIL_DEMO=launch the launch demo mailbox, but only to an
    // explicitly isolated store, never to the real mail cache.
    if let Some(demo) = std::env::var("EMAIL_DEMO").ok().filter(|value| !value.is_empty()) {
        if let Some(request) = demo_request(&demo) {
            if store_is_isolated() {
                if let Err(error) = service.request_json(&request) {
                    eprintln!("Demo messages could not be added: {error}");
                }
            } else if isolated_store_value().is_some() {
                eprintln!("EMAIL_DEMO={demo} is ignored: EMAIL_STORE_PATH is in the default mail directory.");
            } else {
                eprintln!(
                    "EMAIL_DEMO={demo} is ignored without EMAIL_STORE_PATH; demo messages only go into an isolated store."
                );
            }
        } else {
            eprintln!("EMAIL_DEMO={demo} is not a demo set. Use 1 (or small) or launch.");
        }
    }
    let (completed, receiver) = mpsc::channel();
    let (attachment_completed, attachment_receiver) = mpsc::channel();
    let (dispatch_completed, dispatch_receiver) = mpsc::channel();
    // Builds before 11 Nov 2026 wrote hidden previews as their default; reset once.
    let previews_marker = database_path.with_extension("notification-previews-v2");
    if !previews_marker.exists() {
        let _ = std::fs::remove_file(database_path.with_extension("notification-previews"));
        let _ = std::fs::write(&previews_marker, "");
    }
    let account_only: Vec<gtk::Widget> = vec![
        compose_button.clone().upcast(),
        workspace_heading.clone().upcast(),
        views_box.clone().upcast(),
        splits_heading.clone().upcast(),
        splits_box.clone().upcast(),
        new_split.clone().upcast(),
        accounts_heading.clone().upcast(),
        accounts_box.clone().upcast(),
        labels_box.clone().upcast(),
        manage.clone().upcast(),
        agent_access.clone().upcast(),
    ];
    let ui = Rc::new(Ui {
        window: window.clone(),
        workspace,
        mail_stack,
        account_only,
        calendar_host,
        calendar: RefCell::new(None),
        service,
        database_path: database_path.clone(),
        completed,
        attachment_completed,
        items: RefCell::new(Vec::new()),
        activity_navigation: RefCell::new(None),
        list,
        list_scroll: scroll,
        loaded_limit: Cell::new(500),
        loading_older: Cell::new(false),
        can_load_older: Cell::new(true),
        older_count_before: Cell::new(0),
        search,
        inbox_filter: Cell::new(0),
        search_generation: Cell::new(0),
        search_preview: RefCell::new(None),
        goto_pending: Cell::new(false),
        archive_browse_started: Cell::new(false),
        filter_buttons: filter_buttons.clone(),
        selection_bar,
        selection_count,
        selection_buttons,
        marked: RefCell::new(std::collections::HashSet::new()),
        marked_scope: RefCell::new(String::new()),
        row_checks: RefCell::new(std::collections::HashMap::new()),
        updating_checks: Cell::new(false),
        last_triage: RefCell::new(None),
        last_discarded_draft: RefCell::new(None),
        dark: dark.clone(),
        title,
        meta,
        sender_label,
        unsubscribe_button: unsubscribe_button.clone(),
        favorite_button: favorite.clone(),
        recipient_details,
        recipient_button,
        reading_content,
        reader_header: reading_header,
        shortcut_bar,
        compose_pane: RefCell::new(None),
        plain,
        plain_scroll,
        reader: RefCell::new(None),
        remote_images,
        remote_images_menu,
        tracking_activity: tracking_activity.clone(),
        remote_images_notice,
        load_remote_images: load_remote_images.clone(),
        current_html: RefCell::new(None),
        allow_remote_images: Cell::new(image_preference),
        remote_images_default: Cell::new(image_preference),
        remote_images_message: RefCell::new(None),
        compose_covers_reader: Cell::new(false),
        rendered_remote_images: Cell::new(image_preference),
        offers_open_activity: Cell::new(OPEN_TRACKING),
        palette,
        palette_page: RefCell::new("root".into()),
        palette_rows,
        palette_heading,
        palette_empty,
        palette_items: RefCell::new(Vec::new()),
        contact,
        mail_inbox_viewport: inbox_viewport.clone(), mail_reader_viewport: reader_viewport.clone(), mail_reading_host: reading_and_contact.clone(),
        activity_host, activity_panel: RefCell::new(None), activity_return_focus: RefCell::new(None),
        portraits: RefCell::new(std::collections::HashMap::new()),
        compose_to: RefCell::new(None),
        history: RefCell::new(Vec::new()),
        reader_mode: Cell::new(false),
        compact_message: Cell::new(false),
        compact_navigation: Cell::new(false),
        contact_requested: Cell::new(
            std::fs::read_to_string(database_path.with_extension("contact-pane"))
                .is_ok_and(|v| v == "on"),
        ),
        reader_top_pending: Cell::new(false),
        notifications: Cell::new(
            std::fs::read_to_string(database_path.with_extension("notifications"))
                .is_ok_and(|s| s == "on"),
        ),
        notified: RefCell::new(std::collections::HashMap::new()),
        notified_followups: RefCell::new(parity::load_followup_notifications(&database_path)),
        open_check_in_flight: Cell::new(false),
        last_open_check: Cell::new(0),
        notification_generation: Cell::new(0),
        notification_since: Cell::new(now_ms()),
        notification_previews: Cell::new(match std::fs::read_to_string(database_path.with_extension("notification-previews")) { Ok(value) => value == "on", Err(error) => error.kind() == std::io::ErrorKind::NotFound }),
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        theme_mode: RefCell::new(
            std::fs::read_to_string(database_path.with_extension("theme"))
                .unwrap_or_else(|_| "dark".into()),
        ),
        css: css.clone(),
        compose_actions: RefCell::new(std::collections::HashMap::new()),
        compose_fields: RefCell::new(std::collections::HashMap::new()),
        command,
        status,
        accounts_box,
        views_box,
        splits_box,
        labels_box,
        labels_heading,
        view: RefCell::new(MailView::Inbox),
        inbox_label: label,
        thread_actions,
        conversation,
        attachments,
        attachments_scroll,
        current_message: RefCell::new(None),
        active_draft: RefCell::new(None),
        reply_thread: RefCell::new(None),
        last_queued_draft: RefCell::new(None),
        undo_button,
        inbox_sync_running: Arc::new(AtomicBool::new(false)),
        queue_retry_at: Cell::new(0),
    });
    for (i, button) in ui.selection_buttons.iter().enumerate() {
        let weak = Rc::downgrade(&ui);
        button.connect_clicked(move |_| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            match i {
                0 => ui.select_loaded(),
                1 => ui.selection_triage("read"),
                2 => ui.selection_triage("archive"),
                3 => ui.undo_triage(),
                _ => ui.clear_marked(),
            }
        });
    }
    let initial_theme = ui.theme_mode.borrow().trim().to_string();
    ui.set_appearance(&initial_theme);
    ui.contact.connect(&ui);
    {
        let ui = ui.clone();
        contact_toggle.connect_clicked(move |_| ui.toggle_contact());
    }
    {
        let action = gtk::gio::SimpleAction::new(
            "open-thread",
            Some(String::static_variant_type().as_ref()),
        );
        let weak = Rc::downgrade(&ui);
        action.connect_activate(move |_, parameter| {
            if let (Some(ui), Some(id)) =
                (weak.upgrade(), parameter.and_then(|p| p.get::<String>()))
            {
                ui.window.present();
                if ui.compose_pane.borrow().is_some() { return; }
                ui.search.set_text("");
                let (tracked, id) = id.strip_prefix("open:").map(|id|(true,id)).unwrap_or((false,id.as_str()));
                let (followup, id) = id.strip_prefix("followup:").map(|id|(true,id)).unwrap_or((false,id));
                ui.inbox_filter.set(0);
                ui.set_view(if tracked { MailView::Sent } else if followup { MailView::Followups } else { MailView::Inbox });
                let index = ui.items.borrow().iter().position(|m| m.thread_id == id);
                if let Some(i) = index {
                    ui.list.select_row(ui.list.row_at_index(i as i32).as_ref());
                }
            }
        });
        app.add_action(&action);
    }
    {
        // Mark as Read / Archive buttons on a new-mail notification.
        let action = gtk::gio::SimpleAction::new(
            "notification-triage",
            Some(String::static_variant_type().as_ref()),
        );
        let weak = Rc::downgrade(&ui);
        action.connect_activate(move |_, parameter| {
            let (Some(ui), Some(raw)) = (weak.upgrade(), parameter.and_then(|p| p.get::<String>())) else { return };
            let Ok((kind, account, thread)) = serde_json::from_str::<(String, String, String)>(&raw) else { return };
            if !matches!(kind.as_str(), "read" | "archive") || thread.is_empty() { return; }
            let op = operation_id();
            match ui.request(json!({"op":"batch_triage","operation_id":op,"kind":kind,"thread_ids":[thread]})) {
                Ok(result) => {
                    if result["undoable"] == true {
                        *ui.last_triage.borrow_mut() = Some((op, vec![account.clone()]));
                    }
                    ui.refresh();
                    ui.status.set_text(if kind == "archive" { "Archived from notification · saved locally" } else { "Marked read from notification · saved locally" });
                    if !account.is_empty() && !account.starts_with("demo:") {
                        ui.background(json!({"op":"sync_account","account_id":account}));
                    }
                }
                Err(e) => ui.status.set_text(&format!("Notification action failed: {e}")),
            }
        });
        app.add_action(&action);
    }
    if let Some(settings) = gtk::Settings::default() {
        let weak = Rc::downgrade(&ui);
        settings.connect_notify_local(Some("gtk-application-prefer-dark-theme"), move |_, _| {
            if let Some(ui) = weak.upgrade() {
                if ui.theme_mode.borrow().as_str() == "system" {
                    ui.set_appearance("system");
                }
            }
        });
    }

    {
        let ui = ui.clone();
        manage.connect_clicked(move |_| ui.manage_accounts());
    }
    {
        let ui = ui.clone();
        agent_access.connect_clicked(move |_| {
            agent::open(&ui);
        });
    }
    {
        // Cheap local poll for agent requests: badge, notifications, open window.
        agent::poll(&ui);
        let weak = Rc::downgrade(&ui);
        glib::timeout_add_local(std::time::Duration::from_secs(5), move || {
            let Some(ui) = weak.upgrade() else { return glib::ControlFlow::Break };
            agent::poll(&ui);
            glib::ControlFlow::Continue
        });
    }
    {
        let action = gtk::gio::SimpleAction::new("open-agent-access", None);
        let weak = Rc::downgrade(&ui);
        action.connect_activate(move |_, _| {
            if let Some(ui) = weak.upgrade() {
                ui.window.present();
                agent::open(&ui);
            }
        });
        app.add_action(&action);
    }
    {
        let ui = ui.clone();
        commands_footer.connect_clicked(move |_| ui.open_palette("root"));
    }
    {
        let ui = ui.clone();
        support_button.connect_clicked(move |_| ui.show_support(false));
    }
    {
        let ui = ui.clone();
        theme_button.connect_clicked(move |_| {
            ui.set_appearance(if ui.dark.get() { "light" } else { "dark" });
        });
    }
    {
        let ui = ui.clone();
        command_button.connect_clicked(move |_| ui.open_palette("root"));
    }
    {
        let ui = ui.clone();
        let entry = ui.command.clone();
        entry.connect_changed(move |_| ui.update_palette());
    }
    {
        let ui = ui.clone();
        let list = ui.palette_rows.clone();
        list.connect_row_activated(move |_, _| ui.run_palette());
    }
    {
        let ui = ui.clone();
        let panel = ui.palette.clone();
        panel.connect_close_request(move |win| {
            win.hide();
            glib::Propagation::Stop
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, _| {
            match key {
                gdk::Key::Escape => {
                    if ui.palette_page.borrow().as_str() != "root" {
                        ui.open_palette("root");
                    } else {
                        ui.palette.hide();
                    }
                }
                gdk::Key::Down | gdk::Key::Up => {
                    let n = ui
                        .palette_rows
                        .selected_row()
                        .map(|r| r.index())
                        .unwrap_or(0)
                        + if key == gdk::Key::Down { 1 } else { -1 };
                    if let Some(row) = ui.palette_rows.row_at_index(n.max(0)) {
                        ui.palette_rows.select_row(Some(&row));
                        row.grab_focus();
                        ui.command.grab_focus();
                    }
                }
                gdk::Key::Return | gdk::Key::KP_Enter => ui.run_palette(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        panel.add_controller(keys);
    }
    for (index, button) in filter_buttons.iter().enumerate() {
        let ui = ui.clone();
        let buttons = filter_buttons.clone();
        button.connect_clicked(move |_| {
            ui.inbox_filter.set(index as u8);
            for (i, button) in buttons.iter().enumerate() {
                if i == index {
                    button.add_css_class("active");
                } else {
                    button.remove_css_class("active");
                }
            }
            ui.search_generation
                .set(ui.search_generation.get().wrapping_add(1));
            ui.search_preview.borrow_mut().take();
            ui.refresh();
        });
    }
    {
        let ui = ui.clone();
        reply.connect_clicked(move |_| ui.compose(true));
    }
    {
        let ui = ui.clone();
        reply_all.connect_clicked(move |_| ui.compose_with_mode(true, true));
    }
    {
        let ui = ui.clone();
        favorite.connect_clicked(move |_| ui.favorite());
    }
    // Keep scheduled sends working while the app is open, including when the
    // optional user systemd timer has not been installed yet.
    {
        let weak = Rc::downgrade(&ui);
        glib::timeout_add_local(std::time::Duration::from_secs(30), move || {
            let Some(ui) = weak.upgrade() else { return glib::ControlFlow::Break };
            ui.notify_new_mail();
            ui.notify_followups();
            ui.notify_opens();
            ui.refresh_navigation(&ui.accounts());
            glib::ControlFlow::Continue
        });
    }
    let dispatch_in_flight = Arc::new(AtomicBool::new(false));
    dispatch_due_in_background(
        ui.database_path.clone(),
        dispatch_in_flight.clone(),
        dispatch_completed.clone(),
    );
    {
        let path = ui.database_path.clone();
        let in_flight = dispatch_in_flight.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(30), move || {
            dispatch_due_in_background(path.clone(), in_flight.clone(), dispatch_completed.clone());
            glib::ControlFlow::Continue
        });
    }
    {
        let ui = ui.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            while let Ok(changed) = dispatch_receiver.try_recv() {
                if changed {
                    ui.refresh();
                }
            }
            glib::ControlFlow::Continue
        });
    }
    {
        let ui = ui.clone();
        new_split.connect_clicked(move |_| ui.create_split());
    }
    {
        let ui = ui.clone();
        compose_button.connect_clicked(move |_| ui.compose(false));
    }
    {
        let ui = ui.clone();
        label_button.connect_clicked(move |_| ui.open_palette("labels"));
    }
    {
        let ui = ui.clone();
        unsubscribe_button.connect_clicked(move |_| ui.unsubscribe());
    }
    ui.check_saved_open_activity();
    {
        let ui=ui.clone();
        tracking_activity.connect_clicked(move |_|{let id=ui.current_message.borrow().as_ref().map(|message|message.id.clone());if ui.contact.root.is_visible() { ui.refresh_contact_activity(true); ui.contact.activity_refresh.grab_focus(); }
            else { booking::activity(&ui,id); }});
    }
    {
        let ui = ui.clone();
        snooze_button.connect_clicked(move |_| {
            if matches!(*ui.view.borrow(), MailView::Snoozed) {
                ui.unsnooze();
            } else {
                ui.open_palette("snooze");
            }
        });
    }
    {
        let ui = ui.clone();
        let button = ui.undo_button.clone();
        button.connect_clicked(move |_| ui.undo_send());
    }
    {
        let ui = ui.clone();
        add_account.connect_clicked(move |_| ui.add_account());
    }
    {
        let ui = ui.clone();
        welcome_action.connect_clicked(move |_| ui.add_account());
    }
    {
        let ui = ui.clone();
        sync.connect_clicked(move |_| ui.sync_all());
    }
    {
        let ui = ui.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            process_completed(&ui, &receiver);
            glib::ControlFlow::Continue
        });
    }
    {
        let ui = ui.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            process_attachments(&ui, &attachment_receiver);
            glib::ControlFlow::Continue
        });
    }
    {
        let ui = ui.clone();
        let list = ui.list.clone();
        list.connect_row_selected(move |_, _| ui.show_selected());
    }
    {
        let ui = ui.clone();
        let list = ui.list.clone();
        list.connect_row_activated(move |_, _| {
            if matches!(*ui.view.borrow(), MailView::Drafts) {
                if ui.selected().is_some() { ui.compose(false); }
                return;
            }
            ui.compact_navigation.set(false);
            ui.compact_message.set(true);
            ui.reader_mode.set(true);
            ui.set_shortcuts(true);
        });
    }
    {
        let ui = ui.clone();
        ui.list_scroll
            .vadjustment()
            .connect_value_changed(move |adjustment| {
                if adjustment.value() + adjustment.page_size() >= adjustment.upper() - 220.0 {
                    ui.load_older();
                }
            });
    }
    {
        let ui = ui.clone();
        let search = ui.search.clone();
        search.connect_search_changed(move |_| ui.schedule_search());
    }
    {
        let ui = ui.clone();
        archive.connect_clicked(move |_| ui.archive());
    }
    {
        let ui = ui.clone();
        let button = ui.remote_images.clone();
        button.connect_clicked(move |_| ui.show_remote_images_menu());
    }
    {
        let ui = ui.clone();
        load_remote_images.connect_clicked(move |_| ui.load_remote_images_for_message());
    }
    {
        let ui = ui.clone();
        let command = ui.command.clone();
        command.connect_activate(move |_| ui.run_palette());
    }
    {
        let ui = ui.clone();
        search_filters.connect_clicked(move |_| ui.search_filters());
    }
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let ui = ui.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let key = parity::shortcut_key(key, mods);
            if mods.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::k {
                ui.open_palette("root");
                return glib::Propagation::Stop;
            }
            if ui.workspace.visible_child_name().as_deref() == Some("calendar") {
                let calendar = ui.calendar.borrow().clone();
                if let Some(calendar) = calendar {
                    return calendar.handle_key(key, mods);
                }
                return glib::Propagation::Proceed;
            }
            if key == gdk::Key::Escape && focus_within(&ui.contact.root) {
                if !ui.list.grab_focus() { ui.mail_reader_viewport.grab_focus(); }
                return glib::Propagation::Stop;
            }
            if ui.activity_host.is_visible() && focus_within(&ui.activity_host) {
                return glib::Propagation::Proceed;
            }
            if ui.compose_pane.borrow().is_some() {
                return glib::Propagation::Proceed;
            }
            if mods.contains(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK)
                && key == gdk::Key::f
            {
                ui.search_filters();
                return glib::Propagation::Stop;
            }
            if focus_within(&ui.search) && (key == gdk::Key::Escape || key == gdk::Key::Return) {
                if key == gdk::Key::Escape {
                    ui.search.set_text("");
                }
                ui.list.grab_focus();
                return glib::Propagation::Stop;
            }
            if !focus_within(&ui.search) && !focus_within(&ui.command) {
                if mods.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::a {
                    ui.select_loaded();
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Escape && !ui.marked.borrow().is_empty() {
                    ui.clear_marked();
                    return glib::Propagation::Stop;
                }

                let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
                if ctrl && mods.contains(gdk::ModifierType::SHIFT_MASK) && key == gdk::Key::u {
                    ui.unsubscribe();
                    return glib::Propagation::Stop;
                }
                if ctrl && (key == gdk::Key::d || key == gdk::Key::u) {
                    let direction = if key == gdk::Key::d { 1.0 } else { -1.0 };
                    if ui.reader_mode.get() {
                        ui.reader_scroll(direction * 0.5, None);
                    } else {
                        let rows =
                            (ui.list_scroll.vadjustment().page_size() / 72.0 / 2.0).max(1.0) as i32;
                        ui.move_by(rows * direction as i32);
                    }
                    return glib::Propagation::Stop;
                }
                if ctrl && key == gdk::Key::r {
                    ui.sync_all();
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Escape {
                    ui.reader_mode.set(false);
                    ui.compact_message.set(false);
                    ui.compact_navigation.set(false);
                    ui.goto_pending.set(false);
                    ui.set_shortcuts(false);
                    ui.list.grab_focus();
                    return glib::Propagation::Stop;
                }
                if ui.reader_mode.get() && !ctrl {
                    let handled = match key {
                        gdk::Key::j => {
                            ui.reader_scroll(96.0, None);
                            true
                        }
                        gdk::Key::k => {
                            ui.reader_scroll(-96.0, None);
                            true
                        }
                        gdk::Key::space => {
                            ui.reader_scroll(
                                if mods.contains(gdk::ModifierType::SHIFT_MASK) {
                                    -0.5
                                } else {
                                    0.5
                                },
                                None,
                            );
                            true
                        }
                        gdk::Key::h => {
                            ui.navigate_history(-1);
                            true
                        }
                        gdk::Key::l => {
                            ui.navigate_history(1);
                            true
                        }
                        gdk::Key::G => {
                            ui.reader_scroll(0.0, Some(false));
                            true
                        }
                        gdk::Key::g => {
                            if ui.reader_top_pending.replace(!ui.reader_top_pending.get()) {
                                ui.reader_scroll(0.0, Some(true));
                            }
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        return glib::Propagation::Stop;
                    }
                }
                if key == gdk::Key::Return {
                    if matches!(*ui.view.borrow(), MailView::Drafts) {
                        if ui.selected().is_some() { ui.compose(false); }
                        return glib::Propagation::Stop;
                    }
                    ui.reader_mode.set(true);
                    ui.compact_message.set(true);
                    ui.set_shortcuts(true);
                    return glib::Propagation::Stop;
                }
            }
            if mods.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::k {
                ui.open_palette("root");
                return glib::Propagation::Stop;
            }
            if focus_within(&ui.search) || focus_within(&ui.command) {
                return glib::Propagation::Proceed;
            }
            if mods.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SUPER_MASK,
            ) {
                // Modified keys retain their own bindings below; never begin a G sequence.
            } else if ui.goto_pending.replace(false) {
                if key == gdk::Key::c {
                    agenda::open(&ui);
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::m {
                    ui.compact_navigation.set(!ui.compact_navigation.get());
                    ui.refresh_navigation(&ui.accounts());
                    return glib::Propagation::Stop;
                }
                for (button, name) in ui.filter_buttons.iter().zip(["All", "Unread", "Favorites"]) {
                    button.set_label(name);
                }
                let destination = match key {
                    gdk::Key::i | gdk::Key::u | gdk::Key::f => Some(MailView::Inbox),
                    gdk::Key::s => Some(MailView::Snoozed),
                    gdk::Key::w => Some(MailView::Followups),
                    gdk::Key::a => Some(MailView::Archive),
                    gdk::Key::d => Some(MailView::Drafts),
                    gdk::Key::t => Some(MailView::Sent),
                    gdk::Key::o => Some(MailView::Outbox),
                    _ => None,
                };
                if let Some(view) = destination {
                    ui.search.set_text("");
                    let filter = match key {
                        gdk::Key::u => 1,
                        gdk::Key::f => 2,
                        _ => 0,
                    };
                    ui.filter_buttons[filter].emit_clicked();
                    ui.set_view(view);
                    ui.list.grab_focus();
                } else {
                    ui.refresh_navigation(&ui.accounts());
                }
                return glib::Propagation::Stop;
            } else if key == gdk::Key::g {
                ui.goto_pending.set(true);
                for (button, name) in
                    ui.filter_buttons
                        .iter()
                        .zip(["All  i", "Unread  u", "Favorites  f"])
                {
                    button.set_label(name);
                }
                ui.refresh_navigation(&ui.accounts());
                return glib::Propagation::Stop;
            }
            if mods.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::z {
                ui.undo_send();
                return glib::Propagation::Stop;
            }
            if !parity::allows_mail_shortcuts(mods) {
                ui.goto_pending.set(false);
                return glib::Propagation::Proceed;
            }
            if matches!(*ui.view.borrow(), MailView::Drafts)
                && matches!(
                    key,
                    gdk::Key::Delete | gdk::Key::KP_Delete | gdk::Key::BackSpace | gdk::Key::numbersign
                )
            {
                ui.discard_selected_draft();
                return glib::Propagation::Stop;
            }
            match key {
                gdk::Key::x => ui.toggle_marked(),
                gdk::Key::j | gdk::Key::J | gdk::Key::Down => {
                    if mods.contains(gdk::ModifierType::SHIFT_MASK) {
                        ui.extend_marked(1)
                    } else {
                        ui.move_by(1)
                    }
                }
                gdk::Key::k | gdk::Key::K | gdk::Key::Up => {
                    if mods.contains(gdk::ModifierType::SHIFT_MASK) {
                        ui.extend_marked(-1)
                    } else {
                        ui.move_by(-1)
                    }
                }
                gdk::Key::slash => {
                    ui.search.grab_focus();
                }
                gdk::Key::e | gdk::Key::E => {
                    if matches!(*ui.view.borrow(), MailView::Archive)
                        && !mods.contains(gdk::ModifierType::SHIFT_MASK)
                    {
                        ui.status
                            .set_text("Already archived · Shift+E to move to Inbox");
                    } else {
                        ui.archive();
                    }
                }
                gdk::Key::c => ui.compose(false),
                gdk::Key::r | gdk::Key::R => {
                    ui.compose_with_mode(true, mods.contains(gdk::ModifierType::SHIFT_MASK))
                }
                gdk::Key::m => ui.mark(false),
                gdk::Key::f => ui.favorite(),
                gdk::Key::u => ui.mark(true),
                gdk::Key::s => ui.open_palette("snooze"),
                gdk::Key::S => ui.sync_all(),
                gdk::Key::exclam => ui.spam(),
                gdk::Key::question => ui.shortcuts_help(),
                gdk::Key::_0 => ui.toggle_contact(),
                gdk::Key::a => ui.add_account(),
                gdk::Key::l => ui.open_palette("labels"),
                gdk::Key::i => ui.toggle_remote_images(),
                gdk::Key::z => ui.undo_triage(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    window.add_controller(keys);
    {
        let weak = Rc::downgrade(&ui);
        navigation_toggle.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.compact_navigation.set(!ui.compact_navigation.get());
            }
        });
    }
    {
        let weak = Rc::downgrade(&ui);
        back_to_inbox.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.close_compose();
                ui.compact_message.set(false);
                ui.compact_navigation.set(false);
                ui.reader_mode.set(false);
                ui.set_shortcuts(false);
                ui.list.grab_focus();
            }
        });
    }
    {
        let weak = Rc::downgrade(&ui);
        window.add_tick_callback(move |window, _| {
            let Some(ui) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let width = window.width();
            // Keep labeled navigation alongside the list or message in normal tiles.
            // Only phone-sized allocations need a separate mailbox view.
            let compact = width < 620;
            let single = width < 1100;
            let navigation = compact && ui.compact_navigation.get();
            let message = ui.compact_message.get() || ui.compose_pane.borrow().is_some();
            let calendar = ui.workspace.visible_child_name().as_deref() == Some("calendar");
            compact_bar.set_visible(!calendar && (compact || (single && message)));
            navigation_toggle.set_visible(compact);
            back_to_inbox.set_visible(single && message && !navigation);
            sidebar_scroll.set_visible(calendar || !compact || navigation);
            columns.set_visible(!navigation);
            ui.update_activity_layout();
            glib::ControlFlow::Continue
        });
    }
    ui.restore_triage_undo();
    ui.refresh();
    window.present();
    ui.sync_inbox_if_configured();
    let weak = Rc::downgrade(&ui);
    let mut idle_ticks = 0u32;
    glib::timeout_add_seconds_local(5, move || {
        let Some(ui) = weak.upgrade().filter(|ui| ui.window.is_visible()) else {
            return glib::ControlFlow::Break;
        };
        idle_ticks = idle_ticks.wrapping_add(1);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let retry = ui.queue_retry_at.get();
        if idle_ticks % 12 == 0 || (retry > 0 && retry <= now) {
            ui.queue_retry_at.set(0);
            ui.sync_inbox_if_configured();
        }
        glib::ControlFlow::Continue
    });
}
fn main() {
    // Check a disposable store before GTK starts, so a bad path is a clear error
    // instead of a panic during window setup.
    let path = match database_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Email: {error}");
            std::process::exit(2);
        }
    };
    let app = gtk::Application::builder()
        .application_id(application_id(store_is_isolated().then(|| path.as_path())))
        .build();
    app.connect_activate(|app| {
        // A relaunch with the same application ID activates this process again.
        // Present the existing window instead of building a second one (and a
        // second MailService) over the same store.
        if let Some(window) = app
            .windows()
            .into_iter()
            .find(|window| window.is::<gtk::ApplicationWindow>())
        {
            window.present();
            return;
        }
        build(app);
    });
    app.run();
}

#[cfg(test)]
mod tests {
    use super::{
        application_id, document, has_remote_images, remote_images_enabled, resolve_store_path,
        snooze_duration, store_path_override, unsubscribe_url,
    };

    #[test]
    fn isolated_stores_get_their_own_valid_application_ids() {
        let base = mail_service::profile::LINUX_APP_ID;
        assert_eq!(application_id(None), base);
        let demo = application_id(Some(std::path::Path::new("/repo/dist/demo/mail.sqlite")));
        assert!(demo.starts_with(&format!("{base}.Isolated.s")) && demo.len() == base.len() + 23);
        assert!(super::gtk::gio::Application::id_is_valid(&demo));
        assert_eq!(demo, application_id(Some(std::path::Path::new("/repo/dist/demo/mail.sqlite"))));
        assert_ne!(demo, application_id(Some(std::path::Path::new("/other/mail.sqlite"))));
    }

    #[test]
    fn store_override_uses_a_private_directory() {
        assert!(store_path_override(None).unwrap().is_none());
        assert!(store_path_override(Some("".into())).is_err());
        assert!(store_path_override(Some("/".into())).is_err());
        let directory = std::env::temp_dir().join(format!(
            "email-linux-store-override-{}",
            std::process::id()
        ));
        let path = directory.join("nested/mail.sqlite");
        let resolved = store_path_override(Some(path.clone().into())).unwrap().unwrap();
        assert_eq!(
            resolved,
            std::fs::canonicalize(path.parent().unwrap()).unwrap().join("mail.sqlite")
        );
        let relative = resolve_store_path(Some("relative-store/mail.sqlite".into()))
            .unwrap()
            .unwrap();
        assert!(relative.is_absolute() && relative.ends_with("relative-store/mail.sqlite"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
            // An existing directory is used as is, as on macOS.
            let shared = directory.join("shared");
            std::fs::create_dir(&shared).unwrap();
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(store_path_override(Some(shared.join("mail.sqlite").into())).is_ok());
            let file_link = path.parent().unwrap().join("linked.sqlite");
            std::os::unix::fs::symlink(&path, &file_link).unwrap();
            assert!(store_path_override(Some(file_link.into())).is_err());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn remote_images_are_blocked_unless_explicitly_enabled() {
        let directory = std::env::temp_dir().join(format!("email-remote-images-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("mail.sqlite");
        let preference = directory.join("remote-images-enabled");
        let _ = std::fs::remove_file(&preference);
        assert!(!remote_images_enabled(&database), "a new install blocks remote images");
        std::fs::write(&preference, "1\n").unwrap();
        assert!(remote_images_enabled(&database), "an explicit choice to load is kept");
        std::fs::write(&preference, "0\n").unwrap();
        assert!(!remote_images_enabled(&database), "an explicit choice to block is kept");
        std::fs::write(&preference, "").unwrap();
        assert!(!remote_images_enabled(&database), "an unreadable value blocks");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn remote_image_detection_matches_the_reader_policy() {
        for html in [
            r#"<img src="https://example.com/a.png">"#,
            r#"<IMG alt="x" SRC='https://example.com/a.png'>"#,
            r#"<td style="background:url(https://example.com/b.png)">"#,
            r#"<td style="background-image: url(&quot;https://example.com/b.png&quot;)">"#,
            r#"<td style="background-image: image-set(&quot;https://example.com/b.png&quot; 1x)">"#,
            r#"<td style="background:u\72l(https://example.com/b.png)">"#,
            r#"<img alt="Save > 50%" src="https://cdn.example/x.png">"#,
        ] {
            assert!(has_remote_images(html), "detects {html}");
        }
        for html in [
            r#"<img src="data:image/png;base64,AAAA">"#,
            r#"<img src="cid:logo@example">"#,
            r#"<img srcset="https://example.com/a.png 2x" src="cid:a">"#,
            r#"<img alt="Use src=https://cdn.example/x" src="cid:logo">"#,
            r#"<imgx src="https://example.com/a.png">"#,
            r#"<td style="background:url(http://example.com/never-allowed.png)">"#,
            r#"<a href="https://example.com">https://example.com</a>"#,
            "<p>Plain text</p>",
            r#"<p style="color:red">See url(https://example.com) in text</p>"#,
        ] {
            assert!(!has_remote_images(html), "ignores {html}");
        }
    }

    #[test]
    fn blocked_documents_allow_only_inline_images() {
        assert!(document("<p>x</p>", false, false).contains("img-src data:;"));
        assert!(document("<p>x</p>", true, false).contains("img-src data: https:;"));
    }

    #[test]
    fn load_images_tooltip_matches_macos_and_names_the_default() {
        assert_eq!(
            super::load_images_tooltip(true),
            "Load remote images in this email. I to block remote images by default"
        );
    }

    #[test]
    fn image_control_tooltip_matches_macos_and_names_the_default() {
        assert_eq!(
            super::remote_images_tooltip(true, false),
            "Remote images are loaded in this email. Image options · I to always load remote images by default"
        );
        assert_eq!(
            super::remote_images_tooltip(false, true),
            "Remote images are blocked. Image options · I to block remote images by default"
        );
    }

    #[test]
    fn snooze_duration_is_explicit_and_bounded() {
        assert_eq!(snooze_duration("5"), Some((300, "In 5 minutes".into())));
        assert_eq!(snooze_duration("3h"), Some((10_800, "In 3 hours".into())));
        assert_eq!(snooze_duration("1d"), Some((86_400, "In 1 day".into())));
        assert!(snooze_duration("0m").is_none());
        assert!(snooze_duration("400d").is_none());
        assert!(snooze_duration("tomorrow maybe").is_none());
    }

    #[test]
    fn unsubscribe_uses_only_confirmable_links() {
        let html = r#"<a href="javascript:alert(1)">Unsubscribe</a><a href="https://example.org/remove?a=1&amp;b=2">Unsubscribe</a>"#;
        assert_eq!(
            unsubscribe_url(html).unwrap().as_str(),
            "https://example.org/remove?a=1&b=2"
        );
        assert!(unsubscribe_url(r#"<a href="file:///etc/hosts">Unsubscribe</a>"#).is_none());
        assert_eq!(
            unsubscribe_url(r#"<a href="mailto:leave@example.org" aria-label="Opt out"></a>"#)
                .unwrap()
                .as_str(),
            "mailto:leave@example.org"
        );
        // A mailto the reader sanitizer rejects must not hide a later usable link.
        assert_eq!(
            unsubscribe_url(
                r#"<a href="mailto:leave@example.org?subject=a%0Ab">Unsubscribe</a> <a href="https://example.org/leave">Unsubscribe</a>"#
            )
            .unwrap()
            .as_str(),
            "https://example.org/leave"
        );
    }
}
