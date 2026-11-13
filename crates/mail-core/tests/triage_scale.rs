//! A realistic cache-sized workflow, separate from machine-specific latency budgets.
use mail_core::MailStore;
use rusqlite::params;
use std::time::Instant;

#[test]
fn four_thousand_conversation_archive_and_restart_undo() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scale.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let mut db = rusqlite::Connection::open(&path).unwrap();
    let tx = db.transaction().unwrap();
    let ids: Vec<String> = (0..4000).map(|n| format!("scale:{n}")).collect();
    for id in &ids {
        tx.execute("INSERT INTO threads VALUES(?1,'demo:personal','Newsletter','news@example.org','Preview',100,1,1,'Inbox')",[id]).unwrap();
        tx.execute("INSERT INTO messages(id,thread_id,account_id,sender,to_json,subject,body_plain,received_at,is_read,is_draft,is_sent) VALUES(?1,?1,'demo:personal','news@example.org','[]','Newsletter','Cached body',100,0,0,0)", [id]).unwrap();
    }
    tx.commit().unwrap();
    let start = Instant::now();
    assert_eq!(
        store
            .batch_triage("scale-archive", "archive", &ids)
            .unwrap()["changed"],
        4000
    );
    let archive = start.elapsed();
    assert_eq!(store.list_threads(None, 10, 0).unwrap().len(), 3);
    assert_eq!(
        store.get_message(&ids[0]).unwrap().unwrap().body_plain,
        "Cached body"
    );
    drop(store);
    let store = MailStore::open(&path).unwrap();
    let start = Instant::now();
    assert_eq!(store.latest_triage_undo().unwrap().unwrap()["count"], 4000);
    let discover = start.elapsed();
    let start = Instant::now();
    assert_eq!(store.undo_triage("scale-archive").unwrap()["changed"], 4000);
    let undo = start.elapsed();
    assert!(store.latest_triage_undo().unwrap().is_none());
    assert_eq!(store.pending_actions().unwrap().len(), 8000);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM threads WHERE id LIKE 'scale:%' AND is_inbox=1",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        4000
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM messages WHERE id LIKE 'scale:%' AND is_read=0 AND body_plain=?1",
            params!["Cached body"],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        4000
    );
    eprintln!(
        "4000 conversations: local archive={archive:?}, restart undo discovery={discover:?}, local undo={undo:?}; no network involved"
    );
}
