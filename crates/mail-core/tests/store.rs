use mail_core::{Account, MailAction, MailAdapter, MailStore, Provider, SyncBatch};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

// A separate process must not mistake another process's open cache for the
// last connection and unlink the live WAL. This catches POSIX lock loss when
// opening an additional service/bridge in the parent process.
#[cfg(unix)]
#[test]
fn wal_child_connection() {
    let Some(path) = std::env::var_os("EMAIL_WAL_CHILD_PATH") else {
        return;
    };
    let store = MailStore::open(path).unwrap();
    assert!(!store.list_accounts().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn opening_another_bridge_preserves_cross_process_wal_locks() {
    use std::os::unix::fs::MetadataExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    let first = MailStore::open(&path).unwrap();
    first.seed_demo().unwrap();
    let wal = directory.path().join("mail.sqlite-wal");
    let inode = std::fs::metadata(&wal).unwrap().ino();
    let second = MailStore::open(&path).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "wal_child_connection", "--nocapture"])
        .env("EMAIL_WAL_CHILD_PATH", &path)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        std::fs::metadata(&wal)
            .expect("a peer deleted our live WAL")
            .ino(),
        inode
    );
    first
        .upsert_account(&Account {
            id: "after-peer-close".into(),
            provider: Provider::Gmail,
            email: "test@example.org".into(),
            display_name: "Lock test".into(),
        })
        .unwrap();
    assert_eq!(second.list_accounts().unwrap().len(), 2);
}

#[test]
fn old_cache_gains_unsubscribe_header_without_losing_messages() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE messages(id TEXT NOT NULL UNIQUE,thread_id TEXT NOT NULL,account_id TEXT NOT NULL,sender TEXT NOT NULL,to_json TEXT NOT NULL,subject TEXT NOT NULL,body_plain TEXT NOT NULL,body_html TEXT,received_at INTEGER NOT NULL,is_read INTEGER NOT NULL,is_draft INTEGER NOT NULL,is_sent INTEGER NOT NULL);
        INSERT INTO messages VALUES('old-message','old-thread','old-account','Sender','[]','Subject','Body',NULL,123,1,0,0);") .unwrap();
    drop(db);
    let store = MailStore::open(&path).unwrap();
    let message = store.get_message("old-message").unwrap().unwrap();
    assert_eq!(message.body_plain, "Body");
    assert_eq!(message.unsubscribe_header, None);
    assert_eq!(message.has_attachments, None);
}

#[cfg(unix)]
#[test]
fn database_permissions_are_private_even_after_reopening_old_cache() {
    use std::os::unix::fs::PermissionsExt;
    let directory = std::env::temp_dir().join(format!(
        "email-core-permissions-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let path = directory.join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(store);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    drop(MailStore::open(&path).unwrap());
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_dir_all(directory).unwrap();
}

struct ResetAdapter;
impl MailAdapter for ResetAdapter {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }
    fn sync(&self, _account: &Account, _cursor: Option<&str>) -> mail_core::Result<SyncBatch> {
        Ok(SyncBatch {
            message_states: Vec::new(),
            messages: vec![],
            thread_labels: vec![],
            removed_message_ids: vec![],
            full_reset: true,
            reset_phase: None,
            next_cursor: "fresh".into(),
        })
    }
}

struct ResetWithMessageAdapter(mail_core::Message);
impl MailAdapter for ResetWithMessageAdapter {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }
    fn sync(&self, _account: &Account, _cursor: Option<&str>) -> mail_core::Result<SyncBatch> {
        Ok(SyncBatch {
            message_states: Vec::new(),
            messages: vec![self.0.clone()],
            thread_labels: vec![(self.0.thread_id.clone(), vec![])],
            removed_message_ids: vec![],
            full_reset: true,
            reset_phase: None,
            next_cursor: "reset-complete".into(),
        })
    }
}

struct LabeledAdapter {
    message: mail_core::Message,
    labels: Vec<String>,
}
impl MailAdapter for LabeledAdapter {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }
    fn sync(&self, _account: &Account, _cursor: Option<&str>) -> mail_core::Result<SyncBatch> {
        Ok(SyncBatch {
            message_states: Vec::new(),
            messages: vec![self.message.clone()],
            thread_labels: vec![(self.message.thread_id.clone(), self.labels.clone())],
            removed_message_ids: vec![],
            full_reset: false,
            reset_phase: None,
            next_cursor: "labeled".into(),
        })
    }
}

#[test]
fn provider_labels_replace_cached_labels_and_feed_saved_splits() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let message = store.get_message("demo:1").unwrap().unwrap();
    store
        .run_sync(
            &LabeledAdapter {
                message: message.clone(),
                labels: vec!["Clients".into()],
            },
            "demo:personal",
        )
        .unwrap();
    assert_eq!(store.list_labels().unwrap()[0].name, "Clients");
    let split = store
        .upsert_split(None, "Client mail", "label:Clients")
        .unwrap();
    assert_eq!(
        store
            .list_split_threads(&split.id, None, false, 10, 0)
            .unwrap()
            .len(),
        1
    );
    store
        .run_sync(
            &LabeledAdapter {
                message,
                labels: vec!["Projects".into()],
            },
            "demo:personal",
        )
        .unwrap();
    assert_eq!(store.list_labels().unwrap()[0].name, "Projects");
    assert!(
        store
            .list_split_threads(&split.id, None, false, 10, 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn stale_sync_cannot_undo_pending_or_recently_confirmed_changes() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let stale = store.get_message("demo:1").unwrap().unwrap();
    assert!(!stale.is_read);

    store
        .apply_action(
            "read-1",
            MailAction::MarkRead {
                thread_id: "demo:1".into(),
                read: true,
            },
        )
        .unwrap();
    store
        .apply_action(
            "label-1",
            MailAction::AddLabel {
                thread_id: "demo:1".into(),
                label: "Follow up".into(),
            },
        )
        .unwrap();
    store
        .run_sync(
            &LabeledAdapter {
                message: stale.clone(),
                labels: vec![],
            },
            "demo:personal",
        )
        .unwrap();
    assert!(store.get_message("demo:1").unwrap().unwrap().is_read);
    assert!(
        store
            .list_label_threads("Follow up", None, 10, 0)
            .unwrap()
            .iter()
            .any(|t| t.id == "demo:1")
    );

    store.confirm_action("read-1").unwrap();
    store.confirm_action("label-1").unwrap();
    store
        .run_sync(
            &LabeledAdapter {
                message: stale,
                labels: vec![],
            },
            "demo:personal",
        )
        .unwrap();
    assert!(store.get_message("demo:1").unwrap().unwrap().is_read);
    assert!(
        store
            .list_label_threads("Follow up", None, 10, 0)
            .unwrap()
            .iter()
            .any(|t| t.id == "demo:1")
    );
}

#[test]
fn confirmed_action_overlay_expires_and_provider_state_wins() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let stale = store.get_message("demo:1").unwrap().unwrap();
    store
        .apply_action(
            "read-expiring",
            MailAction::MarkRead {
                thread_id: "demo:1".into(),
                read: true,
            },
        )
        .unwrap();
    store.confirm_action("read-expiring").unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE actions SET confirmed_at_ms=?1 WHERE operation_id='read-expiring'",
        [now_ms() - 10 * 60 * 1000],
    )
    .unwrap();
    store
        .run_sync(
            &LabeledAdapter {
                message: stale,
                labels: vec![],
            },
            "demo:personal",
        )
        .unwrap();
    assert!(!store.get_message("demo:1").unwrap().unwrap().is_read);
}

#[test]
fn old_action_table_gains_indexed_thread_ids_without_losing_pending_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    {
        let store = MailStore::open(&path).unwrap();
        store.seed_demo().unwrap();
        store
            .apply_action(
                "old-read",
                MailAction::MarkRead {
                    thread_id: "demo:1".into(),
                    read: true,
                },
            )
            .unwrap();
    }
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "DROP INDEX actions_active_thread;
         CREATE TABLE old_actions(operation_id TEXT PRIMARY KEY,action_json TEXT NOT NULL,status TEXT NOT NULL);
         INSERT INTO old_actions SELECT operation_id,action_json,status FROM actions;
         DROP TABLE actions;
         ALTER TABLE old_actions RENAME TO actions;",
    ).unwrap();
    drop(db);

    let store = MailStore::open(&path).unwrap();
    let stale = {
        let mut message = store.get_message("demo:1").unwrap().unwrap();
        message.is_read = false;
        message
    };
    store
        .run_sync(
            &LabeledAdapter {
                message: stale,
                labels: vec![],
            },
            "demo:personal",
        )
        .unwrap();
    assert!(store.get_message("demo:1").unwrap().unwrap().is_read);
}

#[test]
fn reset_snapshot_preserves_recent_archive_and_read_action() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let stale = store.get_message("demo:1").unwrap().unwrap();
    store
        .apply_action(
            "archive-reset",
            MailAction::Archive {
                thread_id: "demo:1".into(),
            },
        )
        .unwrap();
    store
        .apply_action(
            "read-reset",
            MailAction::MarkRead {
                thread_id: "demo:1".into(),
                read: true,
            },
        )
        .unwrap();
    store.confirm_action("archive-reset").unwrap();
    store.confirm_action("read-reset").unwrap();
    store
        .run_sync(&ResetWithMessageAdapter(stale), "demo:personal")
        .unwrap();
    assert!(
        store
            .list_threads(None, 10, 0)
            .unwrap()
            .iter()
            .all(|thread| thread.id != "demo:1")
    );
    assert!(store.get_message("demo:1").unwrap().unwrap().is_read);
}

#[test]
fn label_mailbox_finds_archived_threads_and_filters_by_subject() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    store
        .apply_action(
            "label-design",
            MailAction::AddLabel {
                thread_id: "demo:1".into(),
                label: "Projects".into(),
            },
        )
        .unwrap();
    store
        .apply_action(
            "archive-design",
            MailAction::Archive {
                thread_id: "demo:1".into(),
            },
        )
        .unwrap();

    assert!(
        store
            .list_threads(None, 10, 0)
            .unwrap()
            .iter()
            .all(|thread| thread.id != "demo:1")
    );
    let labeled = store.list_label_threads("projects", None, 10, 0).unwrap();
    assert_eq!(labeled.len(), 1);
    assert_eq!(labeled[0].id, "demo:1");
    assert_eq!(
        store
            .list_label_threads("Projects", Some("design"), 10, 0)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .list_label_threads("Projects", Some("no match"), 10, 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn complete_provider_reset_removes_remote_cache_but_keeps_local_drafts() {
    let store = MailStore::open_in_memory().unwrap();
    let account = Account {
        id: "gmail:test".into(),
        provider: Provider::Gmail,
        email: "me@example.org".into(),
        display_name: "Me".into(),
    };
    store.upsert_account(&account).unwrap();
    let draft = store
        .compose_draft(&account.id, "friend@example.org", "Draft", "Still writing")
        .unwrap();
    let mut remote = draft.clone();
    remote.id = "gmail:test:gmail-message:old".into();
    remote.thread_id = "gmail:test:gmail-thread:old".into();
    remote.is_draft = false;
    remote.is_read = false;
    store.upsert_message(&remote).unwrap();
    store.run_sync(&ResetAdapter, &account.id).unwrap();
    assert!(store.get_message(&remote.id).unwrap().is_none());
    assert!(store.get_message(&draft.id).unwrap().is_some());
    assert_eq!(
        store.sync_cursor(&account.id).unwrap().unwrap().value,
        "fresh"
    );
}

#[test]
fn fixture_inbox_lists_and_searches() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    assert_eq!(store.list_accounts().unwrap().len(), 1);
    let inbox = store.list_threads(None, 10, 0).unwrap();
    assert_eq!(inbox.len(), 3);
    assert_eq!(inbox[0].subject, "Design review on Friday");
    assert_eq!(inbox[0].unread_count, 1);
    assert_eq!(store.search("mockups", 10).unwrap()[0].id, "demo:1");
    assert_eq!(store.search("design review", 10).unwrap()[0].id, "demo:1");
    assert!(store.search("unmatched:quote\"", 10).is_err());
    assert_eq!(
        store
            .get_message("demo:1")
            .unwrap()
            .unwrap()
            .body_plain
            .contains("mockups"),
        true
    );
}

#[test]
fn snooze_and_saved_splits_filter_before_pagination() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    store.snooze_thread("demo:1", now_ms() + 60_000).unwrap();
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 2);
    let snoozed = store.list_snoozed_threads().unwrap();
    assert_eq!(snoozed.len(), 1);
    assert_eq!(snoozed[0].id, "demo:1");
    assert!(snoozed[0].snoozed_until_ms.is_some());
    store.unsnooze_thread("demo:1").unwrap();
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 3);
    let split = store.upsert_split(None, "Design", "from:maya").unwrap();
    assert_eq!(store.list_splits().unwrap(), vec![split.clone()]);
    let matches = store
        .list_split_threads(&split.id, None, false, 1, 0)
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "demo:1");
    assert!(
        store
            .list_split_threads(&split.id, None, false, 1, 1)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_threads_filtered(None, true, 10, 0)
            .unwrap()
            .iter()
            .all(|t| t.unread_count > 0)
    );
    store.delete_split(&split.id).unwrap();
}

#[test]
fn scheduled_send_can_be_undone_and_claimed_once() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft("demo:personal", "a@example.org", "Hello", "Body")
        .unwrap();
    let scheduled = store
        .schedule_draft(&draft.id, now_ms() + 60_000, "scheduled")
        .unwrap();
    assert_eq!(scheduled.status, "scheduled");
    assert!(store.list_drafts(None).unwrap().is_empty());
    assert!(
        store
            .update_draft(&draft.id, "b@example.org", "Changed", "Body")
            .is_err()
    );
    store.cancel_scheduled_draft(&draft.id).unwrap();
    assert_eq!(store.list_drafts(None).unwrap().len(), 1);
    store
        .schedule_draft(&draft.id, now_ms() + 100, "undoable")
        .unwrap();
    assert!(!store.claim_due_outbox(&draft.id).unwrap());
    std::thread::sleep(std::time::Duration::from_millis(120));
    assert!(store.claim_due_outbox(&draft.id).unwrap());
    assert!(!store.claim_due_outbox(&draft.id).unwrap());
    store.fail_outbox(&draft.id, "offline").unwrap();
    assert_eq!(
        store.outbox_status(&draft.id).unwrap().as_deref(),
        Some("failed")
    );
    store.cancel_scheduled_draft(&draft.id).unwrap();
    assert_eq!(store.list_drafts(None).unwrap().len(), 1);
}

#[test]
fn draft_keeps_chosen_from_address_across_edits() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft_from(
            "demo:personal",
            Some("alias@example.org"),
            "friend@example.org",
            "Hello",
            "First",
        )
        .unwrap();
    assert_eq!(draft.sender, "alias@example.org");
    let edited = store
        .update_draft_from(
            &draft.id,
            Some("second@example.org"),
            "friend@example.org",
            "Hello",
            "Second",
        )
        .unwrap();
    assert_eq!(edited.sender, "second@example.org");
    assert_eq!(
        store.list_drafts(None).unwrap()[0].sender,
        "second@example.org"
    );
}

#[test]
fn draft_attachments_are_bounded_and_removed_after_send() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft("demo:personal", "a@example.org", "File", "See file")
        .unwrap();
    assert!(
        store
            .add_draft_attachment(&draft.id, "../secret", "text/plain", b"no")
            .is_err()
    );
    let meta = store
        .add_draft_attachment(&draft.id, "report.txt", "text/plain", b"hello")
        .unwrap();
    assert_eq!(meta.size, 5);
    assert_eq!(store.list_draft_attachments(&draft.id).unwrap(), vec![meta]);
    assert_eq!(
        store.outgoing_attachments(&draft.id).unwrap()[0].bytes,
        b"hello"
    );
    store.mark_draft_sent(&draft.id).unwrap();
    assert!(store.outgoing_attachments(&draft.id).unwrap().is_empty());
}

#[test]
fn actions_are_local_queued_and_idempotent() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let action = MailAction::MarkRead {
        thread_id: "demo:1".into(),
        read: true,
    };
    assert!(store.apply_action("op-1", action.clone()).unwrap().applied);
    assert!(!store.apply_action("op-1", action).unwrap().applied);
    assert_eq!(store.list_threads(None, 10, 0).unwrap()[0].unread_count, 0);
    store
        .apply_action(
            "op-2",
            MailAction::AddLabel {
                thread_id: "demo:1".into(),
                label: "Follow Up".into(),
            },
        )
        .unwrap();
    assert_eq!(
        store.list_threads(None, 10, 0).unwrap()[0].labels,
        vec!["Follow Up"]
    );
    store
        .apply_action(
            "op-3",
            MailAction::Archive {
                thread_id: "demo:1".into(),
            },
        )
        .unwrap();
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 2);
    assert_eq!(store.pending_actions().unwrap().len(), 3);
    store.confirm_action("op-1").unwrap();
    assert_eq!(store.pending_actions().unwrap().len(), 2);
}

#[test]
fn pinned_mail_remains_findable_after_archive_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    store.set_thread_pinned("demo:1", true).unwrap();
    store
        .apply_action(
            "archive-pinned",
            MailAction::Archive {
                thread_id: "demo:1".into(),
            },
        )
        .unwrap();
    assert_eq!(store.list_pinned_threads(10, 0).unwrap()[0].id, "demo:1");
    drop(store);
    let reopened = MailStore::open(&path).unwrap();
    assert!(reopened.list_pinned_threads(10, 0).unwrap()[0].is_pinned);
    reopened.set_thread_pinned("demo:1", false).unwrap();
    assert!(reopened.list_pinned_threads(10, 0).unwrap().is_empty());
}

#[test]
fn drafts_do_not_send_without_provider() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft("demo:personal", "sam@example.org", "Hi", "Hello")
        .unwrap();
    assert!(draft.is_draft);
    assert!(!draft.is_sent);
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 3);
    store
        .apply_action(
            "send-1",
            MailAction::SendDraft {
                message_id: draft.id.clone(),
            },
        )
        .unwrap();
    let after = store.get_message(&draft.id).unwrap().unwrap();
    assert!(after.is_draft);
    assert!(!after.is_sent);
    assert_eq!(store.pending_actions().unwrap().len(), 1);
}

#[test]
fn updating_message_replaces_search_index() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let mut message = store.get_message("demo:1").unwrap().unwrap();
    message.body_plain = "The revised prototype is ready".into();
    store.upsert_message(&message).unwrap();
    assert!(store.search("mockups", 10).unwrap().is_empty());
    assert_eq!(store.search("prototype", 10).unwrap().len(), 1);
}

#[test]
fn ids_cannot_cross_accounts() {
    use mail_core::{Account, Provider};
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    store
        .upsert_account(&Account {
            id: "demo:second".into(),
            provider: Provider::Microsoft,
            email: "other@example.net".into(),
            display_name: "Other".into(),
        })
        .unwrap();
    let mut message = store.get_message("demo:1").unwrap().unwrap();
    message.account_id = "demo:second".into();
    assert!(store.upsert_message(&message).is_err());
    assert_eq!(
        store.get_message("demo:1").unwrap().unwrap().account_id,
        "demo:personal"
    );
    let other = store.list_threads(Some("demo:second"), 10, 0).unwrap();
    assert!(other.is_empty());
}

#[test]
fn operation_ids_cannot_represent_two_actions() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    store
        .apply_action(
            "same",
            MailAction::Archive {
                thread_id: "demo:1".into(),
            },
        )
        .unwrap();
    assert!(
        store
            .apply_action(
                "same",
                MailAction::Archive {
                    thread_id: "demo:2".into()
                }
            )
            .is_err()
    );
}

#[test]
fn latest_message_uses_thread_not_message_id() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let mut message = store.get_message("demo:1").unwrap().unwrap();
    message.id = "demo:1:reply".into();
    message.received_at += 1;
    message.body_plain = "Latest reply".into();
    store.upsert_message(&message).unwrap();
    assert_eq!(store.list_thread_messages("demo:1").unwrap().len(), 2);
    assert_eq!(
        store
            .get_latest_message_for_thread("demo:1")
            .unwrap()
            .unwrap()
            .id,
        "demo:1:reply"
    );
}

#[test]
fn removing_a_message_cleans_search_and_respects_account() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    assert!(!store.remove_message("wrong-account", "demo:1").unwrap());
    assert_eq!(store.search("mockups", 10).unwrap().len(), 1);
    assert!(store.remove_message("demo:personal", "demo:1").unwrap());
    assert!(!store.remove_message("demo:personal", "demo:1").unwrap());
    assert!(store.search("mockups", 10).unwrap().is_empty());
    assert!(
        store
            .get_latest_message_for_thread("demo:1")
            .unwrap()
            .is_none()
    );
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 2);
}

#[test]
fn drafts_can_be_edited_and_marked_sent_after_delivery() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft("demo:personal", "a@example.org", "Old", "Old body")
        .unwrap();
    assert_eq!(store.list_drafts(Some("demo:personal")).unwrap().len(), 1);
    let changed = store
        .update_draft(&draft.id, "b@example.org", "New", "New body")
        .unwrap();
    assert_eq!(changed.to, vec!["b@example.org"]);
    assert_eq!(changed.subject, "New");
    store.mark_draft_sent(&draft.id).unwrap();
    assert!(store.list_drafts(None).unwrap().is_empty());
    let sent = store.get_message(&draft.id).unwrap().unwrap();
    assert!(sent.is_sent);
    assert!(!sent.is_draft);
    assert!(store.mark_draft_sent(&draft.id).is_err());
}

#[test]
fn removing_account_clears_cached_mail_and_queued_actions() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    store
        .apply_action(
            "archive-one",
            MailAction::Archive {
                thread_id: "demo:1".into(),
            },
        )
        .unwrap();
    store.remove_account("demo:personal").unwrap();
    assert!(store.list_accounts().unwrap().is_empty());
    assert!(store.list_threads(None, 10, 0).unwrap().is_empty());
    assert!(store.search("mockups", 10).unwrap().is_empty());
    assert!(store.pending_actions().unwrap().is_empty());
}

#[test]
fn immediate_send_claim_is_unique_across_connections() {
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.sqlite");
    let setup = MailStore::open(&path).unwrap();
    setup.seed_demo().unwrap();
    let draft = setup
        .compose_draft("demo:personal", "a@example.org", "Once", "Only once")
        .unwrap();
    drop(setup);
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let store = MailStore::open(&path).unwrap();
            let barrier = barrier.clone();
            let id = draft.id.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.claim_immediate_send(&id).unwrap()
            })
        })
        .collect();
    let claims = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(claims.iter().filter(|&&claimed| claimed).count(), 1);
    let check = MailStore::open(&path).unwrap();
    assert_eq!(
        check.outbox_status(&draft.id).unwrap().as_deref(),
        Some("sending")
    );
    assert!(
        check
            .update_draft(&draft.id, "b@example.org", "Late", "Too late")
            .is_err()
    );
}

#[test]
fn scheduling_and_editing_serialize_across_connections() {
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.sqlite");
    let setup = MailStore::open(&path).unwrap();
    setup.seed_demo().unwrap();
    let draft = setup
        .compose_draft(
            "demo:personal",
            "a@example.org",
            "Original",
            "Original body",
        )
        .unwrap();
    drop(setup);
    let barrier = Arc::new(Barrier::new(2));
    let edit = {
        let store = MailStore::open(&path).unwrap();
        let barrier = barrier.clone();
        let id = draft.id.clone();
        std::thread::spawn(move || {
            barrier.wait();
            store
                .update_draft(&id, "b@example.org", "Edited", "Edited body")
                .is_ok()
        })
    };
    let schedule = {
        let store = MailStore::open(&path).unwrap();
        let barrier = barrier.clone();
        let id = draft.id.clone();
        std::thread::spawn(move || {
            barrier.wait();
            store
                .schedule_draft(&id, now_ms() + 60_000, "scheduled")
                .is_ok()
        })
    };
    let edited = edit.join().unwrap();
    assert!(schedule.join().unwrap());
    let check = MailStore::open(&path).unwrap();
    let current = check.get_message(&draft.id).unwrap().unwrap();
    assert_eq!(current.subject, if edited { "Edited" } else { "Original" });
    assert!(
        check
            .update_draft(&draft.id, "c@example.org", "Too late", "No")
            .is_err()
    );
    assert_eq!(
        check.outbox_status(&draft.id).unwrap().as_deref(),
        Some("scheduled")
    );
}

struct InterruptedResetAdapter {
    first: mail_core::Message,
    second: mail_core::Message,
    fail_after_first: bool,
}
impl MailAdapter for InterruptedResetAdapter {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }
    fn sync(&self, _account: &Account, cursor: Option<&str>) -> mail_core::Result<SyncBatch> {
        match cursor {
            None => Ok(SyncBatch {
                message_states: Vec::new(),
                messages: vec![self.first.clone()],
                thread_labels: vec![],
                removed_message_ids: vec![],
                full_reset: false,
                reset_phase: Some(mail_core::ResetPhase::Start),
                next_cursor: "reset-page-2".into(),
            }),
            Some("reset-page-2") if self.fail_after_first => Err(mail_core::MailError::Provider(
                "temporary provider failure".into(),
            )),
            Some("reset-page-2") => Ok(SyncBatch {
                message_states: Vec::new(),
                messages: vec![self.second.clone()],
                thread_labels: vec![(self.second.thread_id.clone(), vec!["Work".into()])],
                removed_message_ids: vec![],
                full_reset: false,
                reset_phase: Some(mail_core::ResetPhase::Complete),
                next_cursor: "final-cursor".into(),
            }),
            other => panic!("unexpected cursor: {other:?}"),
        }
    }
}

#[test]
fn staged_reset_keeps_old_mail_until_complete_even_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft("demo:personal", "a@example.org", "Draft", "Keep me")
        .unwrap();
    let mut first = store.get_message("demo:1").unwrap().unwrap();
    first.id = "demo:new1".into();
    first.thread_id = first.id.clone();
    first.subject = "New one".into();
    let mut second = first.clone();
    second.id = "demo:new2".into();
    second.thread_id = second.id.clone();
    second.subject = "New two".into();
    let interrupted = InterruptedResetAdapter {
        first: first.clone(),
        second: second.clone(),
        fail_after_first: true,
    };
    assert!(store.run_sync(&interrupted, "demo:personal").is_err());
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 3);
    assert!(store.get_message("demo:1").unwrap().is_some());
    assert!(store.get_message(&first.id).unwrap().is_none());
    assert!(store.sync_cursor("demo:personal").unwrap().is_none());
    drop(store);

    let reopened = MailStore::open(&path).unwrap();
    let resumed = InterruptedResetAdapter {
        first: first.clone(),
        second: second.clone(),
        fail_after_first: false,
    };
    assert_eq!(reopened.run_sync(&resumed, "demo:personal").unwrap(), 1);
    assert!(reopened.get_message("demo:1").unwrap().is_none());
    assert!(reopened.get_message(&first.id).unwrap().is_some());
    assert!(reopened.get_message(&second.id).unwrap().is_some());
    assert!(reopened.get_message(&draft.id).unwrap().is_some());
    assert_eq!(
        reopened
            .sync_cursor("demo:personal")
            .unwrap()
            .unwrap()
            .value,
        "final-cursor"
    );
    assert_eq!(
        reopened
            .list_threads(None, 10, 0)
            .unwrap()
            .iter()
            .find(|t| t.id == second.id)
            .unwrap()
            .labels,
        vec!["Work"]
    );
}

struct SinglePageResetAdapter {
    message: mail_core::Message,
}
impl MailAdapter for SinglePageResetAdapter {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }
    fn sync(&self, _account: &Account, _cursor: Option<&str>) -> mail_core::Result<SyncBatch> {
        Ok(SyncBatch {
            message_states: Vec::new(),
            messages: vec![self.message.clone()],
            thread_labels: vec![],
            removed_message_ids: vec![],
            full_reset: false,
            reset_phase: Some(mail_core::ResetPhase::StartComplete),
            next_cursor: "fresh-single-page".into(),
        })
    }
}

#[test]
fn single_page_reset_discards_abandoned_staged_snapshot() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let mut stale = store.get_message("demo:1").unwrap().unwrap();
    stale.id = "demo:stale".into();
    stale.thread_id = stale.id.clone();
    let mut fresh = stale.clone();
    fresh.id = "demo:fresh".into();
    fresh.thread_id = fresh.id.clone();
    let interrupted = InterruptedResetAdapter {
        first: stale.clone(),
        second: fresh.clone(),
        fail_after_first: true,
    };
    assert!(store.run_sync(&interrupted, "demo:personal").is_err());
    assert!(store.get_message(&stale.id).unwrap().is_none());
    let replacement = SinglePageResetAdapter {
        message: fresh.clone(),
    };
    assert_eq!(store.run_sync(&replacement, "demo:personal").unwrap(), 1);
    assert!(store.get_message(&stale.id).unwrap().is_none());
    assert!(store.get_message(&fresh.id).unwrap().is_some());
    assert_eq!(
        store.sync_cursor("demo:personal").unwrap().unwrap().value,
        "fresh-single-page"
    );
}

struct TombstoneResetAdapter {
    message: mail_core::Message,
}
impl MailAdapter for TombstoneResetAdapter {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }
    fn sync(&self, _account: &Account, cursor: Option<&str>) -> mail_core::Result<SyncBatch> {
        match cursor {
            None => Ok(SyncBatch {
                message_states: Vec::new(),
                messages: vec![self.message.clone()],
                thread_labels: vec![(self.message.thread_id.clone(), vec!["Work".into()])],
                removed_message_ids: vec![],
                full_reset: false,
                reset_phase: Some(mail_core::ResetPhase::Start),
                next_cursor: "tombstone-page".into(),
            }),
            Some("tombstone-page") => Ok(SyncBatch {
                message_states: Vec::new(),
                messages: vec![],
                thread_labels: vec![],
                removed_message_ids: vec![self.message.id.clone()],
                full_reset: false,
                reset_phase: Some(mail_core::ResetPhase::Complete),
                next_cursor: "tombstone-final".into(),
            }),
            other => panic!("unexpected cursor: {other:?}"),
        }
    }
}

#[test]
fn reset_tombstone_removes_staged_message_and_orphan_labels() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let mut vanished = store.get_message("demo:1").unwrap().unwrap();
    vanished.id = "demo:vanished".into();
    vanished.thread_id = vanished.id.clone();
    assert_eq!(
        store
            .run_sync(
                &TombstoneResetAdapter {
                    message: vanished.clone()
                },
                "demo:personal"
            )
            .unwrap(),
        1
    );
    assert!(store.get_message(&vanished.id).unwrap().is_none());
    assert!(store.list_threads(None, 10, 0).unwrap().is_empty());
    assert_eq!(
        store.sync_cursor("demo:personal").unwrap().unwrap().value,
        "tombstone-final"
    );
}

#[test]
fn split_wildcards_match_addresses_recipients_and_alternatives_without_sql_wildcards() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let matches = store
        .preview_split("*@example.org, *@example.net", 10)
        .unwrap();
    assert_eq!(matches.len(), 2);
    assert!(
        store
            .preview_split("*@evil-example.org", 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .preview_split("from:*%@example.org", 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .preview_split("to:alex@example.com", 10)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(store.preview_split("subject:Friday", 10).unwrap().len(), 1);
    let split = store
        .upsert_split(None, "People", "*@example.org, *@example.net")
        .unwrap();
    assert_eq!(
        store
            .list_split_threads(&split.id, None, false, 10, 0)
            .unwrap()
            .len(),
        2
    );
    assert!(
        store
            .upsert_split(None, "Duplicate", "*@EXAMPLE.ORG, *@example.net")
            .is_err()
    );
    assert!(store.preview_split("from:", 10).is_err());
    assert!(store.preview_split("unknown:value", 10).is_err());
}

#[test]
fn archived_pages_survive_inbox_reset_and_restore_optimistically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("archive.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let message = store.get_message("demo:1").unwrap().unwrap();
    let batch = SyncBatch {
        message_states: Vec::new(),
        messages: vec![message.clone()],
        thread_labels: vec![(message.thread_id.clone(), vec!["__EMAIL_ARCHIVE__".into()])],
        removed_message_ids: vec![],
        next_cursor: "unused-folder-cursor".into(),
        full_reset: false,
        reset_phase: None,
    };
    store
        .cache_archive_page("demo:personal", &batch, Some("next-page"))
        .unwrap();
    assert_eq!(store.list_archived_threads(10, 0).unwrap().len(), 1);
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 2);
    assert!(
        !store
            .list_labels()
            .unwrap()
            .iter()
            .any(|label| label.name == "__EMAIL_ARCHIVE__")
    );
    store.run_sync(&ResetAdapter, "demo:personal").unwrap();
    assert!(store.get_message(&message.id).unwrap().is_some());
    assert_eq!(store.list_archived_threads(10, 0).unwrap().len(), 1);
    drop(store);
    let store = MailStore::open(&path).unwrap();
    assert_eq!(
        store.archive_page_cursor("demo:personal").unwrap(),
        (Some("next-page".into()), false)
    );
    store
        .apply_action(
            "restore-archived",
            MailAction::Move {
                thread_id: message.thread_id.clone(),
                folder: "Inbox".into(),
            },
        )
        .unwrap();
    assert!(store.list_archived_threads(10, 0).unwrap().is_empty());
    assert!(
        store
            .list_threads(None, 10, 0)
            .unwrap()
            .iter()
            .any(|thread| thread.id == message.thread_id)
    );
    // A stale archive page must not undo a pending restore or touch the live cursor.
    let cursor = store.sync_cursor("demo:personal").unwrap();
    store
        .cache_archive_page("demo:personal", &batch, None)
        .unwrap();
    assert!(store.list_archived_threads(10, 0).unwrap().is_empty());
    assert_eq!(
        store.sync_cursor("demo:personal").unwrap().map(|c| c.value),
        cursor.map(|c| c.value)
    );
}

#[test]
fn recipient_header_backfill_persists_without_changing_body_or_read_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("headers.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let thread = store.list_threads(None, 10, 0).unwrap().remove(0);
    let before = store
        .get_latest_message_for_thread(&thread.id)
        .unwrap()
        .unwrap();
    let headers = mail_core::RecipientHeaders {
        cc: vec!["cc@example.com".into()],
        bcc: vec![],
        reply_to: vec!["reply@example.com".into()],
    };
    store
        .update_recipient_headers(&before.id, &["alias@example.com".into()], &headers)
        .unwrap();
    drop(store);
    let store = MailStore::open(&path).unwrap();
    let after = store.get_message(&before.id).unwrap().unwrap();
    assert_eq!(after.to, vec!["alias@example.com"]);
    assert_eq!(after.recipient_headers, Some(headers));
    assert_eq!(after.body_plain, before.body_plain);
    assert_eq!(after.is_read, before.is_read);
    assert_eq!(after.received_at, before.received_at);
}

#[test]
fn sender_cleanup_covers_whole_cache_and_is_atomic_and_retryable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let mut db = rusqlite::Connection::open(&path).unwrap();
    let tx = db.transaction().unwrap();
    for n in 0..4001 {
        // Display-name differences must match; lookalike addresses must not.
        let sender = if n == 4000 {
            "News <news@other.example>"
        } else if n % 2 == 0 {
            "News <NEWS@example.org>"
        } else {
            "news@example.org"
        };
        let id = format!("bulk-test:{n}");
        tx.execute("INSERT INTO threads VALUES(?1,'demo:personal','Newsletter',?2,'Preview',100,1,1,'Inbox')",rusqlite::params![id,sender]).unwrap();
        tx.execute("INSERT INTO messages(id,thread_id,account_id,sender,to_json,subject,body_plain,received_at,is_read,is_draft,is_sent) VALUES(?1,?1,'demo:personal',?2,'[]','Newsletter','Body',100,0,0,0)",rusqlite::params![id,sender]).unwrap();
    }
    tx.commit().unwrap();
    let before = store.unread_counts().unwrap()["total"].as_u64().unwrap();
    let ids = store
        .unread_from_sender("demo:personal", "Newsletter <news@example.org>")
        .unwrap();
    assert_eq!(ids.len(), 4000);
    assert!(
        store
            .bulk_mark_read("bulk:invalid", "different-account", &ids)
            .is_err()
    );
    assert_eq!(
        store.unread_counts().unwrap()["total"].as_u64(),
        Some(before)
    );
    assert!(store.pending_actions().unwrap().is_empty());
    assert_eq!(
        store
            .bulk_mark_read("bulk:test", "demo:personal", &ids)
            .unwrap(),
        4000
    );
    assert_eq!(
        store
            .bulk_mark_read("bulk:test", "demo:personal", &ids)
            .unwrap(),
        0
    );
    assert_eq!(store.pending_actions().unwrap().len(), 4000);
    assert_eq!(
        store.unread_counts().unwrap()["total"].as_u64(),
        Some(before - 4000)
    );
    assert_eq!(
        store
            .unread_from_sender("demo:personal", "news@other.example")
            .unwrap()
            .len(),
        1
    );
    drop(store);
    let reopened = MailStore::open(&path).unwrap();
    assert_eq!(reopened.pending_actions().unwrap().len(), 4000);
    assert!(
        reopened
            .unread_from_sender("demo:personal", "news@example.org")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cached_reads_do_not_wait_for_a_stalled_network_sync() {
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;
    struct PausedAdapter {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl MailAdapter for PausedAdapter {
        fn provider(&self) -> Provider {
            Provider::Gmail
        }
        fn sync(&self, _: &Account, _: Option<&str>) -> mail_core::Result<SyncBatch> {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(SyncBatch {
                message_states: Vec::new(),
                messages: vec![],
                thread_labels: vec![],
                removed_message_ids: vec![],
                full_reset: false,
                reset_phase: None,
                next_cursor: "after-stall".into(),
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(MailStore::open(dir.path().join("mail.sqlite")).unwrap());
    store.seed_demo().unwrap();
    let id = store.list_threads(None, 1, 0).unwrap()[0].id.clone();
    let (entered, ready) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let sync_store = store.clone();
    let sync = std::thread::spawn(move || {
        sync_store.run_sync(
            &PausedAdapter {
                entered,
                release: Mutex::new(blocked),
            },
            "demo:personal",
        )
    });
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let (done, reads) = mpsc::channel();
    let reader = store.clone();
    let read = std::thread::spawn(move || {
        let messages = reader.list_thread_messages(&id).unwrap();
        let counts = reader.unread_counts().unwrap();
        done.send((messages.len(), counts)).unwrap();
    });
    let result = reads.recv_timeout(Duration::from_millis(500));
    release.send(()).unwrap();
    sync.join().unwrap().unwrap();
    read.join().unwrap();
    assert!(result.is_ok(), "local reads waited for the provider");
    assert!(result.unwrap().0 > 0);
}

#[test]
fn inbox_search_stays_archived_after_refresh_and_restore() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    assert_eq!(
        store
            .search_in_folder("mockups", 1, Some(true))
            .unwrap()
            .len(),
        1
    );
    store
        .apply_action(
            "archive-search",
            MailAction::Archive {
                thread_id: "demo:1".into(),
            },
        )
        .unwrap();
    // Re-query as both native UIs do immediately after E.
    assert!(
        store
            .search_in_folder("mockups", 1, Some(true))
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.search("mockups", 1).unwrap().len(), 1);
    assert_eq!(
        store
            .search_in_folder("mockups", 1, Some(false))
            .unwrap()
            .len(),
        1
    );
    store
        .apply_action(
            "restore-search",
            MailAction::Move {
                thread_id: "demo:1".into(),
                folder: "Inbox".into(),
            },
        )
        .unwrap();
    assert_eq!(
        store
            .search_in_folder("mockups", 1, Some(true))
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .search_in_folder("mockups", 1, Some(false))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn triage_selection_is_atomic_persistent_and_undoable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("triage.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let ids = vec!["demo:1".into(), "demo:2".into(), "demo:1".into()];
    assert_eq!(
        store.batch_triage("selection-1", "archive", &ids).unwrap()["changed"],
        2
    );
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 1);
    assert_eq!(
        store.batch_triage("selection-1", "archive", &ids).unwrap()["changed"],
        0
    );
    assert!(store.batch_triage("selection-1", "read", &ids).is_err());
    assert_eq!(store.pending_actions().unwrap().len(), 2);
    drop(store);
    let store = MailStore::open(&path).unwrap();
    assert_eq!(store.undo_triage("selection-1").unwrap()["changed"], 2);
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 3);
    assert_eq!(store.pending_actions().unwrap().len(), 4);
    assert_eq!(store.undo_triage("selection-1").unwrap()["changed"], 0);
}

#[test]
fn triage_rejects_partial_selections_and_stale_undo() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let ids = vec!["demo:1".into(), "missing".into()];
    assert!(store.batch_triage("bad", "archive", &ids).is_err());
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 3);
    assert!(store.pending_actions().unwrap().is_empty());
    store
        .batch_triage("good", "archive", &["demo:1".into(), "demo:2".into()])
        .unwrap();
    store
        .apply_action(
            "newer",
            MailAction::Move {
                thread_id: "demo:2".into(),
                folder: "Spam".into(),
            },
        )
        .unwrap();
    assert!(store.undo_triage("good").is_err());
    assert_eq!(store.list_archived_threads(10, 0).unwrap().len(), 1);
    // A failed undo cannot restore even its otherwise unchanged first member.
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 1);
}

#[test]
fn triage_read_preserves_placement_and_undo_preserves_original_folders() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    store
        .apply_action(
            "setup",
            MailAction::Move {
                thread_id: "demo:2".into(),
                folder: "Archive".into(),
            },
        )
        .unwrap();
    let ids = vec!["demo:1".into(), "demo:2".into()];
    store.batch_triage("read", "read", &ids).unwrap();
    assert_eq!(store.unread_counts().unwrap()["total"], 1); // Studio Notes remains unread
    assert!(store.undo_triage("read").is_err());
    store.batch_triage("spam", "spam", &ids).unwrap();
    store.undo_triage("spam").unwrap();
    assert!(
        store
            .list_threads(None, 10, 0)
            .unwrap()
            .iter()
            .any(|t| t.id == "demo:1")
    );
    assert!(
        store
            .list_archived_threads(10, 0)
            .unwrap()
            .iter()
            .any(|t| t.id == "demo:2")
    );
}

#[test]
fn latest_triage_undo_survives_restart_and_skips_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("undo-history.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    assert!(store.latest_triage_undo().unwrap().is_none());
    store
        .batch_triage("first", "archive", &["demo:1".into()])
        .unwrap();
    store
        .batch_triage("second", "archive", &["demo:2".into()])
        .unwrap();
    drop(store);
    let store = MailStore::open(&path).unwrap();
    let undo = store.latest_triage_undo().unwrap().unwrap();
    assert_eq!(undo["operation_id"], "second");
    assert_eq!(undo["count"], 1);
    assert_eq!(undo["account_ids"][0], "demo:personal");
    store.undo_triage("second").unwrap();
    assert_eq!(
        store.latest_triage_undo().unwrap().unwrap()["operation_id"],
        "first"
    );
    store
        .apply_action(
            "newer-read",
            MailAction::MarkRead {
                thread_id: "demo:1".into(),
                read: true,
            },
        )
        .unwrap();
    assert!(store.latest_triage_undo().unwrap().is_none());
}

#[test]
fn queue_routing_reads_only_ids_and_account_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    store
        .apply_action(
            "routing-archive",
            MailAction::Archive {
                thread_id: "demo:1".into(),
            },
        )
        .unwrap();
    // Deliberately invalid payload fields make full Message decoding fail. Routing
    // and ID enumeration must still work because they never select those fields.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE messages SET to_json='not JSON',body_plain=zeroblob(1000000) WHERE id='demo:1'",
            [],
        )
        .unwrap();
    assert!(store.get_message("demo:1").is_err());
    assert_eq!(
        store.thread_account_id("demo:1").unwrap().as_deref(),
        Some("demo:personal")
    );
    assert_eq!(
        store.message_account_id("demo:1").unwrap().as_deref(),
        Some("demo:personal")
    );
    assert_eq!(store.thread_message_ids("demo:1").unwrap(), vec!["demo:1"]);
    assert_eq!(
        store
            .pending_action_count_for_account("demo:personal")
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .pending_actions_for_account("demo:personal", 1)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .pending_actions_for_account("other-account", 10)
            .unwrap()
            .is_empty()
    );
}


#[test]
fn sent_reply_stays_with_incoming_history_and_preserves_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let parent = store.get_message("demo:1").unwrap().unwrap();
    let draft = store.compose_draft(&parent.account_id, "friend@example.org", "Re: Design review", "Reply").unwrap();
    store.attach_draft_to_thread(&draft.id, &parent.thread_id).unwrap();
    assert_eq!(store.list_thread_messages(&parent.thread_id).unwrap().len(), 1, "unsent draft must not appear in history");
    store.mark_draft_sent(&draft.id).unwrap();
    let rows = store.list_thread_messages(&parent.thread_id).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|m| m.is_sent));
    let db = rusqlite::Connection::open(&path).unwrap();
    let inbox: bool = db.query_row("SELECT is_inbox FROM threads WHERE id=?1", [&parent.thread_id], |r| r.get(0)).unwrap();
    assert!(inbox);
}

#[test]
fn reply_cannot_attach_to_another_account() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    store.upsert_account(&Account {id:"other".into(), provider:Provider::Gmail, email:"other@example.org".into(),display_name:"Other".into()}).unwrap();
    let draft = store.compose_draft("other", "friend@example.org", "Re: Design review", "Reply").unwrap();
    assert!(store.attach_draft_to_thread(&draft.id, "demo:1").is_err());
    assert_eq!(store.get_message(&draft.id).unwrap().unwrap().thread_id,draft.thread_id);
}

#[test]
fn recovered_sent_reply_reads_original_history_without_subject_merging() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let parent = store.get_message("demo:1").unwrap().unwrap();
    let draft = store.compose_draft(&parent.account_id, "friend@example.org", "Re: Design review", "Reply").unwrap();
    store.mark_draft_sent(&draft.id).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("INSERT INTO local_conversation_links VALUES(?1,?2)", [&draft.thread_id,&parent.thread_id]).unwrap();
    assert_eq!(store.list_thread_messages(&draft.thread_id).unwrap().len(), 2);
    assert_eq!(store.list_thread_messages(&parent.thread_id).unwrap().len(), 2);
    let next = store.compose_draft(&parent.account_id, "friend@example.org", "Re: Design review", "Another reply").unwrap();
    assert_eq!(store.attach_draft_to_thread(&next.id, &draft.thread_id).unwrap().thread_id, parent.thread_id);
}

#[test]
fn failed_legacy_upgrade_rolls_back_schema_and_keeps_drafts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft("demo:personal", "a@example.org", "Unsent", "Keep me")
        .unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("DROP INDEX actions_active_thread;
        ALTER TABLE actions DROP COLUMN thread_id;
        ALTER TABLE actions DROP COLUMN confirmed_at_ms;
        ALTER TABLE messages DROP COLUMN unsubscribe_header;
        ALTER TABLE messages DROP COLUMN has_attachments;
        ALTER TABLE messages DROP COLUMN recipient_headers_json;
        INSERT INTO actions(operation_id,action_json,status) VALUES('broken','not json','pending');").unwrap();
    drop(connection);

    assert!(MailStore::open(&path).is_err());
    let connection = rusqlite::Connection::open(&path).unwrap();
    for (table, absent) in [
        ("messages", "unsubscribe_header"),
        ("messages", "has_attachments"),
        ("messages", "recipient_headers_json"),
        ("actions", "confirmed_at_ms"),
        ("actions", "thread_id"),
    ] {
        let columns = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            !columns.iter().any(|column| column == absent),
            "partial upgrade retained {table}.{absent}"
        );
    }
    assert_eq!(
        connection
            .query_row(
                "SELECT body_plain FROM messages WHERE id=?1",
                [&draft.id],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "Keep me"
    );
    connection
        .execute("DELETE FROM actions WHERE operation_id='broken'", [])
        .unwrap();
    drop(connection);
    let upgraded = MailStore::open(&path).unwrap();
    assert_eq!(upgraded.list_drafts(None).unwrap()[0].id, draft.id);
}

#[test]
fn reopening_preserves_draft_attachments_and_never_requeues_ambiguous_send() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft("demo:personal", "a@example.org", "Unsent", "Keep me")
        .unwrap();
    let attachment = store
        .add_draft_attachment(&draft.id, "note.txt", "text/plain", b"draft bytes")
        .unwrap();
    let ambiguous = store
        .compose_draft(
            "demo:personal",
            "b@example.org",
            "Sending",
            "May have been accepted",
        )
        .unwrap();
    assert!(store.claim_immediate_send(&ambiguous.id).unwrap());
    drop(store);

    let reopened = MailStore::open(&path).unwrap();
    assert_eq!(reopened.list_drafts(None).unwrap()[0].id, draft.id);
    assert_eq!(
        reopened.list_draft_attachments(&draft.id).unwrap(),
        vec![attachment]
    );
    assert_eq!(
        reopened.outgoing_attachments(&draft.id).unwrap()[0].bytes,
        b"draft bytes"
    );
    assert_eq!(
        reopened.outbox_status(&ambiguous.id).unwrap().as_deref(),
        Some("sending")
    );
    assert!(!reopened.claim_immediate_send(&ambiguous.id).unwrap());
    assert!(!reopened.due_outbox(100).unwrap().contains(&ambiguous.id));
    assert!(reopened.cancel_scheduled_draft(&ambiguous.id).is_err());
}

// Exit without destructors so recovery must read the WAL, rather than a clean
// connection close checkpoint. Only synthetic data is written by this child.
#[test]
fn interrupted_writer_child() {
    let Some(path) = std::env::var_os("EMAIL_RECOVERY_CHILD_PATH") else {
        return;
    };
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft(
            "demo:personal",
            "a@example.org",
            "Recovered draft",
            "Committed body",
        )
        .unwrap();
    store
        .add_draft_attachment(&draft.id, "note.txt", "text/plain", b"committed attachment")
        .unwrap();
    let queued = store
        .compose_draft(
            "demo:personal",
            "b@example.org",
            "Recovered queue",
            "Queued body",
        )
        .unwrap();
    store
        .schedule_draft(&queued.id, now_ms() + 3_600_000, "scheduled")
        .unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    connection
        .execute(
            "UPDATE messages SET body_plain='Uncommitted overwrite' WHERE id=?1",
            [&draft.id],
        )
        .unwrap();
    std::process::exit(0);
}

#[test]
fn interrupted_writer_recovers_committed_draft_and_queue_only() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "interrupted_writer_child", "--nocapture"])
        .env("EMAIL_RECOVERY_CHILD_PATH", &path)
        .status()
        .unwrap();
    assert!(result.success());
    let recovered = MailStore::open(&path).unwrap();
    let drafts = recovered.list_drafts(None).unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].subject, "Recovered draft");
    assert_eq!(drafts[0].body_plain, "Committed body");
    assert_eq!(
        recovered.outgoing_attachments(&drafts[0].id).unwrap()[0].bytes,
        b"committed attachment"
    );
    let queue = recovered.list_outbox().unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].subject, "Recovered queue");
    assert_eq!(queue[0].status, "scheduled");
    assert!(recovered.due_outbox(100).unwrap().is_empty());
}
fn removal_fixture() -> (tempfile::TempDir, std::path::PathBuf, MailStore) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    store
        .upsert_account(&Account {
            id: "kept-account".into(),
            provider: Provider::Gmail,
            email: "kept@example.invalid".into(),
            display_name: "Kept".into(),
        })
        .unwrap();
    let template = store.get_message("demo:1").unwrap().unwrap();
    for (id, account, body) in [
        (
            "removed-sensitive",
            "demo:personal",
            "removed-sensitive-body",
        ),
        ("kept-sensitive", "kept-account", "kept-sensitive-body"),
        ("kept-secondary", "kept-account", "kept-secondary-body"),
    ] {
        let mut message = template.clone();
        message.id = format!("{id}-message");
        message.thread_id = format!("{id}-thread");
        message.account_id = account.into();
        message.subject = body.into();
        message.body_plain = body.into();
        store.upsert_message(&message).unwrap();
    }
    for thread in ["removed-sensitive-thread", "kept-sensitive-thread"] {
        store.set_thread_pinned(thread, true).unwrap();
    }
    store
        .batch_triage(
            "removed-only-batch",
            "archive",
            &["removed-sensitive-thread".into()],
        )
        .unwrap();
    store
        .batch_triage(
            "mixed-batch",
            "archive",
            &[
                "removed-sensitive-thread".into(),
                "kept-sensitive-thread".into(),
            ],
        )
        .unwrap();
    store
        .upsert_snippet(None, "Shared snippet", "Shared text across accounts")
        .unwrap();
    store
        .upsert_split(None, "Shared split", "from:shared@example.invalid")
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("INSERT INTO local_conversation_links VALUES('removed-sensitive-thread','kept-secondary-thread')",[]).unwrap();
    db.execute("INSERT INTO local_conversation_links VALUES('kept-sensitive-thread','kept-secondary-thread')",[]).unwrap();
    db.execute("INSERT INTO threads SELECT 'removed-empty-thread',account_id,subject,sender,snippet,last_message_at,unread_count,is_inbox,folder FROM threads WHERE id='removed-sensitive-thread'",[]).unwrap();
    drop(db);
    store
        .set_thread_pinned("removed-empty-thread", true)
        .unwrap();
    store
        .apply_action(
            "empty-thread-action",
            MailAction::Archive {
                thread_id: "removed-empty-thread".into(),
            },
        )
        .unwrap();
    (directory, path, store)
}

#[test]
fn account_removal_clears_scoped_remnants_and_preserves_shared_and_other_account_data() {
    let (_directory, path, store) = removal_fixture();
    store.remove_account("demo:personal").unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    for query in [
        "SELECT COUNT(*) FROM pinned_threads WHERE thread_id LIKE 'removed-%'",
        "SELECT COUNT(*) FROM local_conversation_links WHERE source_thread LIKE 'removed-%' OR target_thread LIKE 'removed-%'",
        "SELECT COUNT(*) FROM triage_batches WHERE request_json LIKE '%removed-sensitive%' OR inverse_json LIKE '%removed-sensitive%'",
        "SELECT COUNT(*) FROM actions WHERE action_json LIKE '%removed-%'",
        "SELECT COUNT(*) FROM accounts WHERE id='demo:personal'",
        "SELECT COUNT(*) FROM messages WHERE account_id='demo:personal'",
        "SELECT COUNT(*) FROM threads WHERE account_id='demo:personal'",
    ] {
        assert_eq!(
            db.query_row(query, [], |row| row.get::<_, i64>(0)).unwrap(),
            0,
            "{query}"
        );
    }
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM triage_batches WHERE operation_id='removed-only-batch'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(db.query_row("SELECT COUNT(*) FROM local_conversation_links WHERE source_thread='kept-sensitive-thread' AND target_thread='kept-secondary-thread'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert!(
        store
            .get_message("kept-sensitive-message")
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .get_message("kept-secondary-message")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store.list_pinned_threads(10, 0).unwrap()[0].id,
        "kept-sensitive-thread"
    );
    assert_eq!(
        store.list_snippets().unwrap()[0].body,
        "Shared text across accounts"
    );
    assert_eq!(store.list_splits().unwrap()[0].name, "Shared split");
    assert_eq!(store.latest_triage_undo().unwrap().unwrap()["count"], 1);
    assert_eq!(store.undo_triage("mixed-batch").unwrap()["changed"], 1);
    assert!(store.search("removed-sensitive", 10).unwrap().is_empty());
}

#[test]
fn account_removal_rolls_back_remnant_cleanup_and_mixed_undo_if_account_delete_fails() {
    let (_directory, path, store) = removal_fixture();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_account_removal BEFORE DELETE ON accounts WHEN OLD.id='demo:personal' BEGIN SELECT RAISE(ABORT,'synthetic removal failure'); END;").unwrap();
    let before: (String, String) = db
        .query_row(
            "SELECT request_json,inverse_json FROM triage_batches WHERE operation_id='mixed-batch'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(store.remove_account("demo:personal").is_err());
    assert_eq!(
        db.query_row(
            "SELECT request_json,inverse_json FROM triage_batches WHERE operation_id='mixed-batch'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        )
        .unwrap(),
        before
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM pinned_threads WHERE thread_id LIKE 'removed-%'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM local_conversation_links", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        2
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM triage_batches WHERE operation_id='removed-only-batch'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert!(
        store
            .get_message("removed-sensitive-message")
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .get_message("kept-sensitive-message")
            .unwrap()
            .is_some()
    );
    assert!(!store.search("removed-sensitive", 10).unwrap().is_empty());
    assert_eq!(store.latest_triage_undo().unwrap().unwrap()["count"], 2);
}
