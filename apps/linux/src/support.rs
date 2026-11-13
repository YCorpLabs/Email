//! User-reviewed support drafts. No automatic collection or delivery.
use super::*;

/// Shown instead of the support chooser when the build has no channels.
/// Keep identical to DistributionProfile.supportUnavailableMessage on macOS.
const SUPPORT_UNAVAILABLE: &str = "This build has no support address configured, so Email can't start a support or feedback message.\n\nIf someone else gave you this build, ask them for help. If you built it yourself, see FORKING.md to add your own support channels.";

/// Support and feedback destinations come only from the distribution profile.
/// Builds without both get none; there is never a fallback address.
fn channels() -> Option<(&'static str, &'static str)> {
    Some((mail_service::profile::SUPPORT_ADDRESS?, mail_service::profile::FEEDBACK_ADDRESS?))
}

impl Ui {
    fn show_support_unavailable(&self) {
        let dialog = gtk::Window::builder()
            .title("Support & feedback")
            .transient_for(&self.window)
            .modal(true)
            .default_width(420)
            .resizable(true)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 12);
        content.add_css_class("settings-card");
        let message = gtk::Label::new(Some(SUPPORT_UNAVAILABLE));
        message.set_wrap(true);
        message.set_xalign(0.0);
        message.set_max_width_chars(48);
        content.append(&message);
        let ok = gtk::Button::with_label("OK");
        let close = dialog.clone();
        ok.connect_clicked(move |_| close.close());
        content.append(&ok);
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
        dialog.set_child(Some(&content));
        dialog.present();
        ok.grab_focus();
    }

    pub(super) fn show_support(self: &Rc<Self>, feedback: bool) {
        let Some((support_address, feedback_address)) = channels() else {
            self.show_support_unavailable();
            return;
        };
        if self.compose_pane.borrow().is_some() {
            self.status
                .set_text("Save and close your current draft before opening support or feedback.");
            return;
        }
        let dialog = gtk::Window::builder()
            .title("Support & feedback")
            .transient_for(&self.window)
            .modal(true)
            .default_width(420)
            .resizable(true)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 12);
        content.add_css_class("settings-card");
        let intro = gtk::Label::new(Some(&format!(
            "Ask for help at {support_address} or share an idea at {feedback_address}. Review and send the email yourself.\n\nUse Attach file in the composer to add screenshots or logs. Attachments are optional and can be removed. Check them for private information before sending.",
        )));
        intro.set_wrap(true);
        intro.set_xalign(0.0);
        intro.set_max_width_chars(48);
        content.append(&intro);
        let include = gtk::CheckButton::with_label("Include basic app information");
        include.set_active(false);
        content.append(&include);
        let info = format!(
            "Email {}\nBuild: {}\nLinux / {}",
            env!("CARGO_PKG_VERSION"),
            option_env!("EMAIL_BUILD_ID").unwrap_or("development"),
            std::env::consts::ARCH
        );
        let preview = gtk::Label::new(Some(&format!(
            "{info}\n\nOnly the information above is included. No mail, accounts or logs are collected."
        )));
        preview.set_wrap(true);
        preview.set_xalign(0.0);
        preview.set_selectable(true);
        preview.set_max_width_chars(48);
        content.append(&preview);
        let mut first = None;
        for is_feedback in [feedback, !feedback] {
            let button = gtk::Button::with_label(if is_feedback {
                "Give feedback"
            } else {
                "Ask for support"
            });
            let ui = self.clone();
            let dialog = dialog.clone();
            let include = include.clone();
            let info = info.clone();
            button.connect_clicked(move |_| {
                // Another window/action must never replace an unsaved composer.
                if ui.compose_pane.borrow().is_some() { return; }
                let recipient = if is_feedback { feedback_address } else { support_address };
                let subject = if is_feedback { "Email feedback" } else { "Email support request" };
                let mut body = if is_feedback { "My idea or feedback:\n\n" } else { "What happened?\n\nWhat did you expect?\n\nSteps to reproduce (if relevant):\n\n" }.to_string();
                if include.is_active() { body.push_str(&format!("\nApp information (optional):\n{info}\n")); }
                if ui.accounts().is_empty() {
                    let mut link = Url::parse(&format!("mailto:{recipient}")).expect("profile address is validated at build time");
                    link.query_pairs_mut().append_pair("subject", subject).append_pair("body", &body);
                    let uri = link.as_str().replace('+', "%20");
                    gtk::gio::AppInfo::launch_default_for_uri_async(&uri, None::<&gtk::gio::AppLaunchContext>, None::<&gtk::gio::Cancellable>, {
                        let ui = ui.clone();
                        move |result| if result.is_err() { ui.status.set_text(&format!("Could not open a mail app. Email {recipient}, or add an account to Email and try again.")); }
                    });
                } else {
                    ui.compose_with_content(false, false, Some((recipient, subject, &body)));
                }
                dialog.close();
            });
            content.append(&button);
            if first.is_none() {
                first = Some(button);
            }
        }
        let cancel = gtk::Button::with_label("Cancel");
        let close = dialog.clone();
        cancel.connect_clicked(move |_| close.close());
        content.append(&cancel);
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
        dialog.set_child(Some(&content));
        dialog.present();
        if let Some(first) = first {
            first.grab_focus();
        }
    }
}

pub(super) fn is_feedback(to: &str) -> bool {
    is_feedback_for(to, channels().map(|(_, feedback)| feedback))
}

fn is_feedback_for(to: &str, feedback: Option<&str>) -> bool {
    let Some(feedback) = feedback else { return false };
    mail_core::parse_outgoing_recipients(to).is_ok_and(|recipients| {
        recipients
            .iter()
            .any(|recipient| parity::address(recipient).eq_ignore_ascii_case(feedback))
    })
}

impl Ui {
    pub(super) fn show_feedback_thanks(&self) {
        let Some(host) = self.window.child().and_downcast::<gtk::Overlay>() else {
            return;
        };
        // Replace an earlier celebration instead of stacking cards.
        let mut child = host.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if widget.has_css_class("feedback-celebration") {
                host.remove_overlay(&widget);
            }
        }
        let effect = gtk::Overlay::new();
        effect.add_css_class("feedback-celebration");
        effect.set_can_target(false);
        effect.set_focusable(false);
        let confetti = gtk::DrawingArea::new();
        confetti.set_hexpand(true);
        confetti.set_vexpand(true);
        let reduced_motion =
            gtk::Settings::default().is_some_and(|settings| !settings.is_gtk_enable_animations());
        let started = std::time::Instant::now();
        confetti.set_draw_func(move |_, context, width, height| {
            if reduced_motion {
                return;
            }
            let t = started.elapsed().as_secs_f64();
            let colors = [
                (0.65, 0.4, 0.95),
                (0.95, 0.35, 0.6),
                (0.95, 0.8, 0.25),
                (0.25, 0.8, 0.75),
                (0.3, 0.6, 0.95),
            ];
            if t >= 2.1 {
                return;
            }
            let spread = (width as f64 * 0.28).min(180.0);
            for index in 0..36 {
                let fraction = index as f64 / 35.0;
                let x = width as f64 / 2.0 + (fraction - 0.5) * spread * 2.0 * t;
                let y = height as f64 - 72.0 - (160.0 + ((index * 23) % 110) as f64) * t
                    + 115.0 * t * t;
                let (r, g, b) = colors[index % colors.len()];
                let _ = context.save();
                context.translate(x, y);
                context.rotate(((index * 31) as f64 + t * 130.0).to_radians());
                context.set_source_rgba(r, g, b, (t / 0.08).min(1.0) * (1.0 - t / 2.1).max(0.0));
                context.rectangle(-2.0, -3.5, 4.0, 7.0);
                let _ = context.fill();
                let _ = context.restore();
            }
        });
        effect.set_child(Some(&confetti));
        let card = gtk::Box::new(Orientation::Horizontal, 12);
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::End);
        card.set_margin_bottom(40);
        card.set_margin_start(16);
        card.set_margin_end(16);
        card.add_css_class("feedback-thanks");
        let heart = gtk::Label::new(Some("♥"));
        heart.add_css_class("feedback-heart");
        card.append(&heart);
        let title = gtk::Label::new(Some("Thanks for the feedback!"));
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.add_css_class("feedback-title");
        card.append(&title);
        effect.add_overlay(&card);
        host.add_overlay(&effect);
        if !reduced_motion {
            card.set_opacity(0.0);
            let card = card.downgrade();
            confetti.add_tick_callback(move |area, _| {
                let t = started.elapsed().as_secs_f64();
                if t >= 3.2 {
                    return glib::ControlFlow::Break;
                }
                if let Some(card) = card.upgrade() {
                    let entrance = (t / 0.22).min(1.0);
                    let eased = 1.0 - (1.0 - entrance).powi(3);
                    card.set_opacity(eased.min(((3.2 - t) / 0.35).max(0.0)));
                    card.set_margin_bottom(40 - (8.0 * (1.0 - eased)).round() as i32);
                }
                area.queue_draw();
                glib::ControlFlow::Continue
            });
        }
        let host = host.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(3200), move || {
            if let Some(host) = host.upgrade() {
                if effect.parent().is_some() {
                    host.remove_overlay(&effect);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_match_uses_only_the_configured_mailbox() {
        let feedback = Some("ideas@example.test");
        assert!(is_feedback_for("Ideas <IDEAS@example.test>", feedback));
        assert!(is_feedback_for("other@example.test, ideas@example.test", feedback));
        assert!(!is_feedback_for("help@example.test", feedback));
        assert!(!is_feedback_for("ideas@example.test.evil.test", feedback));
        assert!(!is_feedback_for("ideas@example.test", None));
        assert_eq!(
            channels().is_some(),
            mail_service::profile::SUPPORT_ADDRESS.is_some()
                && mail_service::profile::FEEDBACK_ADDRESS.is_some()
        );
    }
}
