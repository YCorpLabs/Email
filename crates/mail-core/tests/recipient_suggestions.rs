use mail_core::{Account, MailStore, Provider, RecipientHeaders, parse_outgoing_recipients};

fn store() -> MailStore {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    store
}
fn message(store: &MailStore, id: &str, sender: &str, time: i64) -> mail_core::Message {
    let mut m = store.get_message("demo:1").unwrap().unwrap();
    m.id = id.into();
    m.thread_id = id.into();
    m.sender = sender.into();
    m.to.clear();
    m.recipient_headers = None;
    m.received_at = time;
    m.is_draft = false;
    m.is_sent = false;
    m
}

#[test]
fn suggestions_are_account_scoped_and_never_use_received_bcc_or_bodies() {
    let store = store();
    store
        .upsert_account(&Account {
            id: "other".into(),
            provider: Provider::Gmail,
            email: "other@example.com".into(),
            display_name: "Other".into(),
        })
        .unwrap();
    let mut m = message(
        &store,
        "privacy",
        "Visible Sender <visible@example.org>",
        10,
    );
    m.to = vec!["To Person <to-person@example.org>".into()];
    m.recipient_headers = Some(RecipientHeaders {
        cc: vec!["Copy Person <copy@example.org>".into()],
        bcc: vec!["Secret Person <secret@example.org>".into()],
        reply_to: vec![],
    });
    m.body_plain = "Body-only Contact <body-only@example.org>".into();
    store.upsert_message(&m).unwrap();
    m.id = "other-message".into();
    m.thread_id = m.id.clone();
    m.account_id = "other".into();
    m.sender = "Forbidden <forbidden@example.org>".into();
    store.upsert_message(&m).unwrap();
    let mut remote_draft = message(&store, "remote-draft", "Remote <remote@example.org>", 99);
    remote_draft.is_draft = true;
    remote_draft.recipient_headers = Some(RecipientHeaders {
        bcc: vec!["hidden-remote@example.org".into()],
        ..Default::default()
    });
    store.upsert_message(&remote_draft).unwrap();
    assert!(
        store
            .suggest_recipients("demo:personal", "hidden-remote", 6)
            .unwrap()
            .is_empty()
    );
    for query in ["secret", "body-only", "forbidden"] {
        assert!(
            store
                .suggest_recipients("demo:personal", query, 6)
                .unwrap()
                .is_empty(),
            "{query}"
        );
    }
    for query in ["visible", "to-person", "copy"] {
        assert_eq!(
            store
                .suggest_recipients("demo:personal", query, 6)
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(
        store.suggest_recipients("other", "forbidden", 6).unwrap()[0].email,
        "forbidden@example.org"
    );
    assert!(store.suggest_recipients("missing", "anything", 6).is_err());
}

#[test]
fn quoted_names_deduplicate_and_insert_as_one_valid_recipient() {
    let store = store();
    let first = message(&store, "name", "\"Doe, Jane\" <JANE@Example.org>", 11);
    store.upsert_message(&first).unwrap();
    let second = message(&store, "bare", "jane@example.org", 12);
    store.upsert_message(&second).unwrap();
    let results = store
        .suggest_recipients("demo:personal", "jAnE", 6)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].email, "jane@example.org");
    assert_eq!(results[0].display_name, "Doe, Jane");
    assert_eq!(
        parse_outgoing_recipients(&results[0].formatted)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .suggest_recipients("demo:personal", "' OR 1=1 --", 6)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .suggest_recipients("demo:personal", "%", 6)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn completed_saved_draft_recipients_are_suggested_but_incomplete_or_injected_are_not() {
    let store = store();
    store
        .compose_draft_with_recipients(
            "demo:personal",
            None,
            "unfinished@",
            Some("Draft Copy <draft-copy@example.org>"),
            Some("Draft Private <draft-private@example.org>"),
            "Draft",
            "Body",
        )
        .unwrap();
    assert_eq!(
        store
            .suggest_recipients("demo:personal", "draft-copy", 6)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .suggest_recipients("demo:personal", "draft-private", 6)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .suggest_recipients("demo:personal", "unfinished", 6)
            .unwrap()
            .is_empty()
    );
    let m = message(&store, "injected", "Bad\r\nBcc: injected@example.org", 100);
    store.upsert_message(&m).unwrap();
    assert!(
        store
            .suggest_recipients("demo:personal", "injected", 6)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .suggest_recipients("demo:personal", "injected\n", 6)
            .is_err()
    );
}

#[test]
fn ranking_prefers_exact_then_email_prefix_then_names_and_is_bounded() {
    let store = store();
    for (id, sender, time) in [
        ("a", "Bob <bob@example.org>", 10),
        ("b", "Bobby <bobby@example.org>", 20),
        ("c", "Bob Builder <builder@example.org>", 30),
        ("d", "Bob Smith <smith@example.org>", 40),
    ] {
        store
            .upsert_message(&message(&store, id, sender, time))
            .unwrap();
    }
    let results = store.suggest_recipients("demo:personal", "bob", 3).unwrap();
    assert_eq!(
        results.iter().map(|s| s.email.as_str()).collect::<Vec<_>>(),
        vec!["bobby@example.org", "bob@example.org", "smith@example.org"]
    );
    assert_eq!(
        store
            .suggest_recipients("demo:personal", "bob@example.org", 6)
            .unwrap()[0]
            .email,
        "bob@example.org"
    );
    let mut m = message(&store, "many", "sender@example.net", 50);
    m.to = (0..40)
        .map(|i| format!("many-{i:02}@example.org"))
        .collect();
    store.upsert_message(&m).unwrap();
    assert_eq!(
        store
            .suggest_recipients("demo:personal", "many-", 999)
            .unwrap()
            .len(),
        20
    );
    assert!(
        store
            .suggest_recipients("demo:personal", "many-", 0)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .suggest_recipients("demo:personal", "  ", 6)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .suggest_recipients("demo:personal", &"x".repeat(129), 6)
            .is_err()
    );
}

#[test]
fn recent_window_is_scoped_before_limiting() {
    let store = store();
    store
        .upsert_account(&Account {
            id: "other".into(),
            provider: Provider::Gmail,
            email: "other@example.com".into(),
            display_name: "Other".into(),
        })
        .unwrap();
    store
        .upsert_message(&message(
            &store,
            "mine",
            "Old But Relevant <relevant@example.org>",
            1,
        ))
        .unwrap();
    for i in 0..5_001 {
        let mut m = message(
            &store,
            &format!("other-{i}"),
            "Noisy <noise@example.org>",
            i + 100,
        );
        m.account_id = "other".into();
        store.upsert_message(&m).unwrap();
    }
    assert_eq!(
        store
            .suggest_recipients("demo:personal", "relevant", 6)
            .unwrap()
            .len(),
        1
    );
    let mut m = message(&store, "new", "Newer <new@example.org>", 100_000);
    for i in 0..5_000 {
        m.id = format!("mine-{i}");
        m.thread_id = m.id.clone();
        m.received_at += 1;
        store.upsert_message(&m).unwrap();
    }
    assert!(
        store
            .suggest_recipients("demo:personal", "relevant", 6)
            .unwrap()
            .is_empty()
    );
}
