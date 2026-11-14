use mail_core::{Account, MailAction, MailStore, Provider, RecipientHeaders, SearchScope};
fn store() -> MailStore {
    let s = MailStore::open_in_memory().unwrap();
    s.seed_demo().unwrap();
    s
}
fn ids(s: &MailStore, q: &str) -> Vec<String> {
    s.search(q, 100)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect()
}
#[test]
fn structured_fields_phrases_dates_and_combined_predicates_are_local() {
    let s = store();
    let mut m = s.get_message("demo:1").unwrap().unwrap();
    m.to = vec![
        "Alex <alex@example.com>".into(),
        "Pat Peer <pat@example.net>".into(),
    ];
    m.recipient_headers = Some(RecipientHeaders {
        cc: vec!["Casey Copy <casey@example.org>".into()],
        bcc: vec!["Secret <secret@example.org>".into()],
        reply_to: vec![],
    });
    m.body_plain = "A design review with a detailed invoice is ready".into();
    m.has_attachments = Some(true);
    m.received_at = 1_767_225_600_000;
    s.upsert_message(&m).unwrap(); // 2026-01-01 UTC
    s.apply_action(
        "label",
        MailAction::AddLabel {
            thread_id: m.thread_id.clone(),
            label: "Work Invoices".into(),
        },
    )
    .unwrap();
    for q in [
        "from:maya@example.org",
        "from:\"Maya Chen\" to:pat@example.net cc:Casey",
        "subject:\"Design review\"",
        "\"design review\" invoice",
        "has:attachment is:unread label:\"Work Invoices\"",
        "after:2026-01-01 before:2026-01-02",
        "in:inbox subject:Friday",
    ] {
        assert_eq!(ids(&s, q), vec!["demo:1"], "{q}");
    }
    assert!(ids(&s, "\"review design\"").is_empty());
    assert!(ids(&s, "cc:secret@example.org").is_empty()); // Bcc is never a Cc match.
    assert!(ids(&s, "to:casey@example.org").is_empty());
    assert!(ids(&s, "is:read has:attachment").is_empty());
    assert!(ids(&s, "before:2026-01-01 has:attachment").is_empty());
    assert!(ids(&s, "after:2026-01-02 has:attachment").is_empty());
}
#[test]
fn recognized_invalid_operators_and_unclosed_quotes_fail_without_broadening() {
    let s = store();
    for q in [
        "from:",
        "subject:\"\"",
        "cc:",
        "label:",
        "is:urgent",
        "in:everywhere",
        "has:virus",
        "before:2026-02-30",
        "after:26-1-1",
        "before:2026-01-01junk",
        "subject:\"open",
        "\"",
        "from:\"\"",
    ] {
        assert!(s.search(q, 100).is_err(), "{q}");
    }
    assert!(s.search(&"a".repeat(4097), 100).is_err());
    assert!(s.search(&vec!["a"; 65].join(" "), 100).is_err());
    assert!(ids(&s, "from:\"x') OR 1=1 --\"").is_empty());
    assert!(ids(&s, "label:\"x' OR 1=1 --\"").is_empty());
    assert!(s.search("https://example.com", 100).is_ok());
}
#[test]
fn unknown_attachment_and_recipient_metadata_never_trigger_fetch_or_false_match() {
    let s = store();
    let mut m = s.get_message("demo:1").unwrap().unwrap();
    m.has_attachments = None;
    m.recipient_headers = None;
    s.upsert_message(&m).unwrap();
    assert!(ids(&s, "has:attachment").is_empty());
    assert!(ids(&s, "cc:casey").is_empty());
    let draft = s
        .compose_draft("demo:personal", "x@example.com", "Draft with file", "body")
        .unwrap();
    s.add_draft_attachment(&draft.id, "test.txt", "text/plain", b"test")
        .unwrap();
    assert_eq!(ids(&s, "has:attachment"), vec![draft.thread_id]);
}
#[test]
fn scope_filters_precede_limit_paging_and_intersect_query_operators() {
    let s = store();
    s.upsert_account(&Account {
        id: "other".into(),
        provider: Provider::Gmail,
        email: "other@example.org".into(),
        display_name: "Other".into(),
    })
    .unwrap();
    let mut m = s.get_message("demo:1").unwrap().unwrap();
    for n in 0..15 {
        m.id = format!("other:{n}");
        m.thread_id = m.id.clone();
        m.account_id = "other".into();
        m.received_at += 1;
        s.upsert_message(&m).unwrap();
    }
    let scope = SearchScope {
        account_ids: Some(vec!["demo:personal".into()]),
        is_inbox: Some(true),
        unread_only: true,
        ..Default::default()
    };
    let own = s.search_scoped("", 1, 0, &scope).unwrap();
    assert_eq!(own[0].id, "demo:1");
    let next = s.search_scoped("", 1, 1, &scope).unwrap();
    assert_eq!(next[0].id, "demo:3");
    assert!(
        s.search_scoped("is:read", 100, 0, &scope)
            .unwrap()
            .is_empty()
    );
    assert!(
        s.search_scoped(
            "",
            100,
            0,
            &SearchScope {
                account_ids: Some(vec![]),
                ..Default::default()
            }
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        s.search_for_accounts("Friday", 1, &["demo:personal".into()])
            .unwrap()[0]
            .id,
        "demo:1"
    );
}
#[test]
fn favorites_labels_split_archive_and_snoozed_constraints_apply_before_limit() {
    let s = store();
    s.apply_action(
        "label",
        MailAction::AddLabel {
            thread_id: "demo:1".into(),
            label: "Work".into(),
        },
    )
    .unwrap();
    s.apply_action(
        "star",
        MailAction::AddLabel {
            thread_id: "demo:1".into(),
            label: "STARRED".into(),
        },
    )
    .unwrap();
    let split = s
        .upsert_split(None, "Maya", "from:maya@example.org")
        .unwrap();
    let scope = SearchScope {
        label: Some("Work".into()),
        starred_only: true,
        split_id: Some(split.id),
        ..Default::default()
    };
    assert_eq!(
        s.search_scoped("review", 1, 0, &scope).unwrap()[0].id,
        "demo:1"
    );
    assert!(s.search_scoped("", 1, 1, &scope).unwrap().is_empty());
    s.set_thread_pinned("demo:2", true).unwrap();
    assert_eq!(ids(&s, "is:starred").len(), 2);
    s.apply_action(
        "archive",
        MailAction::Archive {
            thread_id: "demo:1".into(),
        },
    )
    .unwrap();
    s.apply_action(
        "spam",
        MailAction::Move {
            thread_id: "demo:3".into(),
            folder: "Spam".into(),
        },
    )
    .unwrap();
    assert_eq!(ids(&s, "in:archive"), vec!["demo:1"]);
    assert_eq!(ids(&s, "in:spam"), vec!["demo:3"]);
    assert_eq!(
        s.search_scoped(
            "",
            1,
            0,
            &SearchScope {
                folder: Some("archive".into()),
                ..Default::default()
            }
        )
        .unwrap()[0]
            .id,
        "demo:1"
    );
    s.snooze_thread("demo:2", 4_102_444_800_000).unwrap();
    assert!(
        s.search_scoped(
            "",
            10,
            0,
            &SearchScope {
                is_inbox: Some(true),
                ..Default::default()
            }
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        s.search_scoped(
            "",
            10,
            0,
            &SearchScope {
                snoozed_only: true,
                ..Default::default()
            }
        )
        .unwrap()[0]
            .id,
        "demo:2"
    );
}

#[test]
fn sent_scope_keeps_delivered_mail_after_outbox_removal() {
    let s = store();
    let draft = s.compose_draft("demo:personal", "pat@example.net", "Delivery regression", "Hello").unwrap();
    let scope = SearchScope { folder: Some("sent".into()), ..Default::default() };
    assert!(s.search_scoped("", 100, 0, &scope).unwrap().is_empty());
    s.mark_draft_sent(&draft.id).unwrap();
    let rows = s.search_scoped("", 100, 0, &scope).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, draft.thread_id);
    assert_eq!(ids(&s, "in:sent"), vec![draft.thread_id]);
    // Sent is message-based: replies remain discoverable in mixed inbox threads.
    let mut incoming = s.get_message("demo:1").unwrap().unwrap();
    incoming.is_sent = true;
    incoming.is_draft = false;
    s.upsert_message(&incoming).unwrap();
    assert!(s.search_scoped("", 100, 0, &scope).unwrap().iter().any(|t| t.id == incoming.thread_id));
}

/// What Gmail sync stores for mail sent from Gmail or a phone: the message is
/// marked sent, and the thread's labels never include Gmail's SENT label.
struct GmailSentAdapter;
impl mail_core::MailAdapter for GmailSentAdapter {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }
    fn sync(&self, account: &Account, _: Option<&str>) -> mail_core::Result<mail_core::SyncBatch> {
        let mut message = mail_core::Message {
            id: "demo:personal:gmail-message:sent-elsewhere".into(),
            thread_id: "demo:personal:gmail-thread:sent-elsewhere".into(),
            account_id: account.id.clone(),
            sender: format!("Alex <{}>", account.email),
            to: vec!["pat@example.net".into()],
            subject: "Venue proposal".into(),
            body_plain: "Here is the venue proposal we discussed.".into(),
            body_html: None,
            unsubscribe_header: None,
            has_attachments: Some(false),
            received_at: 1_767_225_600_000,
            is_read: true,
            is_draft: false,
            is_sent: true,
            recipient_headers: None,
        };
        message.recipient_headers = Some(RecipientHeaders::default());
        Ok(mail_core::SyncBatch {
            message_states: vec![],
            thread_labels: vec![(
                message.thread_id.clone(),
                vec!["IMPORTANT".into(), "__EMAIL_ARCHIVE__".into()],
            )],
            messages: vec![message],
            removed_message_ids: vec![],
            full_reset: false,
            reset_phase: None,
            next_cursor: "sent".into(),
        })
    }
}

#[test]
fn gmail_sent_mail_is_found_by_every_sent_spelling_and_for_agents() {
    let s = store();
    s.run_sync(&GmailSentAdapter, "demo:personal").unwrap();
    let thread = "demo:personal:gmail-thread:sent-elsewhere".to_string();
    // Agents read Gmail's label list, which has INBOX and IMPORTANT but no SENT.
    assert!(!s.list_labels().unwrap().iter().any(|l| l.name.eq_ignore_ascii_case("sent")));
    for q in ["in:sent", "in:Sent", "label:SENT", "label:sent", "is:sent", "label:SENT venue", "is:sent subject:proposal"] {
        assert_eq!(ids(&s, q), vec![thread.clone()], "{q}");
    }
    // The app's Sent folder and the MCP account-scoped search agree.
    let scope = SearchScope { folder: Some("sent".into()), ..Default::default() };
    assert!(s.search_scoped("", 100, 0, &scope).unwrap().iter().any(|t| t.id == thread));
    let scoped = |accounts: &[&str]| -> Vec<String> {
        let accounts: Vec<String> = accounts.iter().map(|a| a.to_string()).collect();
        s.search_for_accounts("label:SENT", 100, &accounts).unwrap().into_iter().map(|t| t.id).collect()
    };
    assert_eq!(scoped(&["demo:personal"]), vec![thread.clone()]);
    assert!(scoped(&["other"]).is_empty());
    // Received mail is not sent mail, and a real label named Sent still matches.
    assert!(!ids(&s, "label:sent").contains(&"demo:1".to_string()));
    s.apply_action("sent-label", MailAction::AddLabel { thread_id: "demo:1".into(), label: "Sent".into() }).unwrap();
    assert!(ids(&s, "label:sent").contains(&"demo:1".to_string()));
    assert!(!ids(&s, "in:sent").contains(&"demo:1".to_string()));
}
