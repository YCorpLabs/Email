use mail_core::{MailStore, parse_outgoing_recipients};

#[test]
fn normalizes_lists_quoted_names_and_groups_without_header_injection() {
    let recipients =
        parse_outgoing_recipients("\"Doe, Jane\" <jane@example.com>, bob@example.com").unwrap();
    assert_eq!(recipients.len(), 2);
    assert!(recipients[0].contains("jane@example.com"));
    assert_eq!(recipients[1], "bob@example.com");
    assert_eq!(
        parse_outgoing_recipients("Team: a@example.com, b@example.com;")
            .unwrap()
            .len(),
        2
    );
    assert!(parse_outgoing_recipients("a@example.com\r\nBcc: thief@example.com").is_err());
    assert!(parse_outgoing_recipients("").is_err());
    assert!(parse_outgoing_recipients("unfinished@").is_err());
}

#[test]
fn drafts_keep_incomplete_input_and_normalize_valid_edits() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft("demo:personal", "unfinished@", "Subject", "Text")
        .unwrap();
    assert_eq!(draft.to, vec!["unfinished@"]);
    let edited = store
        .update_draft(
            &draft.id,
            "Jane Doe <jane@example.com>, bob@example.com",
            "Subject",
            "Text",
        )
        .unwrap();
    assert_eq!(edited.to.len(), 2);
    assert!(edited.to[0].contains("jane@example.com"));
    assert_eq!(edited.to[1], "bob@example.com");
    assert!(
        store
            .update_draft(&draft.id, "", "Subject", "Text")
            .unwrap()
            .to
            .is_empty()
    );
}

#[test]
fn malformed_recipients_never_claim_or_schedule_a_draft() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    for raw in [
        "a@@example.com",
        "foo@bar baz",
        "\"A\u{1b}B\" <a@example.com>",
    ] {
        assert!(parse_outgoing_recipients(raw).is_err(), "{raw:?}");
        let draft = store
            .compose_draft("demo:personal", raw, "Subject", "Body")
            .unwrap();
        assert!(store.claim_immediate_send(&draft.id).is_err());
        assert!(
            store
                .schedule_draft(&draft.id, i64::MAX, "scheduled")
                .is_err()
        );
        assert!(store.list_outbox().unwrap().is_empty());
        store
            .update_draft(&draft.id, "valid@example.com", "Subject", "Still editable")
            .unwrap();
    }
}

#[test]
fn cc_bcc_persist_and_legacy_edits_preserve_them() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft_with_recipients(
            "demo:personal",
            None,
            "to@example.com",
            Some("\"Doe, Jane\" <cc@example.com>"),
            Some("hidden@example.com"),
            "Subject",
            "Body",
        )
        .unwrap();
    let headers = draft.recipient_headers.unwrap();
    assert_eq!(headers.cc.len(), 1);
    assert!(headers.cc[0].contains("cc@example.com"));
    assert_eq!(headers.bcc, ["hidden@example.com"]);
    let edited = store
        .update_draft(&draft.id, "to@example.com", "Changed", "Body")
        .unwrap();
    assert_eq!(edited.recipient_headers.as_ref().unwrap(), &headers);
    let cleared = store
        .update_draft_with_recipients(
            &draft.id,
            None,
            "to@example.com",
            Some(""),
            None,
            "Changed",
            "Body",
        )
        .unwrap();
    assert!(cleared.recipient_headers.as_ref().unwrap().cc.is_empty());
    assert_eq!(
        cleared.recipient_headers.unwrap().bcc,
        ["hidden@example.com"]
    );
}

#[test]
fn all_recipient_classes_validate_before_queue_and_bcc_only_is_valid() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    for (cc, bcc) in [("broken@@example.com", ""), ("", "broken@@example.com")] {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft_with_recipients(
                "demo:personal",
                None,
                "to@example.com",
                Some(cc),
                Some(bcc),
                "Subject",
                "Body",
            )
            .unwrap();
        assert!(store.claim_immediate_send(&draft.id).is_err());
        assert!(
            store
                .schedule_draft(&draft.id, i64::MAX, "scheduled")
                .is_err()
        );
        assert!(store.list_outbox().unwrap().is_empty());
        store
            .update_draft_with_recipients(
                &draft.id,
                None,
                "",
                Some(""),
                Some("hidden@example.com"),
                "Subject",
                "Body",
            )
            .unwrap();
        store
            .schedule_draft(&draft.id, i64::MAX, "scheduled")
            .unwrap();
        assert_eq!(store.list_outbox().unwrap().len(), 1);
    }
}
