use mail_core::MailStore;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn store() -> MailStore {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    store
}

#[test]
fn discarded_draft_leaves_drafts_and_search_and_restores_exactly() {
    let store = store();
    let draft = store
        .compose_draft_with_recipients(
            "demo:personal",
            Some("alias@example.org"),
            "a@example.org",
            Some("c@example.org"),
            Some("b@example.org"),
            "Quarterly zebra plan",
            "Unique body zebracorn",
        )
        .unwrap();
    store
        .add_draft_attachment(&draft.id, "plan.txt", "text/plain", b"attached")
        .unwrap();
    let tracking = store.prepare_email_tracking(&draft.id).unwrap();
    store
        .set_email_tracking(&draft.id, true, Some("host-1"))
        .unwrap();

    let discarded = store.discard_draft(&draft.id).unwrap();
    assert_eq!(discarded.message_id, draft.id);
    assert_eq!(discarded.subject, "Quarterly zebra plan");
    assert!(discarded.restorable_until_ms > now_ms());
    assert!(store.list_drafts(None).unwrap().is_empty());
    assert!(store.get_message(&draft.id).unwrap().is_none());
    assert!(store.search("zebracorn", 10).unwrap().is_empty());
    assert!(store.list_draft_attachments(&draft.id).unwrap().is_empty());
    // Editing or sending a discarded draft fails instead of resurrecting it.
    assert!(store.update_draft(&draft.id, "x@example.org", "x", "x").is_err());
    assert!(!store.claim_immediate_send(&draft.id).unwrap());

    let restored = store.restore_discarded_draft(&draft.id).unwrap();
    assert_eq!(restored.id, draft.id);
    assert_eq!(restored.thread_id, draft.thread_id);
    assert_eq!(restored.sender, "alias@example.org");
    assert_eq!(restored.to, draft.to);
    assert_eq!(restored.recipient_headers, draft.recipient_headers);
    assert_eq!(restored.body_plain, draft.body_plain);
    assert!(restored.is_draft && !restored.is_sent);
    assert_eq!(store.list_drafts(None).unwrap().len(), 1);
    assert_eq!(store.search("zebracorn", 10).unwrap().len(), 1);
    let files = store.list_draft_attachments(&draft.id).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].filename, "plan.txt");
    assert_eq!(store.outgoing_attachments(&draft.id).unwrap()[0].bytes, b"attached");
    let restored_tracking = store.email_tracking(&draft.id).unwrap().unwrap();
    assert_eq!(restored_tracking.tracking_id, tracking.tracking_id);
    assert!(restored_tracking.enabled);
    assert_eq!(restored_tracking.host_id.as_deref(), Some("host-1"));
    // A restore is single use; the restored draft can be edited normally.
    assert!(store.restore_discarded_draft(&draft.id).is_err());
    store
        .update_draft(&draft.id, "a@example.org", "Edited", "Body")
        .unwrap();
}

#[test]
fn discarding_a_reply_keeps_the_conversation() {
    let store = store();
    let thread = store.list_threads(None, 10, 0).unwrap()[0].clone();
    let before = store.list_thread_messages(&thread.id).unwrap();
    let draft = store
        .compose_draft("demo:personal", "a@example.org", "Re: thread", "Reply text")
        .unwrap();
    let reply = store.attach_draft_to_thread(&draft.id, &thread.id).unwrap();
    assert_eq!(reply.thread_id, thread.id);

    store.discard_draft(&draft.id).unwrap();
    let after = store.list_threads(None, 50, 0).unwrap();
    let kept = after.iter().find(|t| t.id == thread.id).expect("conversation remains");
    assert_eq!(kept.subject, thread.subject);
    assert_eq!(store.list_thread_messages(&thread.id).unwrap(), before);

    let restored = store.restore_discarded_draft(&draft.id).unwrap();
    assert_eq!(restored.thread_id, thread.id);
    assert_eq!(store.list_drafts(None).unwrap()[0].id, draft.id);
}

#[test]
fn queued_sent_and_unknown_drafts_cannot_be_discarded() {
    let store = store();
    let draft = store
        .compose_draft("demo:personal", "a@example.org", "Hello", "Body")
        .unwrap();
    store
        .schedule_draft(&draft.id, now_ms() + 60_000, "scheduled")
        .unwrap();
    let error = store.discard_draft(&draft.id).unwrap_err().to_string();
    assert!(error.contains("Cancel the scheduled send"), "{error}");
    store.cancel_scheduled_draft(&draft.id).unwrap();
    assert!(store.claim_immediate_send(&draft.id).unwrap());
    let error = store.discard_draft(&draft.id).unwrap_err().to_string();
    assert!(error.contains("being sent"), "{error}");
    store.mark_draft_sent(&draft.id).unwrap();
    assert!(store.discard_draft(&draft.id).is_err());
    assert!(store.discard_draft("local:missing").is_err());
    // Received mail is never a draft.
    let thread = store.list_threads(None, 1, 0).unwrap()[0].clone();
    let message = &store.list_thread_messages(&thread.id).unwrap()[0];
    assert!(store.discard_draft(&message.id).is_err());
    assert!(store.get_message(&message.id).unwrap().is_some());
}

#[test]
fn discarded_drafts_survive_reopen_until_restored() {
    let dir = std::env::temp_dir().join(format!("discard-{}", uuid_like()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mail.sqlite");
    let id = {
        let store = MailStore::open(&path).unwrap();
        store.seed_demo().unwrap();
        let draft = store
            .compose_draft("demo:personal", "a@example.org", "Keep", "Body")
            .unwrap();
        store.discard_draft(&draft.id).unwrap();
        draft.id
    };
    let store = MailStore::open(&path).unwrap();
    assert!(store.list_drafts(None).unwrap().is_empty());
    assert_eq!(store.restore_discarded_draft(&id).unwrap().subject, "Keep");
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

fn uuid_like() -> String {
    format!("{}-{}", std::process::id(), now_ms())
}
