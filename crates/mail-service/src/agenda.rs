//! Connected calendars keep OAuth credentials separate from mail credentials.
use super::*;

impl MailService {
    pub(super) fn calendar_vault_id(account_id: &str) -> String {
        format!("calendar:{account_id}")
    }

    pub(super) fn calendar_status(&self, account_id: &str) -> Result<Value, String> {
        let account = self.account(account_id)?;
        let connected = self
            .saved_config(account_id)
            .map(|c| c.google_calendar_enabled)
            .unwrap_or(false);
        Ok(
            json!({"connected":connected,"supported":account.provider == Provider::Gmail,"provider":"google"}),
        )
    }

    pub(super) fn connect_calendar(&self, account_id: &str) -> Result<Value, String> {
        let account = self.account(account_id)?;
        let saved = self.saved_config(account_id)?;
        if saved.provider != Provider::Gmail {
            return Err("Calendar connection currently supports Google accounts.".into());
        }
        let config = OAuthConfig {
            provider: CloudProvider::Gmail,
            client_id: saved.client_id.clone().ok_or("Google client ID missing")?,
            client_secret: saved.client_secret.clone(),
        };
        // This is called only from the explicit Connect Calendar UI. Never during mail sync.
        let tokens = mail_cloud::authorize_google_calendar(&config, &account.email)
            .map_err(|e| e.to_string())?;
        if tokens.refresh_token.is_none() {
            return Err(
                "Google did not issue offline calendar access. Please reconnect Calendar.".into(),
            );
        }
        let client = CloudClient::new(config, tokens).map_err(|e| e.to_string())?;
        let (_, email) = client
            .google_calendar_identity()
            .map_err(|e| e.to_string())?;
        if !email.eq_ignore_ascii_case(&account.email) {
            return Err(
                "Choose the same Google account as this mailbox. Calendar was not connected."
                    .into(),
            );
        }
        self.commit_scoped_grant(&saved,true,client.tokens())?;
        self.calendar_status(account_id)
    }

    fn calendar_client(&self, account_id: &str) -> Result<AccountCloudClient, String> {
        let saved = self.saved_config(account_id)?;
        if saved.provider != Provider::Gmail || !saved.google_calendar_enabled {
            return Err("Connect Google Calendar first. Your email remains available.".into());
        }
        self.client_for_generation(&saved, true)
    }
    fn save_calendar_tokens(
        &self,
        account_id: &str,
        client: &AccountCloudClient,
    ) -> Result<(), String> {
        if account_id != client.account_id {
            return Err("Account authorization changed".into());
        }
        self.commit_client_tokens(client, true)
    }
    pub(super) fn sync_calendar(
        &self,
        account_id: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Value, String> {
        validate_listing_window(start_ms, end_ms)?;
        let client = self.calendar_client(account_id)?;
        let result = (|| {
            let calendars = client
                .google_calendars(account_id)
                .map_err(|e| e.to_string())?;
            if calendars.len() > 32 {
                return Err("More than 32 calendars are connected. Calendar selection is needed before syncing this account.".into());
            }
            // Fetch everything before replacing the cache: failed provider requests retain saved data.
            let mut windows = Vec::new();
            for calendar in &calendars {
                windows.push((
                    calendar.id.clone(),
                    client
                        .google_calendar_events(account_id, &calendar.id, start_ms, end_ms)
                        .map_err(|e| e.to_string())?,
                ));
            }
            self.store
                .replace_agenda_snapshot(account_id, &calendars, start_ms, end_ms, &windows)
                .map_err(|e| e.to_string())?;
            self.value(
                self.store
                    .list_agenda_events(Some(account_id), start_ms, end_ms, 10000),
            )
        })();
        let saved = self.save_calendar_tokens(account_id, &client);
        result.and_then(|value| saved.map(|_| value))
    }

    fn writable_calendar(&self, account_id: &str, calendar_id: &str) -> Result<(), String> {
        self.account(account_id)?;
        if !self
            .store
            .list_calendar_collections(Some(account_id))
            .map_err(|e| e.to_string())?
            .iter()
            .any(|c| c.id == calendar_id && c.can_write)
        {
            return Err(
                "Refresh Calendar and select a writable calendar belonging to this account.".into(),
            );
        }
        Ok(())
    }

    pub(super) fn respond_calendar(
        &self,
        account_id: &str,
        calendar_id: &str,
        event_id: &str,
        response: &str,
    ) -> Result<Value, String> {
        if !matches!(response, "accepted" | "declined" | "tentative") {
            return Err("Choose Yes, Maybe or No.".into());
        }
        self.writable_calendar(account_id, calendar_id)?;
        let client = self.calendar_client(account_id)?;
        self.store
            .invalidate_agenda_coverage(account_id, calendar_id)
            .map_err(|e| e.to_string())?;
        let result = client
            .google_calendar_respond(calendar_id, event_id, response)
            .map_err(|e| e.to_string());
        let saved = self.save_calendar_tokens(account_id, &client);
        result?;
        saved?;
        Ok(json!({"sent":true,"response":response,"refresh_required":true}))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn create_calendar_event(
        &self,
        account_id: &str,
        calendar_id: &str,
        title: &str,
        start_ms: i64,
        end_ms: i64,
        attendee_emails: &[String],
        description: Option<&str>,
        location: Option<&str>,
    ) -> Result<Value, String> {
        validate_window(start_ms, end_ms)?;
        self.writable_calendar(account_id, calendar_id)?;
        let client = self.calendar_client(account_id)?;
        self.store
            .invalidate_agenda_coverage(account_id, calendar_id)
            .map_err(|e| e.to_string())?;
        let result = client
            .google_calendar_create_with_location(
                account_id,
                calendar_id,
                title,
                start_ms,
                end_ms,
                attendee_emails,
                description,
                location,
            )
            .map_err(|e| e.to_string());
        let saved = self.save_calendar_tokens(account_id, &client);
        let event=result.map_err(|error|format!("Creation was not confirmed. Check Google Calendar before trying again; the event may have been created. {error}"))?;
        if let Err(error) = saved {
            return Ok(
                json!({"created":true,"event":event,"refresh_required":true,"warning":format!("Event created, but refreshed credentials could not be saved: {error}")}),
            );
        }
        let cache_warning = self
            .store
            .cache_agenda_event(&event)
            .err()
            .map(|e| e.to_string());
        // Never retry a create automatically: a failed response could follow a successful write.
        Ok(
            json!({"created":true,"event":event,"refresh_required":true,"cache_warning":cache_warning}),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn update_calendar_event(
        &self,
        account_id: &str,
        calendar_id: &str,
        event_id: &str,
        expected_etag: &str,
        edit_scope: &str,
        changes: mail_cloud::CalendarEventChanges,
    ) -> Result<Value, String> {
        changes
            .validate(expected_etag, edit_scope)
            .map_err(|e| e.to_string())?;
        self.writable_calendar(account_id, calendar_id)?;
        let client = self.calendar_client(account_id)?;
        self.store
            .invalidate_agenda_coverage(account_id, calendar_id)
            .map_err(|e| e.to_string())?;
        let result = client.google_calendar_update(
            account_id,
            calendar_id,
            event_id,
            expected_etag,
            edit_scope,
            &changes,
        );
        let saved = self.save_calendar_tokens(account_id, &client);
        let event = result.map_err(|error| match error {
            mail_cloud::CloudError::CalendarConflict => error.to_string(),
            _ => format!("Update was not confirmed. Refresh Google Calendar before trying again; the change may have been saved. {error}"),
        })?;
        let warning = saved
            .err()
            .map(|e| format!("Event updated, but refreshed credentials could not be saved: {e}"));
        let cache_warning = self
            .store
            .cache_agenda_event(&event)
            .err()
            .map(|e| e.to_string());
        Ok(
            json!({"updated":true,"event":event,"refresh_required":true,"warning":warning,"cache_warning":cache_warning}),
        )
    }

    pub(super) fn calendar_availability(
        &self,
        account_id: Option<&str>,
        start_ms: i64,
        end_ms: i64,
        duration_minutes: u32,
    ) -> Result<Value, String> {
        validate_window(start_ms, end_ms)?;
        let calendars = self
            .store
            .list_calendar_collections(account_id)
            .map_err(|e| e.to_string())?;
        let mut complete = !calendars.is_empty();
        for c in &calendars {
            complete &= self
                .store
                .agenda_window_coverage(&c.account_id, &c.id, start_ms, end_ms)
                .map_err(|e| e.to_string())?;
        }
        // Cache coverage cannot prove there are no unconnected calendars. Say precisely what was checked.
        let slots = self
            .store
            .availability_slots(account_id, start_ms, end_ms, duration_minutes)
            .map_err(|e| e.to_string())?;
        Ok(
            json!({"slots":slots,"coverage_complete":complete,"source":"cached_calendars","calendars_checked":calendars.len(),"notice":"Suggestions from cached connected calendars only; refresh and review before sharing. Other calendars may contain conflicts."}),
        )
    }
}

fn validate_window(start: i64, end: i64) -> Result<(), String> {
    if start < 0 || end <= start || end.saturating_sub(start) > 93 * 24 * 60 * 60 * 1000 {
        Err("Choose a calendar range between one moment and 93 days.".into())
    } else {
        Ok(())
    }
}

fn validate_listing_window(start: i64, end: i64) -> Result<(), String> {
    if start < 0 || end <= start || end.saturating_sub(start) > 400 * 24 * 60 * 60 * 1000 {
        Err("Choose a calendar range between one moment and 400 days.".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod range_tests {
    use super::{validate_listing_window, validate_window};

    #[test]
    fn year_listing_does_not_expand_mutation_or_availability_range() {
        let day = 86_400_000;
        assert!(validate_listing_window(0, 366 * day).is_ok());
        assert!(validate_listing_window(0, 400 * day).is_ok());
        assert!(validate_listing_window(0, 400 * day + 1).is_err());
        assert!(validate_listing_window(-1, day).is_err());
        assert!(validate_listing_window(i64::MIN, i64::MAX).is_err());
        assert!(validate_window(0, 94 * day).is_err());
        assert!(validate_window(0, 93 * day).is_ok());
    }
}
