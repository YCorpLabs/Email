//! Explicit scheduling-settings authorization and reviewable configuration plans.
use super::*;

const PLAN_TTL_MS: u64 = 10 * 60 * 1000;
#[derive(Clone)]
struct SetupPlan {
    host_id: String,
    patch: Value,
    revision: String,
    expires_at: u64,
    attempted: bool,
    result: Option<Value>,
}
fn plans() -> &'static Mutex<std::collections::HashMap<(std::path::PathBuf, String), SetupPlan>> {
    static PLANS: OnceLock<
        Mutex<std::collections::HashMap<(std::path::PathBuf, String), SetupPlan>>,
    > = OnceLock::new();
    PLANS.get_or_init(Default::default)
}
fn settings_permission(value: &Secrets) -> bool {
    bearer(value).is_ok() && value.settings_write
}
fn require_settings_permission(value: &Secrets) -> Result<(), String> {
    if settings_permission(value) {
        Ok(())
    } else {
        Err("Scheduling setup requires the separately approved booking:settings permission. Start setup authorization and approve it in the browser.".into())
    }
}
fn setup_call(
    api: &Api,
    secret: &Secrets,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let token = bearer(secret)?;
    let (status, data) = api.call(method, path, Some(token), body)?;
    match status {
        200=>Ok(data),
        401=>{invalidate_rejected_token(token)?;Err(status_error(status))},
        403 if data.get("code").and_then(Value::as_str)==Some("settings_permission_required")=>Err("Scheduling setup requires separately approved booking:settings permission. Existing read access is unchanged.".into()),
        409=>Err("Scheduling settings changed. Read current settings and create a new preview before applying.".into()),
        400|422=>Err("Scheduling configuration was rejected. Check the preview requirements and calendar selection.".into()),
        _=>Err(status_error(status)),
    }
}
fn revision(value: &Value) -> Result<String, String> {
    value
        .get("revision")
        .and_then(Value::as_str)
        .filter(|s| {
            (32..=128).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .map(str::to_owned)
        .ok_or_else(|| "Scheduling service did not provide a valid settings revision.".into())
}
fn safe_value(value: &Value, depth: usize) -> Value {
    if depth > 12 {
        return Value::Null;
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| {
                    let lower = key.to_ascii_lowercase();
                    ![
                        "token",
                        "secret",
                        "password",
                        "grant",
                        "authorization",
                        "managementurl",
                        "capability",
                    ]
                    .iter()
                    .any(|needle| lower.contains(needle))
                })
                .take(100)
                .map(|(k, v)| (k.clone(), safe_value(v, depth + 1)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(256)
                .map(|v| safe_value(v, depth + 1))
                .collect(),
        ),
        Value::String(text) => json!(text.chars().take(4000).collect::<String>()),
        _ => value.clone(),
    }
}
fn settings_snapshot(data: &Value) -> Value {
    let profile = selected(
        &data["profile"],
        &["id", "name", "email", "company"],
        &[],
        &[],
    );
    let accounts: Vec<_> = data
        .get("accounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(20)
        .map(|a| {
            let mut account = selected(
                a,
                &["id", "email", "destinationCalendarId"],
                &[],
                &["connected"],
            );
            if let Some(ids) = a.get("conflictCalendarIds").and_then(Value::as_array) {
                account["conflictCalendarIds"] = json!(
                    ids.iter()
                        .filter_map(Value::as_str)
                        .filter(|s| s.len() <= 1024)
                        .take(50)
                        .collect::<Vec<_>>()
                );
            }
            account
        })
        .collect();
    let meetings: Vec<_> = data
        .get("meetingTypes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(20)
        .map(|t| {
            let fields = [
                "id",
                "slug",
                "published",
                "host",
                "company",
                "title",
                "description",
                "duration",
                "zone",
                "startHour",
                "endHour",
                "buffer",
                "bufferBefore",
                "bufferAfter",
                "slotInterval",
                "notice",
                "horizon",
                "days",
                "accent",
                "location",
                "locationType",
                "question",
                "required",
                "reminder",
                "reminderMinutes",
                "followUpMinutes",
                "followUpSubject",
                "followUpBody",
                "theme",
                "accountId",
                "calendarId",
                "useDefaultCalendar",
                "weeklyHours",
                "dateOverrides",
                "scheduleId",
                "useDefaultSchedule",
                "dailyBookingLimit",
                "weeklyBookingLimit",
                "dailyBookedMinutesLimit",
                "weeklyBookedMinutesLimit",
                "monthlyBookingLimit",
                "yearlyBookingLimit",
                "monthlyBookedMinutesLimit",
                "yearlyBookedMinutesLimit",
                "questions",
                "extraGuestsLimit",
            ];
            let mut meeting = Value::Object(
                fields
                    .iter()
                    .filter_map(|key| t.get(key).map(|v| ((*key).to_owned(), safe_value(v, 0))))
                    .collect(),
            );
            meeting["useDefaultCalendar"] = json!(
                t.get("useDefaultCalendar")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            );
            meeting["useDefaultSchedule"] = json!(
                t.get("useDefaultSchedule")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            );
            meeting
        })
        .collect();
    let schedules: Vec<_> = data
        .get("availabilitySchedules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(20)
        .map(|schedule| {
            let fields = ["id", "name", "zone", "weeklyHours", "dateOverrides"];
            Value::Object(
                fields
                    .iter()
                    .filter_map(|key| {
                        schedule
                            .get(key)
                            .map(|value| ((*key).to_owned(), safe_value(value, 0)))
                    })
                    .collect(),
            )
        })
        .collect();
    json!({"profile":profile,"accounts":accounts,"meetingTypes":meetings,"availabilitySchedules":schedules,"defaultAvailabilityScheduleId":data.get("defaultAvailabilityScheduleId").and_then(schedule_reference)})
}
fn calendar_inventory(data: &Value) -> Value {
    let accounts: Vec<_> = data
        .get("accounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(20)
        .map(|a| {
            let mut account = selected(a, &["id", "email"], &[], &[]);
            let calendars: Vec<_> = a
                .get("calendars")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(500)
                .map(|c| {
                    selected(
                        c,
                        &[
                            "id",
                            "summary",
                            "description",
                            "timeZone",
                            "accessRole",
                            "backgroundColor",
                            "foregroundColor",
                        ],
                        &[],
                        &["primary", "selected", "hidden", "deleted"],
                    )
                })
                .collect();
            account["calendars"] = json!(calendars);
            account
        })
        .collect();
    json!({"accounts":accounts})
}
fn validate_patch(patch: &Value) -> Result<(), String> {
    let object = patch
        .as_object()
        .ok_or("Supply a scheduling settings patch object.")?;
    if object.is_empty()
        || object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "profile"
                    | "meetingTypes"
                    | "availabilitySchedules"
                    | "defaultAvailabilityScheduleId"
            )
        })
    {
        return Err("Only profile name/company, meetingTypes, availabilitySchedules and defaultAvailabilityScheduleId can be configured.".into());
    }
    if let Some(id) = object.get("defaultAvailabilityScheduleId") {
        if !id.is_null() && schedule_reference(id).is_none() {
            return Err("defaultAvailabilityScheduleId must be null or a schedule identifier of up to 64 letters, digits, underscores or hyphens.".into());
        }
    }
    if let Some(profile) = object.get("profile") {
        let fields = profile.as_object().ok_or("Profile must be an object.")?;
        if fields
            .keys()
            .any(|key| !matches!(key.as_str(), "name" | "company"))
            || fields.values().any(|v| !v.is_string())
        {
            return Err("Profile changes allow name and company text only.".into());
        }
    }
    if let Some(meetings) = object.get("meetingTypes") {
        if !meetings
            .as_array()
            .is_some_and(|items| items.len() <= 20 && items.iter().all(Value::is_object))
        {
            return Err(
                "meetingTypes must be the complete array of at most 20 meeting types.".into(),
            );
        }
    }
    if let Some(meetings) = object.get("meetingTypes").and_then(Value::as_array) {
        for key in ["useDefaultSchedule", "useDefaultCalendar"] {
            if meetings
                .iter()
                .any(|meeting| meeting.get(key).is_some_and(|value| !value.is_boolean()))
            {
                return Err(format!("{key} must be true or false when supplied."));
            }
        }
    }
    if let Some(meetings) = object.get("meetingTypes").and_then(Value::as_array) {
        for meeting in meetings {
            if meeting.get("followUpMinutes").is_some_and(|value| {
                !value.is_null()
                    && !value
                        .as_u64()
                        .is_some_and(|minutes| (5..=10080).contains(&minutes))
            }) {
                return Err(
                    "followUpMinutes must be null or an integer between 5 and 10080.".into(),
                );
            }
            for (key, max, multiline) in [
                ("followUpSubject", 160, false),
                ("followUpBody", 4000, true),
            ] {
                if meeting.get(key).is_some_and(|value| {
                    !value.as_str().is_some_and(|text| {
                        text.encode_utf16().count() <= max
                            && (multiline || !text.chars().any(char::is_control))
                    })
                }) {
                    return Err(
                        "Follow-up templates must be plain text within their subject/body limits."
                            .into(),
                    );
                }
            }
        }
    }
    if let Some(meetings) = object.get("meetingTypes").and_then(Value::as_array) {
        for meeting in meetings {
            for (key, max) in [
                ("dailyBookedMinutesLimit", 1440),
                ("weeklyBookedMinutesLimit", 10080),
                ("monthlyBookedMinutesLimit", 44640),
                ("yearlyBookedMinutesLimit", 527040),
            ] {
                if meeting.get(key).is_some_and(|value| {
                    !value
                        .as_u64()
                        .is_some_and(|minutes| minutes <= max && minutes % 5 == 0)
                }) {
                    return Err("Booked-minute limits must be integers in five-minute steps: daily 0–1440, weekly 0–10080, monthly 0–44640, yearly 0–527040; zero disables the limit.".into());
                }
            }
        }
    }
    if let Some(meetings) = object.get("meetingTypes").and_then(Value::as_array) {
        for meeting in meetings {
            for (key, max) in [("monthlyBookingLimit", 1500), ("yearlyBookingLimit", 18000)] {
                if meeting
                    .get(key)
                    .is_some_and(|value| !value.as_u64().is_some_and(|count| count <= max))
                {
                    return Err("Monthly/yearly booking-count limits must be integers: monthly 0–1500, yearly 0–18000; zero disables the limit.".into());
                }
            }
        }
    }
    if let Some(schedules) = object.get("availabilitySchedules") {
        if !schedules
            .as_array()
            .is_some_and(|items| items.len() <= 20 && items.iter().all(Value::is_object))
        {
            return Err(
                "availabilitySchedules must be the complete array of at most 20 schedules.".into(),
            );
        }
    }
    if serde_json::to_vec(patch)
        .map_err(|_| "Invalid scheduling patch.")?
        .len()
        > 100_000
    {
        return Err("Scheduling configuration is too large.".into());
    }
    Ok(())
}
fn reviewable_preview(data: &Value) -> Value {
    let changes:Vec<_>=data.get("changes").and_then(Value::as_array).into_iter().flatten().take(200).map(|c|json!({"path":string(c,"path",256).unwrap_or_default(),"before":safe_value(&c["before"],0),"after":safe_value(&c["after"],0)})).collect();
    let warnings: Vec<_> = data
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|s| s.chars().take(500).collect::<String>())
        .take(30)
        .collect();
    json!({"settings":settings_snapshot(&data["settings"]),"changes":changes,"warnings":warnings})
}
fn check_plan(plan: &SetupPlan, host: Option<&str>, time: u64) -> Result<(), String> {
    if plan.expires_at <= time {
        return Err("Scheduling preview expired. Create a new preview.".into());
    }
    if host != Some(plan.host_id.as_str()) {
        return Err(
            "Scheduling account changed. Create a new preview for the connected account.".into(),
        );
    }
    if plan.attempted && plan.result.is_none() {
        return Err("The prior settings apply outcome needs review. Read current settings and create a new preview; do not retry the old plan.".into());
    }
    Ok(())
}
impl MailService {
    pub(crate) fn booking_setup_status(&self) -> Result<Value, String> {
        let origin = super::origin()?;
        let _guard = lock()?;
        let secret = secrets()?;
        let connected = bearer(&secret).is_ok();
        Ok(
            json!({"connected":connected,"settings_write":settings_permission(&secret),"required_permission":"booking:settings","requires_authorization":!settings_permission(&secret),"service_url":origin,"setup_url":format!("{origin}/studio"),"calendar_connections":"Browser-managed Google connections; MCP cannot request OAuth grants or connect calendars."}),
        )
    }
    pub(crate) fn booking_setup_start(&self, name: Option<String>) -> Result<Value, String> {
        self.booking_connect_start_with_settings(
            Some(name.unwrap_or_else(|| "Email MCP scheduling setup".into())),
            true,
        )
    }
    pub(crate) fn booking_setup_poll(&self) -> Result<Value, String> {
        {
            let pending = pending_connections()
                .lock()
                .map_err(|_| "Booking connection is busy.")?;
            if !pending
                .get(&self.config_path)
                .is_some_and(|p| p.settings_write)
            {
                return Err("Start an explicit scheduling settings authorization first.".into());
            }
        }
        self.booking_connect_poll()
    }
    pub(crate) fn booking_setup_read(&self) -> Result<Value, String> {
        let _guard = lock()?;
        let secret = secrets()?;
        let api = Api::new()?;
        let data = setup_call(&api, &secret, Method::GET, "/api/device/settings", None)?;
        let calendars = if settings_permission(&secret) {
            calendar_inventory(&setup_call(
                &api,
                &secret,
                Method::GET,
                "/api/device/calendars",
                None,
            )?)
        } else {
            json!({"accounts":[],"requires_authorization":true})
        };
        Ok(
            json!({"settings":settings_snapshot(&data),"revision":revision(&data)?,"calendars":calendars,"settings_write":settings_permission(&secret),"required_permission":"booking:settings"}),
        )
    }
    pub(crate) fn booking_setup_preview(&self, patch: Value) -> Result<Value, String> {
        validate_patch(&patch)?;
        let _guard = lock()?;
        let secret = secrets()?;
        let api = Api::new()?;
        let host = host_identity(&api.get("/api/device/me", bearer(&secret)?)?)?;
        let host_id = host["host_id"]
            .as_str()
            .ok_or("Connect scheduling before previewing settings.")?
            .to_owned();
        let data = setup_call(
            &api,
            &secret,
            Method::POST,
            "/api/device/settings/preview",
            Some(json!({"patch":patch})),
        )?;
        let revision = revision(&data)?;
        let patch = data
            .get("patch")
            .cloned()
            .ok_or("Scheduling preview did not provide a normalized patch.")?;
        validate_patch(&patch)?;
        let id = Uuid::new_v4().to_string();
        let expires_at = now() + PLAN_TTL_MS;
        let mut registry = plans().lock().map_err(|_| "Scheduling preview is busy.")?;
        registry.retain(|_, plan| plan.expires_at > now());
        if registry.len() >= 100 {
            return Err(
                "Too many pending scheduling previews. Wait for an old preview to expire.".into(),
            );
        }
        registry.insert(
            (self.config_path.clone(), id.clone()),
            SetupPlan {
                host_id,
                patch,
                revision: revision.clone(),
                expires_at,
                attempted: false,
                result: None,
            },
        );
        let mut result = reviewable_preview(&data);
        result["plan_id"] = json!(id);
        result["revision"] = json!(revision);
        result["expires_at"] = json!(expires_at);
        result["settings_write"] = json!(settings_permission(&secret));
        result["applied"] = json!(false);
        Ok(result)
    }
    pub(crate) fn booking_setup_apply(
        &self,
        plan_id: &str,
        confirm: bool,
    ) -> Result<Value, String> {
        if !confirm {
            return Err(
                "Review the scheduling preview and explicitly confirm this plan before applying."
                    .into(),
            );
        }
        if Uuid::parse_str(plan_id).is_err() {
            return Err("Scheduling preview identifier is invalid.".into());
        }
        let _guard = lock()?;
        let secret = secrets()?;
        require_settings_permission(&secret)?;
        let key = (self.config_path.clone(), plan_id.to_owned());
        let mut registry = plans().lock().map_err(|_| "Scheduling preview is busy.")?;
        let plan = registry
            .get_mut(&key)
            .ok_or("Scheduling preview was not found in this MCP session. Create a new preview.")?;
        check_plan(plan, secret.host_id.as_deref(), now())?;
        if let Some(result) = &plan.result {
            let mut result = result.clone();
            result["replayed"] = json!(true);
            return Ok(result);
        }
        // Never automatically replay a write whose network outcome is unknown.
        plan.attempted = true;
        let data = setup_call(
            &Api::new()?,
            &secret,
            Method::POST,
            "/api/device/settings",
            Some(json!({"patch":plan.patch,"expectedRevision":plan.revision})),
        )?;
        let settings = if data.get("settings").is_some() {
            &data["settings"]
        } else {
            &data
        };
        let mut result = json!({"applied":true,"plan_id":plan_id,"settings":settings_snapshot(settings),"revision":revision(&data)?});
        // Acceptance is memoized before local cache work, which cannot turn a
        // confirmed remote update into an instruction to send the patch again.
        plan.result = Some(result.clone());
        if let Ok(mut cache) = account_cache(&self.booking_cache_path(), &secret) {
            cache.meeting_types = types(settings);
            profile(&mut cache.host, settings);
            if save_cache(&self.booking_cache_path(), &cache).is_err() {
                result["cache_warning"] =
                    json!("Settings were applied; refresh the desktop snapshot.");
            }
        }
        plan.result = Some(result.clone());
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn per_link_calendar_choice_survives_unrelated_mcp_edits_without_matching_account_default() {
        for flag in [false, true] {
            let original = json!({"id":"intro","slug":"intro","title":"Original","accountId":"account","calendarId":"custom@example.test","useDefaultCalendar":flag});
            let data = json!({"accounts":[{"id":"account","destinationCalendarId":"default@example.test"}],"meetingTypes":[original.clone()]});
            let mut read = settings_snapshot(&data);
            read["meetingTypes"][0]["title"] = json!("Reviewed title");
            let patch = json!({"meetingTypes":read["meetingTypes"]});
            validate_patch(&patch).unwrap();
            let plan = SetupPlan {
                host_id: "host".into(),
                patch: patch.clone(),
                revision: "reviewed-revision".into(),
                expires_at: 200,
                attempted: false,
                result: None,
            };
            let request = json!({"patch":plan.patch,"expectedRevision":plan.revision});
            let reviewed = reviewable_preview(
                &json!({"settings":request["patch"],"changes":[],"warnings":[]}),
            );
            let applied = settings_snapshot(&reviewed["settings"]);
            let native = types(&applied);
            for key in ["accountId", "calendarId", "useDefaultCalendar"] {
                for meeting in [
                    &read["meetingTypes"][0],
                    &request["patch"]["meetingTypes"][0],
                    &reviewed["settings"]["meetingTypes"][0],
                    &applied["meetingTypes"][0],
                    &native[0],
                ] {
                    assert_eq!(
                        meeting.get(key),
                        original.get(key),
                        "destination field {key} was clobbered"
                    );
                }
            }
            assert_eq!(applied["meetingTypes"][0]["title"], "Reviewed title");
        }
    }

    #[test]
    fn legacy_calendar_defaults_true_while_pinned_false_stays_false_and_nonbool_is_rejected() {
        let data = json!({"meetingTypes":[{"id":"legacy","slug":"legacy","accountId":"account","calendarId":"default"},{"id":"pinned","slug":"pinned","accountId":"account","calendarId":"custom","useDefaultCalendar":false}]});
        let read = settings_snapshot(&data);
        let native = types(&data);
        assert_eq!(read["meetingTypes"][0]["useDefaultCalendar"], true);
        assert_eq!(native[0]["useDefaultCalendar"], true);
        assert_eq!(read["meetingTypes"][1]["useDefaultCalendar"], false);
        assert_eq!(native[1]["useDefaultCalendar"], false);
        assert_eq!(read["meetingTypes"][1]["calendarId"], "custom");
        assert!(
            validate_patch(&json!({"meetingTypes":[{"id":"pinned","calendarId":"custom"}]}))
                .is_ok(),
            "older omission is passed through for server-side preservation"
        );
        for value in [Value::Null, json!(0), json!("false"), json!({}), json!([])] {
            assert!(
                validate_patch(&json!({"meetingTypes":[{"useDefaultCalendar":value}]})).is_err()
            );
        }
        for value in [false, true] {
            assert!(validate_patch(&json!({"meetingTypes":[{"accountId":"account","calendarId":"custom","useDefaultCalendar":value}]})).is_ok());
        }
    }

    #[test]
    fn monthly_yearly_caps_survive_review_apply_and_do_not_clobber_other_periods() {
        for (monthly_count, yearly_count, monthly_minutes, yearly_minutes) in
            [(0, 0, 0, 0), (1, 1, 5, 5), (1500, 18000, 44640, 527040)]
        {
            let original = json!({"id":"intro","slug":"intro","title":"Original","monthlyBookingLimit":monthly_count,"yearlyBookingLimit":yearly_count,"monthlyBookedMinutesLimit":monthly_minutes,"yearlyBookedMinutesLimit":yearly_minutes,"dailyBookingLimit":4,"weeklyBookingLimit":20,"dailyBookedMinutesLimit":120,"weeklyBookedMinutesLimit":600});
            let mut read = settings_snapshot(&json!({"meetingTypes":[original.clone()]}));
            read["meetingTypes"][0]["title"] = json!("Reviewed title");
            let patch = json!({"meetingTypes":read["meetingTypes"]});
            validate_patch(&patch).unwrap();
            let plan = SetupPlan {
                host_id: "host".into(),
                patch: patch.clone(),
                revision: "reviewed-revision".into(),
                expires_at: 200,
                attempted: false,
                result: None,
            };
            let request = json!({"patch":plan.patch,"expectedRevision":plan.revision});
            let reviewed = reviewable_preview(
                &json!({"settings":request["patch"],"changes":[],"warnings":[]}),
            );
            let applied = settings_snapshot(&reviewed["settings"]);
            let native = types(&applied);
            for key in [
                "monthlyBookingLimit",
                "yearlyBookingLimit",
                "monthlyBookedMinutesLimit",
                "yearlyBookedMinutesLimit",
            ] {
                for meeting in [
                    &read["meetingTypes"][0],
                    &request["patch"]["meetingTypes"][0],
                    &reviewed["settings"]["meetingTypes"][0],
                    &applied["meetingTypes"][0],
                    &native[0],
                ] {
                    assert_eq!(
                        meeting.get(key),
                        original.get(key),
                        "monthly/yearly field {key} lost or changed"
                    );
                }
            }
            for key in [
                "dailyBookingLimit",
                "weeklyBookingLimit",
                "dailyBookedMinutesLimit",
                "weeklyBookedMinutesLimit",
            ] {
                assert_eq!(applied["meetingTypes"][0][key], original[key]);
            }
            assert_eq!(applied["meetingTypes"][0]["title"], "Reviewed title");
        }
    }

    #[test]
    fn monthly_yearly_caps_keep_legacy_omission_and_reject_null_or_invalid_bounds() {
        let legacy = json!({"meetingTypes":[{"id":"intro","slug":"intro"}]});
        let read = settings_snapshot(&legacy);
        let native = types(&legacy);
        for (key, max, step) in [
            ("monthlyBookingLimit", 1500, 1),
            ("yearlyBookingLimit", 18000, 1),
            ("monthlyBookedMinutesLimit", 44640, 5),
            ("yearlyBookedMinutesLimit", 527040, 5),
        ] {
            assert!(read["meetingTypes"][0].get(key).is_none());
            assert!(native[0].get(key).is_none());
            for value in [
                Value::Null,
                json!(false),
                json!("0"),
                json!(-1),
                json!(1.5),
                json!(max + step),
            ] {
                let mut patch = json!({"meetingTypes":[{}]});
                patch["meetingTypes"][0][key] = value;
                assert!(
                    validate_patch(&patch).is_err(),
                    "invalid value for {key} accepted"
                );
            }
            if step == 5 {
                for value in [1, 6] {
                    let mut patch = json!({"meetingTypes":[{}]});
                    patch["meetingTypes"][0][key] = json!(value);
                    assert!(validate_patch(&patch).is_err());
                }
            }
            for value in [0, step, max] {
                let mut patch = json!({"meetingTypes":[{}]});
                patch["meetingTypes"][0][key] = json!(value);
                assert!(validate_patch(&patch).is_ok());
            }
        }
        assert!(validate_patch(&json!({"meetingTypes":read["meetingTypes"]})).is_ok());
    }

    #[test]
    fn booked_minute_limits_survive_read_review_apply_without_changing_count_limits() {
        for (daily, weekly) in [(0, 0), (5, 5), (1440, 10080)] {
            let original = json!({"id":"intro","slug":"intro","title":"Original","dailyBookedMinutesLimit":daily,"weeklyBookedMinutesLimit":weekly,"dailyBookingLimit":4,"weeklyBookingLimit":20});
            let mut read = settings_snapshot(&json!({"meetingTypes":[original.clone()]}));
            read["meetingTypes"][0]["title"] = json!("Reviewed title");
            let patch = json!({"meetingTypes":read["meetingTypes"]});
            validate_patch(&patch).unwrap();
            let plan = SetupPlan {
                host_id: "host".into(),
                patch: patch.clone(),
                revision: "reviewed-revision".into(),
                expires_at: 200,
                attempted: false,
                result: None,
            };
            let request = json!({"patch":plan.patch,"expectedRevision":plan.revision});
            let reviewed = reviewable_preview(
                &json!({"settings":request["patch"],"changes":[],"warnings":[]}),
            );
            let applied = settings_snapshot(&reviewed["settings"]);
            let native = types(&applied);
            for key in ["dailyBookedMinutesLimit", "weeklyBookedMinutesLimit"] {
                for meeting in [
                    &read["meetingTypes"][0],
                    &request["patch"]["meetingTypes"][0],
                    &reviewed["settings"]["meetingTypes"][0],
                    &applied["meetingTypes"][0],
                    &native[0],
                ] {
                    assert_eq!(
                        meeting.get(key),
                        original.get(key),
                        "minute limit {key} changed during an unrelated title edit"
                    );
                }
            }
            assert_eq!(applied["meetingTypes"][0]["dailyBookingLimit"], 4);
            assert_eq!(applied["meetingTypes"][0]["weeklyBookingLimit"], 20);
        }
    }

    #[test]
    fn booked_minute_limits_preserve_legacy_omission_and_reject_invalid_local_bounds() {
        let legacy = json!({"meetingTypes":[{"id":"intro","slug":"intro"}]});
        let snapshot = settings_snapshot(&legacy);
        let native = types(&legacy);
        for (key, max) in [
            ("dailyBookedMinutesLimit", 1440),
            ("weeklyBookedMinutesLimit", 10080),
        ] {
            assert!(snapshot["meetingTypes"][0].get(key).is_none());
            assert!(native[0].get(key).is_none());
            for value in [
                Value::Null,
                json!(true),
                json!("0"),
                json!(-5),
                json!(1),
                json!(6),
                json!(5.5),
                json!(max + 5),
            ] {
                let mut patch = json!({"meetingTypes":[{}]});
                patch["meetingTypes"][0][key] = value;
                assert!(
                    validate_patch(&patch).is_err(),
                    "invalid limit for {key} passed preview validation"
                );
            }
            for value in [0, 5, max] {
                let mut patch = json!({"meetingTypes":[{}]});
                patch["meetingTypes"][0][key] = json!(value);
                assert!(validate_patch(&patch).is_ok());
            }
        }
        assert!(validate_patch(&json!({"meetingTypes":snapshot["meetingTypes"]})).is_ok());
    }

    #[test]
    fn followup_policy_survives_read_preview_apply_and_an_unrelated_title_edit() {
        let unicode_body = format!(
            "Hello {{guest_name}},\n{}\n{{meeting_title}} — {{host_name}}",
            "日本語".repeat(700)
        );
        assert!(
            unicode_body.len() > 4000,
            "exercise byte length versus template character limit"
        );
        for (minutes, subject, body) in [
            (Value::Null, String::new(), String::new()),
            (json!(5), String::new(), String::new()),
            (json!(10080), "Thanks, {guest_name}".into(), unicode_body),
        ] {
            let original = json!({"id":"intro","slug":"intro","title":"Original","followUpMinutes":minutes,"followUpSubject":subject,"followUpBody":body,"reminder":false});
            let mut read = settings_snapshot(&json!({"meetingTypes":[original.clone()]}));
            read["meetingTypes"][0]["title"] = json!("Reviewed title");
            let patch = json!({"meetingTypes":read["meetingTypes"]});
            validate_patch(&patch).unwrap();
            let plan = SetupPlan {
                host_id: "host".into(),
                patch: patch.clone(),
                revision: "reviewed-revision".into(),
                expires_at: 200,
                attempted: false,
                result: None,
            };
            let request = json!({"patch":plan.patch,"expectedRevision":plan.revision});
            let reviewed = reviewable_preview(
                &json!({"settings":request["patch"],"changes":[],"warnings":[]}),
            );
            let applied = settings_snapshot(&reviewed["settings"]);
            let native = types(&applied);
            for key in [
                "followUpMinutes",
                "followUpSubject",
                "followUpBody",
                "reminder",
            ] {
                for meeting in [
                    &read["meetingTypes"][0],
                    &request["patch"]["meetingTypes"][0],
                    &reviewed["settings"]["meetingTypes"][0],
                    &applied["meetingTypes"][0],
                    &native[0],
                ] {
                    assert_eq!(
                        meeting.get(key),
                        original.get(key),
                        "follow-up field {key} lost or rendered while projecting settings"
                    );
                }
            }
            assert_eq!(applied["meetingTypes"][0]["title"], "Reviewed title");
        }
    }

    #[test]
    fn followup_legacy_omission_and_explicit_disable_are_distinct_and_patch_bounds_are_local() {
        let legacy = json!({"meetingTypes":[{"id":"intro","slug":"intro","title":"Legacy"}]});
        let read = settings_snapshot(&legacy);
        let native = types(&legacy);
        for key in ["followUpMinutes", "followUpSubject", "followUpBody"] {
            assert!(read["meetingTypes"][0].get(key).is_none());
            assert!(native[0].get(key).is_none());
        }
        assert!(validate_patch(&json!({"meetingTypes":read["meetingTypes"]})).is_ok());
        assert!(validate_patch(&json!({"meetingTypes":[{"followUpMinutes":null,"followUpSubject":"","followUpBody":""}]})).is_ok());
        for value in [
            json!(0),
            json!(4),
            json!(10081),
            json!(5.5),
            json!(true),
            json!("60"),
        ] {
            assert!(validate_patch(&json!({"meetingTypes":[{"followUpMinutes":value}]})).is_err());
        }
        for value in [
            Value::Null,
            json!({}),
            json!("x".repeat(161)),
            json!("Header\nInjection"),
        ] {
            assert!(validate_patch(&json!({"meetingTypes":[{"followUpSubject":value}]})).is_err());
        }
        for value in [Value::Null, json!([]), json!("x".repeat(4001))] {
            assert!(validate_patch(&json!({"meetingTypes":[{"followUpBody":value}]})).is_err());
        }
        assert!(validate_patch(&json!({"meetingTypes":[{"followUpSubject":"界".repeat(160),"followUpBody":"界".repeat(4000)}]})).is_ok());
    }

    #[test]
    fn default_schedule_round_trip_preserves_host_reference_and_raw_type_fallbacks() {
        for inherited in [false, true] {
            let original = json!({"id":"intro","slug":"intro","title":"Original","zone":"UTC","scheduleId":"fallback","useDefaultSchedule":inherited,"weeklyHours":{"3":[{"start":580,"end":900}]},"dateOverrides":{"2026-12-31":[]}});
            let settings = json!({"defaultAvailabilityScheduleId":"work","availabilitySchedules":[{"id":"work","name":"Host default","zone":"Asia/Kathmandu","weeklyHours":{"1":[{"start":550,"end":1000}]},"dateOverrides":{}},{"id":"fallback","name":"Explicit fallback","zone":"Europe/Oslo","weeklyHours":{"2":[{"start":600,"end":1020}]},"dateOverrides":{}}],"meetingTypes":[original.clone()]});
            let mut read = settings_snapshot(&settings);
            read["meetingTypes"][0]["title"] = json!("Reviewed title");
            let patch = json!({"defaultAvailabilityScheduleId":read["defaultAvailabilityScheduleId"],"availabilitySchedules":read["availabilitySchedules"],"meetingTypes":read["meetingTypes"]});
            validate_patch(&patch).unwrap();
            let plan = SetupPlan {
                host_id: "host".into(),
                patch: patch.clone(),
                revision: "reviewed-revision".into(),
                expires_at: 200,
                attempted: false,
                result: None,
            };
            let request = json!({"patch":plan.patch,"expectedRevision":plan.revision});
            let reviewed = reviewable_preview(
                &json!({"settings":request["patch"],"changes":[],"warnings":[]}),
            );
            let applied = settings_snapshot(&reviewed["settings"]);
            for view in [&read, &request["patch"], &reviewed["settings"], &applied] {
                assert_eq!(view["defaultAvailabilityScheduleId"], "work");
                assert_eq!(
                    view["availabilitySchedules"],
                    settings["availabilitySchedules"]
                );
                for key in [
                    "zone",
                    "scheduleId",
                    "useDefaultSchedule",
                    "weeklyHours",
                    "dateOverrides",
                ] {
                    assert_eq!(
                        view["meetingTypes"][0][key], original[key],
                        "unrelated title edit changed fallback {key}"
                    );
                }
            }
            assert_eq!(
                types(&applied)[0]["zone"],
                if inherited {
                    "Asia/Kathmandu"
                } else {
                    "Europe/Oslo"
                }
            );
            assert_eq!(applied["meetingTypes"][0]["title"], "Reviewed title");
        }
    }

    #[test]
    fn legacy_settings_do_not_inherit_a_host_default_without_an_explicit_flag() {
        let legacy =
            json!({"meetingTypes":[{"id":"intro","slug":"intro","scheduleId":null,"zone":"UTC"}]});
        let read = settings_snapshot(&legacy);
        assert_eq!(read["defaultAvailabilityScheduleId"], Value::Null);
        assert_eq!(read["meetingTypes"][0]["useDefaultSchedule"], false);
        assert_eq!(read["meetingTypes"][0]["scheduleId"], Value::Null);
        let with_default = json!({"defaultAvailabilityScheduleId":"work","availabilitySchedules":[{"id":"work","zone":"Europe/Oslo"}],"meetingTypes":read["meetingTypes"]});
        assert_eq!(types(&with_default)[0]["zone"], "UTC");
        assert!(validate_patch(&json!({"defaultAvailabilityScheduleId":null})).is_ok());
        assert!(validate_patch(&json!({"defaultAvailabilityScheduleId":"work"})).is_ok());
        for value in [
            json!(false),
            json!({}),
            json!([]),
            json!(""),
            json!("bad reference"),
            json!("x".repeat(65)),
        ] {
            assert!(validate_patch(&json!({"defaultAvailabilityScheduleId":value})).is_err());
        }
        for value in [Value::Null, json!(0), json!("true"), json!({})] {
            assert!(
                validate_patch(
                    &json!({"meetingTypes":[{"id":"intro","useDefaultSchedule":value}]})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn reusable_schedules_survive_review_projection_without_credentials() {
        let data = json!({"availabilitySchedules":[{"id":"work","name":"Work hours","zone":"Europe/Oslo","weeklyHours":{"1":[{"start":540,"end":1020}]},"dateOverrides":{},"refreshToken":"private"}],"meetingTypes":[{"id":"intro","scheduleId":"work"}]});
        let snapshot = settings_snapshot(&data);
        assert_eq!(snapshot["meetingTypes"][0]["scheduleId"], "work");
        assert_eq!(snapshot["availabilitySchedules"][0]["name"], "Work hours");
        assert!(
            snapshot["availabilitySchedules"][0]
                .get("refreshToken")
                .is_none()
        );
        assert!(
            validate_patch(&json!({"availabilitySchedules":snapshot["availabilitySchedules"]}))
                .is_ok()
        );
        assert!(validate_patch(&json!({"availabilitySchedules":{}})).is_err());
    }

    #[test]
    fn complete_meeting_array_round_trip_keeps_new_policy_fields_and_explicit_inheritance() {
        for (before, after, location) in [
            (Value::Null, json!(30), "google_meet"),
            (json!(0), Value::Null, "custom"),
            (json!(10), json!(60), "google_meet"),
        ] {
            let original = json!({"id":"intro","slug":"intro","title":"Original","buffer":15,"bufferBefore":before,"bufferAfter":after,"slotInterval":45,"weeklyBookingLimit":250,"locationType":location,"questions":[{"id":"topic","type":"select","label":"Topic","required":true,"options":["Design","Engineering"]},{"id":"consent","type":"checkbox","label":"I agree","required":false}],"extraGuestsLimit":3,"reminder":true,"reminderMinutes":[1440,60,15]});
            let settings =
                json!({"profile":{"name":"Owner"},"accounts":[],"meetingTypes":[original.clone()]});
            let native = types(&settings);
            let mut read = settings_snapshot(&settings);
            // An MCP caller changes a different field in the complete array.
            read["meetingTypes"][0]["title"] = json!("Reviewed title");
            let patch = json!({"meetingTypes":read["meetingTypes"]});
            validate_patch(&patch).unwrap();
            let plan = SetupPlan {
                host_id: "host".into(),
                patch: patch.clone(),
                revision: "reviewed-revision".into(),
                expires_at: 200,
                attempted: false,
                result: None,
            };
            let request = json!({"patch":plan.patch,"expectedRevision":plan.revision});
            let reviewed = reviewable_preview(
                &json!({"settings":{"meetingTypes":patch["meetingTypes"]},"changes":[],"warnings":[]}),
            );
            let applied = settings_snapshot(&reviewed["settings"]);
            for key in [
                "bufferBefore",
                "bufferAfter",
                "slotInterval",
                "weeklyBookingLimit",
                "locationType",
                "questions",
                "extraGuestsLimit",
                "reminder",
                "reminderMinutes",
            ] {
                for meeting in [
                    &native[0],
                    &read["meetingTypes"][0],
                    &request["patch"]["meetingTypes"][0],
                    &reviewed["settings"]["meetingTypes"][0],
                    &applied["meetingTypes"][0],
                ] {
                    assert_eq!(
                        meeting.get(key),
                        original.get(key),
                        "policy field {key} was lost or changed"
                    );
                }
            }
            assert_eq!(applied["meetingTypes"][0]["title"], "Reviewed title");
        }
    }
    #[test]
    fn intake_projection_preserves_legacy_and_explicitly_empty_forms() {
        for questions in [Value::Null, json!([])] {
            let data = json!({"meetingTypes":[{"id":"intro","slug":"intro","questions":questions,"extraGuestsLimit":0}]});
            let snapshot = settings_snapshot(&data);
            assert_eq!(snapshot["meetingTypes"][0]["questions"], questions);
            assert_eq!(types(&data)[0]["questions"], questions);
            assert_eq!(types(&data)[0]["extraGuestsLimit"], 0);
        }
    }
    #[test]
    fn settings_permission_is_separate_and_old_credentials_default_to_read_only() {
        let old = json!({"token":"safe-token","expires_at":now()+60000,"host_id":"h_abcdefghijklmnopqrstuvwx"});
        let mut secret: Secrets = serde_json::from_value(old).unwrap();
        assert!(!settings_permission(&secret));
        assert!(require_settings_permission(&secret).is_err());
        secret.settings_write = true;
        assert!(settings_permission(&secret));
        secret.expires_at = 1;
        assert!(!settings_permission(&secret));
        let write = json!({"accessToken":"safe-token","expiresAt":now()+60000,"tokenType":"Bearer","scope":SETTINGS_SCOPE});
        assert!(issued_credential(&write, false).is_err());
        assert!(issued_credential(&write, true).is_ok());
        let read = json!({"accessToken":"safe-token","expiresAt":now()+60000,"tokenType":"Bearer","scope":DEVICE_SCOPE});
        assert!(issued_credential(&read, true).is_err());
    }
    #[test]
    fn preview_does_not_allow_credentials_calendar_connection_changes_or_arbitrary_patch_fields() {
        for patch in [
            json!({"accounts":[]}),
            json!({"grant":"secret"}),
            json!({"profile":{"email":"other@example.com"}}),
            json!({"profile":{"token":"secret"}}),
            json!({"meetingTypes":{}}),
            json!({}),
        ] {
            assert!(validate_patch(&patch).is_err());
        }
        assert!(
            validate_patch(&json!({"profile":{"name":"Host","company":"Team"},"meetingTypes":[]}))
                .is_ok()
        );
        let snapshot = settings_snapshot(
            &json!({"profile":{"name":"Host","refreshToken":"secret"},"accounts":[{"id":"a","grant":{"access_token":"secret"}}],"meetingTypes":[{"id":"intro","slug":"intro","weeklyHours":{"1":[{"start":540,"end":1020,"secret":"secret"}]},"token":"secret"}]}),
        );
        assert!(!snapshot.to_string().contains("secret"));
        assert!(snapshot.to_string().contains("weeklyHours"));
    }
    #[test]
    fn plans_are_bound_to_account_revision_expiry_and_a_single_write_attempt() {
        let mut plan = SetupPlan {
            host_id: "host-a".into(),
            patch: json!({"profile":{"name":"New"}}),
            revision: "revision-before".into(),
            expires_at: 200,
            attempted: false,
            result: None,
        };
        assert!(check_plan(&plan, Some("host-a"), 100).is_ok());
        assert!(check_plan(&plan, Some("host-b"), 100).is_err());
        assert!(check_plan(&plan, Some("host-a"), 200).is_err());
        plan.attempted = true;
        assert!(check_plan(&plan, Some("host-a"), 100).is_err());
        plan.result = Some(json!({"applied":true}));
        assert!(check_plan(&plan, Some("host-a"), 100).is_ok());
        assert_eq!(plan.revision, "revision-before");
    }
    #[test]
    fn invalid_or_unconfirmed_setup_never_reaches_secure_storage_or_the_network() {
        let root = std::env::temp_dir().join(format!("email-setup-local-{}", Uuid::new_v4()));
        let service = MailService::open(root.join("mail.sqlite")).unwrap();
        assert!(
            service
                .booking_setup_preview(json!({"grant":"never uploaded"}))
                .unwrap_err()
                .contains("Only profile")
        );
        assert!(
            service
                .booking_setup_apply(&Uuid::new_v4().to_string(), false)
                .unwrap_err()
                .contains("explicitly confirm")
        );
        assert!(
            service
                .booking_setup_apply("not-a-plan", true)
                .unwrap_err()
                .contains("identifier is invalid")
        );
        assert!(
            service
                .booking_setup_poll()
                .unwrap_err()
                .contains("Start an explicit")
        );
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
}
