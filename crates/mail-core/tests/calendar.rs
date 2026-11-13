use mail_core::{MailStore, parse_calendar_invitations};

fn ics(properties: &str) -> Vec<u8> {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:event-1@example.org\r\n{properties}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").into_bytes()
}

#[test]
fn utc_folded_fields_attendees_and_escaped_text_are_preserved() {
    let events=parse_calendar_invitations(&ics("DTSTART:20261109T103000Z\r\nDTEND:20261109T110000Z\r\nSUMMARY:Design review \\;\r\n with team\r\nDESCRIPTION:One\\nTwo\\, three\r\nLOCATION:Room 2\r\nORGANIZER;CN=\"Doe; Jane\":mailto:jane@example.org\r\nATTENDEE;CN=Alex;RSVP=TRUE;PARTSTAT=NEEDS-ACTION:mailto:alex@example.com")).unwrap();
    let event = &events[0];
    assert_eq!(event.summary, "Design review ;with team");
    assert_eq!(event.description, "One\nTwo, three");
    assert_eq!(event.start.utc_ms, Some(1794220200000));
    assert_eq!(event.start.display, "2026-11-09 10:30:00");
    assert_eq!(event.method.as_deref(), Some("REQUEST"));
    assert_eq!(
        event.organizer.as_ref().unwrap().name.as_deref(),
        Some("Doe; Jane")
    );
    assert!(event.attendees[0].response_requested);
    assert_eq!(
        event.attendees[0].participation_status.as_deref(),
        Some("NEEDS-ACTION")
    );
    assert!(event.warnings.is_empty());
}

#[test]
fn floating_dst_named_zones_all_day_and_recurrence_never_invent_instants() {
    for start in [
        "DTSTART;TZID=Europe/Oslo:20261025T023000",
        "DTSTART:20261025T023000",
        "DTSTART;VALUE=DATE:20261109",
    ] {
        let event = parse_calendar_invitations(&ics(&format!(
            "{start}\r\nSUMMARY:Meeting\r\nRRULE:FREQ=WEEKLY"
        )))
        .unwrap()
        .remove(0);
        assert_eq!(event.start.utc_ms, None);
        assert!(event.warnings.iter().any(|w| w.contains("Recurring")));
        if start.contains("TZID") {
            assert_eq!(event.start.timezone.as_deref(), Some("Europe/Oslo"));
        }
        if start.contains("VALUE=DATE") {
            assert!(event.start.all_day);
            assert_eq!(event.start.display, "2026-11-09");
        }
    }
    let event = parse_calendar_invitations(&ics("DTSTART:20261109T100000Z\r\nDURATION:PT30M"))
        .unwrap()
        .remove(0);
    assert_eq!(event.end, None);
    assert!(event.warnings.iter().any(|w| w.contains("duration")));
}

#[test]
fn invalid_dates_injection_duplicates_and_truncation_fail_closed() {
    for invalid in [
        "DTSTART:20260229T100000Z",
        "DTSTART:20261109T240000Z",
        "DTSTART:20261109T105960Z",
        "DTSTART;TZID=Europe/Oslo:20261109T100000Z",
        "DTSTART;VALUE=DATE:20261109Z",
        "DTSTART:20261109T100000Z\r\nDTSTART:20261110T100000Z",
        "DTSTART:20261109T100000Z\r\nDTEND:20261109T090000Z",
        "DTSTART:20261109T100000Z\r\nATTENDEE:mailto:bad@@example.org",
        "DTSTART:20261109T100000Z\r\nSUMMARY:Hello\u{0}there",
        "DTSTART:20261109T100000Z\r\nDTEND;VALUE=DATE:20261110",
        "DTSTART:20261109T100000Z\r\nDTEND:20261109T110000Z\r\nDURATION:PT30M",
    ] {
        assert!(
            parse_calendar_invitations(&ics(invalid)).is_err(),
            "{invalid}"
        );
    }
    assert!(
        parse_calendar_invitations(
            b"BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\nDTSTART:20261109T100000Z"
        )
        .is_err()
    );
    assert!(
        parse_calendar_invitations(&ics(&format!(
            "DTSTART:20261109T100000Z\r\nDESCRIPTION:{}",
            "x".repeat(1024 * 1024)
        )))
        .is_err()
    );
    assert!(parse_calendar_invitations(&vec![b'x'; 40 * 1024 * 1024 + 1]).is_err());
    assert!(
        parse_calendar_invitations(&ics(&format!(
            "DTSTART:20261109T100000Z\r\nSUMMARY:{}",
            "x".repeat(17000)
        )))
        .is_err()
    );
}

#[test]
fn decoded_ics_mime_attachment_is_explicit_and_plain_mail_is_not_guessed() {
    let raw = String::from_utf8(ics("DTSTART:20261109T100000Z\r\nSUMMARY:MIME meeting")).unwrap();
    let mime = format!(
        "MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=calendar\r\n\r\n--calendar\r\nContent-Type: text/plain\r\n\r\nSee meeting\r\n--calendar\r\nContent-Type: text/calendar; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{}\r\n--calendar--\r\n",
        raw.replace('=', "=3D")
    );
    assert_eq!(
        parse_calendar_invitations(mime.as_bytes()).unwrap()[0].summary,
        "MIME meeting"
    );
    let plain = format!("Content-Type: text/plain\r\n\r\n{raw}");
    assert!(
        parse_calendar_invitations(plain.as_bytes())
            .unwrap()
            .is_empty()
    );
    let html = ics(
        "DTSTART:20261109T100000Z\r\nSUMMARY:<script>alert(1)</script>\r\nATTACH:https://sender.example/track",
    );
    assert_eq!(
        parse_calendar_invitations(&html).unwrap()[0].summary,
        "<script>alert(1)</script>"
    );
}

#[test]
fn method_order_and_multiple_events_remain_explicit() {
    let raw = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\nDTSTART:19700101T000000Z\nEND:VEVENT\nMETHOD:CANCEL\nEND:VCALENDAR\n";
    let e = parse_calendar_invitations(raw.as_bytes()).unwrap();
    assert_eq!(e[0].method.as_deref(), Some("CANCEL"));
    assert_eq!(e[0].start.utc_ms, Some(0));
    assert_eq!(
        parse_calendar_invitations(&ics("DTSTART;VALUE=DATE:20240229")).unwrap()[0]
            .start
            .display,
        "2024-02-29"
    );
    assert_eq!(
        parse_calendar_invitations(raw.to_ascii_lowercase().as_bytes()).unwrap()[0]
            .method
            .as_deref(),
        Some("CANCEL")
    );
}

#[test]
fn cache_refresh_is_atomic_scoped_persistent_and_keeps_old_data_on_parse_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calendar.sqlite");
    let store = MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    let drafts = store
        .compose_draft("demo:personal", "a@example.org", "Meeting", "Body")
        .unwrap();
    let raw = ics("DTSTART:20261109T100000Z\r\nSUMMARY:Cached meeting");
    let stored = store.cache_calendar_invitations(&drafts.id, &raw).unwrap();
    assert_eq!(stored[0].account_id, "demo:personal");
    assert!(
        store
            .cache_calendar_invitations(&drafts.id, b"BEGIN:VCALENDAR\ninvalid")
            .is_err()
    );
    assert_eq!(store.calendar_invitation(&drafts.id).unwrap(), stored);
    assert_eq!(
        store
            .list_calendar_invitations(Some("demo:personal"), 100)
            .unwrap(),
        stored
    );
    assert!(
        store
            .list_calendar_invitations(Some("unknown"), 100)
            .is_err()
    );
    let mut other = store.list_accounts().unwrap()[0].clone();
    other.id = "calendar:other".into();
    other.email = "other@example.com".into();
    store.upsert_account(&other).unwrap();
    assert!(
        store
            .list_calendar_invitations(Some(&other.id), 100)
            .unwrap()
            .is_empty()
    );
    assert!(store.calendar_invitation("unknown").is_err());
    assert!(store.cache_calendar_invitations("unknown", &raw).is_err());
    assert!(store.list_calendar_invitations(None, 0).unwrap().is_empty());
    drop(store);
    let reopened = MailStore::open(&path).unwrap();
    assert_eq!(reopened.calendar_invitation(&drafts.id).unwrap(), stored);
    reopened.cache_calendar_events(&drafts.id, &[]).unwrap();
    assert!(reopened.calendar_invitation(&drafts.id).unwrap().is_empty());
}
