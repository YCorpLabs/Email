use mail_core::{Account, MailAction, MailStore, Message, Provider};

fn store() -> MailStore {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    store
}
fn message(store: &MailStore, id: &str, at: i64) -> Message {
    let mut message = store.get_message("demo:1").unwrap().unwrap();
    message.id = id.into();
    message.received_at = at;
    message
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn reply_closes_conditional_reminder_but_old_import_and_cached_future_mail_do_not() {
    let store = store();
    let future = message(&store, "cached-future-date", now() + 60_000);
    store.upsert_message(&future).unwrap();
    let follow = store.set_followup("demo:1", now() + 30_000, true).unwrap();
    let history = message(&store, "imported-history", follow.created_at_ms - 1);
    store.upsert_message(&history).unwrap();
    assert_eq!(store.list_followups(None).unwrap()[0].status, "waiting");
    let reply = message(&store, "new-reply", follow.created_at_ms + 1);
    store.upsert_message(&reply).unwrap();
    let result = &store.list_followups(None).unwrap()[0];
    assert_eq!(result.status, "replied");
    assert_eq!(result.reply_message_id.as_deref(), Some("new-reply"));
    // Removing the downloaded reply does not resurrect a terminal reminder.
    store.remove_message("demo:personal", "new-reply").unwrap();
    assert_eq!(store.list_followups(None).unwrap()[0].status, "replied");
}

#[test]
fn own_sends_drafts_and_aliases_do_not_count_as_incoming_replies() {
    let store = store();
    let mut alias_sent = message(&store, "known-alias", now() - 10_000);
    alias_sent.sender = "Alex at work <alex@work.example>".into();
    alias_sent.is_sent = true;
    store.upsert_message(&alias_sent).unwrap();
    let follow = store
        .set_followup_with_own_addresses(
            "demo:1",
            now() + 30_000,
            true,
            &["new@work.example".into()],
        )
        .unwrap();
    for (id, sender, sent, draft) in [
        ("sent-after", "new-unknown@work.example", true, false),
        ("draft-after", "maya@example.org", false, true),
        ("own-after", "Alex <alex@example.com>", false, false),
        ("alias-after", "Alex <alex@work.example>", false, false),
        (
            "verified-alias-after",
            "Alex <new@work.example>",
            false,
            false,
        ),
    ] {
        let mut m = message(&store, id, follow.created_at_ms + 1);
        m.sender = sender.into();
        m.is_sent = sent;
        m.is_draft = draft;
        store.upsert_message(&m).unwrap();
    }
    assert_eq!(store.list_followups(None).unwrap()[0].status, "waiting");
    let mut incoming = message(&store, "external-after", follow.created_at_ms + 2);
    incoming.sender = "Other Person <other@example.org>".into();
    store.upsert_message(&incoming).unwrap();
    assert_eq!(store.list_followups(None).unwrap()[0].status, "replied");
}

#[test]
fn unconditional_reminder_remains_waiting_after_reply_and_cancel_is_durable() {
    let store = store();
    let follow = store.set_followup("demo:1", now() + 30_000, false).unwrap();
    store
        .upsert_message(&message(&store, "new-reply", follow.created_at_ms + 1))
        .unwrap();
    assert_eq!(store.list_followups(None).unwrap()[0].status, "waiting");
    store.cancel_followup("demo:1").unwrap();
    assert_eq!(store.list_followups(None).unwrap()[0].status, "cancelled");
    let reset = store.set_followup("demo:1", now() + 40_000, true).unwrap();
    assert_eq!(reset.status, "waiting");
    assert_eq!(store.list_followups(None).unwrap()[0].status, "waiting");
}

#[test]
fn due_state_and_baseline_survive_store_reopen_without_background_process() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.sqlite");
    let follow;
    {
        let store = MailStore::open(&path).unwrap();
        store.seed_demo().unwrap();
        follow = store.set_followup("demo:1", now() + 30_000, true).unwrap();
    }
    // Simulate the persisted clock deadline passing, without timing-dependent sleeps.
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE followups SET due_at_ms=?1", [now() - 1])
        .unwrap();
    drop(db);
    let store = MailStore::open(&path).unwrap();
    let result = &store.list_followups(None).unwrap()[0];
    assert_eq!(result.status, "due");
    assert_eq!(store.unread_counts().unwrap()["followups"], serde_json::json!({"active":1,"due":1}));
    assert_eq!(result.created_at_ms, follow.created_at_ms);
    assert!(result.reply_message_id.is_none());
    // Reminder evaluation does not move, mark read or send anything.
    let thread = store
        .list_threads(None, 10, 0)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "demo:1")
        .unwrap();
    assert!(thread.is_inbox);
    assert_eq!(thread.unread_count, 1);
    assert!(store.pending_actions().unwrap().is_empty());
}

#[test]
fn account_scope_filters_before_results_and_unknown_account_errors() {
    let store = store();
    store
        .upsert_account(&Account {
            id: "other".into(),
            provider: Provider::Gmail,
            email: "other@example.com".into(),
            display_name: "Other".into(),
        })
        .unwrap();
    let mut foreign = message(&store, "foreign", now());
    foreign.account_id = "other".into();
    foreign.thread_id = "foreign-thread".into();
    store.upsert_message(&foreign).unwrap();
    store.set_followup("demo:1", now() + 30_000, true).unwrap();
    store
        .set_followup("foreign-thread", now() + 40_000, true)
        .unwrap();
    let own = store.list_followups(Some("demo:personal")).unwrap();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].thread_id, "demo:1");
    assert_eq!(store.list_followups(Some("other")).unwrap().len(), 1);
    assert!(store.list_followups(Some("unknown")).is_err());
}

#[test]
fn malformed_senders_and_untrusted_draft_alias_do_not_silently_dismiss_reminder() {
    let store = store();
    let follow = store.set_followup("demo:1", now() + 30_000, true).unwrap();
    for sender in [
        "not an address",
        "Victim <one@example.org>, Other <two@example.org>",
        "Injected\r\n <evil@example.org>",
    ] {
        let mut m = message(
            &store,
            &format!("bad-{}", sender.len()),
            follow.created_at_ms + 1,
        );
        m.sender = sender.into();
        store.upsert_message(&m).unwrap();
    }
    assert_eq!(store.list_followups(None).unwrap()[0].status, "waiting");
    assert!(
        store
            .set_followup_with_own_addresses("demo:1", now() + 30_000, true, &["invalid".into()])
            .is_err()
    );
    assert_eq!(store.list_followups(None).unwrap()[0].status, "waiting");
}

#[test]
fn invalid_time_unknown_thread_and_nonexistent_cancel_are_errors() {
    let store = store();
    for date in [0, now() - 1, i64::MAX] {
        assert!(store.set_followup("demo:1", date, true).is_err());
    }
    assert!(store.set_followup("missing", now() + 30_000, true).is_err());
    assert!(store.cancel_followup("missing").is_err());
    assert!(store.list_followups(None).unwrap().is_empty());
}

#[test]
fn active_followup_view_includes_archive_and_omits_cancelled_threads_in_due_order() {
    let store = store();
    store.set_followup("demo:1", now() + 60_000, true).unwrap();
    store.set_followup("demo:2", now() + 30_000, false).unwrap();
    store.set_followup("demo:3", now() + 20_000, false).unwrap();
    store.cancel_followup("demo:3").unwrap();
    store
        .apply_action(
            "archive-for-followup",
            MailAction::Archive {
                thread_id: "demo:2".into(),
            },
        )
        .unwrap();
    let threads = store.list_followup_threads(Some("demo:personal")).unwrap();
    assert_eq!(
        threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec!["demo:2", "demo:1"]
    );
    assert!(!threads[0].is_inbox);
    assert_eq!(threads[0].folder, "Archive");
}

#[test]
fn bounded_reply_evaluation_advances_and_late_import_does_not_fall_behind_a_cursor() {
    let store = store();
    let follow = store.set_followup("demo:1", now() + 30_000, true).unwrap();
    for n in 0..520 {
        let mut m = message(&store, &format!("own-{n:04}"), follow.created_at_ms + 2 + n);
        m.sender = "alex@example.com".into();
        store.upsert_message(&m).unwrap();
    }
    assert_eq!(store.list_followups(None).unwrap()[0].status, "waiting");
    let m = message(&store, "late-delivered-reply", follow.created_at_ms + 1);
    store.upsert_message(&m).unwrap();
    let result = &store.list_followups(None).unwrap()[0];
    assert_eq!(result.status, "replied");
    assert_eq!(
        result.reply_message_id.as_deref(),
        Some("late-delivered-reply")
    );
}

#[test]
fn inbox_counts_include_actionable_followups_after_reply_evaluation() {
    let store = store();
    assert_eq!(store.unread_counts().unwrap()["followups"], serde_json::json!({"active":0,"due":0}));
    let reminder = store.set_followup("demo:1", now()+30_000, true).unwrap();
    assert_eq!(store.unread_counts().unwrap()["followups"], serde_json::json!({"active":1,"due":0}));
    store.upsert_message(&message(&store, "count-reply", reminder.created_at_ms+1)).unwrap();
    store.list_followups(None).unwrap();
    assert_eq!(store.unread_counts().unwrap()["followups"], serde_json::json!({"active":0,"due":0}));
}
