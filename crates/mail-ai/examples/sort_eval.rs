//! Accuracy check for the local sorter on fixed synthetic emails.
//!
//! cargo run --release -p mail-ai --example sort_eval -- <sorter.safetensors> <tokenizer.json> [records]
//!
//! Uses the app's starter filters and thresholds. Without a record file it
//! runs the 30 built-in emails; a record file is `group \x1f from \x1f subject
//! \x1f body \x1e` per email. "Real mail hidden" counts emails a person should
//! see that would leave the inbox; it is the number that matters.

use mail_ai::{Category, Classifier, Email};
use std::time::Instant;

const STARTERS: [(&str, &str, f32); 4] = [
    ("Cold outreach", "Unsolicited sales pitches: a stranger offering their own service or product, even when written personally or as a question. Agencies, dev shops, outsourcing, SEO, lead generation, growth services, recruiters offering candidates, sales demos, follow-ups to their own earlier pitch.", 0.95),
    ("Promotions", "Marketing from companies trying to sell: sales, discounts, offers, product launches, events selling tickets. Not receipts, tickets you bought, bookings, bills or account messages.", 0.8),
    ("Newsletters", "Newsletters, digests and blog posts I subscribed to.", 0.8),
    ("Notifications", "Automated messages from services: alerts, GitHub and CI, receipts, invoices and bills, bookings and tickets, shipping, reminders, security and account messages.", 0.8),
];

const SET: &[(&str, &str, &str, &str)] = &[
    ("Cold outreach", "Ravi Menon <ravi@brightforge.io>", "Quick question about Email's roadmap", "Hi Thomas, saw Email on GitHub, love the local-first angle. We're a 40-person studio that has shipped desktop apps and could take a mobile client off your plate. Open to a 15-minute call next week? Here's my calendar."),
    ("Cold outreach", "Marcus Hale <marcus@outboundlabs.co>", "Re: Following up", "Hi Thomas, just bumping this to the top of your inbox. We help founders book 20+ qualified demos a month. Worth a chat?"),
    ("Cold outreach", "Priya at Stackhire <priya@stackhire.dev>", "Senior Rust engineers available now", "Hi Thomas, we have 3 vetted senior Rust engineers available immediately for contract work. Would you like profiles?"),
    ("Cold outreach", "Dana Wu <dana@rankrocket.agency>", "Your site isn't ranking for 'email client'", "Hey, I ran an audit of ycorplabs.com and found 14 issues hurting your Google ranking. We can fix them in 30 days. Want the report?"),
    ("Cold outreach", "Tom from LaunchPad <tom@launchpadgrowth.com>", "10k users in 30 days?", "Thomas — we've helped 120 dev tools get their first 10k users with our Product Hunt + Reddit playbook. Pricing starts at $2,500. Interested?"),
    ("Cold outreach", "Elif Kaya <elif@nordicdevpartners.com>", "Partnership opportunity", "Hi Thomas, I lead partnerships at Nordic Dev Partners. We offer white-label development teams in Istanbul at 40% lower cost. Could we schedule a short intro call?"),
    ("Cold outreach", "Ben Carter <ben@pitchdeckpro.com>", "Investors are asking for this", "Founders who use our pitch deck design service raise 2x faster. Can I send you three examples from your space?"),
    ("Cold outreach", "Olivia <olivia@leadflow.ai>", "Thomas, saw you're hiring", "Noticed YCorp is growing. Our AI SDR books meetings on autopilot. Would a 10 min demo be crazy?"),
    ("Cold outreach", "Kari Nilsen <kari@vekstbyraa.no>", "Mer synlighet for YCorp?", "Hei Thomas! Vi hjelper oppstartsselskaper med SEO og annonsering. Har du 15 minutter neste uke til en uforpliktende prat?"),
    ("People", "Sara Lind <sara@acme-logistics.no>", "Bug: sync stops after sleep", "Hey! Using your app daily, but after my Mac wakes from sleep the Gmail account stops syncing until I restart. Happy to send logs if useful. Thanks for building this."),
    ("People", "Jonas Berg <jonas.berg@gmail.com>", "Middag på fredag?", "Hei! Skal vi ta middag på fredag? Tenkte vi kunne prøve den nye plassen på Grünerløkka."),
    ("People", "Maya Chen <maya@quillboard.com>", "Would love to try Email at Quillboard", "Hi Thomas, our team of 25 is looking for a keyboard-first mail client. Do you have a team plan or a way to pay for 25 seats? We'd happily be early customers."),
    ("People", "Alex Rivera <alex@seedcamp.example>", "Intro: Email by YCorp", "Hi Thomas, Sofie suggested I reach out. I'm a partner at a seed fund and we invest in developer tools. Would love to hear what you're building, no deck needed."),
    ("People", "Hannah Ross <hannah@theverge.example>", "Interview request: local-first email", "Hi Thomas, I'm writing a piece about local-first software and would love to include Email. Do you have 20 minutes this week for a call?"),
    ("People", "Erik Solheim <erik@ntnu.no>", "Guest lecture in March?", "Hi Thomas, would you be up for giving a guest lecture to our entrepreneurship students in March? Happy to work around your schedule."),
    ("People", "Lucas Martin <lucas.martin@protonmail.com>", "Feature idea: snooze until reply", "Hey, love the app. Would it be possible to snooze a thread until someone replies? I'd pay for that. Keep it up!"),
    ("People", "Nora Haugen <nora@fernhill.example>", "Contract for the pilot", "Hi Thomas, attached is the signed contract for the pilot. Let me know when you want to kick off."),
    ("People", "Sam Okafor <sam@okafor.dev>", "Contributing to Email", "Hi! I'd like to contribute the Linux tray icon. Is there an issue I should pick up first? I have done GTK work before."),
    ("Promotions", "Fjellsport <nyhetsbrev@fjellsport.no>", "Black Week: opptil 50% på alt", "Black Week er i gang! Spar opptil 50 % på jakker, sko og utstyr. Kun denne uken."),
    ("Promotions", "Apple <news@insideapple.apple.com>", "The new MacBook Pro is here", "Meet the most powerful MacBook Pro ever. Order now and get it by Friday. Trade in your current Mac and save."),
    ("Promotions", "Figma <events@figma.com>", "Config tickets are on sale", "Join us in San Francisco for Config 2027. Early-bird tickets are 30% off until Friday."),
    ("Promotions", "Notion <team@makenotion.com>", "Try Notion AI free for 30 days", "Upgrade your workspace with Notion AI. Start your free trial today, cancel anytime."),
    ("Newsletters", "Lenny's Newsletter <lenny@substack.com>", "How the best PMs prioritize", "This week: a framework for prioritization from 20 top product leaders. Read in app. Unsubscribe."),
    ("Newsletters", "This Week in Rust <twir@this-week-in-rust.org>", "This Week in Rust 620", "Hello and welcome to another issue of This Week in Rust! Updates from Rust community, crate of the week, call for participation."),
    ("Newsletters", "Benedict Evans <list@ben-evans.com>", "Benedict's Newsletter: No. 612", "Welcome to the weekly newsletter. Top news: Apple, OpenAI, regulation. Paid subscribers get the full column."),
    ("Notifications", "GitHub <noreply@github.com>", "[YCorpLabs/Email] Run failed: validate", "The workflow validate failed on main. View run."),
    ("Notifications", "Stripe <receipts@stripe.com>", "Your receipt from Hetzner #2231-7781", "Amount paid: €12.40. Date paid: Nov 17, 2026. Payment method: Visa 4242."),
    ("Notifications", "Google <no-reply@accounts.google.com>", "Security alert", "A new sign-in to your Google Account was detected on a Mac. If this was you, you don't need to do anything."),
    ("Notifications", "Posten <varsel@posten.no>", "Pakken din er klar til henting", "Pakken din fra Komplett er klar til henting på Coop Extra Grünerløkka. Hentekode: 4821."),
    ("Notifications", "Linear <notifications@linear.app>", "ENG-412 was assigned to you", "Sofie assigned ENG-412: Crash when opening calendar invite to you."),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let started = Instant::now();
    let classifier = Classifier::load(args[1].as_ref(), args[2].as_ref()).expect("load");
    eprintln!("loaded in {:?}", started.elapsed());
    let filters: Vec<Category> = STARTERS.iter().map(|(n, d, _)| Category { name: (*n).into(), description: (*d).into() }).collect();
    let owned: Vec<(String, String, String, String)> = match args.get(3) {
        Some(path) => std::fs::read_to_string(path)
            .expect("records")
            .split('\x1e')
            .filter(|r| !r.is_empty())
            .map(|r| {
                let f: Vec<&str> = r.split('\x1f').collect();
                (f[0].into(), f[1].into(), f[2].into(), f[3].into())
            })
            .collect(),
        None => SET.iter().map(|(g, f, s, b)| ((*g).into(), (*f).into(), (*s).into(), (*b).into())).collect(),
    };
    let (mut right, mut hidden, mut times) = (0, 0, Vec::new());
    for (expected, from, subject, body) in &owned {
        let bulk = body.contains("[Bulk mail with an unsubscribe link]") || body.contains("Unsubscribe");
        let body = body.replace("\n[Bulk mail with an unsubscribe link]", "");
        let t = Instant::now();
        let v = classifier
            .classify(&filters, &Email { from: from.clone(), subject: subject.clone(), body }, bulk)
            .expect("classify")
            .expect("filters");
        times.push(t.elapsed());
        let (name, _, need) = STARTERS[v.index];
        let got = if v.confidence >= need { name } else { "People" };
        let ok = got == expected;
        right += ok as usize;
        let leaves_inbox = got != "People" && got != "Notifications";
        hidden += (expected == "People" && leaves_inbox) as usize;
        if !ok || args.get(3).is_none() {
            println!("{} {:<14} {:>4.0}%  {}", if ok { "ok " } else { "XX " }, got, v.confidence * 100.0, subject.chars().take(60).collect::<String>());
        }
    }
    times.sort();
    println!(
        "\n{right}/{} right · real mail hidden {hidden} · median {:?} · max {:?}",
        owned.len(),
        times[times.len() / 2],
        times[times.len() - 1]
    );
}
