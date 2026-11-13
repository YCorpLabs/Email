//! The launch demo's people, companies and conversations. All invented.
use super::clock::{Clock, DAY};
use super::files::{self, Invite, Part};
use super::{Demo, Draft, Msg, P, Thread, from, me};

pub(crate) const ALEX: P = P("Alex Morgan", "alex@fernhill.example");
pub(crate) const ALEX_HOME: P = P("Alex Morgan", "alex@example.com");

// Fernhill, the demo user's employer.
pub(super) const INGRID: P = P("Ingrid Solberg", "ingrid@fernhill.example");
pub(super) const MAYA: P = P("Maya Chen", "maya@fernhill.example");
pub(super) const JONAS: P = P("Jonas Berg", "jonas@fernhill.example");
pub(super) const PRIYA: P = P("Priya Raman", "priya@fernhill.example");
pub(super) const SAM: P = P("Sam Okafor", "sam@fernhill.example");
pub(super) const LUCAS: P = P("Lucas Moreau", "lucas@fernhill.example");
pub(super) const HANNA: P = P("Hanna Lie", "hanna@fernhill.example");
pub(super) const TOMAS: P = P("Tomás Silva", "tomas@fernhill.example");
pub(super) const TEAM: P = P("Fernhill Team", "team@fernhill.example");
pub(super) const PAYROLL: P = P("Fernhill Lønn", "lonn@fernhill.example");

// Customers, partners and contacts.
pub(super) const ELENA: P = P("Elena Rossi", "elena.rossi@quaypoint.example");
pub(super) const ADA: P = P("Ada Lind", "ada@brightwater.example");
pub(super) const BRIGHTWATER_LEGAL: P = P("Brightwater Legal", "legal@brightwater.example");
pub(super) const DANIEL: P = P("Daniel Park", "daniel@kittiwake.test");
pub(super) const MARCUS: P = P("Marcus Webb", "marcus@harbourlight.example");
pub(super) const NOOR: P = P("Noor Haddad", "noor@ellipsis-studio.example");
pub(super) const OLIVIA: P = P("Olivia Grant", "olivia.grant@lakeview-clinics.example");
pub(super) const KARI: P = P("Kari Nygaard", "kari@nygaard-regnskap.example");
pub(super) const LARKMOOR: P = P("Larkmoor Security", "reports@larkmoor.test");
pub(super) const PRODUCT_DAYS: P = P("Oslo Product Days", "speakers@productdays.example");
pub(super) const REGISTRAR: P = P("Namecove", "renewals@namecove.test");

// Friends and family.
pub(super) const ERIK: P = P("Erik Haugen", "erik.haugen@example.org");
pub(super) const MAMMA: P = P("Randi Morgan", "randi.morgan@example.net");
pub(super) const ROWAN: P = P("Rowan Lee", "rowan@example.net");
pub(super) const SOFIA: P = P("Sofia Andersen", "sofia@example.org");

// Services.
pub(super) const CI: P = P("Forgeyard Actions", "actions@forgeyard.test");
pub(super) const FORGE: &str = "notifications@forgeyard.test";
pub(super) const QUILL: &str = "comments@quillboard.test";
pub(super) const PINGWELL: P = P("Pingwell Alerts", "alerts@pingwell.test");
pub(super) const CIRRUS: P = P("Cirrusbay Billing", "billing@cirrusbay.test");
pub(super) const TALLY: P = P("Tallyhouse Receipts", "receipts@tallyhouse.test");
pub(super) const NORDLYS: P = P("Nordlys Air", "booking@nordlys-air.test");
pub(super) const HOTEL: P = P("Hotel Kaiholmen", "reservations@kaiholmen.example");
pub(super) const SKYRAIL: P = P("Skyrail Nordic", "tickets@skyrail.test");
pub(super) const PARCEL: P = P("Parcelpoint", "track@parcelpoint.test");
pub(super) const BANK: P = P("Fjellby Bank", "varsel@fjellbybank.example");
pub(super) const POWER: P = P("Fossekraft Strøm", "faktura@fossekraft.example");
pub(super) const COOP: P = P("Borettslaget Solsiden", "styret@solsiden.example");
pub(super) const DENTIST: P = P("Tannlegene på Hjørnet", "post@tannhjornet.example");
pub(super) const LIBRARY: P = P("Byens bibliotek", "varsel@byensbibliotek.example");
pub(super) const CLIMB: P = P("Sentrum Klatresenter", "medlem@klatresenter.example");

// Newsletters and promotions.
pub(super) const QUIET: P = P("The Quiet Interface", "hello@quietinterface.example");
pub(super) const SYSTEMS: P = P("Systems Digest", "digest@systemsdigest.example");
pub(super) const FOUNDER: P = P("Founder Notes", "notes@foundernotes.example");
pub(super) const HARBOUR: P = P("Harbour Light", "newsletter@harbourlight.example");
pub(super) const FJORDBYTE: P = P("Fjordbyte Weekly", "weekly@fjordbyte.example");
pub(super) const FOREST: P = P("Fjord & Forest", "news@fjordforest.example");
pub(super) const PAPERLEAF: P = P("Paperleaf Books", "hello@paperleaf.example");
pub(super) const BEAN: P = P("Lille Bønne Kaffe", "hei@lillebonne.example");
pub(super) const TIDY: P = P("Tidy Desk Co.", "offers@tidydesk.example");

/// A message from a sender whose display name varies, such as notifications.
fn from_named(name: &str, email: &str, at: i64, body: impl Into<String>) -> Msg {
    Msg {
        from: Some(format!("{name} <{email}>")),
        ..from(ALEX, at, body)
    }
}

/// Deterministic pseudo-random numbers, so every run is identical.
pub(super) struct Rng(u64);
impl Rng {
    pub(super) fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub(super) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

pub(crate) fn launch(c: &Clock) -> Demo {
    let mut demo = Demo::default();
    work(c, &mut demo);
    calendar(c, &mut demo);
    personal(c, &mut demo);
    norwegian(c, &mut demo);
    travel(c, &mut demo);
    receipts(c, &mut demo);
    notifications(c, &mut demo);
    newsletters(c, &mut demo);
    promotions(c, &mut demo);
    support(c, &mut demo);
    sent(c, &mut demo);
    older(c, &mut demo);
    drafts(c, &mut demo);
    demo.splits = vec![
        ("demo-split-team", "Team", "from:*@fernhill.example"),
        (
            "demo-split-notifications",
            "Notifications",
            "*@forgeyard.test, *@pingwell.test, *@quillboard.test",
        ),
        ("demo-split-newsletters", "Newsletters", "label:Newsletters"),
        ("demo-split-promotions", "Promotions", "label:Promotions"),
    ];
    demo.snippets = vec![
        (
            "Intro call times",
            "Happy to! Here are a few times that work for me next week:\n\n",
        ),
        (
            "Thanks, briefly",
            "Thanks for this! I’ll take a look and get back to you by the end of the week.",
        ),
        (
            "Tusen takk",
            "Tusen takk! Jeg ser på det og kommer tilbake til deg i løpet av uken.",
        ),
    ];
    demo.signatures = vec![(
        super::DEMO_WORK_ACCOUNT,
        "alex@fernhill.example",
        "Alex Morgan\nHead of Product · Fernhill\nhttps://fernhill.example",
    )];
    demo
}

/// The launch is the next Tuesday at least three days away.
fn launch_day(c: &Clock) -> i64 {
    let days = c.days_until(1);
    if days < 3 { days + 7 } else { days }
}

fn work(c: &Clock, d: &mut Demo) {
    let launch = c.ahead(launch_day(c), 9, 0);
    let go = c.ahead(launch_day(c) - 1, 10, 0);
    let podcast = c.ahead(launch_day(c) + 2, 16, 0);

    d.threads.push(
        Thread::work("elena-intro", "Intro call next week?")
            .label("Work")
            .msg(from(ELENA, c.ago(3), "Hi Alex,\n\nLovely to meet you at the logistics meetup last week, and thanks again for showing us the new sync engine.\n\nWe’re evaluating tools for our dispatch team (about 60 people across Oslo and Rotterdam), and I’d love a 30-minute intro call next week to walk through our setup and see whether Fernhill is a fit.\n\nCould you send over a few times that work for you? I’m on CET and fairly flexible Tuesday to Thursday, ideally between 09:00 and 15:00.\n\nBest,\nElena\n\nElena Rossi\nHead of Operations, Quaypoint Logistics\nhttps://quaypoint.example").unread()),
    );

    let empty_state = files::mockup_picture();
    d.threads.push(
        Thread::work("onboarding-v3", "Onboarding v3: final screens")
            .label("Work")
            .label("Launch")
            .msg(from(MAYA, c.day(1, 16, 20), "Hi Alex,\n\nHere are the final screens for onboarding v3. Main changes since Tuesday:\n\n• The welcome screen is down to one sentence and a single button\n• Import moved to step two, with a clear way to skip it\n• The empty inbox suggests three keyboard shortcuts instead of a tour\n\nFull file: https://quillboard.test/d/onboarding-v3\n\nCould you take a look before tomorrow’s review? I’d like to hand off to Tomás by Friday.\n\nMaya")
                .part(Part::file("onboarding-v3-welcome.png", "image/png", empty_state.clone())))
            .msg(me(c.ago(95), "These look great. The single button on the welcome screen is exactly right.\n\nOne question: should the skip link on step two say “Skip for now” rather than “Skip”? People worry they can’t come back to it.\n\nAlex").to(&[MAYA]))
            .msg(from(MAYA, c.ago(9), "Good call, changed to “Skip for now”. The updated empty state is below, with the shortcuts hint.\n\nHanding off to Tomás this afternoon unless you shout.\n\nMaya")
                .html("<p>Good call, changed to “Skip for now”. The updated empty state is below, with the shortcuts hint.</p><p><img src=\"cid:empty-state@fernhill.example\" alt=\"Updated empty state\" width=\"560\" style=\"border:1px solid #e3ded5;border-radius:8px\"></p><p>Handing off to Tomás this afternoon unless you shout.</p><p>Maya</p>")
                .part(Part::inline("empty-state.png", "empty-state@fernhill.example", empty_state))
                .unread()),
    );

    let checklist = format!(
        "Hi all,\n\nWe’re go for launching Fernhill 2.0 on {}. Here’s the plan. Reply here with anything I’ve missed.\n\n08:00  Final smoke test on production (Jonas)\n09:00  Release goes live, status page updated (Jonas, Lucas)\n09:30  Announcement post and newsletter (Priya)\n10:00  Community launch post and social (Alex)\n10:00–18:00  Support on rotation, two-hour shifts (Sam)\n\nThe go/no-go meeting is {} at 10:00. Invite coming.\n\nChecklist: https://quillboard.test/d/launch-checklist\n\nIngrid",
        c.long_date(launch),
        c.long_date(go)
    );
    d.threads.push(
        Thread::work("launch-plan", "Launch plan: Fernhill 2.0")
            .label("Work")
            .label("Launch")
            .pinned()
            .msg(from(INGRID, c.day(2, 8, 40), checklist).to(&[TEAM]))
            .msg(from(PRIYA, c.day(2, 10, 5), "The announcement post is in review (draft 4). The newsletter goes to 18,400 subscribers and is scheduled for 09:30 sharp.\n\nAlex, can you own the community launch post? I’d keep it plain: what it is, why we built it, what’s next.\n\nPriya").to(&[TEAM]))
            .msg(me(c.day(2, 11, 12), "Yes, I’ll take the launch post. I’ll share a draft by tomorrow noon.\n\nI also suggest we keep the first two hours free of meetings for everyone on the launch rota.\n\nAlex").to(&[TEAM]))
            .msg(from(JONAS, c.day(1, 9, 47), "Release candidate 3 is on staging: https://staging.fernhill.example\n\nAll migrations ran cleanly against a copy of production. Two known issues, neither blocking:\n– Large attachment previews can take about two seconds on first open\n– The importer shows the wrong count for archived folders (fixed in rc.4)\n\nJonas").to(&[TEAM]))
            .msg(from(SAM, c.ago(48), "The support rota is in the checklist. I’ve drafted replies for the five questions we expect most: pricing, import, data export, mobile and keyboard shortcuts.\n\nWe’ll have two people on from 09:00 to 13:00, when traffic peaks.\n\nSam").to(&[TEAM]).unread())
            .msg(from(INGRID, c.ago(34), format!("Great work, everyone. Unless something changes at the go/no-go, we ship on {}. 🚀\n\nIngrid", c.weekday_name(launch))).to(&[TEAM]).unread()),
    );

    d.threads.push(
        Thread::work("investor-update", "November investor update")
            .label("Work")
            .msg(me(c.day(3, 7, 55), format!("Hi Daniel,\n\nHere’s our November update.\n\nHighlights\n• Fernhill 2.0 launches on {}\n• 412 teams on the beta, up from 260 in October\n• Weekly active users up 38% month on month\n• Net revenue retention at 118%\n\nLowlights\n• Hiring for the senior iOS role is slower than planned\n• Support load doubled after the beta opened; we’re adding saved replies and a second rota\n\nAsks\n• Introductions to operations leaders at logistics companies in the Nordics and Benelux\n\nThanks for the continued support.\n\nAlex", c.long_date(launch))).to(&[DANIEL]))
            .msg(from(DANIEL, c.ago(96), "Thanks Alex, strong month. The retention number stands out.\n\nHappy to make two intros: one to the COO of a freight forwarder in Rotterdam, one to a Bergen-based shipping software team. I’ll send separate emails today.\n\nGood luck with the launch!\n\nDaniel\n\nDaniel Park · Partner, Kittiwake Capital").unread()),
    );

    d.threads.push(
        Thread::work("pricing-copy", "Pricing page copy review")
            .label("Work")
            .label("Launch")
            .msg(from(PRIYA, c.day(1, 13, 30), "Hi Alex, Maya,\n\nThe new pricing page is on staging: https://staging.fernhill.example/pricing\n\nThree tiers: Personal (free), Team (€8 per user per month) and Business (€14). I’ve kept the copy short. The main open question: do we lead with “Fast, private email for teams” or “Email that keeps up with you”?\n\nPriya").cc(&[MAYA]))
            .msg(from(MAYA, c.day(1, 14, 2), "I prefer “Fast, private email for teams”. It says what it is. The other line sounds like every product launch this year.\n\nMaya").to(&[PRIYA, ALEX]))
            .msg(me(c.day(1, 15, 40), "Agree with Maya. Two small things:\n– “Unlimited history” should say “Search across all your mail”\n– Drop the comparison table for now; it’s too long on a phone\n\nOtherwise ready from my side.\n\nAlex").to(&[PRIYA, MAYA]))
            .msg(from(PRIYA, c.ago(140), "Done and done. Shipping it with the release on launch day.\n\nPriya").to(&[ALEX, MAYA])),
    );

    d.threads.push(
        Thread::work("release-notes", "Release notes 2.0 – draft")
            .label("Work")
            .label("Launch")
            .msg(from(JONAS, c.day(3, 14, 10), "Draft release notes for 2.0: https://quillboard.test/d/release-notes-2-0\n\nHighlights I’d lead with:\n1. Search across every account in a few milliseconds\n2. Offline first: everything you’ve downloaded works without a connection\n3. Split inboxes\n4. Scheduled send and snooze\n\nCould you tighten the intro? I write like an engineer.\n\nJonas"))
            .msg(me(c.day(3, 16, 25), "Tightened. I moved offline first to the top; it’s the thing people notice in the first minute.\n\nAlex").to(&[JONAS]))
            .msg(from(JONAS, c.day(2, 9, 2), "Perfect, merged. Thanks!\n\nJonas")),
    );

    d.threads.push(
        Thread::work("sync-rfc", "RFC: conflict resolution in the sync engine")
            .label("Work")
            .msg(from(LUCAS, c.day(4, 10, 15), "Hi both,\n\nI’ve written up how we resolve conflicting edits when two devices change the same draft offline: https://forgeyard.test/fernhill/sync-engine/discussions/88\n\nShort version: last writer wins per field, not per document, with a visible “Restored your earlier version” notice when we discard a change. It keeps merges predictable and never silently drops a whole draft.\n\nI’d love your eyes on the edge cases before Thursday.\n\nLucas").cc(&[JONAS]))
            .msg(from(JONAS, c.day(4, 13, 40), "Read it twice. Per-field merging makes sense. I’m worried about recipients: merging To lists from two devices could re-add someone who was deliberately removed. Can we treat recipients as one field?\n\nJonas").to(&[LUCAS, ALEX]))
            .msg(from(LUCAS, c.day(3, 9, 5), "Good catch. Recipients are now one field. The doc is updated.\n\nLucas").to(&[JONAS, ALEX]))
            .msg(me(c.day(3, 11, 30), "Approved from the product side. Let’s make sure the notice links to the version history so people can recover anything we discarded.\n\nAlex").to(&[LUCAS, JONAS])),
    );

    let pilot = files::invoice_pdf(
        "Brightwater",
        &["Customer Experience", "Pilot summary · Fernhill"],
        "Six-week pilot results",
        &[
            ("Team", "24 support agents".into()),
            ("Period", "Six weeks".into()),
        ],
        &[
            ("Conversations answered per agent per day", "+22%"),
            ("Median first response time", "3 h 40 min → 1 h 55 min"),
            ("Agents at inbox zero on a typical day", "2 → 9"),
            ("Satisfaction score", "4.1 → 4.6"),
        ],
        "Recommend rollout",
        "Prepared by Ada Lind for Fernhill. Fictional demo document.",
    );
    d.threads.push(
        Thread::work("brightwater-pilot", "Pilot results – Brightwater × Fernhill")
            .label("Work")
            .pinned()
            .msg(from(ADA, c.day(5, 9, 12), "Hi Alex,\n\nAttached is our summary of the six-week pilot. The headline: the support team answered 22% more conversations per day, and median first response time went from 3 h 40 min to 1 h 55 min.\n\nThe team especially liked split inboxes and the keyboard shortcuts. Two requests: shared drafts, and a way to see who else is reading a conversation.\n\nCan we talk about rolling out to the remaining 140 seats in January?\n\nBest,\nAda\n\nAda Lind · Head of Customer Experience, Brightwater")
                .part(Part::file("Brightwater-pilot-summary.pdf", "application/pdf", pilot)))
            .msg(me(c.day(5, 11, 3), "Ada, this is great to read. Thank you for the detailed write-up. Copying Ingrid.\n\nShared drafts are on our Q1 roadmap. I’ll come back with a proposal for the January rollout by the end of next week.\n\nAlex").to(&[ADA]).cc(&[INGRID]))
            .msg(from(INGRID, c.day(4, 8, 30), "Thanks Ada, and congratulations to your team. I’ll put together pricing for 160 seats with annual billing.\n\nIngrid").to(&[ADA, ALEX]))
            .msg(from(ADA, c.day(1, 17, 5), "Sounds good. One more data point: two people on the team said they now reach inbox zero most days, which hasn’t happened since 2019. 🙂\n\nAda").to(&[ALEX, INGRID])),
    );

    d.threads.push(
        Thread::work("podcast", "Would you join us on Harbour Light?")
            .label("Work")
            .pinned()
            .msg(from(MARCUS, c.day(1, 11, 20), format!("Hi Alex,\n\nI host Harbour Light, a weekly podcast about the people who build calm, useful software. I’ve used the Fernhill beta for a month and I’d love to have you on to talk about building an email client in 2026: offline first, privacy, and why you chose native apps.\n\nWe record remotely and it takes about 45 minutes. Would {} at 16:00 work? I’ve sent a calendar hold.\n\nMarcus Webb\nhttps://harbourlight.example", c.long_date(podcast))))
            .msg(me(c.day(1, 13, 45), format!("Hi Marcus, I’d be glad to. {} at 16:00 works, and I’ve accepted the invite.\n\nAlex", c.weekday_name(podcast))).to(&[MARCUS]))
            .msg(from(MARCUS, c.day(1, 14, 10), "Wonderful! Here’s a short prep doc with the topics: https://harbourlight.example/guests/prep\n\nMarcus")),
    );

    d.threads.push(
        Thread::work("noor-feedback", "Love the new search")
            .label("Work")
            .msg(from(NOOR, c.day(2, 20, 15), "Hi Fernhill team,\n\nJust wanted to say: the new search is ridiculous, in a good way. I typed the name of a supplier I last emailed in 2021 and the conversation was there before I finished typing.\n\nOne small wish: could search remember my last few queries?\n\nThanks for building this,\nNoor\n\nNoor Haddad · Ellipsis Studio")),
    );

    let csv = "ticket,opened,customer,topic,status\n4812,Mon 08:14,Lakeview Clinics,Import stuck at 92%,solved\n4813,Mon 09:02,Brightwater,Shared drafts request,feature request\n4815,Mon 11:40,Quaypoint Logistics,SSO setup,solved\n4817,Tue 07:55,Ellipsis Studio,Search history,feature request\n4818,Tue 10:21,Northgate Dental,Sync stalls on Preparing,investigating\n4819,Tue 13:05,Harbour Light,Invoice address,solved\n4821,Wed 08:47,Amberline Freight,Sync stalls on Preparing,investigating\n4822,Wed 09:30,Kittiwake Capital,Calendar invite timezone,solved\n4824,Wed 14:12,Moss & Pine,Keyboard shortcuts on Linux,solved\n4826,Thu 08:03,Lakeview Clinics,Sync stalls on Preparing,investigating\n";
    d.threads.push(
        Thread::work("support-digest", "Support digest: 14 new tickets, sync stalls trending")
            .label("Work")
            .label("Support")
            .msg(from(SAM, c.ago(230), "Morning!\n\nThis week so far: 14 new tickets, 9 solved, median first response 38 minutes.\n\nTrending: three customers report sync stalling on “Preparing” after importing large archives. Lucas is investigating; it looks like the same retry bug as rc.2.\n\nFull list attached as CSV.\n\nSam")
                .part(Part::file("support-tickets-this-week.csv", "text/csv", csv.as_bytes().to_vec()))),
    );

    let pentest = files::invoice_pdf(
        "Larkmoor Security",
        &["Application security assessments", "reports@larkmoor.test"],
        "Penetration test summary – Fernhill 2.0",
        &[
            ("Scope", "Desktop apps, sync API, website".into()),
            ("Testing window", "Ten working days".into()),
        ],
        &[
            ("Critical findings", "0"),
            ("High findings", "0"),
            ("Medium findings (fixed)", "2"),
            ("Low findings", "5"),
        ],
        "Pass",
        "Summary for Fernhill. Fictional demo document.",
    );
    d.threads.push(
        Thread::work("pentest", "Penetration test report – Fernhill 2.0")
            .label("Work")
            .msg(from(LARKMOOR, c.day(6, 15, 10), "Hello Alex,\n\nAttached is the summary of our assessment of Fernhill 2.0. No critical or high findings. The two medium findings (a missing rate limit on the password reset endpoint and verbose errors on the sync API) were fixed and verified during the test window.\n\nThe full report is available in your client portal: https://portal.larkmoor.test/reports/fernhill-2-0\n\nKind regards,\nLarkmoor Security")
                .part(Part::file("Larkmoor-pentest-summary.pdf", "application/pdf", pentest))),
    );

    d.threads.push(
        Thread::work("offsite", "Team offsite in Bergen – logistics")
            .label("Work")
            .label("Travel")
            .msg(from(HANNA, c.day(5, 10, 0), format!("Hi all,\n\nThe offsite is confirmed for {} and {}. Hotel Kaiholmen has rooms for all of us; confirmations go out separately.\n\nTrain: 08:03 from Oslo S, arriving in Bergen at 14:52 (tickets come from Skyrail Nordic)\nDinner: Thursday 19:00 at the hotel\nFriday: workshops 09:00–15:00, then a walk up the mountain if the weather allows\n\nPlease tell me about dietary requirements by Monday.\n\nHanna", c.long_date(c.ahead(c.days_until(3) + 7, 8, 0)), c.long_date(c.ahead(c.days_until(3) + 8, 8, 0)))).to(&[TEAM]))
            .msg(me(c.day(5, 10, 40), "Vegetarian for me, thanks Hanna!").to(&[HANNA]))
            .msg(from(TOMAS, c.day(5, 11, 15), "Could I take the later train? I have a dentist appointment on Thursday morning.\n\nTomás").to(&[HANNA, TEAM]))
            .msg(from(HANNA, c.day(4, 9, 20), "Of course, Tomás. I’ve booked you on the 11:58.\n\nHanna").to(&[TOMAS, TEAM])),
    );

    d.threads.push(
        Thread::work("hiring-ios", "Senior iOS engineer – final round")
            .label("Work")
            .msg(from(TOMAS, c.day(6, 13, 0), "Hi Alex, Ingrid,\n\nThree candidates for the final round. Short notes:\n\nR.K.: strongest on offline sync, great take-home. Wants to start in February.\nJ.M.: excellent UI craft, less backend experience. Available now.\nS.A.: solid all-round, led a small team before. Two months’ notice.\n\nI’d bring all three in for the product conversation with you. Can you each hold an hour next week?\n\nTomás").cc(&[INGRID]))
            .msg(from(INGRID, c.day(6, 15, 30), "Yes. I’ve put holds in for Tuesday afternoon.\n\nIngrid").to(&[TOMAS, ALEX]))
            .msg(me(c.day(5, 8, 5), "Same here. I’ll focus on how they make trade-offs between polish and shipping.\n\nAlex").to(&[TOMAS, INGRID])),
    );

    let quote = files::invoice_pdf(
        "Fernhill",
        &["Fernhill AS · Oslo", "sales@fernhill.example"],
        "Quote Q-2026-118 – Lakeview Clinics",
        &[
            ("Plan", "Business, annual billing".into()),
            ("Seats", "40".into()),
            ("Valid for", "30 days".into()),
        ],
        &[
            ("Business plan, 40 seats × 12 months", "€6,720.00"),
            ("Onboarding session for the front desk team", "Included"),
            ("Two-year commitment discount (10%)", "–€672.00"),
        ],
        "€6,048.00 per year",
        "Fictional demo document.",
    );
    d.threads.push(
        Thread::work("lakeview-quote", "Quote for 40 seats – Lakeview Clinics")
            .label("Work")
            .sent()
            .follow_up(c.ahead(1, 9, 0))
            .msg(me(c.day(4, 15, 30), "Hi Olivia,\n\nAs promised, here’s the quote for 40 seats on the Business plan with annual billing, with a 10% discount for a two-year commitment.\n\nIt includes single sign-on, priority support and an onboarding session for your front desk team.\n\nHappy to walk through it on a call.\n\nAlex").to(&[OLIVIA])
                .part(Part::file("Fernhill-quote-Q-2026-118.pdf", "application/pdf", quote))),
    );

    d.threads.push(
        Thread::work("brightwater-order", "Order form for countersignature")
            .label("Work")
            .sent()
            .follow_up(c.ahead(2, 9, 0))
            .msg(me(c.day(2, 16, 50), "Hello,\n\nPlease find the order form for the Brightwater rollout (160 seats, annual billing) in the signing portal: https://sign.fernhill.example/o/brightwater-2027\n\nLet me know if your team needs anything else from us.\n\nBest,\nAlex").to(&[BRIGHTWATER_LEGAL]).cc(&[ADA])),
    );

    d.threads.push(
        Thread::work("expenses", "Reminder: October expenses due Friday")
            .label("Work")
            .snoozed(c.ahead(1, 8, 0))
            .msg(from(HANNA, c.day(3, 9, 0), "Hi everyone,\n\nFriendly reminder to submit your October expenses by Friday so they’re included in this month’s payroll. Receipts go in the shared folder: https://quillboard.test/d/expenses-october\n\nThanks!\nHanna").to(&[TEAM])),
    );
    d.threads.push(
        Thread::work("q1-kickoff", "Q1 planning – pre-read")
            .label("Work")
            .snoozed(c.ahead(c.days_until(0), 8, 30))
            .msg(from(INGRID, c.day(5, 17, 0), "Hi Alex,\n\nAhead of Q1 planning, here’s the pre-read: https://quillboard.test/d/q1-planning\n\nNo need to do anything before the launch. Have a look the week after.\n\nIngrid")),
    );
    d.threads.push(
        Thread::work("domain-renewal", "fernhill.example renews in 30 days")
            .snoozed(c.ahead(c.days_until(0), 9, 0))
            .msg(from(REGISTRAR, c.day(6, 6, 0), "Hello,\n\nYour domain fernhill.example is set to renew automatically in 30 days for €14.00 per year.\n\nNo action is needed if your payment details are up to date. Manage renewals: https://namecove.test/account/domains\n\nNamecove")),
    );
    d.threads.push(
        Thread::work("go-no-go-prep", "Launch go/no-go: status per area")
            .label("Work")
            .label("Launch")
            .msg(from(JONAS, c.ago(410), format!("Morning! Status ahead of {}:\n\nRelease: rc.4 is on staging, all green\nMigrations: rehearsed twice, 4 minutes each\nRollback: tested, 6 minutes\nMonitoring: new dashboards for sync and search latency\n\nI’m confident. See you at 10.\n\nJonas", c.weekday_name(go)))),
    );
    let _ = podcast;
}

fn invite(
    c: &Clock,
    key: &str,
    organizer: P,
    title: &str,
    start: i64,
    minutes: i64,
    location: &str,
    description: &str,
    guests: &[P],
    method: &str,
    rrule: Option<&str>,
) -> Part {
    let attendees: Vec<(&str, &str)> = guests.iter().map(|p| (p.0, p.1)).collect();
    let bytes = files::ics(
        &Invite {
            uid: &format!("{key}@calendar.fernhill.example"),
            method,
            summary: title,
            description,
            location,
            start_ms: start,
            end_ms: start + minutes * 60_000,
            organizer: (organizer.0, organizer.1),
            attendees: &attendees,
            rrule,
        },
        c.now,
    );
    Part::file(
        "invite.ics",
        &format!("text/calendar; method={method}"),
        bytes,
    )
}

fn invite_body(
    c: &Clock,
    organizer: P,
    title: &str,
    start: i64,
    minutes: i64,
    location: &str,
    guests: &[P],
    agenda: &str,
) -> String {
    let names: Vec<&str> = guests.iter().map(|p| p.0).collect();
    format!(
        "{} has invited you to {}\n\nWhen: {} · {} – {} ({})\nWhere: {}\nOrganizer: {}\nGuests: {}\n\n{}\n\nGoing? Yes · Maybe · No",
        organizer.0,
        title,
        c.long_date(start),
        c.time(start),
        c.time(start + minutes * 60_000),
        c.zone(),
        location,
        organizer.0,
        names.join(", "),
        agenda
    )
}

fn calendar(c: &Clock, d: &mut Demo) {
    let launch_in = launch_day(c);
    let go = c.ahead(launch_in - 1, 10, 0);
    let review = c.ahead(2, 14, 0);
    let podcast = c.ahead(launch_in + 2, 16, 0);
    let sync = c.ahead(c.days_until(0), 9, 30);
    let party = c.ahead(c.days_until(4) + 14, 19, 0);

    let guests = [ALEX, JONAS, PRIYA, SAM, MAYA];
    let agenda = "Agenda\n1. Release candidate status (Jonas)\n2. Support readiness (Sam)\n3. Announcement and press (Priya, Alex)\n4. Decision";
    let room = "Fjellet meeting room · https://meet.fernhill.example/go-no-go";
    d.threads.push(
        Thread::work(
            "invite-go-no-go",
            format!(
                "Invitation: Launch go/no-go @ {} {} – {} ({})",
                c.short_date(go),
                c.time(go),
                c.time(go + 30 * 60_000),
                c.zone()
            ),
        )
        .label("Work")
        .label("Launch")
        .msg(
            from(
                INGRID,
                c.ago(52),
                invite_body(c, INGRID, "Launch go/no-go", go, 30, room, &guests, agenda),
            )
            .part(invite(
                c,
                "go-no-go",
                INGRID,
                "Launch go/no-go",
                go,
                30,
                room,
                agenda,
                &guests,
                "REQUEST",
                None,
            ))
            .unread(),
        ),
    );
    let review_guests = [ALEX, TOMAS, PRIYA];
    let review_room = "Studio · https://meet.fernhill.example/design";
    let review_agenda = "We’ll walk through onboarding v3 end to end, then decide what ships in 2.0 and what waits for 2.1.";
    d.threads.push(
        Thread::work(
            "invite-design-review",
            format!(
                "Invitation: Design review: onboarding v3 @ {} {} – {} ({})",
                c.short_date(review),
                c.time(review),
                c.time(review + 60 * 60_000),
                c.zone()
            ),
        )
        .label("Work")
        .msg(
            from(
                MAYA,
                c.day(2, 15, 0),
                invite_body(
                    c,
                    MAYA,
                    "Design review: onboarding v3",
                    review,
                    60,
                    review_room,
                    &review_guests,
                    review_agenda,
                ),
            )
            .part(invite(
                c,
                "design-review",
                MAYA,
                "Design review: onboarding v3",
                review,
                60,
                review_room,
                review_agenda,
                &review_guests,
                "REQUEST",
                None,
            )),
        ),
    );
    let podcast_where = "Remote · https://harbourlight.example/studio/alex";
    let podcast_agenda = "Recording for Harbour Light. Topics: building an email client in 2026, offline first, privacy by default, native apps.";
    d.threads.push(
        Thread::work(
            "invite-podcast",
            format!(
                "Invitation: Harbour Light recording @ {} {} – {} ({})",
                c.short_date(podcast),
                c.time(podcast),
                c.time(podcast + 60 * 60_000),
                c.zone()
            ),
        )
        .label("Work")
        .msg(
            from(
                MARCUS,
                c.day(1, 11, 22),
                invite_body(
                    c,
                    MARCUS,
                    "Harbour Light recording",
                    podcast,
                    60,
                    podcast_where,
                    &[ALEX],
                    podcast_agenda,
                ),
            )
            .part(invite(
                c,
                "podcast",
                MARCUS,
                "Harbour Light recording",
                podcast,
                60,
                podcast_where,
                podcast_agenda,
                &[ALEX],
                "REQUEST",
                None,
            )),
        ),
    );
    let sync_room = "Fjellet meeting room · https://meet.fernhill.example/sync";
    let sync_agenda = "Weekly product sync. Updated: we now meet at 09:30 so the support stand-up can finish first.";
    let team = [ALEX, MAYA, JONAS, PRIYA, SAM, LUCAS, TOMAS];
    d.threads.push(
        Thread::work(
            "invite-weekly-sync",
            format!(
                "Updated invitation: Weekly product sync @ Weekly from {} to {} on Monday ({})",
                c.time(sync),
                c.time(sync + 30 * 60_000),
                c.zone()
            ),
        )
        .label("Work")
        .msg(
            from(
                INGRID,
                c.day(6, 9, 0),
                invite_body(
                    c,
                    INGRID,
                    "Weekly product sync",
                    sync,
                    30,
                    sync_room,
                    &team,
                    sync_agenda,
                ),
            )
            .part(invite(
                c,
                "weekly-sync",
                INGRID,
                "Weekly product sync",
                sync,
                30,
                sync_room,
                sync_agenda,
                &team,
                "REQUEST",
                Some("FREQ=WEEKLY;BYDAY=MO"),
            )),
        ),
    );
    let party_where = "Restaurant Kaikanten, Brygga 3";
    let party_text = format!(
        "Hei alle sammen!\n\nÅrets julebord blir {} {}. Vi starter med gløgg klokka 19, og middagen serveres 19.30. Partnere er hjertelig velkomne.\n\nSvar innen fredag, og gi beskjed om allergier eller andre hensyn.\n\nGod helg!\nHanna",
        c.norwegian_weekday(party),
        c.norwegian_date(party)
    );
    d.threads.push(
        Thread::work(
            "invite-julebord",
            format!(
                "Invitasjon: Julebord {} @ {} {} – 23:30 ({})",
                c.year(party),
                c.norwegian_date(party),
                c.time(party),
                c.zone()
            ),
        )
        .label("Work")
        .msg(
            from(HANNA, c.day(4, 14, 30), party_text.clone()).part(invite(
                c,
                "julebord",
                HANNA,
                &format!("Julebord {}", c.year(party)),
                party,
                270,
                party_where,
                &party_text,
                &team,
                "REQUEST",
                None,
            )),
        ),
    );
    let vendor = c.ahead(1, 13, 0);
    let vendor_agenda = "This meeting has been cancelled. We’ll pick it up again after the launch.";
    d.threads.push(
        Thread::work("invite-cancelled", format!("Cancelled event: Storage tiers walkthrough @ {} {} ({})", c.short_date(vendor), c.time(vendor), c.zone()))
            .label("Work")
            .archived()
            .msg(from(HANNA, c.day(3, 12, 0), format!("This event has been cancelled.\n\nStorage tiers walkthrough\nWhen: {} · {} ({})\n\n{}", c.long_date(vendor), c.time(vendor), c.zone(), vendor_agenda))
                .part(invite(c, "storage-tiers", HANNA, "Storage tiers walkthrough", vendor, 45, "https://meet.fernhill.example/vendor", vendor_agenda, &[ALEX, LUCAS], "CANCEL", None))),
    );
    // Replies to the go/no-go invite.
    for (index, (person, verb)) in [
        (JONAS, "Accepted"),
        (PRIYA, "Accepted"),
        (SAM, "Accepted"),
        (MAYA, "Tentatively accepted"),
    ]
    .iter()
    .enumerate()
    {
        d.threads.push(
            Thread::work(
                format!("invite-reply-{index}"),
                format!(
                    "{verb}: Launch go/no-go @ {} {} ({})",
                    c.short_date(go),
                    c.time(go),
                    c.zone()
                ),
            )
            .label("Work")
            .archived_if(index > 1)
            .msg(
                from(
                    *person,
                    c.ago(185 + index as i64 * 40),
                    format!(
                        "{} has {} this invitation.\n\nLaunch go/no-go\nWhen: {} · {} ({})",
                        person.0,
                        verb.to_lowercase(),
                        c.long_date(go),
                        c.time(go),
                        c.zone()
                    ),
                )
                .to(&[INGRID]),
            ),
        );
    }
}

fn personal(c: &Clock, d: &mut Demo) {
    d.threads.push(
        Thread::home("climbing", "Climbing Thursday?")
            .msg(from(ROWAN, c.day(2, 12, 30), "Bouldering on Thursday at 18:00? The new problems went up on Monday and apparently the purple one is evil.\n\nRowan"))
            .msg(me(c.day(2, 13, 2), "I’m in. Bringing chalk and low expectations.").to(&[ROWAN]))
            .msg(from(ROWAN, c.day(2, 13, 10), "Ha. See you there 🧗")),
    );
    d.threads.push(
        Thread::home("book-club", "Book club: December pick")
            .snoozed(c.ahead(c.days_until(5), 9, 0))
            .msg(from(SOFIA, c.day(2, 21, 0), "Hi book people!\n\nDecember’s pick is “The Lighthouse Accounts”. It’s short, so no excuses this time. We’ll meet at mine on the 18th at 19:00; I’ll make soup.\n\nVote for January’s book here: https://bookclub.example.org/vote\n\nSofia")),
    );
    d.threads.push(
        Thread::home("parcel", "Your parcel is out for delivery")
            .msg(from(PARCEL, c.ago(205), "Your parcel 7J4-228-901 from Paperleaf Books is out for delivery and will arrive today between 12:00 and 16:00.\n\nNot home? Choose a pickup point: https://parcelpoint.test/t/7J4228901\n\nParcelpoint")),
    );
}

fn norwegian(c: &Clock, d: &mut Demo) {
    let photos = [
        Part::photo(
            "hytta-utsikt.png",
            "utsikt@example.org",
            files::fjord_picture(),
        ),
        Part::photo(
            "nordlys.png",
            "nordlys@example.org",
            files::aurora_picture(),
        ),
    ];
    d.threads.push(
        Thread::home("hyttetur", "Hyttetur i helgen? 🏔️")
            .label("Family")
            .pinned()
            .msg(from(ERIK, c.day(1, 19, 30), "Hei!\n\nSkal vi ta helgen på hytta? Meldingen sier sol og minus fem, perfekt for en tur opp til toppen på lørdag. Jeg kan kjøre fra Oslo fredag klokka 16.\n\nSi fra om du blir med, så handler jeg inn til fredagskvelden. Taco er obligatorisk.\n\nErik"))
            .msg(me(c.day(1, 21, 5), "Ja, gjerne! Jeg blir med. Jeg tar med ved og pannekakerøre til lørdag morgen.\n\nAlex").to(&[ERIK]))
            .msg(from(ERIK, c.ago(160), "Herlig! Her er utsikten fra forrige helg, så du vet hva du har i vente. 😄\n\nVi ses fredag!\n\nErik")
                .html("<p>Herlig! Her er utsikten fra forrige helg, så du vet hva du har i vente. 😄</p><p><img src=\"cid:utsikt@example.org\" alt=\"Utsikten fra hytta\" width=\"480\" style=\"border-radius:8px\"></p><p><img src=\"cid:nordlys@example.org\" alt=\"Nordlys over fjellet\" width=\"480\" style=\"border-radius:8px\"></p><p>Vi ses fredag!</p><p>Erik</p>")
                .part(photos[0].clone())
                .part(photos[1].clone())
                .unread()),
    );
    d.threads.push(
        Thread::home("middag", "Middag på søndag?")
            .label("Family")
            .msg(from(MAMMA, c.day(1, 18, 2), "Hei vennen!\n\nPappa og jeg tenkte å lage fårikål på søndag. Har du lyst til å komme? Ta gjerne med Rowan også.\n\nKlem fra mamma"))
            .msg(me(c.day(1, 20, 10), "Hei mamma! Det høres nydelig ut. Jeg kommer, men Rowan er bortreist. Skal jeg ta med dessert?\n\nKlem").to(&[MAMMA]))
            .msg(from(MAMMA, c.ago(290), "Ja takk, ta med eplekake! Vi spiser klokka fem.\n\nKlem")),
    );
    let saturday = c.ahead(c.days_until(5), 10, 0);
    d.threads.push(
        Thread::home("dugnad", format!("Dugnad lørdag {}. – vi trenger deg!", c.day_of_month(saturday)))
            .snoozed(c.ahead(c.days_until(5), 8, 0))
            .msg(from(COOP, c.day(3, 10, 0), format!("Kjære beboere,\n\nHøstens dugnad blir lørdag {} klokka 10–14. Vi rydder løv, setter bort hagemøblene og vasker felles boder. Styret stiller med kaffe, boller og pølser.\n\nHar du ikke anledning, gi beskjed til styret. Dugnaden er også en fin anledning til å bli kjent med naboene!\n\nHilsen styret i Borettslaget Solsiden", c.norwegian_date(saturday)))),
    );
    let appointment = c.ahead(c.days_until(3), 9, 30);
    d.threads.push(
        Thread::home("tannlege", format!("Påminnelse: time hos tannlegen {} klokka 09.30", c.norwegian_weekday(appointment)))
            .msg(from(DENTIST, c.day(1, 8, 0), format!("Hei Alex,\n\nDette er en påminnelse om timen din hos tannlege Lunde {} {} klokka 09.30.\n\nTrenger du å flytte eller avbestille timen, må du gjøre det senest 24 timer før: https://tannhjornet.example/min-side\n\nVennlig hilsen\nTannlegene på Hjørnet", c.norwegian_weekday(appointment), c.norwegian_date(appointment)))),
    );
    d.threads.push(
        Thread::home("efaktura", "Ny eFaktura fra Fossekraft Strøm")
            .label("Receipts")
            .msg(from(BANK, c.day(2, 7, 5), format!("Du har mottatt en ny eFaktura.\n\nFra: Fossekraft Strøm\nBeløp: 1 284,50 kr\nForfallsdato: {}\n\nLogg inn i nettbanken for å se og godkjenne fakturaen: https://nettbank.fjellbybank.example\n\nVennlig hilsen\nFjellby Bank", c.norwegian_date(c.ahead(12, 0, 0))))),
    );
    let power = files::invoice_pdf(
        "Fossekraft Strøm",
        &[
            "Kundeservice: kunde@fossekraft.example",
            "Fictional demo invoice",
        ],
        "Faktura – strøm for forrige måned",
        &[
            ("Kundenummer", "40 118 552".into()),
            ("Forfallsdato", c.norwegian_date(c.ahead(12, 0, 0))),
        ],
        &[
            ("Strøm, 612 kWh", "861,40 kr"),
            ("Fastbeløp", "49,00 kr"),
            ("Nettleie", "374,10 kr"),
        ],
        "1 284,50 kr",
        "Fossekraft Strøm · Fictional demo invoice · Not a real company",
    );
    d.threads.push(
        Thread::home("strom", "Fakturaen din for forrige måned er klar")
            .label("Receipts")
            .msg(from(POWER, c.day(2, 7, 0), "Hei Alex,\n\nFakturaen din er klar. Du brukte 612 kWh forrige måned, 8 % mindre enn samme måned i fjor.\n\nBeløp: 1 284,50 kr\nFakturaen er sendt som eFaktura til nettbanken din.\n\nSe forbruket ditt: https://fossekraft.example/min-side\n\nVennlig hilsen\nFossekraft Strøm")
                .part(Part::file("Fossekraft-faktura.pdf", "application/pdf", power))),
    );
    d.threads.push(
        Thread::home("bibliotek", "Lånet ditt forfaller snart")
            .msg(from(LIBRARY, c.day(4, 9, 0), format!("Hei!\n\nDette lånet forfaller om tre dager:\n\n• «Stillheten etter stormen» – forfaller {}\n\nDu kan fornye lånet på https://byensbibliotek.example/mine-lan\n\nVennlig hilsen\nByens bibliotek", c.norwegian_date(c.ahead(3, 0, 0))))),
    );
    d.threads.push(
        Thread::home("klatring", "Medlemskapet ditt fornyes snart")
            .archived()
            .msg(from(CLIMB, c.day(5, 12, 0), "Hei Alex!\n\nMedlemskapet ditt fornyes automatisk den 1. neste måned. Prisen er uendret: 545 kr i måneden.\n\nVil du fryse eller endre medlemskapet? Gå til https://klatresenter.example/medlem\n\nVi sees på veggen!\nSentrum Klatresenter")),
    );
    let logo = files::roaster_logo();
    d.threads.push(
        Thread::home("kaffe-kvittering", "Kvittering for bestilling #10482")
            .label("Receipts")
            .msg(from(BEAN, c.day(1, 10, 12), "Takk for bestillingen!\n\nBestilling #10482\n2 × Høstblend, 250 g – 278,00 kr\n1 × Etiopia Guji, 250 g – 169,00 kr\nFrakt – 0,00 kr\nTotalt – 447,00 kr\n\nVi brenner kaffen på mandag og sender den samme dag.\n\nLille Bønne Kaffe")
                .html("<div style=\"font-family:Georgia,serif;color:#3c2618;max-width:520px\"><p><img src=\"cid:logo@lillebonne.example\" alt=\"Lille Bønne Kaffe\" width=\"180\" height=\"56\"></p><h2 style=\"font-weight:normal\">Takk for bestillingen!</h2><p>Bestilling <b>#10482</b></p><table style=\"width:100%;border-collapse:collapse\"><tr><td>2 × Høstblend, 250 g</td><td align=\"right\">278,00 kr</td></tr><tr><td>1 × Etiopia Guji, 250 g</td><td align=\"right\">169,00 kr</td></tr><tr><td>Frakt</td><td align=\"right\">0,00 kr</td></tr><tr><td style=\"border-top:1px solid #d9cbbb\"><b>Totalt</b></td><td align=\"right\" style=\"border-top:1px solid #d9cbbb\"><b>447,00 kr</b></td></tr></table><p>Vi brenner kaffen på mandag og sender den samme dag.</p><p style=\"color:#8a6a52\">Lille Bønne Kaffe</p></div>")
                .part(Part::inline("lille-bonne-logo.png", "logo@lillebonne.example", logo))),
    );
    let payslip = files::invoice_pdf(
        "Fernhill AS",
        &["Lønnsslipp", "Fictional demo document"],
        &format!("Lønnsslipp for {}", c.norwegian_month(c.now)),
        &[
            ("Ansatt", "Alex Morgan".into()),
            ("Utbetalt", c.norwegian_date(c.ahead(5, 0, 0))),
        ],
        &[
            ("Fastlønn", "62 500,00 kr"),
            ("Forskuddstrekk", "–19 640,00 kr"),
            ("Pensjon (2 %)", "–1 250,00 kr"),
        ],
        "41 610,00 kr",
        "Fernhill AS · Fictional demo document",
    );
    d.threads.push(
        Thread::work("lonnsslipp", format!("Lønnsslipp for {} er klar", c.norwegian_month(c.now)))
            .label("Receipts")
            .msg(from(PAYROLL, c.day(1, 6, 0), format!("Hei Alex,\n\nLønnsslippen din for {} er klar og ligger vedlagt. Lønnen utbetales {}.\n\nSpørsmål? Svar på denne e-posten.\n\nHilsen Hanna\nFernhill AS", c.norwegian_month(c.now), c.norwegian_date(c.ahead(5, 0, 0))))
                .part(Part::file("Lonnsslipp.pdf", "application/pdf", payslip))),
    );
    d.threads.push(
        Thread::work("styremote", "Kort oppsummering fra styremøtet")
            .label("Work")
            .msg(from(INGRID, c.day(3, 18, 10), "Hei Alex,\n\nKort oppsummert fra styremøtet i dag:\n\n• Styret er fornøyd med veksten og støtter lanseringsplanen.\n• Vi får klarsignal til å ansette to personer til i første kvartal (iOS og support).\n• Neste styremøte blir i januar. Jeg sender innkalling.\n\nTakk for god forberedelse. Presentasjonen din om produktplanen ble godt mottatt.\n\nIngrid")),
    );
    let vat = files::invoice_pdf(
        "Nygaard Regnskap AS",
        &[
            "Autorisert regnskapsførerselskap",
            "Fictional demo document",
        ],
        "MVA-melding – utkast",
        &[
            ("Selskap", "Fernhill AS".into()),
            ("Termin", "September–oktober".into()),
        ],
        &[
            ("Utgående merverdiavgift", "214 380 kr"),
            ("Inngående merverdiavgift", "–96 120 kr"),
        ],
        "118 260 kr å betale",
        "Utkast til gjennomgang · Fictional demo document",
    );
    d.threads.push(
        Thread::work("mva", "MVA-melding for september–oktober")
            .label("Work")
            .msg(from(KARI, c.day(6, 9, 30), "Hei Alex,\n\nVedlagt er utkast til MVA-meldingen for september–oktober. Utgående merverdiavgift er 214 380 kr og inngående 96 120 kr, så dere skal betale 118 260 kr.\n\nKan du se over og bekrefte innen fredag? Gi beskjed hvis det er kjøp jeg ikke har fått bilag for.\n\nVennlig hilsen\nKari Nygaard\nNygaard Regnskap AS")
                .part(Part::file("MVA-melding-utkast.pdf", "application/pdf", vat)))
            .msg(me(c.day(5, 8, 15), "Hei Kari,\n\nTakk, det ser bra ut. Det mangler bilag for to kjøp fra Cirrusbay i oktober. Jeg sender dem i dag.\n\nHilsen Alex").to(&[KARI]))
            .msg(from(KARI, c.day(5, 9, 40), "Supert, da sender jeg inn meldingen når bilagene er på plass. 👍\n\nKari")),
    );
}

fn travel(c: &Clock, d: &mut Demo) {
    let out = c.ahead(launch_day(c) + 8, 7, 25);
    let back = c.ahead(launch_day(c) + 10, 18, 40);
    let itinerary = files::invoice_pdf(
        "Nordlys Air",
        &["E-ticket and itinerary", "Booking reference FH7QX2"],
        "Your itinerary",
        &[
            ("Passenger", "Alex Morgan".into()),
            ("Booking reference", "FH7QX2".into()),
        ],
        &[
            (
                "Flight 412  Oslo (OSL) → London (LHR)",
                &format!("{} 07:25", c.iso_date(out)),
            ),
            (
                "Flight 419  London (LHR) → Oslo (OSL)",
                &format!("{} 18:40", c.iso_date(back)),
            ),
            ("Fare", "Light · 1 cabin bag"),
        ],
        "NOK 2,184",
        "Nordlys Air · Fictional demo itinerary",
    );
    d.threads.push(
        Thread::work("flight", "Your trip to London is confirmed – booking FH7QX2")
            .label("Travel")
            .pinned()
            .msg(from(NORDLYS, c.day(2, 18, 45), format!("Hi Alex,\n\nThank you for flying with Nordlys Air. Your booking is confirmed.\n\nBooking reference: FH7QX2\n\nOutbound · {}\nFlight 412  Oslo (OSL) 07:25 → London (LHR) 09:20\nSeat 7A · Light fare · 1 cabin bag\n\nReturn · {}\nFlight 419  London (LHR) 18:40 → Oslo (OSL) 21:35\nSeat 9C\n\nCheck-in opens 24 hours before departure: https://nordlys-air.test/checkin\n\nHave a good trip!\nNordlys Air", c.long_date(out), c.long_date(back)))
                .part(Part::file("Nordlys-Air-itinerary-FH7QX2.pdf", "application/pdf", itinerary))),
    );
    let arrive = c.ahead(c.days_until(3) + 7, 15, 0);
    d.threads.push(
        Thread::work("hotel", "Reservation confirmed: Hotel Kaiholmen, Bergen")
            .label("Travel")
            .msg(from(HOTEL, c.day(4, 16, 0), format!("Dear Alex Morgan,\n\nWe look forward to welcoming you.\n\nCheck-in: {} from 15:00\nCheck-out: {} until 11:00\nRoom: Superior double, fjord view\nRate: Fernhill corporate rate, breakfast included\nConfirmation number: 58213-B\n\nManage your booking: https://kaiholmen.example/booking/58213-B\n\nHotel Kaiholmen · Bergen", c.long_date(arrive), c.long_date(arrive + DAY)))),
    );
    let train = files::invoice_pdf(
        "Skyrail Nordic",
        &["Ticket · Oslo S → Bergen", "Show this ticket on board"],
        "Ticket 66-2041-7781",
        &[
            ("Passenger", "Alex Morgan".into()),
            ("Date", c.iso_date(arrive)),
        ],
        &[
            ("Oslo S 08:03 → Bergen 14:52", "Car 4, seat 37"),
            ("Fare", "Standard, flexible"),
        ],
        "NOK 949",
        "Skyrail Nordic · Fictional demo ticket",
    );
    d.threads.push(
        Thread::work("train", "Your tickets: Oslo S → Bergen")
            .label("Travel")
            .msg(from(SKYRAIL, c.day(4, 16, 30), format!("Hi Alex,\n\nHere is your ticket for {}.\n\nOslo S 08:03 → Bergen 14:52\nCar 4, seat 37 (window, quiet carriage)\n\nYour ticket is attached as a PDF. You can also show it in the app.\n\nSkyrail Nordic", c.long_date(arrive)))
                .part(Part::file("Skyrail-ticket-66-2041-7781.pdf", "application/pdf", train))),
    );
    let pass = files::invoice_pdf(
        "Oslo Product Days",
        &["Speaker pass", "speakers@productdays.example"],
        "Speaker pass – Alex Morgan",
        &[
            ("Talk", "Email, rebuilt: what we learned".into()),
            ("Stage", "Main hall".into()),
        ],
        &[("Speaker check-in", "08:15"), ("Your slot", "11:30–12:00")],
        "Access: all areas",
        "Oslo Product Days · Fictional demo document",
    );
    d.threads.push(
        Thread::work("conference", "You’re confirmed as a speaker at Oslo Product Days")
            .label("Work")
            .label("Travel")
            .pinned()
            .msg(from(PRODUCT_DAYS, c.day(6, 11, 0), "Hi Alex,\n\nWe’re delighted to confirm your talk, “Email, rebuilt: what we learned”, on the main stage at 11:30.\n\nYour speaker pass is attached. Please send us your slides by the Friday before, and a short bio and photo for the programme: https://productdays.example/speakers/portal\n\nSee you there!\nThe Oslo Product Days team")
                .part(Part::file("Speaker-pass-Alex-Morgan.pdf", "application/pdf", pass))),
    );
}

fn receipts(c: &Clock, d: &mut Demo) {
    // Monthly cloud invoices, newest first.
    let amounts = [
        "€1,284.60",
        "€1,190.25",
        "€1,102.80",
        "€987.45",
        "€941.10",
        "€902.35",
        "€876.90",
    ];
    for (k, amount) in amounts.iter().enumerate() {
        let at = if k == 0 {
            c.ago(70)
        } else {
            c.day(k as i64 * 30 + 1, 6, 12)
        };
        let month = c.month_name(at - 15 * DAY);
        let pdf = files::invoice_pdf(
            "Cirrusbay Cloud",
            &["Cirrusbay Cloud Services", "billing@cirrusbay.test"],
            &format!("Invoice CB-{}-{:04}", c.year(at), 4410 + 37 * (7 - k)),
            &[("Account", "Fernhill AS".into()), ("Period", month.into())],
            &[
                ("Compute", "62%"),
                ("Storage and backups", "24%"),
                ("Network egress", "14%"),
            ],
            amount,
            "Cirrusbay Cloud · Fictional demo invoice",
        );
        d.threads.push(
            Thread::work(format!("cirrusbay-{k}"), format!("Your Cirrusbay invoice for {month} is ready"))
                .label("Receipts")
                .archived_if(k > 3)
                .msg(from(CIRRUS, at, format!("Hello Fernhill AS,\n\nYour invoice for {month} is ready.\n\nAmount due: {amount}\nPayment method: card ending 4421, charged automatically on the 5th\n\nView usage and download past invoices: https://console.cirrusbay.test/billing\n\nCirrusbay Billing"))
                    .part(Part::file(&format!("Cirrusbay-invoice-{month}.pdf"), "application/pdf", pdf))),
        );
    }
    let receipts = [
        ("Pagewell Analytics", "€49.00", "Team plan, monthly"),
        ("Signfold", "€12.00", "Pro, 1 seat"),
        ("Copydeck", "€24.00", "Writer plan"),
        ("Pagewell Analytics", "€49.00", "Team plan, monthly"),
        ("Meetbox Rooms", "€30.00", "3 rooms"),
        ("Signfold", "€12.00", "Pro, 1 seat"),
        ("Copydeck", "€24.00", "Writer plan"),
        ("Statusboard", "€19.00", "Starter"),
        ("Meetbox Rooms", "€30.00", "3 rooms"),
    ];
    for (k, (vendor, amount, plan)) in receipts.iter().enumerate() {
        let at = c.day(1 + k as i64 * 5, 4 + (k as i64 % 3), 10 + k as i64 * 4);
        let number = 20_419 + k * 113;
        let mut msg = from(
            TALLY,
            at,
            format!(
                "Receipt from {vendor}\n\nAmount paid: {amount}\nPlan: {plan}\nPaid with: card ending 4421\nReceipt number: {number}\n\nNeed a VAT invoice for your records? Download it at https://tallyhouse.test/r/{number}/invoice\n\nQuestions about this charge? Contact {vendor} at https://tallyhouse.test/r/{number}\n\nTallyhouse processes payments for {vendor}."
            ),
        );
        if k % 3 == 0 {
            msg = msg.part(Part::file(
                &format!("Receipt-{number}.pdf"),
                "application/pdf",
                files::invoice_pdf(
                    vendor,
                    &["Receipt via Tallyhouse"],
                    &format!("Receipt {number}"),
                    &[("Billed to", "Fernhill AS".into())],
                    &[(plan, amount)],
                    amount,
                    "Fictional demo receipt",
                ),
            ));
        }
        d.threads.push(
            Thread::work(
                format!("receipt-{k}"),
                format!("Your receipt from {vendor} [#{number}]"),
            )
            .label("Receipts")
            .archived_if(k > 4)
            .msg(msg),
        );
    }
    d.threads.push(
        Thread::work("tally-renewal", "Your annual plan renews in 14 days")
            .label("Receipts")
            .snoozed(c.ahead(3, 9, 0))
            .msg(from(TALLY, c.day(4, 5, 30), "Hello,\n\nYour annual Copydeck plan renews in 14 days for €240.00.\n\nTo change or cancel, visit https://tallyhouse.test/subscriptions before the renewal date.\n\nTallyhouse")),
    );
}

pub(super) const REPOS: [&str; 4] = [
    "fernhill/app",
    "fernhill/sync-engine",
    "fernhill/website",
    "fernhill/design-system",
];
pub(super) const PULLS: [&str; 24] = [
    "Fix flaky sync test on CI",
    "Onboarding: “Skip for now” on step two",
    "Speed up the first attachment preview",
    "Importer: correct counts for archived folders",
    "Split inbox rules: support domain wildcards",
    "Snooze: remember the last custom time",
    "Reduce memory use during the initial sync",
    "Keyboard: G then A opens Archive",
    "New pricing page",
    "Docs: keyboard shortcut reference",
    "Design tokens: darker focus ring",
    "Offline queue: retry with backoff",
    "Search: rank exact subject matches first",
    "Composer: keep recipients when switching drafts",
    "Accessibility: label every toolbar button",
    "Status page: launch day copy",
    "Bump SQLite to 3.47",
    "Calendar: show the invitation’s time zone",
    "Fix crash when a label name is empty",
    "Dark mode: unread dot contrast",
    "Launch announcement post",
    "Export: include attachments in the archive",
    "Notifications: group by conversation",
    "Release 2.0.0-rc.4",
];
pub(super) const COMMENTS: [&str; 14] = [
    "Looks good to me. One nit: can we name the flag after what it does rather than when it runs?",
    "I ran this against the large test mailbox; memory peaks at 310 MB instead of 540 MB. 🎉",
    "Could you add a test for the empty-label case? Otherwise approved.",
    "This fixes the flake for me locally. 50 runs, all green.",
    "Small copy suggestion: “Search across all your mail” reads better than “Unlimited history”.",
    "Do we need a migration for existing snoozes, or is the default fine?",
    "Tested on Linux and macOS. Focus ring is visible in both themes now.",
    "Let’s land this after rc.4 so we don’t change behaviour right before launch.",
    "Approved. Thanks for splitting it into small commits, easy to review.",
    "The retry delay caps at 5 minutes, right? Worth a comment in the code.",
    "Screenshot attached in the description. The dark mode dot was #6B7280, now #9CA3AF.",
    "Merging. I’ll keep an eye on the error dashboard for the next hour.",
    "This also fixes the stuck “Preparing” state Sam reported in support.",
    "Nice. Can we backport this to the 1.9 branch for existing customers?",
];

fn notifications(c: &Clock, d: &mut Demo) {
    let mut rng = Rng(0x2026_1112);
    let people = [JONAS, LUCAS, TOMAS, MAYA, PRIYA, SAM];
    for i in 0..66usize {
        let days = (i as i64 * 41) / 66;
        let at = if days == 0 {
            c.ago(26 + i as i64 * 41)
        } else {
            c.day(days, 7 + rng.below(11) as i64, rng.below(60) as i64)
        };
        let repo = REPOS[if i % 5 == 0 { 2 } else { rng.below(2) }];
        let title = *rng.pick(&PULLS);
        let number = 380 + (66 - i) * 2 + rng.below(2);
        let person = *rng.pick(&people);
        let kind = rng.below(10);
        let (subject, body, sender) = match kind {
            0 | 1 => (
                format!("[{repo}] {title} (PR #{number})"),
                format!(
                    "{} requested your review on this pull request.\n\n{title}\n\n{} files changed · +{} −{}\n\nView it on Forgeyard: https://forgeyard.test/{repo}/pull/{number}\n\n—\nYou are receiving this because your review was requested.",
                    person.0,
                    2 + rng.below(9),
                    12 + rng.below(240),
                    rng.below(120)
                ),
                person.0,
            ),
            2 | 3 | 4 => (
                format!("Re: [{repo}] {title} (PR #{number})"),
                format!(
                    "{}\n\n—\nReply to this email directly or view it on Forgeyard: https://forgeyard.test/{repo}/pull/{number}#comment-{}",
                    rng.pick(&COMMENTS),
                    90_000 + i * 31
                ),
                person.0,
            ),
            5 | 6 => (
                format!("Re: [{repo}] {title} (PR #{number})"),
                format!(
                    "Merged #{number} into main.\n\n—\nView it on Forgeyard: https://forgeyard.test/{repo}/pull/{number}"
                ),
                person.0,
            ),
            7 => {
                let sha = format!("{:07x}", rng.next() & 0xfff_ffff);
                (
                    format!("[{repo}] Run failed: tests – main ({sha})"),
                    format!(
                        "Run failed for tests on main.\n\nJob: integration (macos, linux)\nCommit: {sha} {title}\nDuration: {}m {}s\n\nView the run: https://forgeyard.test/{repo}/actions/runs/{}",
                        3 + rng.below(6),
                        rng.below(60),
                        7_100_000 + i * 97
                    ),
                    CI.0,
                )
            }
            8 => {
                let sha = format!("{:07x}", rng.next() & 0xfff_ffff);
                (
                    format!("[{repo}] Run fixed: tests – main ({sha})"),
                    format!(
                        "All jobs passed for tests on main.\n\nCommit: {sha} {title}\n\nView the run: https://forgeyard.test/{repo}/actions/runs/{}",
                        7_100_000 + i * 97
                    ),
                    CI.0,
                )
            }
            _ => (
                format!("[{repo}] {title} (Issue #{number})"),
                format!(
                    "{} opened an issue.\n\n{}\n\nSteps to reproduce are in the issue: https://forgeyard.test/{repo}/issues/{number}",
                    person.0,
                    rng.pick(&COMMENTS)
                ),
                person.0,
            ),
        };
        let email = if sender == CI.0 { CI.1 } else { FORGE };
        d.threads.push(
            Thread::work(format!("forge-{i}"), subject)
                .label("Notifications")
                .archived_if(days > 2)
                .msg(from_named(sender, email, at, body).read_if(days > 0 && i % 3 != 0)),
        );
    }
    // Uptime monitoring.
    let checks = [
        "api.fernhill.example",
        "sync.fernhill.example",
        "fernhill.example",
        "status.fernhill.example",
    ];
    for i in 0..14usize {
        let days = 1 + (i as i64 * 29) / 14;
        let at = c.day(days, rng.below(24) as i64, rng.below(60) as i64);
        let host = checks[i % checks.len()];
        let (subject, body) = match i % 4 {
            0 => (format!("Down: {host} is not responding"), format!("{host} has been unreachable for 2 minutes.\n\nCheck: HTTPS, every 60 seconds from Oslo, Frankfurt and Dublin\nError: connection timed out\n\nView incident: https://pingwell.test/i/{}", 55_100 + i)),
            1 => (format!("Resolved: {host} is back up"), format!("{host} is responding again after an outage of {} minutes.\n\nView incident: https://pingwell.test/i/{}", 3 + rng.below(9), 55_100 + i - 1)),
            2 => (format!("Slow: {host} p95 latency above 800 ms"), format!("The 95th percentile response time for {host} was {} ms over the last 10 minutes.\n\nView check: https://pingwell.test/c/{host}", 820 + rng.below(400))),
            _ => ("Weekly uptime report: 99.98%".to_string(), "Uptime across 4 checks last week: 99.98%\n\napi.fernhill.example  99.97%\nsync.fernhill.example  99.99%\nfernhill.example  100.00%\nstatus.fernhill.example  100.00%\n\nFull report: https://pingwell.test/reports/weekly".to_string()),
        };
        d.threads.push(
            Thread::work(format!("pingwell-{i}"), subject)
                .label("Notifications")
                .archived_if(days > 2)
                .msg(from(PINGWELL, at, body)),
        );
    }
    d.threads.push(
        Thread::work("pingwell-latest", "Resolved: api.fernhill.example p95 latency back to normal")
            .label("Notifications")
            .msg(from(PINGWELL, c.ago(300), "The 95th percentile response time for api.fernhill.example is back under 300 ms.\n\nThe slowdown lasted 14 minutes.\n\nView check: https://pingwell.test/c/api.fernhill.example")),
    );
    // Document comments.
    let docs = [
        ("Launch announcement – draft 4", "launch-announcement"),
        ("Release notes 2.0", "release-notes-2-0"),
        ("Launch checklist", "launch-checklist"),
        ("Onboarding v3", "onboarding-v3"),
        ("Q1 planning", "q1-planning"),
    ];
    let remarks = [
        "Love this opening line. Can we cut the second paragraph?",
        "Should we mention the Linux app here too?",
        "I’ve added the support rota under launch day.",
        "Numbers updated with this morning’s beta stats.",
        "Can we say “in milliseconds” instead of quoting a number?",
        "Resolved: kept the original headline.",
        "Added two customer quotes from the pilot.",
    ];
    for i in 0..12usize {
        let days = (i as i64 * 19) / 12;
        let at = if days == 0 {
            c.ago(130 + i as i64 * 40)
        } else {
            c.day(days, 8 + rng.below(9) as i64, rng.below(60) as i64)
        };
        let person = *rng.pick(&people);
        let (doc, slug) = *rng.pick(&docs);
        d.threads.push(
            Thread::work(format!("quill-{i}"), format!("{} commented on “{doc}”", person.0))
                .label("Notifications")
                .archived_if(days > 2)
                .msg(from_named(&format!("{} (Quillboard)", person.0), QUILL, at, format!("“{}”\n\nOpen the document: https://quillboard.test/d/{slug}\n\nYou’re receiving this because you’re following “{doc}”.", rng.pick(&remarks))).read_if(days > 0)),
        );
    }
}

struct Section {
    title: &'static str,
    text: &'static str,
}

fn newsletter_html(
    sender: P,
    accent: &str,
    host: &str,
    slug: &str,
    headline: &str,
    intro: &str,
    sections: &[Section],
    images: bool,
) -> String {
    let image = |name: &str, alt: &str, height: u32| {
        if images {
            format!(
                "<img src=\"https://img.{host}/{slug}/{name}.jpg\" width=\"544\" height=\"{height}\" alt=\"{alt}\" style=\"display:block;width:100%;height:auto;border-radius:8px;background:{accent}\">"
            )
        } else {
            String::new()
        }
    };
    let mut html = format!(
        "<div style=\"background:#f4f1ec;padding:24px 12px;font-family:-apple-system,'Helvetica Neue',Arial,sans-serif;color:#2a2d33\"><div style=\"max-width:600px;margin:0 auto;background:#ffffff;border-radius:12px;padding:28px\"><p style=\"font-size:12px;color:#7a7d85;margin:0 0 18px\">{} · <a href=\"https://{host}/issues/{slug}\" style=\"color:#7a7d85\">View in browser</a></p>{}<h1 style=\"font-size:26px;line-height:1.25;margin:22px 0 10px\">{headline}</h1><p style=\"font-size:16px;line-height:1.6\">{intro}</p>",
        sender.0,
        image("header", headline, 240)
    );
    for (index, section) in sections.iter().enumerate() {
        html.push_str(&format!(
            "<hr style=\"border:0;border-top:1px solid #ece7df;margin:26px 0\">{}<h2 style=\"font-size:19px;margin:16px 0 6px\">{}</h2><p style=\"font-size:15px;line-height:1.6\">{}</p><p><a href=\"https://{host}/{slug}/{}\" style=\"color:{accent};font-weight:600;text-decoration:none\">Read more →</a></p>",
            if index == 0 { image("feature", section.title, 200) } else { String::new() },
            section.title,
            section.text,
            index + 1
        ));
    }
    html.push_str(&format!(
        "<p style=\"font-size:12px;color:#9a9ca3;margin-top:30px\">You’re receiving this because you subscribed at {host}. <a href=\"https://{host}/unsubscribe\" style=\"color:#9a9ca3\">Unsubscribe</a></p></div></div>"
    ));
    html
}

fn newsletter_plain(
    headline: &str,
    intro: &str,
    sections: &[Section],
    host: &str,
    slug: &str,
) -> String {
    let mut text = format!("{headline}\n\n{intro}\n");
    for (index, section) in sections.iter().enumerate() {
        text.push_str(&format!(
            "\n{}\n{}\nhttps://{host}/{slug}/{}\n",
            section.title,
            section.text,
            index + 1
        ));
    }
    text.push_str(&format!(
        "\nView in browser: https://{host}/issues/{slug}\nUnsubscribe: https://{host}/unsubscribe"
    ));
    text
}

#[allow(clippy::too_many_arguments)]
fn newsletter(
    d: &mut Demo,
    key: String,
    account: bool,
    sender: P,
    accent: &str,
    host: &str,
    at: i64,
    inbox: bool,
    read: bool,
    headline: &str,
    intro: &str,
    sections: &[Section],
    images: bool,
) {
    let slug = key.clone();
    let thread = if account {
        Thread::work(key, headline)
    } else {
        Thread::home(key, headline)
    };
    d.threads.push(
        thread.label("Newsletters").archived_if(!inbox).msg(
            from(
                sender,
                at,
                newsletter_plain(headline, intro, sections, host, &slug),
            )
            .html(newsletter_html(
                sender, accent, host, &slug, headline, intro, sections, images,
            ))
            .unsubscribe(format!(
                "<https://{host}/unsubscribe>, <mailto:unsubscribe@{host}>"
            ))
            .read_if(read),
        ),
    );
}

fn newsletters(c: &Clock, d: &mut Demo) {
    let quiet: [(&str, &str, [Section; 3]); 8] = [
        (
            "Issue 212: Designing for the second visit",
            "Most products are designed for the first five minutes. This week: what changes when you design for the hundredth.",
            [
                Section {
                    title: "The second visit",
                    text: "First-run polish fades. The details people meet every day, like focus states, undo and the speed of the next screen, decide whether a tool feels calm.",
                },
                Section {
                    title: "Empty states that teach",
                    text: "A good empty state suggests one useful action. Three keyboard shortcuts beat a ten-step tour.",
                },
                Section {
                    title: "From the mailbag",
                    text: "A reader asks how to show progress without a spinner. Our answer: show what is already done.",
                },
            ],
        ),
        (
            "Issue 211: Quiet notifications",
            "How to tell people something changed without stealing their attention.",
            [
                Section {
                    title: "Batch, don’t buzz",
                    text: "Grouping notifications by conversation halves the number of interruptions without hiding anything.",
                },
                Section {
                    title: "Badges with meaning",
                    text: "A count is only useful if people can act on it. Otherwise, use a dot.",
                },
                Section {
                    title: "Further reading",
                    text: "Three essays on designing for focus, and one on designing for interruption.",
                },
            ],
        ),
        (
            "Issue 210: Keyboard first, mouse friendly",
            "Shortcuts are an accessibility feature, a speed feature and a trust feature.",
            [
                Section {
                    title: "Discoverable shortcuts",
                    text: "Show the shortcut next to the action, every time. People learn by seeing.",
                },
                Section {
                    title: "Two-key sequences",
                    text: "G then I is easier to remember than Ctrl-Shift-1, and leaves room to grow.",
                },
                Section {
                    title: "Don’t trap focus",
                    text: "Every dialog needs an obvious way out, and Escape should always work.",
                },
            ],
        ),
        (
            "Issue 209: The case for fewer settings",
            "Every setting is a decision you asked someone else to make.",
            [
                Section {
                    title: "Good defaults",
                    text: "Pick the behaviour most people want and make it easy to change later.",
                },
                Section {
                    title: "Settings as escape hatches",
                    text: "Keep the rare options, but put them where experts look.",
                },
                Section {
                    title: "A small audit",
                    text: "We removed eleven settings from a real product. Nobody asked for them back.",
                },
            ],
        ),
        (
            "Issue 208: Writing interface copy",
            "Plain words, short sentences and the same term for the same thing.",
            [
                Section {
                    title: "Name the action",
                    text: "“Archive” beats “Done”. Buttons should say what happens.",
                },
                Section {
                    title: "Errors that help",
                    text: "Say what happened, why, and what to do next, in that order.",
                },
                Section {
                    title: "One voice",
                    text: "A product with many writers needs one short style guide.",
                },
            ],
        ),
        (
            "Issue 207: Speed is a feature",
            "Fast software feels trustworthy. Here’s how teams measure it.",
            [
                Section {
                    title: "Perceived speed",
                    text: "Respond within 100 ms, even if the real work takes longer.",
                },
                Section {
                    title: "Budget your frames",
                    text: "Treat animation time like any other budget.",
                },
                Section {
                    title: "Offline first",
                    text: "The fastest network request is the one you never make.",
                },
            ],
        ),
        (
            "Issue 206: Colour with care",
            "Contrast, meaning and restraint.",
            [
                Section {
                    title: "Meaningful colour",
                    text: "Use colour to mean something, then use it consistently.",
                },
                Section {
                    title: "Dark mode is a design",
                    text: "Not an inversion. Rebuild contrast for each theme.",
                },
                Section {
                    title: "Accessible palettes",
                    text: "Check every pair, not just text on background.",
                },
            ],
        ),
        (
            "Issue 205: Tools that respect attention",
            "Notes from a month of building calmer software.",
            [
                Section {
                    title: "Calm by default",
                    text: "No badge without a reason, no sound without consent.",
                },
                Section {
                    title: "Respect the inbox",
                    text: "Email you send should be worth reading.",
                },
                Section {
                    title: "What we’re reading",
                    text: "Five links about attention, focus and good tools.",
                },
            ],
        ),
    ];
    for (k, (headline, intro, sections)) in quiet.iter().enumerate() {
        let at = if k == 0 {
            c.ago(125)
        } else {
            c.day(k as i64 * 7, 7, 0)
        };
        newsletter(
            d,
            format!("quiet-{k}"),
            false,
            QUIET,
            "#c86b4a",
            "quietinterface.example",
            at,
            k < 2,
            k > 0,
            headline,
            intro,
            sections,
            true,
        );
    }
    let systems: [(&str, &str); 8] = [
        (
            "Systems Digest #148: SQLite at the edge, CRDTs in practice",
            "This week: embedding SQLite in desktop apps, a practical comparison of CRDT libraries, and why your retry loop needs jitter.",
        ),
        (
            "Systems Digest #147: Backpressure, explained",
            "How queues fall over, and three patterns to stop it.",
        ),
        (
            "Systems Digest #146: The cost of a millisecond",
            "A tour of latency budgets in interactive software.",
        ),
        (
            "Systems Digest #145: Full-text search without a server",
            "Inverted indexes, tokenisers and when FTS is enough.",
        ),
        (
            "Systems Digest #144: Sync engines",
            "Operational transforms, CRDTs and the boring alternative that usually wins.",
        ),
        (
            "Systems Digest #143: Postmortems worth reading",
            "Five incident write-ups and what they teach.",
        ),
        (
            "Systems Digest #142: Memory, measured",
            "Profiling allocators in long-running desktop apps.",
        ),
        (
            "Systems Digest #141: Testing distributed systems",
            "Deterministic simulation for the rest of us.",
        ),
    ];
    for (k, (headline, intro)) in systems.iter().enumerate() {
        let at = c.day(1 + k as i64 * 7, 6, 30);
        let sections = [
            Section {
                title: "Featured",
                text: "A long read on the trade-offs behind the week’s theme, with code and benchmarks.",
            },
            Section {
                title: "Links",
                text: "Seven more articles from engineering blogs, papers and mailing lists.",
            },
            Section {
                title: "Jobs",
                text: "Four teams hiring systems engineers this week.",
            },
        ];
        newsletter(
            d,
            format!("systems-{k}"),
            true,
            SYSTEMS,
            "#2f6f73",
            "systemsdigest.example",
            at,
            k < 1,
            k > 0,
            headline,
            intro,
            &sections,
            false,
        );
    }
    let founder: [(&str, &str); 4] = [
        (
            "Founder Notes: pricing your second product",
            "When to bundle, when to split, and how to explain the difference.",
        ),
        (
            "Founder Notes: the launch after the launch",
            "What to do in the four weeks after a big announcement.",
        ),
        (
            "Founder Notes: hiring your first support lead",
            "The role that shapes how customers feel about you.",
        ),
        (
            "Founder Notes: board meetings that help",
            "A simple agenda that turns updates into decisions.",
        ),
    ];
    for (k, (headline, intro)) in founder.iter().enumerate() {
        let at = c.day(2 + k as i64 * 14, 8, 15);
        let sections = [
            Section {
                title: "This issue",
                text: "A short essay with a checklist you can use this week.",
            },
            Section {
                title: "From the community",
                text: "Three founders share what worked, and what they’d do differently.",
            },
            Section {
                title: "Office hours",
                text: "Reply to this email with a question and we may answer it in a future issue.",
            },
        ];
        newsletter(
            d,
            format!("founder-{k}"),
            true,
            FOUNDER,
            "#5b5bd6",
            "foundernotes.example",
            at,
            k < 1,
            k > 0,
            headline,
            intro,
            &sections,
            true,
        );
    }
    let harbour: [(&str, &str); 5] = [
        (
            "This week on Harbour Light: offline-first design",
            "Our guest built a field-service app that works with no signal. We talk sync, conflicts and trust.",
        ),
        (
            "This week on Harbour Light: small teams, big products",
            "How a team of six ships to a million people.",
        ),
        (
            "This week on Harbour Light: the craft of release notes",
            "Writing for the people who read them.",
        ),
        (
            "This week on Harbour Light: accessibility from day one",
            "Why it’s cheaper to start early.",
        ),
        (
            "This week on Harbour Light: open source sustainability",
            "Funding the tools we all depend on.",
        ),
    ];
    for (k, (headline, intro)) in harbour.iter().enumerate() {
        let at = c.day(1 + k as i64 * 7, 17, 0);
        let sections = [
            Section {
                title: "Listen now",
                text: "The full episode is about 50 minutes, with chapters and a transcript.",
            },
            Section {
                title: "Show notes",
                text: "Links to everything we mentioned, plus a reading list from our guest.",
            },
        ];
        newsletter(
            d,
            format!("harbour-{k}"),
            false,
            HARBOUR,
            "#1f5f8b",
            "harbourlight.example",
            at,
            k < 2,
            k > 0,
            headline,
            intro,
            &sections,
            true,
        );
    }
    let fjordbyte: [(&str, &str); 6] = [
        (
            "Fjordbyte Weekly: Nordic startups raise, ship and hire",
            "Funding rounds, launches and job moves from across the Nordics.",
        ),
        (
            "Fjordbyte Weekly: the Bergen software scene",
            "Ten companies worth following on the west coast.",
        ),
        (
            "Fjordbyte Weekly: climate tech in the north",
            "Batteries, grids and the software in between.",
        ),
        (
            "Fjordbyte Weekly: what founders read this month",
            "Our readers share their favourite books.",
        ),
        (
            "Fjordbyte Weekly: remote work, two years on",
            "How Nordic teams settled into hybrid.",
        ),
        (
            "Fjordbyte Weekly: AI tools in small teams",
            "Where they help, where they don’t.",
        ),
    ];
    for (k, (headline, intro)) in fjordbyte.iter().enumerate() {
        let at = c.day(3 + k as i64 * 7, 9, 0);
        let sections = [
            Section {
                title: "Top story",
                text: "Our pick of the week, with comments from the people involved.",
            },
            Section {
                title: "In brief",
                text: "Twelve short items from Oslo, Stockholm, Copenhagen, Helsinki and Reykjavík.",
            },
        ];
        newsletter(
            d,
            format!("fjordbyte-{k}"),
            true,
            FJORDBYTE,
            "#3d7a5a",
            "fjordbyte.example",
            at,
            k < 1,
            true,
            headline,
            intro,
            &sections,
            false,
        );
    }
}

fn promo_html(
    sender: P,
    accent: &str,
    host: &str,
    slug: &str,
    headline: &str,
    text: &str,
    button: &str,
) -> String {
    format!(
        "<div style=\"background:#f6f3ee;padding:24px 12px;font-family:-apple-system,'Helvetica Neue',Arial,sans-serif;color:#22252b\"><div style=\"max-width:600px;margin:0 auto;background:#ffffff;border-radius:12px;overflow:hidden\"><p style=\"text-align:center;font-size:20px;font-weight:700;letter-spacing:.04em;margin:22px 0 14px\">{}</p><img src=\"https://cdn.{host}/campaigns/{slug}/hero.jpg\" width=\"600\" height=\"320\" alt=\"{headline}\" style=\"display:block;width:100%;height:auto;background:{accent}\"><div style=\"padding:24px 28px;text-align:center\"><h1 style=\"font-size:26px;margin:0 0 10px\">{headline}</h1><p style=\"font-size:16px;line-height:1.55;color:#4a4d55\">{text}</p><p style=\"margin:22px 0\"><a href=\"https://{host}/{slug}\" style=\"background:{accent};color:#ffffff;padding:12px 22px;border-radius:999px;text-decoration:none;font-weight:600\">{button}</a></p></div><table width=\"100%\" style=\"border-collapse:collapse\"><tr><td style=\"padding:8px\"><img src=\"https://cdn.{host}/campaigns/{slug}/item-1.jpg\" width=\"280\" height=\"200\" alt=\"\" style=\"width:100%;height:auto;background:#e8e2d8;border-radius:8px\"></td><td style=\"padding:8px\"><img src=\"https://cdn.{host}/campaigns/{slug}/item-2.jpg\" width=\"280\" height=\"200\" alt=\"\" style=\"width:100%;height:auto;background:#e8e2d8;border-radius:8px\"></td></tr></table><p style=\"font-size:11px;color:#9a9ca3;text-align:center;padding:18px\">{} · <a href=\"https://{host}/unsubscribe\" style=\"color:#9a9ca3\">Unsubscribe</a></p></div></div>",
        sender.0, sender.0
    )
}

fn promotions(c: &Clock, d: &mut Demo) {
    let campaigns: [(P, &str, &str, &[(&str, &str, &str)]); 4] = [
        (
            FOREST,
            "#3f6b4f",
            "fjordforest.example",
            &[
                (
                    "Last chance: 30% off insulated jackets",
                    "Our warmest jackets, now 30% off until midnight. Free returns, as always.",
                    "Shop jackets",
                ),
                (
                    "New in: wool base layers for winter",
                    "Soft, warm and made to last. Merino base layers in six colours.",
                    "See what’s new",
                ),
                (
                    "Your guide to winter hiking",
                    "Layering, footwear and the three things we always pack.",
                    "Read the guide",
                ),
                (
                    "Members get early access this weekend",
                    "The winter sale starts Saturday for members, Monday for everyone else.",
                    "Become a member",
                ),
                (
                    "Free repairs for your old gear",
                    "Bring any of our jackets to a store and we’ll fix it for free.",
                    "Find a store",
                ),
                (
                    "Gift ideas under 500 kr",
                    "Hats, socks, mugs and headlamps for everyone on your list.",
                    "Shop gifts",
                ),
                (
                    "Back in stock: the Ridge 35 backpack",
                    "Our best-selling daypack is back in all three colours.",
                    "Shop backpacks",
                ),
                (
                    "Autumn sale: up to 40% off",
                    "Last season’s colours, same quality.",
                    "Shop the sale",
                ),
            ],
        ),
        (
            PAPERLEAF,
            "#8a5a44",
            "paperleaf.example",
            &[
                (
                    "Books we loved this month",
                    "Ten new novels, three memoirs and one very good cookbook.",
                    "Browse the list",
                ),
                (
                    "Your order has shipped",
                    "Good news: your books are on their way.",
                    "Track your order",
                ),
                (
                    "Signed copies, while they last",
                    "Local authors visited the shop and signed a few hundred books.",
                    "Find signed copies",
                ),
                (
                    "Reading for long evenings",
                    "Our staff picks for the darkest months of the year.",
                    "See staff picks",
                ),
                (
                    "15% off for book clubs",
                    "Order five or more copies of the same book and save 15%.",
                    "Learn more",
                ),
                (
                    "Winter catalogue is here",
                    "Two hundred pages of books, gifts and good intentions.",
                    "Read online",
                ),
            ],
        ),
        (
            BEAN,
            "#6f482d",
            "lillebonne.example",
            &[
                (
                    "Ny sending: Høstblend er tilbake",
                    "Søt, rund og perfekt til mørke morgener. Brent mandag, sendt samme dag.",
                    "Bestill nå",
                ),
                (
                    "Abonnement: få kaffe hver fjerde uke",
                    "Ferskbrent kaffe i postkassen, og 10 % rabatt på hver pose.",
                    "Start abonnement",
                ),
                (
                    "Kaffekurs i desember",
                    "Lær å brygge bedre kaffe hjemme. Tre kvelder, små grupper.",
                    "Meld deg på",
                ),
                (
                    "Julekaffen er her",
                    "Krydret, mørk og med et hint av appelsin.",
                    "Smak julekaffen",
                ),
                (
                    "Gratis frakt hele helgen",
                    "På alle bestillinger over 300 kr.",
                    "Handle nå",
                ),
            ],
        ),
        (
            TIDY,
            "#4a5a8a",
            "tidydesk.example",
            &[
                (
                    "Black Week: 25% off desks and lamps",
                    "Standing desks, task lamps and cable trays at their lowest prices this year.",
                    "Shop the sale",
                ),
                (
                    "A calmer desk in five minutes",
                    "Our guide to cables, notebooks and the one drawer that solves everything.",
                    "Read the guide",
                ),
                (
                    "New: the walnut monitor stand",
                    "Solid walnut, hand-finished, with room for a keyboard underneath.",
                    "Take a look",
                ),
                (
                    "Your cart is waiting",
                    "You left a desk lamp in your cart. It’s still there.",
                    "Return to cart",
                ),
                (
                    "Free shipping on everything",
                    "This week only, no minimum order.",
                    "Start shopping",
                ),
            ],
        ),
    ];
    let mut slot = 0i64;
    for (sender, accent, host, items) in campaigns {
        for (k, (headline, text, button)) in items.iter().enumerate() {
            let days = 1 + (slot * 5 + k as i64 * 3) % 34;
            slot += 1;
            let slug = format!("{}-{k}", host.split('.').next().unwrap_or("promo"));
            let at = if days <= 1 && k == 0 {
                c.ago(260 + slot * 11)
            } else {
                c.day(days, 9 + (slot % 9), (slot * 13) % 60)
            };
            d.threads.push(
                Thread::home(format!("promo-{slug}"), *headline)
                    .label("Promotions")
                    .archived_if(days > 6)
                    .msg(from(sender, at, format!("{headline}\n\n{text}\n\n{button}: https://{host}/{slug}\n\nUnsubscribe: https://{host}/unsubscribe"))
                        .html(promo_html(sender, accent, host, &slug, headline, text, button))
                        .unsubscribe(format!("<https://{host}/unsubscribe>"))
                        .read_if(days > 2)),
            );
        }
    }
}

fn support(c: &Clock, d: &mut Demo) {
    let tickets: [(&str, &str, &str, &str); 16] = [
        (
            "Grace Whitfield",
            "Northgate Dental",
            "Sync stalls on “Preparing”",
            "Since yesterday the desktop app shows “Preparing” and never finishes. We imported about 40,000 messages from our old system last week.",
        ),
        (
            "Henrik Dahl",
            "Amberline Freight",
            "Sync stalls on “Preparing” after import",
            "Same as a colleague reported: stuck on Preparing. Restarting doesn’t help. Logs attached via the diagnostics button.",
        ),
        (
            "Mei Tanaka",
            "Moss & Pine",
            "Keyboard shortcuts on Linux",
            "Is there a way to see all keyboard shortcuts on Linux? The question mark key doesn’t seem to work with my layout.",
        ),
        (
            "Oscar Lindqvist",
            "Lakeview Clinics",
            "Import stuck at 92%",
            "Our import got to 92% and stopped. Is it still working in the background?",
        ),
        (
            "Fatima Noor",
            "Ellipsis Studio",
            "Search history",
            "Could search remember my recent queries? I search for the same three suppliers every day.",
        ),
        (
            "Liam O’Connor",
            "Harbour Light",
            "Invoice address",
            "Could you change the address on our invoices to our new office? Details below.",
        ),
        (
            "Ana Costa",
            "Quaypoint Logistics",
            "SSO setup",
            "We’re setting up single sign-on. Which redirect URL should we use?",
        ),
        (
            "Johan Berglund",
            "Brightwater",
            "Shared drafts",
            "Several people on my team asked whether two people can work on the same draft.",
        ),
        (
            "Sara Ahmed",
            "Kittiwake Capital",
            "Calendar invite shows the wrong time zone",
            "An invitation from a colleague in New York shows the time in UTC instead of my local time.",
        ),
        (
            "Tom Reilly",
            "Northgate Dental",
            "Exporting a mailbox",
            "How do I export one shared mailbox before we close it down?",
        ),
        (
            "Ingeborg Nilsen",
            "Fjordlys Arkitekter",
            "Fakturaspørsmål",
            "Hei! Vi ble fakturert for 12 brukere, men vi har bare 10. Kan dere se på det?",
        ),
        (
            "Pablo Ruiz",
            "Amberline Freight",
            "Snooze until a date",
            "Can I snooze a conversation until a specific date, not just “tomorrow”?",
        ),
        (
            "Chloe Martin",
            "Moss & Pine",
            "Dark mode contrast",
            "In dark mode the unread dot is hard to see on my monitor.",
        ),
        (
            "Nikolai Petrov",
            "Lakeview Clinics",
            "Attachments over 20 MB",
            "Is there a limit on attachment size when previewing PDFs?",
        ),
        (
            "Emma Hughes",
            "Brightwater",
            "Thank you!",
            "Just a quick note to say the new split inboxes have made Mondays much easier for our team.",
        ),
        (
            "Yusuf Kaya",
            "Quaypoint Logistics",
            "Mobile app timing",
            "Do you have a date for the mobile apps? Our drivers would love them.",
        ),
    ];
    let replies = [
        "Thanks for reporting this. We’ve reproduced it and a fix is going out in the next release. In the meantime, quitting and reopening the app resumes the sync.",
        "Good question! You can see every shortcut from the command palette: press Ctrl-K and type “shortcuts”.",
        "Thanks, we’ve passed this on to the product team. I’ll let you know when there’s news.",
        "Done. Your next invoice will show the new address.",
    ];
    for (k, (name, company, topic, text)) in tickets.iter().enumerate() {
        let days = (k as i64 * 13) / 16;
        let at = if days == 0 {
            c.ago(180 + k as i64 * 25)
        } else {
            c.day(days, 8 + (k as i64 % 8), (k as i64 * 17) % 60)
        };
        let number = 4810 + k * 2;
        let mut thread = Thread::work(format!("ticket-{number}"), format!("[#{number}] {topic}"))
            .label("Support")
            .archived_if(days > 3)
            .msg(from_named(&format!("{name} via Fernhill Support"), "support@fernhill.example", at, format!("{text}\n\n{name}\n{company}\n\n—\nTicket #{number} · Reply above this line to respond to the customer.")).read_if(days > 0 || k % 2 == 1));
        if k % 3 == 0 && days > 0 {
            thread = thread.msg(from_named(
                &format!("{} via Fernhill Support", SAM.0),
                "support@fernhill.example",
                at + 47 * 60_000,
                format!(
                    "Hi {},\n\n{}\n\nBest,\nSam\nFernhill Support",
                    name.split(' ').next().unwrap_or(name),
                    replies[k / 3 % replies.len()]
                ),
            ));
        }
        d.threads.push(thread);
    }
}

fn sent(c: &Clock, d: &mut Demo) {
    let work: [(P, &str, &str); 12] = [
        (
            INGRID,
            "Board deck – final",
            "Hi Ingrid,\n\nThe final board deck is in the shared folder. I tightened the product section to four slides and moved the roadmap to the appendix.\n\nAlex",
        ),
        (
            JONAS,
            "Thanks for the late night",
            "Jonas, thank you for staying late to get rc.3 out. It made a real difference to the rest of the team’s week.\n\nAlex",
        ),
        (
            PRIYA,
            "Launch post – first draft",
            "Hi Priya,\n\nFirst draft of the community launch post is here: https://quillboard.test/d/launch-post\n\nI kept it to what it is, why we built it and what’s next. Edits welcome.\n\nAlex",
        ),
        (
            SAM,
            "Saved replies for launch day",
            "Sam, the saved replies look great. One tweak: link the import guide in the import reply rather than pasting the steps.\n\nAlex",
        ),
        (
            ELENA,
            "Great meeting you",
            "Hi Elena,\n\nGreat to meet you at the meetup. As promised, here’s our short overview of Fernhill for operations teams: https://fernhill.example/teams/operations\n\nAlex",
        ),
        (
            NOOR,
            "Re: your search idea",
            "Hi Noor,\n\nThank you for the kind words! Recent searches are a great idea; I’ve added it to our list for 2.1.\n\nAlex",
        ),
        (
            DANIEL,
            "Intro: Quaypoint Logistics",
            "Hi Daniel,\n\nQuick heads-up: I’m speaking with Quaypoint Logistics next week, the team you mentioned in October. I’ll let you know how it goes.\n\nAlex",
        ),
        (
            LUCAS,
            "Retry bug",
            "Lucas, thanks for chasing down the retry bug. Can we add the large-import case to the integration tests?\n\nAlex",
        ),
        (
            MAYA,
            "Icon set",
            "Maya, the new icon set is lovely. Ship it.\n\nAlex",
        ),
        (
            TOMAS,
            "Interview schedule",
            "Tomás, I’ve blocked Tuesday 13:00–17:00 for the final-round interviews.\n\nAlex",
        ),
        (
            HANNA,
            "Bergen dinner",
            "Hanna, thanks for organising the offsite. Could we invite Ada from Brightwater to the Thursday dinner? She’s in Bergen that week.\n\nAlex",
        ),
        (
            MARCUS,
            "Prep doc",
            "Marcus, thanks for the prep doc. Topics look perfect. Talk soon!\n\nAlex",
        ),
    ];
    for (k, (to, subject, body)) in work.iter().enumerate() {
        d.threads.push(
            Thread::work(format!("sent-work-{k}"), *subject).sent().msg(
                me(
                    c.day(
                        1 + k as i64 * 2 % 17,
                        8 + (k as i64 * 3) % 10,
                        (k as i64 * 19) % 60,
                    ),
                    *body,
                )
                .to(&[*to]),
            ),
        );
    }
    let home: [(P, &str, &str); 6] = [
        (
            ERIK,
            "Gratulerer med dagen!",
            "Gratulerer med dagen, Erik! Håper du får en fin dag. Vi tar en øl på fredag?\n\nAlex",
        ),
        (
            ROWAN,
            "Photos from the weekend",
            "Rowan, here are the photos from Saturday. The one of you on the purple problem is my favourite.\n\nAlex",
        ),
        (
            SOFIA,
            "Book club",
            "Count me in for the 18th! I’ll bring bread for the soup.\n\nAlex",
        ),
        (
            MAMMA,
            "Bursdagsgave til pappa",
            "Hei mamma! Jeg tenkte å gi pappa en bok om fjellturer. Hva tror du?\n\nKlem",
        ),
        (
            ROWAN,
            "Climbing gym membership",
            "Hey, the climbing gym has a two-for-one offer this month. Want to sign up together?\n\nAlex",
        ),
        (
            ERIK,
            "Hyttenøkkel",
            "Erik, jeg har lagt hyttenøkkelen i postkassen din. Takk for lånet!\n\nAlex",
        ),
    ];
    for (k, (to, subject, body)) in home.iter().enumerate() {
        d.threads.push(
            Thread::home(format!("sent-home-{k}"), *subject).sent().msg(
                me(
                    c.day(3 + k as i64 * 3, 19 + (k as i64 % 3), (k as i64 * 23) % 60),
                    *body,
                )
                .to(&[*to]),
            ),
        );
    }
}

fn older(c: &Clock, d: &mut Demo) {
    let threads: [(P, &str, &str); 20] = [
        (
            INGRID,
            "Q3 retro notes",
            "Notes from the Q3 retro are in the shared folder. Three actions for Q4: fewer meetings, earlier code freeze, and a support rota.",
        ),
        (
            MAYA,
            "Brand colours v2",
            "Here’s the updated palette. The new green passes contrast on both themes.",
        ),
        (
            JONAS,
            "On-call handover",
            "Handing over on-call to Lucas. Quiet week: one disk alert, resolved.",
        ),
        (
            PRIYA,
            "Beta waitlist numbers",
            "We passed 9,000 people on the waitlist this morning.",
        ),
        (
            SAM,
            "Customer interview notes",
            "Notes from five customer interviews. Common theme: search and keyboard shortcuts.",
        ),
        (
            LUCAS,
            "Database migration plan",
            "Plan for moving the sync service to the new database cluster. Zero downtime, two maintenance windows.",
        ),
        (
            HANNA,
            "New laptop policy",
            "Everyone gets a new laptop every three years. Ordering opens next month.",
        ),
        (
            TOMAS,
            "iOS beta feedback",
            "TestFlight-style beta feedback summary: 41 responses, mostly positive about speed.",
        ),
        (
            ADA,
            "Pilot kickoff",
            "Looking forward to starting the pilot on Monday. The team is ready.",
        ),
        (
            ELENA,
            "Logistics meetup – speakers",
            "Thanks for agreeing to give a demo at the meetup next week!",
        ),
        (
            KARI,
            "Årsoppgjør – tidsplan",
            "Hei Alex, her er tidsplanen for årsoppgjøret. Første frist er i februar.",
        ),
        (
            DANIEL,
            "Q3 investor update – thanks",
            "Thanks for the Q3 update. Great progress on retention.",
        ),
        (
            INGRID,
            "Office move",
            "We move to the new office on the 1st. Pack your desks by Friday, please.",
        ),
        (
            MAYA,
            "User testing schedule",
            "Five sessions booked for next week. Observers welcome.",
        ),
        (
            JONAS,
            "Postmortem: sync outage",
            "Postmortem for last Tuesday’s 12-minute sync outage. Root cause: an expired certificate.",
        ),
        (
            PRIYA,
            "Press list",
            "Draft press list for the launch. 24 journalists and 8 newsletters.",
        ),
        (
            SAM,
            "Support macros v1",
            "First set of support macros is live. Please review.",
        ),
        (
            LUCAS,
            "Search index rebuild",
            "The search index rebuild finished overnight. Queries are about 30% faster.",
        ),
        (
            HANNA,
            "Christmas holidays",
            "Office closed between Christmas and New Year. Support rota to follow.",
        ),
        (
            TOMAS,
            "Accessibility audit",
            "Results of the accessibility audit: 12 issues, 9 already fixed.",
        ),
    ];
    for (k, (person, subject, body)) in threads.iter().enumerate() {
        let days = 9 + k as i64 * 4;
        let mut thread = Thread::work(format!("older-{k}"), *subject)
            .label("Work")
            .archived()
            .msg(from(
                *person,
                c.day(days, 9 + k as i64 % 8, (k as i64 * 11) % 60),
                format!(
                    "Hi Alex,\n\n{body}\n\n{}",
                    person.0.split(' ').next().unwrap_or(person.0)
                ),
            ));
        if k % 2 == 0 {
            thread = thread.msg(
                me(
                    c.day(days, 12 + k as i64 % 5, (k as i64 * 7) % 60),
                    "Thanks, looks good to me.\n\nAlex",
                )
                .to(&[*person]),
            );
        }
        d.threads.push(thread);
    }
}

fn drafts(c: &Clock, d: &mut Demo) {
    let launch = c.ahead(launch_day(c), 8, 0);
    d.drafts = vec![
        Draft {
            key: "launch-post",
            account: super::DEMO_WORK_ACCOUNT,
            to: vec![TEAM.addr()],
            cc: Vec::new(),
            subject: "Re: Launch plan: Fernhill 2.0".into(),
            body: "Here’s a first pass at the community launch post:\n\nFernhill 2.0: fast, private email for teams\n\nHi everyone, I’m Alex, one of the people behind Fernhill. Today we’re releasing 2.0, a native email app that keeps your mail on your device, searches it in milliseconds and works offline.\n\n".into(),
            reply_to: Some("launch-plan"),
            send_at: None,
            saved_at: c.ago(41),
        },
        Draft {
            key: "speaker-bio",
            account: super::DEMO_WORK_ACCOUNT,
            to: vec![PRODUCT_DAYS.addr()],
            cc: Vec::new(),
            subject: "Speaker bio – Alex Morgan".into(),
            body: "Hi,\n\nHere’s my bio for the programme:\n\nAlex Morgan is Head of Product at Fernhill, where they work on fast, private email for teams. Before Fernhill, Alex".into(),
            reply_to: None,
            send_at: None,
            saved_at: c.day(1, 22, 10),
        },
        Draft {
            key: "all-hands",
            account: super::DEMO_WORK_ACCOUNT,
            to: vec![TEAM.addr()],
            cc: Vec::new(),
            subject: "Notes for Monday’s all-hands".into(),
            body: "Agenda\n1. Launch recap\n2. Customer stories\n3. Q1 priorities\n".into(),
            reply_to: None,
            send_at: None,
            saved_at: c.day(2, 17, 45),
        },
        Draft {
            key: "dugnad-svar",
            account: super::DEMO_PERSONAL_ACCOUNT,
            to: vec![COOP.addr()],
            cc: Vec::new(),
            subject: "Dugnad".into(),
            body: "Hei!\n\nJeg kommer på dugnaden, men må gå klokka 12. Jeg kan ta ansvar for".into(),
            reply_to: None,
            send_at: None,
            saved_at: c.day(2, 20, 30),
        },
        Draft {
            key: "launch-thanks",
            account: super::DEMO_WORK_ACCOUNT,
            to: vec![TEAM.addr()],
            cc: Vec::new(),
            subject: "Launch day: thank you, team".into(),
            body: "Good morning, everyone!\n\nBy the time you read this, Fernhill 2.0 is live. Thank you for the care you put into every detail. Enjoy today; we earned it.\n\nAlex".into(),
            reply_to: None,
            send_at: Some(launch),
            saved_at: c.day(1, 23, 5),
        },
        Draft {
            key: "pilot-next-steps",
            account: super::DEMO_WORK_ACCOUNT,
            to: vec![ADA.addr()],
            cc: vec![INGRID.addr()],
            subject: "Re: Pilot results – Brightwater × Fernhill".into(),
            body: "Hi Ada,\n\nAs promised, here’s our proposal for the January rollout: 160 seats on the Business plan, onboarding in two waves, and shared drafts in early access for your team.\n\nAlex".into(),
            reply_to: Some("brightwater-pilot"),
            send_at: Some(c.ahead(1, 8, 30)),
            saved_at: c.day(2, 9, 0),
        },
        Draft {
            key: "bursdag-erik",
            account: super::DEMO_PERSONAL_ACCOUNT,
            to: vec![ERIK.addr()],
            cc: Vec::new(),
            subject: "Takk for turen!".into(),
            body: "Takk for en fantastisk helg på hytta, Erik! Neste gang er det min tur til å lage taco.\n\nAlex".into(),
            reply_to: None,
            send_at: Some(c.ahead(c.days_until(0), 7, 30)),
            saved_at: c.ago(100),
        },
    ];
}
