use mail_core::{Account, AgendaEvent, CalendarCollection, MailStore, Provider};
use mail_service::MailService;
use serde_json::json;

#[test]
fn unified_agenda_filters_before_shared_subscription_dedup_without_mutating_cache() {
    let directory =
        std::env::temp_dir().join(format!("email-unified-calendar-{}", uuid::Uuid::new_v4()));
    let path = directory.join("mail.sqlite");
    let store = MailStore::open(&path).unwrap();
    for account in ["one", "two"] {
        store
            .upsert_account(&Account {
                id: account.into(),
                email: format!("{account}@example.org"),
                display_name: account.into(),
                provider: Provider::Gmail,
            })
            .unwrap();
        let calendar = CalendarCollection {
            id: "norway@holiday".into(),
            account_id: account.into(),
            name: "Holidays in Norway".into(),
            primary: false,
            can_write: false,
        };
        store
            .replace_calendar_collections(account, &[calendar])
            .unwrap();
        let event:AgendaEvent=serde_json::from_value(json!({"account_id":account,"calendar_id":"norway@holiday","id":"holiday","title":"Christmas","start_ms":100,"end_ms":200,"all_day":true,"status":"confirmed","attendees":[]})).unwrap();
        store.cache_agenda_event(&event).unwrap();
    }
    drop(store);
    let service = MailService::open(&path).unwrap();
    let raw = json!({"op":"list_agenda","start_ms":0,"end_ms":1000,"limit":500});
    assert_eq!(
        service
            .request_json(&raw.to_string())
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut unified = raw.clone();
    unified["deduplicate_shared"] = json!(true);
    assert_eq!(
        service
            .request_json(&unified.to_string())
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    unified["hidden_sources"] = json!([{"account_id":"one","calendar_id":"norway@holiday"}]);
    let shown = service.request_json(&unified.to_string()).unwrap();
    assert_eq!(shown.as_array().unwrap().len(), 1);
    assert_eq!(shown[0]["account_id"], "two");
    assert_eq!(
        service
            .request_json(&raw.to_string())
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}
