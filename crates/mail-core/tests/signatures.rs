use mail_core::{Account, MailError, MailStore, Provider};

fn account(store: &MailStore, id: &str, email: &str) {
    store
        .upsert_account(&Account {
            id: id.into(),
            provider: Provider::Gmail,
            email: email.into(),
            display_name: "Signature test".into(),
        })
        .unwrap();
}

#[test]
fn signatures_persist_per_identity_without_cross_account_leaks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signatures.sqlite");
    let store = MailStore::open(&path).unwrap();
    account(&store, "one", "primary@example.com");
    account(&store, "two", "second@example.com");
    let primary = store
        .upsert_signature("one", " PRIMARY@example.com ", "Best,\nAlex", true, false)
        .unwrap();
    assert_eq!(primary.email, "primary@example.com");
    let alias = store
        .upsert_signature(
            "one",
            "alex@company.example",
            "Alex\nCompany",
            false,
            true,
        )
        .unwrap();
    let other = store
        .upsert_signature(
            "two",
            "primary@example.com",
            "Different account",
            true,
            true,
        )
        .unwrap();
    assert_eq!(
        store.get_signature("one", "PRIMARY@example.com").unwrap(),
        Some(primary.clone())
    );
    assert_eq!(
        store.get_signature("two", "primary@example.com").unwrap(),
        Some(other)
    );
    assert_eq!(
        store
            .get_signature("two", "alex@company.example")
            .unwrap(),
        None
    );
    drop(store);
    let reopened = MailStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .get_signature("one", "primary@example.com")
            .unwrap(),
        Some(primary)
    );
    assert_eq!(
        reopened
            .get_signature("one", "alex@company.example")
            .unwrap(),
        Some(alias)
    );
    let updated = reopened
        .upsert_signature(
            "one",
            "PRIMARY@EXAMPLE.COM",
            "Updated\r\nsignature",
            false,
            false,
        )
        .unwrap();
    assert_eq!(
        reopened
            .get_signature("one", "primary@example.com")
            .unwrap(),
        Some(updated)
    );
}

#[test]
fn signature_preferences_do_not_authorize_an_address_for_delivery() {
    let store = MailStore::open_in_memory().unwrap();
    account(&store, "one", "primary@example.com");
    // Preferences may be prepared for an alias before it is connected. Provider
    // delivery still has its own independent verified-From check in mail-service.
    store
        .upsert_signature(
            "one",
            "not-a-verified-alias@example.org",
            "Planned signature",
            true,
            true,
        )
        .unwrap();
    assert_eq!(store.list_accounts().unwrap().len(), 1);
    assert_eq!(
        store.list_accounts().unwrap()[0].email,
        "primary@example.com"
    );
    for operation in [
        store
            .get_signature("missing", "primary@example.com")
            .map(|_| ()),
        store
            .upsert_signature("missing", "primary@example.com", "Text", true, true)
            .map(|_| ()),
        store.delete_signature("missing", "primary@example.com"),
    ] {
        assert!(matches!(operation, Err(MailError::UnknownAccount(_))));
    }
}

#[test]
fn invalid_changes_are_rejected_without_overwriting_the_saved_signature() {
    let store = MailStore::open_in_memory().unwrap();
    account(&store, "one", "primary@example.com");
    let saved = store
        .upsert_signature(
            "one",
            "primary@example.com",
            "Hilsen,\nÅsmund\tNordmann",
            true,
            true,
        )
        .unwrap();
    for email in [
        "",
        "missing-at.example.com",
        "Name <primary@example.com>",
        "\nprimary@example.com",
        "primary@example.com,other@example.com",
        "primary@example.com\r\nBcc:other@example.com",
    ] {
        assert!(
            store
                .upsert_signature("one", email, "Text", true, true)
                .is_err(),
            "{email}"
        );
        assert!(store.get_signature("one", email).is_err());
        assert!(store.delete_signature("one", email).is_err());
    }
    for body in [
        String::new(),
        " \r\n\t".into(),
        "bad\0text".into(),
        "bad\u{7}text".into(),
        "x".repeat(16 * 1024 + 1),
        "ø".repeat(8193),
    ] {
        assert!(
            store
                .upsert_signature("one", "primary@example.com", &body, true, true)
                .is_err()
        );
    }
    assert_eq!(
        store.get_signature("one", "primary@example.com").unwrap(),
        Some(saved)
    );
    assert!(
        store
            .upsert_signature(
                "one",
                "primary@example.com",
                &"x".repeat(16 * 1024),
                true,
                true
            )
            .is_ok()
    );
}

#[test]
fn deletion_is_identity_scoped_idempotent_and_cascades_on_account_removal() {
    let store = MailStore::open_in_memory().unwrap();
    account(&store, "one", "primary@example.com");
    account(&store, "two", "second@example.com");
    store
        .upsert_signature("one", "primary@example.com", "One", true, true)
        .unwrap();
    store
        .upsert_signature("two", "primary@example.com", "Two", true, true)
        .unwrap();
    store
        .delete_signature("one", "PRIMARY@example.com")
        .unwrap();
    store
        .delete_signature("one", "primary@example.com")
        .unwrap();
    assert!(
        store
            .get_signature("one", "primary@example.com")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .get_signature("two", "primary@example.com")
            .unwrap()
            .unwrap()
            .body,
        "Two"
    );
    store.remove_account("two").unwrap();
    account(&store, "two", "second@example.com");
    assert!(
        store
            .get_signature("two", "primary@example.com")
            .unwrap()
            .is_none()
    );
}

#[test]
fn signature_identity_count_is_bounded_and_existing_entries_can_still_be_edited() {
    let store = MailStore::open_in_memory().unwrap();
    account(&store, "one", "primary@example.com");
    for n in 0..256 {
        store
            .upsert_signature("one", &format!("alias{n}@example.com"), "Text", true, true)
            .unwrap();
    }
    assert!(
        store
            .upsert_signature("one", "extra@example.com", "Text", true, true)
            .is_err()
    );
    assert!(
        store
            .upsert_signature("one", "alias0@example.com", "Edited", false, true)
            .is_ok()
    );
    store.delete_signature("one", "alias1@example.com").unwrap();
    assert!(
        store
            .upsert_signature("one", "extra@example.com", "Text", true, true)
            .is_ok()
    );
}
