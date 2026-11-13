use mail_core::MailStore;

#[test]
fn snippets_are_named_local_persistent_and_editable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snippets.sqlite");
    let store = MailStore::open(&path).unwrap();
    let saved = store
        .upsert_snippet(None, " Thank you ", "Thanks for reaching out.\n\nAlex")
        .unwrap();
    assert_eq!(saved.name, "Thank you");
    assert!(
        store
            .upsert_snippet(None, "thank YOU", "Duplicate")
            .is_err()
    );
    drop(store);
    let store = MailStore::open(&path).unwrap();
    assert_eq!(store.list_snippets().unwrap(), vec![saved.clone()]);
    let edited = store
        .upsert_snippet(Some(&saved.id), "Thanks", "Thank you!")
        .unwrap();
    assert_eq!(edited.id, saved.id);
    assert_eq!(store.list_snippets().unwrap()[0].body, "Thank you!");
    store.delete_snippet(&saved.id).unwrap();
    assert!(store.list_snippets().unwrap().is_empty());
    assert!(store.delete_snippet(&saved.id).is_err());
}

#[test]
fn invalid_snippet_edits_leave_previous_content_intact() {
    let store = MailStore::open_in_memory().unwrap();
    let saved = store.upsert_snippet(None, "Greeting", "Hello").unwrap();
    for (name, body) in [
        ("", "Valid"),
        ("bad\nname", "Valid"),
        ("Name", "  "),
        ("Name", "bad\0text"),
    ] {
        assert!(store.upsert_snippet(Some(&saved.id), name, body).is_err());
    }
    assert!(
        store
            .upsert_snippet(Some("missing"), "New", "Text")
            .is_err()
    );
    assert!(
        store
            .upsert_snippet(None, "Large", &"x".repeat(65537))
            .is_err()
    );
    assert_eq!(store.list_snippets().unwrap(), vec![saved]);
}
