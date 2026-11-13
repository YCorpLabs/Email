use mail_core::{MailAction, MailStore};
fn setup() -> MailStore {
    let s = MailStore::open_in_memory().unwrap();
    s.seed_demo().unwrap();
    s
}
fn ids(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!("demo:personal:gmail-message:{i:04x}"))
        .collect()
}
#[test]
fn preview_is_deduplicated_confirmed_bounded_and_restart_safe() {
    let path = std::env::temp_dir().join(format!("email-cleanup-{}.sqlite", uuid::Uuid::new_v4()));
    let s = MailStore::open(&path).unwrap();
    s.seed_demo().unwrap();
    let job = s
        .create_sender_cleanup("demo:personal", "Newsletter <news@example.com>")
        .unwrap();
    assert_eq!(job.sender, "news@example.com");
    assert!(s.confirm_sender_cleanup(&job.id).is_err());
    let first = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    assert!(s.claim_cleanup_step(&job.id).unwrap().is_none());
    let batch = ids(500);
    s.record_cleanup_page(&first, &batch, Some("next")).unwrap();
    assert!(s.record_cleanup_page(&first, &batch, Some("next")).is_err());
    let second = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    let preview = s.record_cleanup_page(&second, &batch, None).unwrap();
    assert_eq!(preview.matched, 500);
    assert_eq!(preview.state, "ready");
    assert!(s.claim_cleanup_step(&job.id).unwrap().is_none());
    drop(s);
    let s = MailStore::open(&path).unwrap();
    s.confirm_sender_cleanup(&job.id).unwrap();
    let apply = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    assert_eq!(apply.message_ids.len(), 500);
    let result = s.finish_cleanup_batch(&apply).unwrap();
    assert_eq!(result.completed, 500);
    assert_eq!(result.state, "completed");
    drop(s);
    let _ = std::fs::remove_file(path);
}
#[test]
fn pause_cancel_and_errors_do_not_lose_progress_or_bypass_confirmation() {
    let s = setup();
    let job = s
        .create_sender_cleanup("demo:personal", "news@example.com")
        .unwrap();
    let lease = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    s.control_sender_cleanup(&job.id, "pause").unwrap();
    let paused = s.record_cleanup_page(&lease, &ids(2), None).unwrap();
    assert_eq!(paused.state, "paused");
    assert_eq!(
        s.control_sender_cleanup(&job.id, "resume").unwrap().state,
        "ready"
    );
    s.confirm_sender_cleanup(&job.id).unwrap();
    let apply = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    let err = s
        .cleanup_step_failed(&apply, "rate limited", 60_000)
        .unwrap();
    assert!(err.retry_at_ms > 0);
    assert!(s.claim_cleanup_step(&job.id).unwrap().is_none());
    assert_eq!(
        s.control_sender_cleanup(&job.id, "cancel").unwrap().state,
        "cancelled"
    );
    assert!(s.control_sender_cleanup(&job.id, "resume").is_err());
    assert!(s.finish_cleanup_batch(&apply).is_err());
}
#[test]
fn rejects_query_injection_cross_account_ids_and_repeated_pages() {
    let s = setup();
    for sender in [
        "a@example.com OR is:unread",
        "a@example.com,b@example.com",
        "a@example.com\n",
    ] {
        assert!(s.create_sender_cleanup("demo:personal", sender).is_err());
    }
    let job = s
        .create_sender_cleanup("demo:personal", "a@example.com")
        .unwrap();
    let lease = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    assert!(
        s.record_cleanup_page(&lease, &["other:gmail-message:123".into()], None)
            .is_err()
    );
    s.record_cleanup_page(&lease, &ids(1), Some("same"))
        .unwrap();
    let lease = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    assert!(
        s.record_cleanup_page(&lease, &ids(1), Some("same"))
            .is_err()
    );
    assert_eq!(s.sender_cleanup(&job.id).unwrap().matched, 1);
}
#[test]
fn newer_user_actions_win_and_other_messages_in_thread_keep_unread() {
    let s = setup();
    let mut original = s.get_message("demo:1").unwrap().unwrap();
    original.id = ids(1)[0].clone();
    original.thread_id = "cleanup-thread".into();
    original.sender = "news@example.com".into();
    original.is_read = false;
    s.upsert_message(&original).unwrap();
    let mut other = original.clone();
    other.id = "demo:personal:gmail-message:other".into();
    other.sender = "person@example.com".into();
    s.upsert_message(&other).unwrap();
    let job = s
        .create_sender_cleanup("demo:personal", "news@example.com")
        .unwrap();
    let scan = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    s.record_cleanup_page(&scan, &[original.id.clone()], None)
        .unwrap();
    s.confirm_sender_cleanup(&job.id).unwrap();
    let apply = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    s.finish_cleanup_batch(&apply).unwrap();
    assert!(s.get_message(&original.id).unwrap().unwrap().is_read);
    assert!(!s.get_message(&other.id).unwrap().unwrap().is_read);
    let job = s
        .create_sender_cleanup("demo:personal", "news@example.com")
        .unwrap();
    let scan = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    s.record_cleanup_page(&scan, &[original.id.clone()], None)
        .unwrap();
    s.confirm_sender_cleanup(&job.id).unwrap();
    s.apply_action(
        "user-unread",
        MailAction::MarkRead {
            thread_id: original.thread_id.clone(),
            read: false,
        },
    )
    .unwrap();
    let lease = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    assert!(lease.message_ids.is_empty());
    let result = s.finish_cleanup_batch(&lease).unwrap();
    assert_eq!(result.skipped, 1);
    assert!(!s.get_message(&original.id).unwrap().unwrap().is_read);
}

#[test]
fn confirmed_concurrent_unread_is_requeued_after_remote_cleanup() {
    let s = setup();
    let mut message = s.get_message("demo:1").unwrap().unwrap();
    message.id = ids(1)[0].clone();
    message.thread_id = "cleanup-concurrent".into();
    message.is_read = false;
    s.upsert_message(&message).unwrap();
    let job = s
        .create_sender_cleanup("demo:personal", "news@example.com")
        .unwrap();
    let scan = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    s.record_cleanup_page(&scan, &[message.id.clone()], None)
        .unwrap();
    s.confirm_sender_cleanup(&job.id).unwrap();
    let apply = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    // User intent is delivered by another network worker while cleanup is in flight.
    s.apply_action(
        "explicit-unread",
        MailAction::MarkRead {
            thread_id: message.thread_id.clone(),
            read: false,
        },
    )
    .unwrap();
    s.confirm_action("explicit-unread").unwrap();
    s.apply_action(
        "explicit-label",
        MailAction::AddLabel {
            thread_id: message.thread_id.clone(),
            label: "Important".into(),
        },
    )
    .unwrap();
    s.confirm_action("explicit-label").unwrap();
    assert!(s.pending_actions().unwrap().is_empty());
    s.finish_cleanup_batch(&apply).unwrap();
    assert!(!s.get_message(&message.id).unwrap().unwrap().is_read);
    let queued = s.pending_actions().unwrap();
    assert_eq!(queued.len(), 1);
    assert!(
        matches!(&queued[0].action, MailAction::MarkRead { thread_id,read:false } if thread_id == &message.thread_id)
    );
    assert_ne!(queued[0].operation_id, "explicit-unread");
    // A duplicate completion cannot enqueue compensation twice.
    assert!(s.finish_cleanup_batch(&apply).is_err());
    assert_eq!(s.pending_actions().unwrap().len(), 1);
}

#[test]
fn stale_failure_cannot_clear_current_lease_or_install_backoff() {
    let s = setup();
    let job = s
        .create_sender_cleanup("demo:personal", "news@example.com")
        .unwrap();
    let lease = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    let mut stale = lease.clone();
    stale.token = "previous-worker".into();
    assert!(
        s.cleanup_step_failed(&stale, "stale failure", 60_000)
            .is_err()
    );
    assert_eq!(s.sender_cleanup(&job.id).unwrap().retry_at_ms, 0);
    assert!(s.sender_cleanup(&job.id).unwrap().last_error.is_none());
    s.record_cleanup_page(&lease, &[], None).unwrap();
}

#[test]
fn active_cleanup_remains_visible_after_many_completed_jobs() {
    let s = setup();
    let paused = s
        .create_sender_cleanup("demo:personal", "old@example.com")
        .unwrap();
    s.control_sender_cleanup(&paused.id, "pause").unwrap();
    for _ in 0..105 {
        let job = s
            .create_sender_cleanup("demo:personal", "new@example.com")
            .unwrap();
        let scan = s.claim_cleanup_step(&job.id).unwrap().unwrap();
        s.record_cleanup_page(&scan, &[], None).unwrap();
        s.confirm_sender_cleanup(&job.id).unwrap();
    }
    let listed = s.list_sender_cleanups(None).unwrap();
    assert_eq!(listed.len(), 100);
    assert_eq!(listed[0].id, paused.id);
}

#[test]
fn duplicate_provider_page_at_snapshot_cap_is_not_rejected() {
    let path =
        std::env::temp_dir().join(format!("email-cleanup-cap-{}.sqlite", uuid::Uuid::new_v4()));
    let s = MailStore::open(&path).unwrap();
    s.seed_demo().unwrap();
    let job = s
        .create_sender_cleanup("demo:personal", "news@example.com")
        .unwrap();
    // Populate the boundary efficiently; discovery normally reaches it over 200 pages.
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("WITH RECURSIVE n(v) AS (VALUES(0) UNION ALL SELECT v+1 FROM n WHERE v<99999) INSERT INTO sender_cleanup_items(job_id,message_id) SELECT ?1,'demo:personal:gmail-message:'||printf('%x',v) FROM n",[&job.id]).unwrap();
    drop(db);
    let scan = s.claim_cleanup_step(&job.id).unwrap().unwrap();
    let duplicate = "demo:personal:gmail-message:0".to_string();
    let ready = s
        .record_cleanup_page(&scan, &[duplicate.clone(), duplicate], None)
        .unwrap();
    assert_eq!(ready.matched, 100_000);
    assert_eq!(ready.state, "ready");
    drop(s);
    let _ = std::fs::remove_file(path);
}
