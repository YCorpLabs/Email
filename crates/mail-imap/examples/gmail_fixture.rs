//! Run the synthetic Gmail IMAP/SMTP fixture for native-shell QA.
//!
//! cargo run -p mail-imap --features test-fixture --example gmail_fixture -- /path/to/fixture-ca.pem
//!
//! Prints the environment for a QA build of the app (built with the
//! `test-fixture` feature), then logs each received command until stopped.
//! It never contacts Google and accepts only the synthetic scenario accounts.
use mail_imap::fixture::{
    FIXTURE_APP_PASSWORD, FIXTURE_APP_PASSWORD_REQUIRED, FIXTURE_IMAP_DISABLED, FIXTURE_SLOW,
    FIXTURE_USER, FIXTURE_WEB_LOGIN, GmailFixture,
};
use std::{io::Write, thread, time::Duration};

fn main() -> std::io::Result<()> {
    let ca_path = std::env::args()
        .nth(1)
        .expect("usage: gmail_fixture <path for the fixture CA PEM>");
    let fixture = GmailFixture::start()?;
    std::fs::write(&ca_path, fixture.ca_pem())?;
    println!("EMAIL_TEST_FIXTURE_CA={ca_path}");
    println!(
        "EMAIL_TEST_FIXTURE_ENDPOINTS=imap.gmail.com:993=localhost:{},smtp.gmail.com:465=localhost:{}",
        fixture.imap_port, fixture.smtp_port
    );
    println!("# Signs in:            {FIXTURE_USER} / {FIXTURE_APP_PASSWORD}");
    println!("# Wrong password:      {FIXTURE_USER} / any other value");
    println!("# Regular password:    {FIXTURE_APP_PASSWORD_REQUIRED} / anything");
    println!("# IMAP off by admin:   {FIXTURE_IMAP_DISABLED} / anything");
    println!("# Web login required:  {FIXTURE_WEB_LOGIN} / anything");
    println!("# Slow, then rejected: {FIXTURE_SLOW} / any 16 letters");
    std::io::stdout().flush()?;
    let mut seen = 0;
    let mut delivered = 0;
    loop {
        thread::sleep(Duration::from_millis(250));
        let commands = fixture.commands();
        for command in &commands[seen..] {
            println!("command: {command}");
        }
        seen = commands.len();
        let deliveries = fixture.deliveries();
        for delivery in &deliveries[delivered..] {
            println!(
                "delivered: from {} to {:?}, {} bytes",
                delivery.mail_from,
                delivery.recipients,
                delivery.data.len()
            );
        }
        delivered = deliveries.len();
        std::io::stdout().flush()?;
    }
}
