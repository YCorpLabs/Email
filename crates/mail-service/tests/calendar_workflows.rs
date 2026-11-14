use mail_core::MailStore;
use mail_service::MailService;
use serde_json::{Value, json};

fn call(service: &MailService, value: Value) -> Value {
    service.request_json(&value.to_string()).unwrap()
}

#[test]
fn cached_invitations_are_readable_without_credentials_and_survive_restart() {
    let directory = std::env::temp_dir().join(format!("email-calendar-{}", uuid::Uuid::new_v4()));
    let path = directory.join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let draft = store
        .compose_draft("demo:personal", "alex@example.com", "Meeting", "Body")
        .unwrap();
    let bytes=b"BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:calendar-service@example.org\r\nDTSTART:20261110T100000Z\r\nDTEND:20261110T103000Z\r\nSUMMARY:Service fixture\r\nATTENDEE;RSVP=TRUE:mailto:alex@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    store.cache_calendar_invitations(&draft.id, bytes).unwrap();
    drop(store);
    let service = MailService::open(&path).unwrap();
    let local = call(
        &service,
        json!({"op":"calendar_invitation","message_id":draft.id}),
    );
    assert_eq!(local[0]["event"]["summary"], "Service fixture");
    assert_eq!(local[0]["account_id"], "demo:personal");
    assert_eq!(
        call(
            &service,
            json!({"op":"list_calendar_invitations","account_id":"demo:personal","limit":100})
        ),
        local
    );
    assert!(
        call(
            &service,
            json!({"op":"list_calendar_invitations","limit":0})
        )
        .as_array()
        .unwrap()
        .is_empty()
    );
    assert!(
        service
            .request_json(&json!({"op":"calendar_invitation","message_id":"unknown"}).to_string())
            .is_err()
    );
    assert!(
        service
            .request_json(
                &json!({"op":"list_calendar_invitations","account_id":"unknown"}).to_string()
            )
            .is_err()
    );
    assert!(
        service
            .request_json(
                &json!({"op":"refresh_calendar_invitation","message_id":"unknown"}).to_string()
            )
            .unwrap_err()
            .contains("message not found")
    );
    // Explicit provider refresh lacks configuration in this disposable account;
    // failure must not erase an already cached invitation.
    assert!(
        service
            .request_json(
                &json!({"op":"refresh_calendar_invitation","message_id":draft.id}).to_string()
            )
            .is_err()
    );
    assert_eq!(
        call(
            &service,
            json!({"op":"calendar_invitation","message_id":draft.id})
        ),
        local
    );
    drop(service);
    let service = MailService::open(&path).unwrap();
    assert_eq!(
        call(
            &service,
            json!({"op":"calendar_invitation","message_id":draft.id})
        ),
        local
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn connected_agenda_is_local_scoped_and_reports_missing_coverage() {
    use mail_core::{AgendaEvent, CalendarCollection};
    let directory =
        std::env::temp_dir().join(format!("email-agenda-service-{}", uuid::Uuid::new_v4()));
    let path = directory.join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let start = 1_794_304_800_000i64;
    let end = start + 3_600_000;
    store
        .replace_calendar_collections(
            "demo:personal",
            &[CalendarCollection {
                id: "primary".into(),
                account_id: "demo:personal".into(),
                name: "Work".into(),
                primary: true,
                can_write: true,
            }],
        )
        .unwrap();
    let event = AgendaEvent {
        id: "meeting".into(),
        calendar_id: "primary".into(),
        account_id: "demo:personal".into(),
        title: "Design review".into(),
        description: None,
        location: None,
        start_ms: start + 900_000,
        end_ms: start + 1_800_000,
        all_day: false,
        status: "confirmed".into(),
        organizer_email: None,
        attendees: vec![],
        html_url: None,
        etag: None,
        transparent: false,
        can_respond: false,
        can_edit: false,
        recurring_event_id: None,
    };
    store
        .replace_agenda_window("demo:personal", "primary", start, end, &[event])
        .unwrap();
    drop(store);
    let service = MailService::open(&path).unwrap();
    let events = call(
        &service,
        json!({"op":"list_agenda","account_id":"demo:personal","start_ms":start,"end_ms":end}),
    );
    assert_eq!(events[0]["title"], "Design review");
    let free = call(
        &service,
        json!({"op":"availability","account_id":"demo:personal","start_ms":start,"end_ms":end,"duration_minutes":15}),
    );
    assert_eq!(free["coverage_complete"], true);
    assert_eq!(free["calendars_checked"], 1);
    assert_eq!(free["slots"].as_array().unwrap().len(), 3);
    let missing = call(
        &service,
        json!({"op":"availability","account_id":"demo:personal","start_ms":start,"end_ms":end+1,"duration_minutes":15}),
    );
    assert_eq!(missing["coverage_complete"], false);
    assert_eq!(
        call(
            &service,
            json!({"op":"calendar_status","account_id":"demo:personal"})
        )["connected"],
        false
    );
    for request in [
        json!({"op":"list_agenda","account_id":"unknown","start_ms":start,"end_ms":end}),
        json!({"op":"respond_calendar_invitation","account_id":"demo:personal","calendar_id":"primary","event_id":"meeting","response":"surprise"}),
        json!({"op":"sync_calendar","account_id":"demo:personal","start_ms":end,"end_ms":start}),
    ] {
        assert!(service.request_json(&request.to_string()).is_err());
    }
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn edit_requests_validate_before_credentials_and_do_not_change_cached_events() {
    let directory =
        std::env::temp_dir().join(format!("email-calendar-edit-{}", uuid::Uuid::new_v4()));
    let path = directory.join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    store
        .replace_calendar_collections(
            "demo:personal",
            &[mail_core::CalendarCollection {
                id: "read-only".into(),
                account_id: "demo:personal".into(),
                name: "Shared".into(),
                primary: false,
                can_write: false,
            }],
        )
        .unwrap();
    drop(store);
    let service = MailService::open(&path).unwrap();
    let base = json!({"op":"update_calendar_event","account_id":"demo:personal","calendar_id":"read-only","event_id":"event","expected_etag":"\"v1\"","title":"Updated"});
    let err = service.request_json(&base.to_string()).unwrap_err();
    assert!(err.contains("writable"), "{err}");
    let mut invalid = base.clone();
    invalid["expected_etag"] = json!("");
    assert!(
        service
            .request_json(&invalid.to_string())
            .unwrap_err()
            .contains("version")
    );
    invalid = base.clone();
    invalid["start_ms"] = json!(1_794_218_400_000i64);
    assert!(
        service
            .request_json(&invalid.to_string())
            .unwrap_err()
            .contains("both")
    );
    invalid = base.clone();
    invalid.as_object_mut().unwrap().remove("expected_etag");
    assert!(service.request_json(&invalid.to_string()).is_err());
    // Old create callers may omit location; parsing should reach validation, not reject missing fields.
    let create = json!({"op":"create_calendar_event","account_id":"demo:personal","calendar_id":"read-only","title":"Meeting","start_ms":1_794_218_400_000i64,"end_ms":1_794_222_000_000i64,"attendee_emails":[]});
    let err = service.request_json(&create.to_string()).unwrap_err();
    assert!(err.contains("writable"), "{err}");
    // Delete names the same version and scope, and only on a writable calendar.
    let delete = json!({"op":"delete_calendar_event","account_id":"demo:personal","calendar_id":"read-only","event_id":"event","expected_etag":"\"v1\""});
    let err = service.request_json(&delete.to_string()).unwrap_err();
    assert!(err.contains("writable"), "{err}");
    for (field, value) in [("expected_etag", json!("")), ("edit_scope", json!("series"))] {
        let mut invalid = delete.clone();
        invalid[field] = value;
        let err = service.request_json(&invalid.to_string()).unwrap_err();
        assert!(err.contains("version") || err.contains("scope"), "{field}: {err}");
    }
    invalid = delete.clone();
    invalid.as_object_mut().unwrap().remove("expected_etag");
    assert!(service.request_json(&invalid.to_string()).is_err());
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}
