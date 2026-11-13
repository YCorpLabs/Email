use mail_core::{
    Account, MailAction, MailAdapter, MailStore, MessageState, Provider, RecipientHeaders,
    SyncBatch,
};
use std::collections::HashMap;
struct MetadataAdapter;
impl MailAdapter for MetadataAdapter {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }
    fn sync(&self, _: &Account, _: Option<&str>) -> mail_core::Result<SyncBatch> {
        panic!("cache-aware path must be used")
    }
    fn sync_with_cached_messages(
        &self,
        _: &Account,
        _: Option<&str>,
        cached: &HashMap<String, String>,
    ) -> mail_core::Result<SyncBatch> {
        assert_eq!(cached.get("demo:1").map(String::as_str), Some("demo:1"));
        Ok(SyncBatch {
            message_states: vec![
                MessageState {
                    id: "demo:1".into(),
                    is_read: true,
                    is_draft: false,
                    is_sent: false,
                },
                MessageState {
                    id: "uncached-other-thread-member".into(),
                    is_read: true,
                    is_draft: false,
                    is_sent: false,
                },
            ],
            messages: vec![],
            thread_labels: vec![(
                "demo:1".into(),
                vec!["INBOX".into(), "Billing".into(), "Work".into()],
            )],
            removed_message_ids: vec![],
            full_reset: false,
            reset_phase: None,
            next_cursor: "metadata-sync".into(),
        })
    }
}
#[test]
fn metadata_sync_keeps_bodies_headers_attachments_and_replays_pending_actions() {
    let store = MailStore::open_in_memory().unwrap();
    store.seed_demo().unwrap();
    let mut rich = store.get_message("demo:1").unwrap().unwrap();
    rich.body_html = Some("<p>cached html</p>".into());
    rich.recipient_headers = Some(RecipientHeaders {
        cc: vec!["cc@example.com".into()],
        bcc: vec![],
        reply_to: vec!["reply@example.com".into()],
    });
    rich.has_attachments = Some(true);
    rich.unsubscribe_header = Some("<https://example.org/unsubscribe>".into());
    store.upsert_message(&rich).unwrap();
    assert_eq!(
        store.run_sync(&MetadataAdapter, "demo:personal").unwrap(),
        0
    );
    let synced = store.get_message("demo:1").unwrap().unwrap();
    assert!(synced.is_read);
    assert_eq!(synced.body_plain, rich.body_plain);
    assert_eq!(synced.body_html, rich.body_html);
    assert_eq!(synced.recipient_headers, rich.recipient_headers);
    assert_eq!(synced.has_attachments, rich.has_attachments);
    assert_eq!(synced.unsubscribe_header, rich.unsubscribe_header);
    assert_eq!(
        store.sync_cursor("demo:personal").unwrap().unwrap().value,
        "metadata-sync"
    );
    let thread = store
        .list_threads(None, 10, 0)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "demo:1")
        .unwrap();
    assert!(thread.labels.contains(&"Billing".into()) && thread.labels.contains(&"Work".into()));
    store
        .apply_action(
            "newer-local-unread",
            MailAction::MarkRead {
                thread_id: "demo:1".into(),
                read: false,
            },
        )
        .unwrap();
    store
        .apply_action(
            "newer-local-archive",
            MailAction::Archive {
                thread_id: "demo:1".into(),
            },
        )
        .unwrap();
    store.run_sync(&MetadataAdapter, "demo:personal").unwrap();
    assert!(!store.get_message("demo:1").unwrap().unwrap().is_read);
    assert!(
        store
            .list_archived_threads(10, 0)
            .unwrap()
            .iter()
            .any(|t| t.id == "demo:1")
    );
}

#[test]
fn network_adapter_does_not_hold_the_cache_lock() {
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;
    struct PausedAdapter {
        entered: mpsc::Sender<()>,
        resume: Mutex<mpsc::Receiver<()>>,
    }
    impl MailAdapter for PausedAdapter {
        fn provider(&self) -> Provider {
            Provider::Gmail
        }
        fn sync(&self, _: &Account, _: Option<&str>) -> mail_core::Result<SyncBatch> {
            self.entered.send(()).unwrap();
            self.resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            Ok(SyncBatch {
                message_states: vec![],
                messages: vec![],
                thread_labels: vec![],
                removed_message_ids: vec![],
                full_reset: false,
                reset_phase: None,
                next_cursor: "paused-sync".into(),
            })
        }
    }
    let store = Arc::new(MailStore::open_in_memory().unwrap());
    store.seed_demo().unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let syncing = store.clone();
    let worker = std::thread::spawn(move || {
        syncing.run_sync(
            &PausedAdapter {
                entered: entered_tx,
                resume: Mutex::new(resume_rx),
            },
            "demo:personal",
        )
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let reading = store.clone();
    let (read_tx, read_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        read_tx
            .send(reading.get_message("demo:1").unwrap().is_some())
            .unwrap()
    });
    let cached_read = read_rx.recv_timeout(Duration::from_secs(1));
    resume_tx.send(()).unwrap();
    worker.join().unwrap().unwrap();
    reader.join().unwrap();
    assert_eq!(
        cached_read.unwrap(),
        true,
        "cached reads must finish while network is paused"
    );
}
