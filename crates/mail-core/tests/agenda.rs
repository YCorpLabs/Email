use mail_core::{Account, AgendaAttendee, AgendaEvent, CalendarCollection, MailStore, Provider};

const MIN: i64 = 60_000;
fn account(store: &MailStore, id: &str) {
    store
        .upsert_account(&Account {
            id: id.into(),
            provider: Provider::Gmail,
            email: format!("{id}@example.com"),
            display_name: id.into(),
        })
        .unwrap();
}
fn calendar(account: &str) -> CalendarCollection {
    CalendarCollection {
        id: "primary".into(),
        account_id: account.into(),
        name: "Calendar".into(),
        primary: true,
        can_write: true,
    }
}
fn event(account: &str, id: &str, start: i64, end: i64) -> AgendaEvent {
    AgendaEvent {
        id: id.into(),
        calendar_id: "primary".into(),
        account_id: account.into(),
        title: id.into(),
        description: None,
        location: None,
        start_ms: start,
        end_ms: end,
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
    }
}
fn store() -> MailStore {
    let s = MailStore::open_in_memory().unwrap();
    for id in ["a", "b"] {
        account(&s, id);
        s.replace_calendar_collections(id, &[calendar(id)]).unwrap();
    }
    s
}

#[test]
fn calendars_and_same_remote_ids_are_isolated_and_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agenda.sqlite");
    let s = MailStore::open(&path).unwrap();
    for id in ["a", "b"] {
        account(&s, id);
        s.replace_calendar_collections(id, &[calendar(id)]).unwrap();
        s.replace_agenda_window(id, "primary", 0, 1000, &[event(id, "same", 100, 200)])
            .unwrap();
    }
    drop(s);
    let s = MailStore::open(path).unwrap();
    assert_eq!(
        s.list_calendar_collections(Some("a")).unwrap(),
        vec![calendar("a")]
    );
    assert_eq!(
        s.list_agenda_events(Some("b"), 0, 1000, 100).unwrap(),
        vec![event("b", "same", 100, 200)]
    );
    assert_eq!(s.list_agenda_events(None, 0, 1000, 100).unwrap().len(), 2);
    assert!(s.list_agenda_events(Some("unknown"), 0, 1000, 100).is_err());
}

#[test]
fn window_refresh_replaces_overlap_only_and_failure_keeps_previous() {
    let s = store();
    s.replace_agenda_window(
        "a",
        "primary",
        0,
        1000,
        &[
            event("a", "early", 100, 200),
            event("a", "middle", 400, 600),
            event("a", "late", 800, 900),
        ],
    )
    .unwrap();
    let bad = event("b", "wrong", 450, 500);
    assert!(
        s.replace_agenda_window("a", "primary", 300, 700, &[bad])
            .is_err()
    );
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap().len(),
        3
    );
    s.replace_agenda_window(
        "a",
        "primary",
        300,
        700,
        &[event("a", "replacement", 350, 750)],
    )
    .unwrap();
    let result = s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap();
    assert_eq!(
        result.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        vec!["early", "replacement", "late"]
    );
    assert!(s.agenda_window_coverage("a", "primary", 300, 700).unwrap());
    assert!(!s.agenda_window_coverage("a", "primary", 0, 1000).unwrap());
    s.replace_agenda_window("a", "primary", 300, 700, &[])
        .unwrap();
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap().len(),
        2
    );
}

#[test]
fn event_move_upserts_outside_old_window_without_duplication() {
    let s = store();
    s.replace_agenda_window("a", "primary", 0, 1000, &[event("a", "meeting", 100, 200)])
        .unwrap();
    s.replace_agenda_window(
        "a",
        "primary",
        2000,
        3000,
        &[event("a", "meeting", 2100, 2200)],
    )
    .unwrap();
    assert!(
        s.list_agenda_events(Some("a"), 0, 1000, 100)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 3000, 100).unwrap().len(),
        1
    );
}

#[test]
fn calendar_refresh_preserves_retained_events_and_removes_deleted_calendars() {
    let s = store();
    s.replace_agenda_window("a", "primary", 0, 1000, &[event("a", "meeting", 100, 200)])
        .unwrap();
    let mut renamed = calendar("a");
    renamed.name = "Renamed".into();
    s.replace_calendar_collections("a", &[renamed]).unwrap();
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 1000, 10).unwrap().len(),
        1
    );
    s.replace_calendar_collections("a", &[]).unwrap();
    assert!(
        s.list_agenda_events(Some("a"), 0, 1000, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        s.replace_agenda_window("a", "primary", 0, 1000, &[])
            .is_err()
    );
    assert_eq!(s.list_calendar_collections(Some("b")).unwrap().len(), 1);
}

#[test]
fn availability_merges_busy_overlaps_and_ignores_transparent_cancelled_events() {
    let s = store();
    let mut cancelled = event("a", "cancelled", 0, 30 * MIN);
    cancelled.status = "cancelled".into();
    let mut transparent = event("a", "free", 150 * MIN, 180 * MIN);
    transparent.transparent = true;
    s.replace_agenda_window(
        "a",
        "primary",
        0,
        180 * MIN,
        &[
            cancelled,
            event("a", "busy1", 30 * MIN, 90 * MIN),
            event("a", "busy2", 60 * MIN, 120 * MIN),
            transparent,
        ],
    )
    .unwrap();
    let slots = s.availability_slots(Some("a"), 0, 180 * MIN, 30).unwrap();
    assert_eq!(
        slots
            .iter()
            .map(|s| (s.start_ms / MIN, s.end_ms / MIN))
            .collect::<Vec<_>>(),
        vec![(0, 30), (120, 150), (150, 180)]
    );
    s.replace_agenda_window(
        "b",
        "primary",
        0,
        180 * MIN,
        &[event("b", "busy", 0, 180 * MIN)],
    )
    .unwrap();
    assert!(
        s.availability_slots(None, 0, 180 * MIN, 30)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        s.availability_slots(Some("a"), 0, 180 * MIN, 30)
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn availability_and_event_validation_are_bounded() {
    let s = store();
    assert!(
        s.list_agenda_events(None, 0, 401 * 24 * 60 * MIN, 1)
            .is_err()
    );
    assert!(s.list_agenda_events(None, i64::MIN, i64::MAX, 1).is_err());
    assert!(s.availability_slots(None, 0, 1000, 0).is_err());
    assert!(s.availability_slots(None, 0, 1000, u32::MAX).is_err());
    assert_eq!(
        s.availability_slots(None, 0, 93 * 24 * 60 * MIN, 5)
            .unwrap()
            .len(),
        500
    );
    let mut invalid = event("a", "bad", 100, 200);
    invalid.end_ms = 100;
    assert!(
        s.replace_agenda_window("a", "primary", 0, 1000, &[invalid])
            .is_err()
    );
    assert!(
        s.replace_agenda_window(
            "a",
            "primary",
            0,
            1000,
            &[event("a", "outside", 2000, 3000)]
        )
        .is_err()
    );
    let duplicate = event("a", "same", 100, 200);
    assert!(
        s.replace_agenda_window("a", "primary", 0, 1000, &[duplicate.clone(), duplicate])
            .is_err()
    );
    let mut foreign = calendar("b");
    foreign.id = "foreign".into();
    assert!(s.replace_calendar_collections("a", &[foreign]).is_err());
}

#[test]
fn expired_coverage_does_not_hide_readable_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agenda.sqlite");
    let s = MailStore::open(&path).unwrap();
    account(&s, "a");
    s.replace_calendar_collections("a", &[calendar("a")])
        .unwrap();
    s.replace_agenda_window("a", "primary", 0, 1000, &[event("a", "old", 100, 200)])
        .unwrap();
    assert!(s.agenda_window_coverage("a", "primary", 0, 1000).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE agenda_windows SET refreshed_ms=0", [])
        .unwrap();
    assert!(!s.agenda_window_coverage("a", "primary", 0, 1000).unwrap());
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap().len(),
        1
    );
}

#[test]
fn mutation_attempt_and_single_event_cache_invalidate_coverage_but_keep_events() {
    let s = store();
    s.replace_agenda_window("a", "primary", 0, 1000, &[event("a", "old", 100, 200)])
        .unwrap();
    s.replace_agenda_window("b", "primary", 0, 1000, &[event("b", "other", 100, 200)])
        .unwrap();
    s.invalidate_agenda_coverage("a", "primary").unwrap();
    assert!(!s.agenda_window_coverage("a", "primary", 0, 1000).unwrap());
    assert!(s.agenda_window_coverage("b", "primary", 0, 1000).unwrap());
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap().len(),
        1
    );
    s.replace_agenda_window("a", "primary", 0, 1000, &[event("a", "old", 100, 200)])
        .unwrap();
    s.cache_agenda_event(&event("a", "new", 300, 400)).unwrap();
    assert!(!s.agenda_window_coverage("a", "primary", 0, 1000).unwrap());
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap().len(),
        2
    );
    s.cache_agenda_event(&event("a", "old", 500, 600)).unwrap();
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap()[1].start_ms,
        500
    );
    assert!(s.invalidate_agenda_coverage("unknown", "primary").is_err());
    let mut bad = event("a", "bad", 600, 700);
    bad.calendar_id = "foreign".into();
    assert!(s.cache_agenda_event(&bad).is_err());
}

#[test]
fn availability_ignores_only_own_declined_invitation() {
    let s = store();
    let mut declined = event("a", "declined", 0, 30 * MIN);
    declined.attendees = vec![AgendaAttendee {
        email: "A@example.com".into(),
        name: None,
        response: Some("declined".into()),
    }];
    let mut someone_else = event("a", "otherdeclined", 30 * MIN, 60 * MIN);
    someone_else.attendees = vec![AgendaAttendee {
        email: "b@example.com".into(),
        name: None,
        response: Some("declined".into()),
    }];
    s.replace_agenda_window("a", "primary", 0, 60 * MIN, &[declined, someone_else])
        .unwrap();
    let slots = s.availability_slots(Some("a"), 0, 60 * MIN, 30).unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].start_ms, 0);
}

#[test]
fn invalid_second_calendar_keeps_complete_prior_account_snapshot() {
    let s = store();
    let mut removed = calendar("a");
    removed.id = "removed".into();
    let mut old_removed = event("a", "keep", 200, 300);
    old_removed.calendar_id = "removed".into();
    s.replace_agenda_snapshot(
        "a",
        &[calendar("a"), removed.clone()],
        0,
        1000,
        &[
            ("primary".into(), vec![event("a", "old", 100, 200)]),
            ("removed".into(), vec![old_removed]),
        ],
    )
    .unwrap();
    let before_calendars = s.list_calendar_collections(Some("a")).unwrap();
    let before_events = s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap();
    let mut renamed = calendar("a");
    renamed.name = "Newname".into();
    let mut added = calendar("a");
    added.id = "added".into();
    let mut invalid = event("a", "bad", 400, 500);
    invalid.calendar_id = "added".into();
    invalid.end_ms = invalid.start_ms;
    assert!(
        s.replace_agenda_snapshot(
            "a",
            &[renamed.clone(), added.clone()],
            0,
            1000,
            &[
                ("primary".into(), vec![event("a", "new", 300, 400)]),
                ("added".into(), vec![invalid])
            ]
        )
        .is_err()
    );
    assert_eq!(
        s.list_calendar_collections(Some("a")).unwrap(),
        before_calendars
    );
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap(),
        before_events
    );
    assert!(s.agenda_window_coverage("a", "removed", 0, 1000).unwrap());
    assert!(
        s.replace_agenda_snapshot(
            "a",
            &[renamed.clone(), added.clone()],
            0,
            1000,
            &[("primary".into(), vec![])]
        )
        .is_err()
    );
    assert!(
        s.replace_agenda_snapshot(
            "a",
            &[renamed.clone(), added.clone()],
            0,
            1000,
            &[("primary".into(), vec![]), ("primary".into(), vec![])]
        )
        .is_err()
    );
    assert!(
        s.replace_agenda_snapshot(
            "a",
            &[renamed.clone(), added.clone()],
            0,
            1000,
            &[("primary".into(), vec![]), ("unknown".into(), vec![])]
        )
        .is_err()
    );
    s.replace_agenda_snapshot(
        "a",
        &[renamed, added],
        0,
        1000,
        &[
            ("primary".into(), vec![event("a", "new", 300, 400)]),
            ("added".into(), vec![]),
        ],
    )
    .unwrap();
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap(),
        vec![event("a", "new", 300, 400)]
    );
    assert!(s.agenda_window_coverage("a", "added", 0, 1000).unwrap());
    assert_eq!(
        s.list_calendar_collections(Some("b")).unwrap(),
        vec![calendar("b")]
    );
}

#[test]
fn database_failure_in_second_calendar_rolls_back_first_and_collection_removal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("atomic-agenda.sqlite");
    let s = MailStore::open(&path).unwrap();
    account(&s, "a");
    let mut second = calendar("a");
    second.id = "second".into();
    let mut removed = calendar("a");
    removed.id = "removed".into();
    let mut previous = event("a", "oldsecond", 200, 300);
    previous.calendar_id = "second".into();
    let mut retained = event("a", "oldremoved", 300, 400);
    retained.calendar_id = "removed".into();
    s.replace_agenda_snapshot(
        "a",
        &[calendar("a"), second.clone(), removed.clone()],
        0,
        1000,
        &[
            ("primary".into(), vec![event("a", "oldprimary", 100, 200)]),
            ("second".into(), vec![previous]),
            ("removed".into(), vec![retained]),
        ],
    )
    .unwrap();
    let before_calendars = s.list_calendar_collections(Some("a")).unwrap();
    let before_events = s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_second_insert BEFORE INSERT ON agenda_events WHEN NEW.id='dbfailure' BEGIN SELECT RAISE(ABORT,'synthetic database failure'); END;").unwrap();
    let mut failing = event("a", "dbfailure", 500, 600);
    failing.calendar_id = "second".into();
    assert!(
        s.replace_agenda_snapshot(
            "a",
            &[calendar("a"), second],
            0,
            1000,
            &[
                ("primary".into(), vec![event("a", "newprimary", 400, 500)]),
                ("second".into(), vec![failing])
            ]
        )
        .is_err()
    );
    assert_eq!(
        s.list_calendar_collections(Some("a")).unwrap(),
        before_calendars
    );
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 1000, 100).unwrap(),
        before_events
    );
    assert!(s.agenda_window_coverage("a", "removed", 0, 1000).unwrap());
    s.replace_agenda_snapshot("a", &[], 0, 1000, &[]).unwrap();
    assert!(s.list_calendar_collections(Some("a")).unwrap().is_empty());
    assert!(
        s.list_agenda_events(Some("a"), 0, 1000, 100)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn year_snapshot_is_readable_and_month_refresh_preserves_other_months() {
    let s = store();
    let day = 24 * 60 * MIN;
    let annual = vec![
        event("a", "january", day, 2 * day),
        event("a", "june", 155 * day, 156 * day),
        event("a", "december", 360 * day, 361 * day),
    ];
    s.replace_agenda_snapshot(
        "a",
        &[calendar("a")],
        0,
        366 * day,
        &[("primary".into(), annual.clone())],
    )
    .unwrap();
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 366 * day, 10000)
            .unwrap(),
        annual
    );
    assert!(
        s.agenda_window_coverage("a", "primary", 0, 366 * day)
            .unwrap()
    );
    // Empty successful June refresh deletes only overlapping June events.
    s.replace_agenda_snapshot(
        "a",
        &[calendar("a")],
        150 * day,
        180 * day,
        &[("primary".into(), vec![])],
    )
    .unwrap();
    let remaining = s
        .list_agenda_events(Some("a"), 0, 366 * day, 10000)
        .unwrap();
    assert_eq!(
        remaining.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        vec!["january", "december"]
    );
    // A partial refresh must not certify the rest of the year as fresh.
    assert!(
        !s.agenda_window_coverage("a", "primary", 0, 366 * day)
            .unwrap()
    );
    assert!(
        s.agenda_window_coverage("a", "primary", 150 * day, 180 * day)
            .unwrap()
    );
    assert!(s.availability_slots(Some("a"), 0, 366 * day, 30).is_err());
}

#[test]
fn complete_year_read_rejects_overflow_instead_of_hiding_events() {
    let s = store();
    let day = 24 * 60 * MIN;
    let january = (0..10000)
        .map(|i| event("a", &format!("january-{i}"), day, 2 * day))
        .collect::<Vec<_>>();
    s.replace_agenda_window("a", "primary", 0, 31 * day, &january)
        .unwrap();
    s.replace_agenda_window(
        "a",
        "primary",
        31 * day,
        60 * day,
        &[event("a", "february", 40 * day, 41 * day)],
    )
    .unwrap();
    assert!(
        s.list_agenda_events(Some("a"), 0, 366 * day, 10000)
            .is_err()
    );
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 31 * day, 10000)
            .unwrap()
            .len(),
        10000
    );
    assert_eq!(
        s.list_agenda_events(Some("a"), 0, 366 * day, 200)
            .unwrap()
            .len(),
        200
    );
}

#[test]
fn legacy_cached_events_do_not_silently_gain_edit_permission() {
    let original = event("a", "event", 0, 60_000);
    let mut value = serde_json::to_value(&original).unwrap();
    value.as_object_mut().unwrap().remove("can_edit");
    value.as_object_mut().unwrap().remove("recurring_event_id");
    let restored: AgendaEvent = serde_json::from_value(value).unwrap();
    assert!(!restored.can_edit);
    assert!(restored.recurring_event_id.is_none());
}
