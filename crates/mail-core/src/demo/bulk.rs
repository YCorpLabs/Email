//! A large, older mail history in the launch demo's style, for search shots
//! and benchmarks. Same people, senders and subjects as the curated demo, so
//! a search for "invoice" or a colleague's name returns believable results.
use super::content::*;
use super::{DEMO_PERSONAL_ACCOUNT, DEMO_WORK_ACCOUNT, P};
use crate::{MailError, MailStore, Message, RecipientHeaders, Result, upsert_message_tx};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchCorpusSummary {
    pub messages: usize,
    pub threads: usize,
}

const DAY: i64 = 86_400_000;
const BATCH: usize = 2_000;

const TOPICS: [&str; 24] = [
    "Roadmap review",
    "Customer interview notes",
    "Hiring plan",
    "Design critique",
    "Weekly metrics",
    "Support escalation",
    "Pricing experiment",
    "Partner integration",
    "Security questionnaire",
    "Offsite agenda",
    "Release checklist",
    "Board preparation",
    "Marketing calendar",
    "Onboarding feedback",
    "Data export request",
    "Performance budget",
    "Accessibility fixes",
    "Quarterly planning",
    "Vendor contract",
    "Office logistics",
    "Search ranking",
    "Sync reliability",
    "Mobile beta",
    "Customer advisory board",
];
const SENTENCES: [&str; 24] = [
    "I went through the numbers again this morning and they hold up.",
    "Can we decide this before Friday so the team can plan next week?",
    "The customer was happy with the outcome and asked for a follow-up call.",
    "I’ve left comments in the document; most are small wording changes.",
    "Let’s keep the scope small and ship the first version behind a flag.",
    "Attaching the notes from the call for anyone who missed it.",
    "The invoice from the vendor came in higher than the quote, so I’ve asked them to correct it.",
    "Support volume is up, but first response times are still under an hour.",
    "We should write this down so new people don’t have to ask again.",
    "Happy to pair on this tomorrow if that helps.",
    "Search latency stayed under ten milliseconds in every test run.",
    "The design team suggested a simpler empty state, which I like.",
    "Legal reviewed the contract and only had two comments.",
    "I’ll send a calendar invite for Thursday afternoon.",
    "The migration ran overnight without errors.",
    "Two customers asked for the same feature this week.",
    "I’d rather delay a week than ship something we’re not proud of.",
    "Finance needs the receipts by the end of the month.",
    "The new onboarding flow cut drop-off in half.",
    "Let me know if anything here is unclear.",
    "We’re on track for the release date.",
    "I booked the meeting room and ordered lunch.",
    "Please review the draft before we send it to the board.",
    "Thanks again for jumping on this so quickly.",
];
const NORWEGIAN: [(&str, &str); 8] = [
    (
        "Faktura fra Fossekraft Strøm",
        "Fakturaen din er klar. Beløpet trekkes automatisk på forfallsdato.",
    ),
    (
        "Ny eFaktura fra Fjellby Bank",
        "Du har mottatt en ny eFaktura. Logg inn i nettbanken for å godkjenne den.",
    ),
    (
        "Påminnelse fra Byens bibliotek",
        "Lånet ditt forfaller snart. Du kan fornye det på nett.",
    ),
    (
        "Middag på søndag?",
        "Har du lyst til å komme på middag? Vi lager fiskesuppe.",
    ),
    (
        "Hyttetur?",
        "Skal vi ta en tur på hytta snart? Meldingen er fin.",
    ),
    (
        "Dugnad i borettslaget",
        "Vi trenger hjelp til å rydde fellesarealene på lørdag.",
    ),
    (
        "Kvittering fra Lille Bønne Kaffe",
        "Takk for bestillingen! Kaffen sendes på mandag.",
    ),
    (
        "Timebekreftelse",
        "Vi bekrefter timen din. Gi beskjed hvis du må avbestille.",
    ),
];

fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn pick<T: Copy>(items: &[T], seed: u64) -> T {
    items[(seed % items.len() as u64) as usize]
}

fn body(seed: u64, count: usize) -> String {
    (0..count)
        .map(|k| pick(&SENTENCES, mix(seed + k as u64)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One synthetic conversation: (account, sender, subject, body, reply).
fn conversation(index: usize, at: i64) -> (&'static str, String, String, String, bool) {
    let seed = mix(index as u64 + 0x5eed);
    let year = 1970 + at / (365 * DAY + DAY / 4);
    let colleagues = [INGRID, MAYA, JONAS, PRIYA, SAM, LUCAS, HANNA, TOMAS];
    let customers = [ELENA, ADA, DANIEL, MARCUS, NOOR, OLIVIA];
    match seed % 100 {
        0..=9 => {
            let number = 1000 + seed % 9000;
            match seed % 4 {
                0 => (
                    DEMO_WORK_ACCOUNT,
                    CIRRUS.addr(),
                    format!("Invoice CB-{year}-{number:04} from Cirrusbay Cloud"),
                    format!(
                        "Your invoice CB-{year}-{number:04} is ready. Amount due: €{}.{:02}. Payment is taken automatically from the card on file. View usage and past invoices at https://console.cirrusbay.test/billing",
                        600 + seed % 900,
                        seed % 100
                    ),
                    false,
                ),
                1 => (
                    DEMO_WORK_ACCOUNT,
                    TALLY.addr(),
                    format!("Your receipt from Signfold [#{number}]"),
                    format!(
                        "Receipt from Signfold. Amount paid: €12.00. Receipt number {number}. Keep this receipt for your records; an invoice PDF is available on request."
                    ),
                    false,
                ),
                2 => (
                    DEMO_WORK_ACCOUNT,
                    KARI.addr(),
                    format!("Faktura {number} – regnskapstjenester"),
                    format!(
                        "Hei Alex, vedlagt er faktura {number} for regnskapstjenester. Invoice for accounting services, due in 14 days. Vennlig hilsen Kari"
                    ),
                    seed % 3 == 0,
                ),
                _ => (
                    DEMO_PERSONAL_ACCOUNT,
                    POWER.addr(),
                    "Faktura fra Fossekraft Strøm".into(),
                    format!(
                        "Fakturaen din er klar. Beløp: {} kr. Invoice sent to your online bank.",
                        700 + seed % 900
                    ),
                    false,
                ),
            }
        }
        10..=33 => {
            let repo = pick(&REPOS, seed >> 8);
            let title = pick(&PULLS, seed >> 16);
            let person = pick(&colleagues, seed >> 24);
            let number = 1 + seed % 380;
            (
                DEMO_WORK_ACCOUNT,
                format!("{} <notifications@forgeyard.test>", person.0),
                format!("Re: [{repo}] {title} (PR #{number})"),
                format!(
                    "{}\n\nView it on Forgeyard: https://forgeyard.test/{repo}/pull/{number}",
                    pick(&COMMENTS, seed >> 32)
                ),
                false,
            )
        }
        34..=47 => {
            let senders = [QUIET, SYSTEMS, FOUNDER, HARBOUR, FJORDBYTE];
            let sender = pick(&senders, seed >> 8);
            let topic = pick(&TOPICS, seed >> 16);
            (
                if seed % 2 == 0 {
                    DEMO_WORK_ACCOUNT
                } else {
                    DEMO_PERSONAL_ACCOUNT
                },
                sender.addr(),
                format!(
                    "{}: {} – issue {}",
                    sender.0,
                    topic.to_lowercase(),
                    10 + seed % 200
                ),
                format!(
                    "This week’s newsletter is about {}. {} Read online, or unsubscribe at any time.",
                    topic.to_lowercase(),
                    body(seed, 4)
                ),
                false,
            )
        }
        48..=57 => {
            let senders = [FOREST, PAPERLEAF, BEAN, TIDY];
            let sender = pick(&senders, seed >> 8);
            (
                DEMO_PERSONAL_ACCOUNT,
                sender.addr(),
                format!("{}: {}% off this weekend", sender.0, 10 + seed % 4 * 10),
                "Our seasonal sale starts now. Free returns, and members get early access. Unsubscribe at any time.".into(),
                false,
            )
        }
        58..=79 => {
            let person = pick(&colleagues, seed >> 8);
            let topic = pick(&TOPICS, seed >> 16);
            (
                DEMO_WORK_ACCOUNT,
                person.addr(),
                if seed % 3 == 0 {
                    format!("Re: {topic}")
                } else {
                    topic.to_string()
                },
                format!(
                    "Hi Alex,\n\n{}\n\n{}",
                    body(seed, 3 + (seed % 4) as usize),
                    person.0.split(' ').next().unwrap_or(person.0)
                ),
                seed % 4 == 0,
            )
        }
        80..=87 => {
            let person = pick(&customers, seed >> 8);
            let topic = pick(&TOPICS, seed >> 16);
            (
                DEMO_WORK_ACCOUNT,
                person.addr(),
                format!("{topic} – follow-up"),
                format!("Hi Alex,\n\n{}\n\nBest,\n{}", body(seed, 3), person.0),
                seed % 2 == 0,
            )
        }
        88..=93 => {
            let (subject, text) = pick(&NORWEGIAN, seed >> 8);
            let people = [ERIK, MAMMA, BANK, POWER, COOP, LIBRARY, BEAN, DENTIST];
            (
                DEMO_PERSONAL_ACCOUNT,
                pick(&people, seed >> 16).addr(),
                subject.into(),
                text.into(),
                false,
            )
        }
        _ => {
            let senders: [(P, &str); 4] = [
                (NORDLYS, "Your trip is confirmed"),
                (HOTEL, "Reservation confirmed"),
                (SKYRAIL, "Your tickets"),
                (PARCEL, "Your parcel has been delivered"),
            ];
            let (sender, subject) = pick(&senders, seed >> 8);
            (
                DEMO_WORK_ACCOUNT,
                sender.addr(),
                format!("{subject} – ref. {:06X}", seed % 0xFF_FFFF),
                format!(
                    "{subject}. Booking reference {:06X}. Manage your booking online.",
                    seed % 0xFF_FFFF
                ),
                false,
            )
        }
    }
}

impl MailStore {
    /// Add `count` older conversations (from three weeks to about four years
    /// ago) to a store that already holds the launch demo. They stay in the
    /// inbox and are mostly read, below the curated mail, so searching from
    /// the inbox covers the whole history. Callers must pass a disposable store.
    pub fn seed_demo_search_corpus(
        &self,
        count: usize,
        now_ms: i64,
    ) -> Result<SearchCorpusSummary> {
        let accounts = self.list_accounts()?;
        if !accounts.iter().any(|a| a.id == DEMO_WORK_ACCOUNT) {
            return Err(MailError::InvalidAction(
                "Seed the launch demo before adding its search history".into(),
            ));
        }
        let span = 4 * 365 * DAY;
        let newest = now_ms - 21 * DAY;
        let mut messages = 0;
        let mut start = 0;
        while start < count {
            let end = (start + BATCH).min(count);
            let mut db = self.lock()?;
            let tx = db.transaction()?;
            for index in start..end {
                let at = newest
                    - (span * index as i64) / count.max(1) as i64
                    - (mix(index as u64) % 3_600_000) as i64;
                let (account, sender, subject, text, reply) = conversation(index, at);
                let me = if account == DEMO_WORK_ACCOUNT {
                    ALEX
                } else {
                    ALEX_HOME
                };
                let thread_id = format!("demo:bulk:{index}");
                let mut message = Message {
                    recipient_headers: Some(RecipientHeaders::default()),
                    id: format!("{thread_id}:0"),
                    thread_id: thread_id.clone(),
                    account_id: account.into(),
                    sender,
                    to: vec![me.addr()],
                    subject: subject.clone(),
                    body_plain: text,
                    body_html: None,
                    unsubscribe_header: None,
                    has_attachments: Some(false),
                    received_at: at,
                    is_read: index % 37 != 0,
                    is_draft: false,
                    is_sent: false,
                };
                upsert_message_tx(&tx, &message)?;
                messages += 1;
                if reply {
                    let to = message.sender.clone();
                    message.id = format!("{thread_id}:1");
                    message.sender = me.addr();
                    message.to = vec![to];
                    message.subject = format!("Re: {}", subject.trim_start_matches("Re: "));
                    message.body_plain =
                        format!("Thanks! {}\n\nAlex", body(mix(index as u64 + 7), 2));
                    message.received_at = at + 40 * 60_000;
                    message.is_read = true;
                    message.is_sent = true;
                    upsert_message_tx(&tx, &message)?;
                    messages += 1;
                }
            }
            tx.commit()?;
            start = end;
        }
        Ok(SearchCorpusSummary {
            messages,
            threads: count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_extends_the_launch_demo_with_searchable_history() {
        let store = MailStore::open_in_memory().unwrap();
        let now = super::super::tests::now();
        assert!(store.seed_demo_search_corpus(10, now).is_err());
        store.seed_launch_demo(now, 0).unwrap();
        let newest_curated = store.list_threads(None, 1, 0).unwrap()[0].last_message_at;
        let summary = store.seed_demo_search_corpus(3_000, now).unwrap();
        assert_eq!(summary.threads, 3_000);
        assert!(summary.messages > 3_000);
        // The curated mail stays on top.
        assert_eq!(
            store.list_threads(None, 1, 0).unwrap()[0].last_message_at,
            newest_curated
        );
        let invoices = store.search_in_folder("invoice", 100, Some(true)).unwrap();
        assert_eq!(invoices.len(), 100);
        let ingrid = store
            .search_in_folder("from:ingrid", 50, Some(true))
            .unwrap();
        assert!(ingrid.len() >= 20);
    }
}
