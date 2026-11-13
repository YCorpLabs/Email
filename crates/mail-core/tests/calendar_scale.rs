//! Repeatable local-cache probe; does not measure native painting or provider latency.
use mail_core::{Account, AgendaEvent, CalendarCollection, MailStore, Provider};
use std::time::Instant;

#[test]
#[ignore = "manual release-mode calendar cache benchmark"]
fn ten_thousand_events_across_twenty_five_calendars() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calendar-scale.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.upsert_account(&Account { id:"scale".into(), provider:Provider::Gmail,
        email:"scale@example.org".into(), display_name:"Scale".into() }).unwrap();
    let day = 86_400_000i64;
    let start = 1_794_182_400_000i64;
    let end = start + 56 * day;
    let calendars:Vec<_> = (0..25).map(|i| CalendarCollection {
        id:format!("calendar-{i}"), account_id:"scale".into(), name:format!("Calendar {i}"),
        primary:i==0, can_write:true,
    }).collect();
    let windows:Vec<_> = calendars.iter().enumerate().map(|(c,calendar)| {
        let events:Vec<AgendaEvent> = (0..400).map(|i| {
            let begins = start + (i % 56) * day + ((i/56)*60 + c as i64)*60_000;
            serde_json::from_value(serde_json::json!({
                "id":format!("event-{i}"), "calendar_id":calendar.id,"account_id":"scale",
                "title":format!("Planning session {i}"),"description":"Synthetic cached event",
                "location":"Meeting room","start_ms":begins,"end_ms":begins+30*60_000,
                "all_day":false,"status":"confirmed","organizer_email":"scale@example.org",
                "attendees":[],"html_url":null,"etag":"\"v1\""
            })).unwrap()
        }).collect();
        (calendar.id.clone(),events)
    }).collect();
    store.replace_agenda_snapshot("scale",&calendars,start,end,&windows).unwrap();
    drop(store);
    let store = MailStore::open(&path).unwrap();
    assert_eq!(store.list_agenda_events(Some("scale"),start,end,10_000).unwrap().len(),10_000);
    let mut samples=Vec::new();
    for i in 0..200 {
        let begins=start+(i%8)*7*day;
        let now=Instant::now();
        let result=store.list_agenda_events(Some("scale"),begins,begins+7*day,10_000).unwrap();
        samples.push(now.elapsed().as_micros());
        assert!(!result.is_empty());
        assert!(result.iter().all(|e|e.start_ms<begins+7*day && e.end_ms>begins));
    }
    samples.sort_unstable();
    eprintln!("calendar-cache: 10000 events / 25 calendars; 200 warm week reads; p50={}us p95={}us max={}us; disk-backed SQLite; excludes UI and network",samples[100],samples[190],samples[199]);
}
