use mail_core::{Account, FilterAction, MailAction, MailStore, Message, Provider};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn store() -> MailStore {
    let store = MailStore::open_in_memory().unwrap();
    store
        .upsert_account(&Account {
            id: "a".into(),
            provider: Provider::Gmail,
            email: "me@fernhill.example".into(),
            display_name: "Me".into(),
        })
        .unwrap();
    store
}

fn message(id: &str, thread: &str, sender: &str, to: &str, minutes_ago: i64, sent: bool) -> Message {
    Message {
        recipient_headers: None,
        id: id.into(),
        thread_id: thread.into(),
        account_id: "a".into(),
        sender: sender.into(),
        to: vec![to.into()],
        subject: format!("Subject {id}"),
        body_plain: format!("Body {id}"),
        body_html: None,
        unsubscribe_header: None,
        has_attachments: None,
        received_at: now_ms() - minutes_ago * 60_000,
        is_read: false,
        is_draft: false,
        is_sent: sent,
    }
}

fn incoming(store: &MailStore, id: &str, thread: &str, sender: &str, minutes_ago: i64) {
    store
        .upsert_message(&message(id, thread, sender, "me@fernhill.example", minutes_ago, false))
        .unwrap();
}

fn enable(store: &MailStore) -> (String, String) {
    store.set_filters_enabled(true, None).unwrap();
    let filters = store.list_filters().unwrap();
    let id = |name: &str| filters.iter().find(|f| f.name == name).unwrap().id.clone();
    (id("Cold outreach"), id("Notifications"))
}

fn queued(store: &MailStore, thread: &str) -> Vec<MailAction> {
    store
        .pending_actions()
        .unwrap()
        .into_iter()
        .map(|q| q.action)
        .filter(|a| match a {
            MailAction::Archive { thread_id } | MailAction::MarkRead { thread_id, .. } | MailAction::Move { thread_id, .. } => thread_id == thread,
            _ => false,
        })
        .collect()
}

#[test]
fn nothing_is_sorted_until_filters_are_turned_on() {
    let store = store();
    incoming(&store, "m1", "t1", "Ravi <ravi@brightforge.io>", 5);
    assert!(store.filter_candidates(10).unwrap().0.is_empty());
    assert!(store.list_filters().unwrap().is_empty());
}

#[test]
fn turning_on_creates_the_starters_once() {
    let store = store();
    store.set_filters_enabled(true, Some(&["cold_outreach".into(), "newsletters".into()])).unwrap();
    let names: Vec<_> = store.list_filters().unwrap().into_iter().map(|f| f.name).collect();
    assert_eq!(names, ["Cold outreach", "Newsletters"]);
    store.set_filters_enabled(false, None).unwrap();
    store.set_filters_enabled(true, None).unwrap();
    assert_eq!(store.list_filters().unwrap().len(), 2);
}

#[test]
fn contacts_colleagues_and_replied_conversations_never_reach_the_model() {
    let store = store();
    // Someone the account wrote to before.
    store
        .upsert_message(&message("s1", "old", "me@fernhill.example", "Sara <sara@acme.no>", 600, true))
        .unwrap();
    incoming(&store, "m1", "t1", "Sara Lind <SARA@acme.no>", 5);
    // A colleague on the company domain.
    incoming(&store, "m2", "t2", "Nora <nora@fernhill.example>", 5);
    // A stranger in a conversation the person replied in.
    incoming(&store, "m3", "t3", "Lucas <lucas@proton.me>", 30);
    store
        .upsert_message(&message("s3", "t3", "me@fernhill.example", "lucas@proton.me", 20, true))
        .unwrap();
    incoming(&store, "m3b", "t3", "Lucas <lucas@proton.me>", 10);
    // A stranger.
    incoming(&store, "m4", "t4", "Ravi <ravi@brightforge.io>", 5);
    enable(&store);
    let (candidates, decisions) = store.filter_candidates(10).unwrap();
    assert_eq!(candidates.iter().map(|c| c.thread_id.as_str()).collect::<Vec<_>>(), ["t4"]);
    assert!(decisions.is_empty());
    // Settled conversations are not offered again.
    assert_eq!(store.filter_candidates(10).unwrap().0.len(), 1);
}

#[test]
fn a_gmail_address_is_not_a_colleague() {
    let store = MailStore::open_in_memory().unwrap();
    store
        .upsert_account(&Account { id: "a".into(), provider: Provider::Gmail, email: "me@gmail.com".into(), display_name: "Me".into() })
        .unwrap();
    incoming(&store, "m1", "t1", "Ravi <ravi@gmail.com>", 5);
    store.set_filters_enabled(true, None).unwrap();
    assert_eq!(store.filter_candidates(10).unwrap().0.len(), 1);
}

#[test]
fn confident_decisions_apply_the_action_and_unsure_ones_stay() {
    let store = store();
    incoming(&store, "m1", "t1", "Ravi <ravi@brightforge.io>", 5);
    incoming(&store, "m2", "t2", "Dana <dana@rankrocket.agency>", 6);
    incoming(&store, "m3", "t3", "GitHub <noreply@github.com>", 7);
    let (cold, notifications) = enable(&store);
    let (candidates, _) = store.filter_candidates(10).unwrap();
    let by = |t: &str| candidates.iter().find(|c| c.thread_id == t).unwrap().clone();

    let d = store.record_model_decision(&by("t1"), Some(&cold), 0.97, "Agency pitch").unwrap();
    assert!(d.moved);
    assert_eq!(
        queued(&store, "t1"),
        [MailAction::Archive { thread_id: "t1".into() }, MailAction::MarkRead { thread_id: "t1".into(), read: true }]
    );
    let in_filter = store.list_filter_threads(&cold, 50, 0).unwrap();
    assert_eq!(in_filter.len(), 1);
    let tag = in_filter[0].filter.clone().unwrap();
    assert_eq!((tag.name.as_str(), tag.reason.as_str(), tag.action), ("Cold outreach", "Agency pitch", FilterAction::Ignore));

    // Unsure: stays in the inbox, untagged. Ignore needs 95%.
    let d = store.record_model_decision(&by("t2"), Some(&cold), 0.9, "maybe").unwrap();
    assert!(!d.moved && d.filter_id.is_none());
    assert!(queued(&store, "t2").is_empty());

    // Keep-in-inbox filters tag without moving.
    let d = store.record_model_decision(&by("t3"), Some(&notifications), 0.99, "CI alert").unwrap();
    assert!(!d.moved);
    assert!(queued(&store, "t3").is_empty());
    let inbox = store.list_threads(None, 50, 0).unwrap();
    let t3 = inbox.iter().find(|t| t.id == "t3").unwrap();
    assert_eq!(t3.filter.as_ref().unwrap().name, "Notifications");
    assert!(store.filter_candidates(10).unwrap().0.is_empty());
}

#[test]
fn moving_by_hand_teaches_the_sender_and_restores_to_inbox() {
    let store = store();
    incoming(&store, "m1", "t1", "Maya <maya@quillboard.com>", 5);
    let (cold, _) = enable(&store);
    let (candidates, _) = store.filter_candidates(10).unwrap();
    store.record_model_decision(&candidates[0], Some(&cold), 0.95, "Pitch").unwrap();
    // Locally archived by the decision.
    assert!(store.list_threads(None, 50, 0).unwrap().iter().all(|t| t.id != "t1"));

    let d = store.move_thread_to_filter("t1", None).unwrap();
    assert!(d.moved);
    assert!(store.list_threads(None, 50, 0).unwrap().iter().any(|t| t.id == "t1"));
    assert!(store.list_filter_threads(&cold, 50, 0).unwrap().is_empty());
    let examples = store.filter_examples().unwrap();
    assert_eq!((examples[0].category.as_str(), examples[0].from.as_str()), ("People", "Maya <maya@quillboard.com>"));

    // The next mail from Maya skips the model and stays.
    incoming(&store, "m2", "t2", "Maya Chen <maya@quillboard.com>", 1);
    let (candidates, decisions) = store.filter_candidates(10).unwrap();
    assert!(candidates.is_empty());
    assert_eq!(decisions[0].filter_id, None);
    assert!(!decisions[0].moved);
}

#[test]
fn a_hand_sorted_sender_is_filtered_without_the_model() {
    let store = store();
    incoming(&store, "m1", "t1", "Marcus <marcus@outboundlabs.co>", 10);
    let (cold, _) = enable(&store);
    store.move_thread_to_filter("t1", Some(&cold)).unwrap();
    incoming(&store, "m2", "t2", "Marcus <marcus@outboundlabs.co>", 1);
    let (candidates, decisions) = store.filter_candidates(10).unwrap();
    assert!(candidates.is_empty());
    assert_eq!(decisions[0].filter_id.as_deref(), Some(cold.as_str()));
    assert!(decisions[0].moved);
    assert_eq!(queued(&store, "t2").len(), 2);
}

#[test]
fn a_follow_up_in_a_filtered_conversation_is_filtered_again() {
    let store = store();
    incoming(&store, "m1", "t1", "Marcus <marcus@outboundlabs.co>", 60);
    let (cold, _) = enable(&store);
    let (candidates, _) = store.filter_candidates(10).unwrap();
    store.record_model_decision(&candidates[0], Some(&cold), 0.99, "Pitch").unwrap();
    // The provider brings the conversation back with "just bumping this".
    store
        .apply_action("server-restore", MailAction::Move { thread_id: "t1".into(), folder: "Inbox".into() })
        .unwrap();
    incoming(&store, "m2", "t1", "Marcus <marcus@outboundlabs.co>", 1);
    let (candidates, decisions) = store.filter_candidates(10).unwrap();
    assert!(candidates.is_empty());
    assert!(decisions[0].moved);
    assert!(store.list_threads(None, 50, 0).unwrap().iter().all(|t| t.id != "t1"));
}

#[test]
fn old_mail_outside_the_window_is_left_alone() {
    let store = store();
    incoming(&store, "m1", "t1", "Ravi <ravi@brightforge.io>", 60 * 24 * 30);
    enable(&store);
    assert!(store.filter_candidates(10).unwrap().0.is_empty());
}

#[test]
fn filter_names_are_checked() {
    let store = store();
    enable(&store);
    assert!(store.upsert_filter(None, "People", "x", FilterAction::Inbox).is_err());
    assert!(store.upsert_filter(None, "cold OUTREACH", "x", FilterAction::Inbox).is_err());
    assert!(store.upsert_filter(None, "Receipts", "", FilterAction::Inbox).is_err());
    let receipts = store.upsert_filter(None, "Receipts", "Receipts and invoices", FilterAction::SkipInbox).unwrap();
    store.delete_filter(&receipts.id).unwrap();
    assert!(store.list_filters().unwrap().iter().all(|f| f.name != "Receipts"));
}
