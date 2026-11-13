use mail_service::MailService;
use serde_json::{Value, json};

fn call(service: &MailService, value: Value) -> Value {
    service.request_json(&value.to_string()).unwrap()
}

#[test]
fn local_workflows_survive_service_restart_without_provider_access() {
    let directory = std::env::temp_dir().join(format!("email-workflows-{}", uuid::Uuid::new_v4()));
    let path = directory.join("mail.sqlite");
    let service = MailService::open(&path).unwrap();
    call(&service, json!({"op":"seed_demo"}));
    let snippet = call(
        &service,
        json!({"op":"upsert_snippet","name":"Greeting","body":"Hello!"}),
    );
    let result = call(
        &service,
        json!({"op":"batch_triage","operation_id":"qa-archive","kind":"archive","thread_ids":["demo:1","demo:2"]}),
    );
    assert_eq!(result["changed"], 2);
    let draft = call(
        &service,
        json!({"op":"compose_draft","account_id":"demo:personal","to":"Jane Doe <jane@example.com>, bob@example.com","subject":"Hello","body_plain":"Text"}),
    );
    assert_eq!(draft["to"].as_array().unwrap().len(), 2);
    drop(service);
    let service = MailService::open(&path).unwrap();
    assert_eq!(
        call(&service, json!({"op":"list_snippets"}))[0]["id"],
        snippet["id"]
    );
    assert_eq!(
        call(&service, json!({"op":"latest_triage_undo"}))["operation_id"],
        "qa-archive"
    );
    assert_eq!(
        call(
            &service,
            json!({"op":"undo_triage","operation_id":"qa-archive"})
        )["changed"],
        2
    );
    assert!(call(&service, json!({"op":"latest_triage_undo"})).is_null());
    call(&service, json!({"op":"delete_snippet","id":snippet["id"]}));
    assert!(
        call(&service, json!({"op":"list_snippets"}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn followup_mailbox_and_identity_signature_are_local_and_persistent() {
    let directory =
        std::env::temp_dir().join(format!("email-followup-service-{}", uuid::Uuid::new_v4()));
    let path = directory.join("mail.sqlite");
    let service = MailService::open(&path).unwrap();
    call(&service, json!({"op":"seed_demo"}));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    call(
        &service,
        json!({"op":"set_followup","thread_id":"demo:1","due_at_ms":now+3_600_000,"only_if_no_reply":true}),
    );
    call(
        &service,
        json!({"op":"batch_triage","operation_id":"followup-archive","kind":"archive","thread_ids":["demo:1"]}),
    );
    let rows = call(
        &service,
        json!({"op":"search","query":"","followups_only":true,"account_id":"demo:personal"}),
    );
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], "demo:1");
    assert_eq!(rows[0]["is_inbox"], false);
    assert!(
        call(
            &service,
            json!({"op":"search","query":"","followups_only":true,"account_id":"demo:work"})
        )
        .as_array()
        .unwrap()
        .is_empty()
    );
    call(
        &service,
        json!({"op":"upsert_signature","account_id":"demo:personal","email":"alex@example.com","body":"Regards,\nAlex","for_new":true,"for_reply":false}),
    );
    drop(service);
    let service = MailService::open(&path).unwrap();
    assert_eq!(
        call(
            &service,
            json!({"op":"get_signature","account_id":"demo:personal","email":"alex@example.com"})
        )["body"],
        "Regards,\nAlex"
    );
    assert_eq!(
        call(
            &service,
            json!({"op":"list_followups","account_id":"demo:personal"})
        )[0]["status"],
        "waiting"
    );
    call(
        &service,
        json!({"op":"cancel_followup","thread_id":"demo:1"}),
    );
    assert!(
        call(
            &service,
            json!({"op":"search","query":"","followups_only":true})
        )
        .as_array()
        .unwrap()
        .is_empty()
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}
