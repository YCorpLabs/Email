//! View filtering never changes provider data. Shared identity is stronger than a matching name.
use mail_core::AgendaEvent;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Deserialize)]
pub(super) struct CalendarSource {
    pub account_id: String,
    pub calendar_id: String,
}

pub(super) fn validate_sources(sources: &[CalendarSource]) -> Result<(), String> {
    if sources.len() > 2048
        || sources.iter().any(|s| {
            [&s.account_id, &s.calendar_id]
                .into_iter()
                .any(|v| v.is_empty() || v.len() > 2048 || v.chars().any(char::is_control))
        })
    {
        return Err("Invalid calendar visibility selection".into());
    }
    Ok(())
}

pub(super) fn visible_events(
    events: Vec<AgendaEvent>,
    hidden: &[CalendarSource],
    deduplicate: bool,
    limit: usize,
) -> Vec<AgendaEvent> {
    let hidden: HashSet<_> = hidden
        .iter()
        .map(|s| (s.account_id.as_str(), s.calendar_id.as_str()))
        .collect();
    let mut seen: HashMap<(String, String, i64, i64), usize> = HashMap::new();
    let mut visible: Vec<AgendaEvent> = Vec::new();
    for event in events {
        if hidden.contains(&(event.account_id.as_str(), event.calendar_id.as_str())) {
            continue;
        }
        // The primary alias is account-relative; a title or a holiday-like name proves nothing.
        if deduplicate && event.calendar_id != "primary" {
            let key = (
                event.calendar_id.clone(),
                event.id.clone(),
                event.start_ms,
                event.end_ms,
            );
            if let Some(&index) = seen.get(&key) {
                if visible[index].account_id != event.account_id {
                    if event.can_edit && !visible[index].can_edit {
                        visible[index] = event;
                    }
                    continue;
                }
            } else {
                seen.insert(key, visible.len());
            }
        }
        visible.push(event);
    }
    visible.truncate(limit);
    visible
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(account: &str, calendar: &str, id: &str) -> AgendaEvent {
        serde_json::from_value(serde_json::json!({"account_id":account,"calendar_id":calendar,"id":id,"title":"Holiday","start_ms":100,"end_ms":200,"all_day":true,"status":"confirmed","attendees":[]})).unwrap()
    }
    #[test]
    fn shared_copy_is_shown_once_but_distinct_calendars_and_primary_aliases_remain() {
        let events = vec![
            event("a", "norway@holiday", "christmas"),
            event("b", "norway@holiday", "christmas"),
            event("b", "work", "christmas"),
            event("a", "primary", "same"),
            event("b", "primary", "same"),
        ];
        assert_eq!(visible_events(events, &[], true, 500).len(), 4);
    }
    #[test]
    fn visibility_is_applied_before_dedup_and_editable_copy_wins() {
        let first = event("a", "shared", "one");
        let mut second = event("b", "shared", "one");
        second.can_edit = true;
        assert_eq!(
            visible_events(vec![first.clone(), second.clone()], &[], true, 500)[0].account_id,
            "b"
        );
        let hidden = vec![CalendarSource {
            account_id: "a".into(),
            calendar_id: "shared".into(),
        }];
        assert_eq!(
            visible_events(vec![first, second], &hidden, true, 500)[0].account_id,
            "b"
        );
    }
    #[test]
    fn occurrence_time_and_event_identity_are_never_collapsed_by_title() {
        let first = event("a", "shared", "one");
        let mut moved = event("b", "shared", "one");
        moved.start_ms = 120;
        assert_eq!(
            visible_events(
                vec![first.clone(), moved, event("b", "shared", "two")],
                &[],
                true,
                500
            )
            .len(),
            3
        );
        assert_eq!(
            visible_events(vec![first.clone(), first], &[], false, 1).len(),
            1
        );
    }
}
