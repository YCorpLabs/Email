//! Booking integration. Bearers use OS secure storage; unapproved PKCE is memory-only.
#[path = "booking_setup.rs"]
mod setup;
use super::{MailService, ensure_private_config};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Method, blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

// The distribution profile's Scheduling service. Unset hides the integration:
// reading booking secrets and every service request fail before any network
// or Keychain access. Tests use a fixed synthetic origin so they never depend
// on the developer's profile.
#[cfg(not(test))]
pub(super) const ORIGIN: Option<&str> = super::profile::SCHEDULING_ORIGIN;
#[cfg(test)]
pub(super) const TEST_ORIGIN: &str = "https://scheduling.example.test";
#[cfg(test)]
pub(super) const ORIGIN: Option<&str> = Some(TEST_ORIGIN);
pub(super) const UNCONFIGURED: &str = "Scheduling isn't configured in this build.";
fn origin() -> Result<&'static str, String> {
    ORIGIN.ok_or_else(|| UNCONFIGURED.to_owned())
}
const SECRET_ID: &str = "booking.desktop.v1";

/// The Scheduling connection is one OS credential for the default store. Any
/// other store this process opened (a demo or test store from scripts/dev.sh,
/// or `mail-mcp --db` on one) gets its own item, so it can never read, prompt
/// for, replace or remove the real connection.
fn secret_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| secret_id_for(super::store_location::opened_store()))
        .clone()
}
fn secret_id_for(store: Option<&std::path::Path>) -> String {
    match store {
        Some(store) if !super::is_default_store(store) => {
            let resolved = super::canonical_store_path(store);
            let digest = format!("{:x}", Sha256::digest(resolved.as_os_str().as_encoded_bytes()));
            format!("{SECRET_ID}.store-{}", &digest[..16])
        }
        _ => SECRET_ID.into(),
    }
}
const MAX_BYTES: u64 = 2 * 1024 * 1024;
pub(super) const DEVICE_SCOPE: &str = "booking:read email:tracking";
const SETTINGS_SCOPE: &str = "booking:read email:tracking booking:settings";

#[derive(Clone, Default, Serialize, Deserialize)]
struct Secrets {
    token: Option<String>,
    expires_at: u64,
    #[serde(default)]
    host_id: Option<String>,
    #[serde(default)]
    settings_write: bool,
}
struct Pending {
    device_code: String,
    verifier: String,
    expires_at: u64,
    next_poll: u64,
    interval: u64,
    settings_write: bool,
}
fn pending_poll_response(
    pending: &mut Pending,
    status: u16,
    data: &Value,
    responded_at: u64,
) -> Option<Value> {
    if !matches!(status, 428 | 429) {
        return None;
    }
    let interval = data
        .get("interval")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(5, 60);
    pending.interval = interval;
    // Begin the wait after receiving the server response. Starting at request
    // dispatch can make the next poll arrive before the server's own window.
    pending.next_poll = responded_at.saturating_add(interval * 1000);
    Some(json!({"pending":true,"interval":interval}))
}
#[derive(Default, Serialize, Deserialize)]
struct Cache {
    host: Value,
    meeting_types: Vec<Value>,
    bookings: Vec<Value>,
    analytics: Value,
    refreshed_at: u64,
}

struct BookingLock {
    _process: std::sync::MutexGuard<'static, ()>,
    #[cfg(target_os = "macos")]
    _cross_process: fs::File,
}
fn lock() -> Result<BookingLock, String> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let process = LOCK
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Booking connection is busy.".to_string())?;
    Ok(BookingLock {
        _process: process,
        #[cfg(target_os = "macos")]
        _cross_process: super::credential_access::booking_lock()?,
    })
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn pending_connections() -> &'static Mutex<std::collections::HashMap<std::path::PathBuf, Pending>> {
    static PENDING: OnceLock<Mutex<std::collections::HashMap<std::path::PathBuf, Pending>>> =
        OnceLock::new();
    PENDING.get_or_init(Default::default)
}
#[cfg(not(target_os = "macos"))]
fn secret_entry() -> Result<keyring::Entry, String> {
    #[cfg(target_os = "linux")]
    let entry = keyring::Entry::new_with_target(
        super::profile::LINUX_SECRET_COLLECTION,
        super::KEYRING_SERVICE,
        &secret_id(),
    );
    #[cfg(not(target_os = "linux"))]
    let entry = MailService::entry(&secret_id());
    // SecretService's default-target legacy lookup fails with NoStorageAccess
    // when a fresh desktop has no default collection. A dedicated OS collection
    // avoids that fallback: absent items are NoEntry, while locked stores and
    // unavailable services still fail. On first write keyring creates/unlocks
    // this collection through the OS; credentials never fall back to files.
    entry.map_err(|_| "Booking secure storage is unavailable.".into())
}
/// Each Keychain read can raise a macOS password prompt when the item was
/// created by a differently signed build. Read once per process and remember
/// the outcome, including a refusal, so background polling cannot re-prompt.
#[cfg(not(target_os = "macos"))]
fn secret_cache() -> &'static Mutex<Option<Result<Secrets, String>>> {
    static CACHE: OnceLock<Mutex<Option<Result<Secrets, String>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
#[cfg(not(target_os = "macos"))]
fn secrets() -> Result<Secrets, String> {
    origin()?;
    let mut cache = secret_cache()
        .lock()
        .map_err(|_| "Booking secure storage is unavailable.".to_string())?;
    if let Some(value) = cache.as_ref() {
        return value.clone();
    }
    #[cfg(target_os = "linux")]
    let read = secret_entry()?.get_password();
    #[cfg(not(target_os = "linux"))]
    let mut read = secret_entry()?.get_password();
    if let (Err(keyring::Error::NoEntry), Some(previous)) = (&read, super::PREVIOUS_KEYRING_SERVICE)
    {
        #[cfg(not(target_os = "linux"))]
        if let Ok(entry) = keyring::Entry::new(previous, &secret_id()) {
            read = entry.get_password();
            if let Ok(value) = &read {
                // Copy to the current service; its item reads without a prompt.
                let _ = secret_entry().map(|e| e.set_password(value));
            }
        }
        #[cfg(target_os = "linux")]
        let _ = previous;
    }
    let value = decode_secret(read);
    *cache = Some(value.clone());
    value
}
#[cfg(not(target_os = "macos"))]
fn remember_secrets(value: Secrets) {
    if let Ok(mut cache) = secret_cache().lock() {
        *cache = Some(Ok(value));
    }
}
fn decode_secret(result: Result<String, keyring::Error>) -> Result<Secrets, String> {
    match result {
        Ok(value) => serde_json::from_str(&value)
            .map_err(|_| "Booking secure storage is invalid. Reconnect booking.".into()),
        Err(keyring::Error::NoEntry) => Ok(Secrets::default()),
        Err(_) => Err("Booking secure storage is unavailable.".into()),
    }
}
#[cfg(not(target_os = "macos"))]
fn save_secrets(value: &Secrets) -> Result<(), String> {
    let encoded = serde_json::to_string(value).map_err(|_| "Could not save booking connection.")?;
    secret_entry()?
        .set_password(&encoded)
        .map_err(|_| "Could not save booking connection in secure storage.".to_string())?;
    remember_secrets(value.clone());
    Ok(())
}
#[cfg(target_os = "macos")]
fn secrets() -> Result<Secrets, String> {
    origin()?;
    use super::lifecycle::{CredentialStore, OsCredentials};
    match OsCredentials.read(&secret_id(), false)? {
        Some(value) => decode_secret(Ok(value)),
        None => Ok(Secrets::default()),
    }
}
#[cfg(target_os = "macos")]
fn save_secrets(value: &Secrets) -> Result<(), String> {
    use super::lifecycle::{CredentialStore, OsCredentials};
    OsCredentials.write(
        &secret_id(),
        &serde_json::to_string(value).map_err(|_| "Could not save booking connection.")?,
    )
}
fn bearer(value: &Secrets) -> Result<&str, String> {
    value
        .token
        .as_deref()
        .filter(|token| safe_token(token) && value.expires_at > now())
        .ok_or_else(|| "Connect booking to continue.".into())
}
fn safe_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && value.bytes().all(|b| b.is_ascii_graphic())
}

struct Api {
    client: Client,
}
impl Api {
    fn new() -> Result<Self, String> {
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .connect_timeout(Duration::from_secs(5))
                .redirect(Policy::none())
                .https_only(true)
                .build()
                .map_err(|_| "Booking connection is unavailable.")?,
        })
    }
    fn call(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<(u16, Value), String> {
        // All paths are static and requests never follow redirects with credentials.
        let mut request = self.client.request(method, format!("{}{path}", origin()?));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .map_err(|_| "Booking service could not be reached. Try again.")?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err("Booking service returned an unexpected redirect.".into());
        }
        if status == 204 {
            return Ok((status, Value::Null));
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Booking response could not be read.")?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Booking response is too large.".into());
        }
        // Do not surface provider bodies, request URLs, or reqwest Debug errors.
        let data = serde_json::from_slice(&bytes)
            .map_err(|_| "Booking service returned an invalid response.")?;
        Ok((status, data))
    }
    fn get(&self, path: &str, token: &str) -> Result<Value, String> {
        let (status, data) = self.call(Method::GET, path, Some(token), None)?;
        if status != 200 {
            if matches!(status, 401 | 403) {
                invalidate_rejected_token(token)?;
            }
            return Err(status_error(status));
        }
        Ok(data)
    }
}
// Clear only the credential that the server actually rejected. Offline and
// transient failures retain the bearer and the account-bound private cache.
fn clear_rejected_token(value: &mut Secrets, rejected: &str) -> bool {
    if value.token.as_deref() != Some(rejected) {
        return false;
    }
    value.token = None;
    value.expires_at = 0;
    value.host_id = None;
    value.settings_write = false;
    true
}
fn invalidate_rejected_token(rejected: &str) -> Result<(), String> {
    let mut value = secrets()?;
    if clear_rejected_token(&mut value, rejected) {
        save_secrets(&value)?;
    }
    Ok(())
}
pub(super) fn tracking_request(
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let _guard = lock()?;
    if path != "/api/device/tracking"
        && !path
            .strip_prefix("/api/device/tracking?id=")
            .is_some_and(|id| Uuid::parse_str(id).is_ok())
    {
        return Err("Tracking request path is invalid.".into());
    }
    let secrets = secrets()?;
    let token = bearer(&secrets)?;
    let (status, data) = Api::new()?.call(method, path, Some(token), body)?;
    if matches!(status, 200 | 201 | 204) {
        return Ok(data);
    }
    if matches!(status, 401 | 403) {
        invalidate_rejected_token(token)?;
    }
    Err(status_error(status))
}
fn verification_url(data: &Value, user_code: &str) -> Result<String, String> {
    // The user code stays in a fragment: it is never sent in a browser request
    // or referrer. The PKCE verifier and device secret never enter this URL.
    let valid_code = user_code.len() == 14
        && user_code.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 9 {
                b == b'-'
            } else {
                b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(&b)
            }
        });
    let expected = format!("{}/device#code={user_code}", origin()?);
    if !valid_code || data.get("verificationUrl").and_then(Value::as_str) != Some(expected.as_str())
    {
        return Err("Booking verification address is not valid.".into());
    }
    Ok(expected)
}
fn issued_credential(data: &Value, settings_write: bool) -> Result<(String, u64), String> {
    let token = string(data, "accessToken", 4096)
        .filter(|t| safe_token(t))
        .ok_or("Booking service returned an invalid connection.")?;
    let expires_at = data
        .get("expiresAt")
        .and_then(Value::as_u64)
        .filter(|e| *e > now())
        .ok_or("Booking connection expiry is invalid.")?;
    if data.get("tokenType").and_then(Value::as_str) != Some("Bearer")
        || data.get("scope").and_then(Value::as_str)
            != Some(if settings_write {
                SETTINGS_SCOPE
            } else {
                DEVICE_SCOPE
            })
    {
        return Err("Booking service returned an invalid connection scope.".into());
    }
    Ok((token, expires_at))
}
fn status_error(status: u16) -> String {
    match status {
        401 | 403 => "Booking connection expired or was revoked. Reconnect booking.",
        429 => "Booking service is busy. Try again shortly.",
        _ => "Booking service could not complete the request. Try again.",
    }
    .into()
}
fn string(value: &Value, key: &str, max: usize) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .filter(|s| s.len() <= max && !s.chars().any(char::is_control))
        .map(str::to_owned)
}
fn host_identity(data: &Value) -> Result<Value, String> {
    let id = string(data, "hostId", 64)
        .filter(|id| valid_host(id))
        .ok_or("Booking service returned an invalid host.")?;
    Ok(
        json!({"host_id":id,"name":string(data,"name",160).unwrap_or_default(),"scope":data.get("scope").and_then(Value::as_str).filter(|s|matches!(*s,DEVICE_SCOPE|SETTINGS_SCOPE)).unwrap_or(DEVICE_SCOPE),"expires_at":data.get("expiresAt").and_then(Value::as_u64)}),
    )
}
fn valid_host(id: &str) -> bool {
    id.len() == 26
        && id.starts_with("h_")
        && id[2..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug.as_bytes()[0].is_ascii_alphanumeric()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn link(host: &str, slug: &str) -> Result<String, String> {
    if !valid_host(host) || !valid_slug(slug) {
        return Err("Booking link is not valid.".into());
    }
    Ok(format!("{}/b/{host}/{slug}?source=email", origin()?))
}
fn selected(data: &Value, strings: &[&str], numbers: &[&str], bools: &[&str]) -> Value {
    let mut out = serde_json::Map::new();
    for key in strings {
        if let Some(value) = string(data, key, 4000) {
            out.insert((*key).into(), json!(value));
        }
    }
    for key in numbers {
        if let Some(value) = data.get(key).filter(|v| v.is_number()) {
            out.insert((*key).into(), value.clone());
        }
    }
    for key in bools {
        if let Some(value) = data.get(key).and_then(Value::as_bool) {
            out.insert((*key).into(), json!(value));
        }
    }
    Value::Object(out)
}
fn schedule_reference(value: &Value) -> Option<&str> {
    value.as_str().filter(|id| {
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    })
}
fn effective_schedule<'a>(data: &'a Value, meeting: &Value) -> Option<&'a Value> {
    let reference = if meeting
        .get("useDefaultSchedule")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        data.get("defaultAvailabilityScheduleId")
    } else {
        meeting.get("scheduleId")
    }
    .and_then(schedule_reference)?;
    data.get("availabilitySchedules")
        .and_then(Value::as_array)?
        .iter()
        .take(20)
        .find(|schedule| schedule.get("id").and_then(Value::as_str) == Some(reference))
}
fn types(data: &Value) -> Vec<Value> {
    data.get("meetingTypes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(20)
        .map(|t| {
            let mut item = selected(
                t,
                &[
                    "id",
                    "slug",
                    "title",
                    "description",
                    "zone",
                    "location",
                    "locationType",
                    "host",
                    "company",
                    "accountId",
                    "calendarId",
                    "scheduleId",
                    "question",
                ],
                &[
                    "duration",
                    "buffer",
                    "notice",
                    "horizon",
                    "startHour",
                    "endHour",
                    "breakStart",
                    "breakEnd",
                    "slotInterval",
                    "weeklyBookingLimit",
                    "dailyBookedMinutesLimit",
                    "weeklyBookedMinutesLimit",
                    "monthlyBookingLimit",
                    "yearlyBookingLimit",
                    "monthlyBookedMinutesLimit",
                    "yearlyBookedMinutesLimit",
                    "extraGuestsLimit",
                ],
                &[
                    "published",
                    "required",
                    "reminder",
                    "useDefaultSchedule",
                    "useDefaultCalendar",
                ],
            );
            item["useDefaultCalendar"] = json!(
                t.get("useDefaultCalendar")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            );
            // Native consumers display the effective timezone; setup snapshots below
            // retain raw fallback policy so editing unrelated fields stays lossless.
            item["useDefaultSchedule"] = json!(
                t.get("useDefaultSchedule")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            );
            if let Some(schedule) = effective_schedule(data, t) {
                if let Some(zone) = string(schedule, "zone", 160) {
                    item["zone"] = json!(zone);
                }
            }
            if let Some(questions) = t.get("questions") {
                if questions.is_null() {
                    item["questions"] = Value::Null;
                } else if let Some(questions) = questions.as_array() {
                    item["questions"] = json!(
                        questions
                            .iter()
                            .take(8)
                            .map(|question| {
                                let mut projected = selected(
                                    question,
                                    &["id", "type", "label"],
                                    &[],
                                    &["required"],
                                );
                                if let Some(options) =
                                    question.get("options").and_then(Value::as_array)
                                {
                                    projected["options"] = json!(
                                        options
                                            .iter()
                                            .filter_map(Value::as_str)
                                            .take(12)
                                            .map(|option| option
                                                .chars()
                                                .take(160)
                                                .collect::<String>())
                                            .collect::<Vec<_>>()
                                    );
                                }
                                projected
                            })
                            .collect::<Vec<_>>()
                    );
                }
            }
            if let Some(value) = t.get("followUpMinutes") {
                if value.is_null()
                    || value
                        .as_u64()
                        .is_some_and(|minutes| (5..=10080).contains(&minutes))
                {
                    item["followUpMinutes"] = value.clone();
                }
            }
            // Templates stay literal in cache/setup projections. In particular,
            // multiline Unicode body text must not use the header-string helper.
            for (key, max, multiline) in [
                ("followUpSubject", 160, false),
                ("followUpBody", 4000, true),
            ] {
                if let Some(text) = t.get(key).and_then(Value::as_str).filter(|text| {
                    text.encode_utf16().count() <= max
                        && (multiline || !text.chars().any(char::is_control))
                }) {
                    item[key] = json!(text);
                }
            }
            if let Some(value) = t.get("reminderMinutes") {
                if value.is_null() {
                    item["reminderMinutes"] = Value::Null;
                } else if let Some(offsets) = value.as_array() {
                    item["reminderMinutes"] = json!(
                        offsets
                            .iter()
                            .filter_map(Value::as_u64)
                            .filter(|minutes| (5..=10080).contains(minutes))
                            .take(3)
                            .collect::<Vec<_>>()
                    );
                }
            }
            if let Some(duration) = item.get("duration").cloned() {
                item["durationMinutes"] = duration;
            }
            for key in ["bufferBefore", "bufferAfter"] {
                if let Some(value) = t
                    .get(key)
                    .filter(|value| value.is_null() || value.is_number())
                {
                    item[key] = value.clone();
                }
            }
            if let Some(days) = t.get("days").and_then(Value::as_array) {
                item["days"] = json!(
                    days.iter()
                        .filter_map(Value::as_u64)
                        .filter(|d| *d <= 6)
                        .take(7)
                        .collect::<Vec<_>>()
                );
            }
            item
        })
        .filter(|t| {
            t.get("id").and_then(Value::as_str).is_some()
                && t.get("slug")
                    .and_then(Value::as_str)
                    .is_some_and(valid_slug)
        })
        .collect()
}
fn profile(host: &mut Value, settings: &Value) {
    if let Some(data) = settings.get("profile") {
        for key in ["name", "email", "company"] {
            if let Some(value) = string(data, key, 320) {
                host[key] = json!(value);
            }
        }
    }
}
fn analytics(data: &Value) -> Value {
    const COUNTS: &[&str] = &[
        "pageViews",
        "bookingRequests",
        "confirmed",
        "cancelled",
        "rescheduled",
        "failed",
    ];
    let period = selected(&data["period"], &["from", "to", "timeZone"], &["days"], &[]);
    let totals = selected(&data["totals"], &[], COUNTS, &[]);
    let sources = selected(&data["sources"], &[], &["direct", "email", "website"], &[]);
    let source_bookings = selected(
        &data["sourceBookings"],
        &[],
        &["direct", "email", "website"],
        &[],
    );
    let series: Vec<_> = data
        .get("series")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(31)
        .map(|row| selected(row, &["day"], COUNTS, &[]))
        .collect();
    json!({"period":period,"trackingStartedAt":data.get("trackingStartedAt").and_then(Value::as_u64),"totals":totals,"sources":sources,"sourceBookings":source_bookings,"series":series})
}
fn bookings(data: &Value) -> Vec<Value> {
    data.get("bookings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(500)
        .map(|b| {
            selected(
                b,
                &[
                    "id",
                    "status",
                    "name",
                    "email",
                    "timeZone",
                    "meetingTypeId",
                    "meetingTypeSlug",
                    "title",
                    "location",
                    "successorId",
                    "replacesId",
                ],
                &["start", "end"],
                &[],
            )
        })
        .collect()
}
fn load_cache(path: &Path) -> Result<Cache, String> {
    ensure_private_config(path)?;
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "Could not read booking cache.")?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err("Booking cache is too large.".into());
            }
            serde_json::from_slice(&bytes)
                .map_err(|_| "Booking cache is invalid. Refresh booking.".into())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Cache::default()),
        Err(_) => Err("Could not read booking cache.".into()),
    }
}
fn account_cache(path: &Path, secrets: &Secrets) -> Result<Cache, String> {
    let cache = load_cache(path)?;
    if secrets
        .host_id
        .as_deref()
        .is_some_and(|id| cache.host.get("host_id").and_then(Value::as_str) == Some(id))
    {
        Ok(cache)
    } else {
        Ok(Cache::default())
    }
}
fn save_cache(path: &Path, cache: &Cache) -> Result<(), String> {
    ensure_private_config(path)?;
    let temp = path.with_extension(format!("booking.tmp.{}", Uuid::new_v4()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .map_err(|_| "Could not create private booking cache.")?;
        file.write_all(&serde_json::to_vec(cache).map_err(|_| "Could not encode booking cache.")?)
            .map_err(|_| "Could not write booking cache.")?;
        file.sync_all()
            .map_err(|_| "Could not save booking cache.")?;
        fs::rename(&temp, path).map_err(|_| "Could not replace booking cache.")
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(str::to_owned)
}

impl MailService {
    fn booking_cache_path(&self) -> std::path::PathBuf {
        self.config_path.with_extension("booking.json")
    }
    pub(super) fn booking_status(&self) -> Result<Value, String> {
        if ORIGIN.is_none() {
            return Ok(json!({"configured":false,"connected":false,"cached":false,"service_url":null}));
        }
        let _guard = lock()?;
        let secrets = secrets()?;
        let cache = account_cache(&self.booking_cache_path(), &secrets)?;
        Ok(
            json!({"connected":bearer(&secrets).is_ok(),"host":cache.host,"cached":true,"configured":true,"service_url":ORIGIN}),
        )
    }
    pub(super) fn booking_connect_start(&self, name: Option<String>) -> Result<Value, String> {
        self.booking_connect_start_with_settings(name, false)
    }
    fn booking_connect_start_with_settings(
        &self,
        name: Option<String>,
        settings_write: bool,
    ) -> Result<Value, String> {
        let _guard = lock()?;
        let name = name.unwrap_or_else(|| {
            format!(
                "Email on {}",
                if cfg!(target_os = "macos") {
                    "Mac"
                } else {
                    "Linux"
                }
            )
        });
        if name.is_empty() || name.len() > 100 || name.chars().any(char::is_control) {
            return Err("Choose a valid device name.".into());
        }
        let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let (status, data) = Api::new()?.call(
            Method::POST,
            "/api/device/start",
            None,
            Some(json!({"name":name,"codeChallenge":challenge,"settingsWrite":settings_write})),
        )?;
        if status != 200 {
            return Err(status_error(status));
        }
        let code = string(&data, "deviceCode", 4096)
            .filter(|v| safe_token(v))
            .ok_or("Booking service returned an invalid connection.")?;
        let user_code = string(&data, "userCode", 32)
            .ok_or("Booking service returned an invalid connection.")?;
        let verification_url = verification_url(&data, &user_code)?;
        let expiry = data
            .get("expiresIn")
            .and_then(Value::as_u64)
            .filter(|s| *s > 0 && *s <= 600)
            .ok_or("Booking connection expiry is invalid.")?;
        let interval = data
            .get("interval")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .clamp(5, 60);
        // Linking preparation is memory-only across bridge handles. In
        // particular, do not create/unlock an OS collection before approval.
        let mut connections = pending_connections()
            .lock()
            .map_err(|_| "Booking connection is busy.")?;
        connections.retain(|_, pending| pending.expires_at > now());
        connections.insert(
            self.config_path.clone(),
            Pending {
                device_code: code,
                verifier,
                expires_at: now() + expiry * 1000,
                next_poll: now() + interval * 1000,
                interval,
                settings_write,
            },
        );
        Ok(
            json!({"user_code":user_code,"verification_url":verification_url,"expires_in":expiry,"interval":interval}),
        )
    }
    pub(super) fn booking_connect_poll(&self) -> Result<Value, String> {
        let _guard = lock()?;
        let mut connections = pending_connections()
            .lock()
            .map_err(|_| "Booking connection is busy.")?;
        let pending = connections
            .get_mut(&self.config_path)
            .ok_or("Start a booking connection first.")?;
        if pending.expires_at <= now() {
            connections.remove(&self.config_path);
            return Err("Booking connection expired. Start again.".into());
        }
        if pending.next_poll > now() {
            return Ok(json!({"pending":true,"interval":pending.interval}));
        }
        pending.next_poll = now() + pending.interval * 1000;
        let payload = json!({"deviceCode":pending.device_code,"codeVerifier":pending.verifier});
        let api = Api::new()?;
        let (status, data) = api.call(Method::POST, "/api/device/token", None, Some(payload))?;
        if let Some(response) = pending_poll_response(pending, status, &data, now()) {
            return Ok(response);
        }
        if status == 410 {
            connections.remove(&self.config_path);
            return Err("Booking connection expired. Start again.".into());
        }
        if status != 200 {
            return Err(status_error(status));
        }
        let settings_write = pending.settings_write;
        let (token, expires_at) = issued_credential(&data, settings_write)?;
        // Persist the bearer before any further network request, so a transient /me
        // failure cannot strand an approved device authorization.
        let mut secrets = Secrets {
            token: Some(token),
            expires_at,
            host_id: None,
            settings_write,
        };
        save_secrets(&secrets)?;
        connections.remove(&self.config_path);
        // Cache errors must not discard a successfully exchanged one-time grant.
        // host_id=None already prevents displaying any previous account's cache.
        save_cache(&self.booking_cache_path(), &Cache::default())?;
        let mut host = host_identity(&api.get("/api/device/me", bearer(&secrets)?)?)?;
        secrets.host_id = host
            .get("host_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        save_secrets(&secrets)?;
        // The account email is nonsecret identity; OAuth grants are never copied.
        if let Ok(settings) = api.get("/api/device/settings", bearer(&secrets)?) {
            profile(&mut host, &settings);
        }
        save_cache(
            &self.booking_cache_path(),
            &Cache {
                host: host.clone(),
                ..Cache::default()
            },
        )?;
        Ok(json!({"connected":true,"host":host}))
    }
    pub(super) fn booking_disconnect(&self) -> Result<Value, String> {
        let _guard = lock()?;
        pending_connections()
            .lock()
            .map_err(|_| "Booking connection is busy.")?
            .remove(&self.config_path);
        let secrets = secrets()?;
        if let Some(token) = secrets.token.as_deref().filter(|t| safe_token(t)) {
            let (status, _) =
                Api::new()?.call(Method::DELETE, "/api/device/token", Some(token), None)?;
            if ![200, 204, 401, 403].contains(&status) {
                return Err(status_error(status));
            }
        }
        // Keep a failed revoke recoverable: local credentials are removed only
        // after the server confirms revocation or says they are already invalid.
        #[cfg(target_os = "macos")]
        {
            use super::lifecycle::{CredentialStore, OsCredentials};
            OsCredentials.delete(&secret_id(), false)?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            match secret_entry()?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {
                    // A copy left under the previous service would reconnect on next read.
                    #[cfg(not(target_os = "linux"))]
                    if let Some(previous) = super::PREVIOUS_KEYRING_SERVICE {
                        match keyring::Entry::new(previous, &secret_id())
                            .map(|e| e.delete_credential())
                        {
                            Ok(Ok(())) | Ok(Err(keyring::Error::NoEntry)) => {}
                            _ => {
                                return Err(
                                    "Could not remove booking connection from secure storage."
                                        .into(),
                                );
                            }
                        }
                    }
                    remember_secrets(Secrets::default())
                }
                Err(_) => {
                    return Err("Could not remove booking connection from secure storage.".into());
                }
            }
        }
        let path = self.booking_cache_path();
        ensure_private_config(&path)?;
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Could not remove booking cache.".into()),
        }
        Ok(json!({"connected":false}))
    }
    pub(super) fn booking_list(&self, refresh: bool) -> Result<Value, String> {
        let _guard = lock()?;
        let mut secrets = secrets()?;
        let connected = bearer(&secrets).is_ok();
        let path = self.booking_cache_path();
        let mut cache = account_cache(&path, &secrets)?;
        if refresh {
            let token = bearer(&secrets)?;
            let api = Api::new()?;
            let mut host = host_identity(&api.get("/api/device/me", token)?)?;
            let settings = api.get("/api/device/settings", token)?;
            profile(&mut host, &settings);
            let meeting_types = types(&settings);
            let bookings = bookings(&api.get("/api/device/bookings", token)?);
            let analytics = analytics(&api.get("/api/device/analytics", token)?);
            cache = Cache {
                host,
                meeting_types,
                bookings,
                analytics,
                refreshed_at: now(),
            };
            secrets.host_id = cache
                .host
                .get("host_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            save_secrets(&secrets)?;
            save_cache(&path, &cache)?;
        }
        Ok(
            json!({"connected":connected,"host":cache.host,"meeting_types":cache.meeting_types,"bookings":cache.bookings,"analytics":cache.analytics,"cached":!refresh,"refreshed_at":cache.refreshed_at}),
        )
    }
    pub(super) fn booking_link(&self, meeting_type_id: &str) -> Result<Value, String> {
        let _guard = lock()?;
        let secrets = secrets()?;
        bearer(&secrets)?;
        let cache = account_cache(&self.booking_cache_path(), &secrets)?;
        let t = cache
            .meeting_types
            .iter()
            .find(|t| {
                t.get("id").and_then(Value::as_str) == Some(meeting_type_id)
                    && t.get("published").and_then(Value::as_bool) == Some(true)
            })
            .ok_or("Refresh booking and choose a published meeting type.")?;
        let url = link(
            cache
                .host
                .get("host_id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            t.get("slug").and_then(Value::as_str).unwrap_or_default(),
        )?;
        Ok(json!({"url":url}))
    }
    pub(super) fn booking_availability(&self, meeting_type_id: &str) -> Result<Value, String> {
        let _guard = lock()?;
        let secrets = secrets()?;
        let cache = account_cache(&self.booking_cache_path(), &secrets)?;
        let meeting = cache
            .meeting_types
            .iter()
            .find(|t| {
                t.get("id").and_then(Value::as_str) == Some(meeting_type_id)
                    && t.get("published").and_then(Value::as_bool) == Some(true)
            })
            .ok_or("Refresh booking and choose a published meeting type.")?;
        let slug = meeting
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let host = cache
            .host
            .get("host_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let public_url = link(host, slug)?;
        let data = Api::new()?.get(
            &format!("/api/device/availability?type={slug}"),
            bearer(&secrets)?,
        )?;
        let mut slots = Vec::new();
        for slot in data
            .get("slots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(500)
        {
            if let (Some(start), Some(end)) = (
                slot.get("start").and_then(Value::as_u64),
                slot.get("end").and_then(Value::as_u64),
            ) {
                if start > now() && end > start && end - start <= 24 * 60 * 60 * 1000 {
                    slots.push(json!({"start":start,"end":end,"url":format!("{public_url}&start={start}")}));
                }
            }
        }
        Ok(
            json!({"slots":slots,"time_zone":meeting.get("zone"),"duration":meeting.get("duration"),"title":meeting.get("title")}),
        )
    }
    pub(super) fn booking_analytics(&self, refresh: bool) -> Result<Value, String> {
        let _guard = lock()?;
        let path = self.booking_cache_path();
        let secrets = secrets()?;
        let mut cache = account_cache(&path, &secrets)?;
        if refresh {
            if cache.host.is_null() {
                return Err("Refresh booking before loading analytics.".into());
            }
            let data = Api::new()?.get("/api/device/analytics", bearer(&secrets)?)?;
            cache.analytics = analytics(&data);
            save_cache(&path, &cache)?;
        }
        Ok(json!({"analytics":cache.analytics,"cached":!refresh}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_inheritance_is_explicit_and_projects_the_effective_availability() {
        let data = json!({
            "defaultAvailabilityScheduleId":"host-hours",
            "availabilitySchedules":[
                {"id":"host-hours","zone":"Asia/Kathmandu","weeklyHours":{"1":[{"start":550,"end":900}]},"dateOverrides":{"2026-12-25":[]}},
                {"id":"saved-hours","zone":"Europe/Oslo","weeklyHours":{"2":[{"start":600,"end":960}]},"dateOverrides":{}}
            ],
            "meetingTypes":[
                {"id":"inherited","slug":"inherited","useDefaultSchedule":true,"scheduleId":"saved-hours","zone":"UTC"},
                {"id":"explicit","slug":"explicit","useDefaultSchedule":false,"scheduleId":"saved-hours","zone":"UTC"},
                {"id":"legacy","slug":"legacy","scheduleId":"saved-hours","zone":"UTC"},
                {"id":"custom","slug":"custom","zone":"America/New_York"}
            ]
        });
        let original = data.clone();
        let projected = types(&data);
        assert_eq!(projected[0]["zone"], "Asia/Kathmandu");
        assert_eq!(projected[0]["useDefaultSchedule"], true);
        assert_eq!(
            projected[0]["scheduleId"], "saved-hours",
            "raw fallback ID is not overwritten"
        );
        for index in [1, 2] {
            assert_eq!(projected[index]["zone"], "Europe/Oslo");
            assert_eq!(projected[index]["useDefaultSchedule"], false);
        }
        assert_eq!(projected[3]["zone"], "America/New_York");
        let schedule = effective_schedule(&data, &data["meetingTypes"][0]).unwrap();
        assert_eq!(schedule["weeklyHours"]["1"][0]["start"], 550);
        assert_eq!(schedule["dateOverrides"]["2026-12-25"], json!([]));
        assert_eq!(
            data, original,
            "effective projection must not mutate saved policy"
        );
        let mut switched = data.clone();
        switched["defaultAvailabilityScheduleId"] = json!("saved-hours");
        assert_eq!(types(&switched)[0]["zone"], "Europe/Oslo");
        assert_eq!(types(&switched)[3]["zone"], "America/New_York");
    }

    #[test]
    fn missing_inherited_default_does_not_select_an_inactive_explicit_reference() {
        let data = json!({"defaultAvailabilityScheduleId":null,"availabilitySchedules":[{"id":"saved","zone":"Europe/Oslo"}]});
        let meeting = json!({"useDefaultSchedule":true,"scheduleId":"saved","zone":"UTC"});
        assert!(effective_schedule(&data, &meeting).is_none());
        let legacy = json!({"useDefaultSchedule":false,"scheduleId":"saved"});
        assert_eq!(
            effective_schedule(&data, &legacy).unwrap()["zone"],
            "Europe/Oslo"
        );
    }

    #[test]
    fn cached_meeting_uses_shared_schedule_timezone() {
        let projected = types(
            &json!({"meetingTypes":[{"id":"intro","slug":"intro","zone":"UTC","scheduleId":"work"}],"availabilitySchedules":[{"id":"work","zone":"Europe/Oslo"}]}),
        );
        assert_eq!(projected[0]["zone"], "Europe/Oslo");
        assert_eq!(projected[0]["scheduleId"], "work");
    }

    #[test]
    fn pending_and_rate_limited_polls_keep_pkce_and_back_off_after_the_response() {
        for status in [428, 429] {
            for (data, interval) in [
                (json!({"interval":5}), 5),
                (json!({"interval":0}), 5),
                (json!({"interval":5000}), 60),
                (json!({}), 5),
                (json!({"interval":"private"}), 5),
            ] {
                let mut pending = Pending {
                    device_code: "private-device-code".into(),
                    verifier: "private-verifier".into(),
                    expires_at: 600000,
                    next_poll: 5000,
                    interval: 5,
                    settings_write: false,
                };
                let response = pending_poll_response(&mut pending, status, &data, 12345).unwrap();
                assert_eq!(response, json!({"pending":true,"interval":interval}));
                assert_eq!(pending.next_poll, 12345 + interval * 1000);
                assert_eq!(pending.device_code, "private-device-code");
                assert_eq!(pending.verifier, "private-verifier");
                assert_eq!(pending.expires_at, 600000);
                assert!(!response.to_string().contains("private"));
            }
        }
        let mut pending = Pending {
            device_code: "code".into(),
            verifier: "verifier".into(),
            expires_at: 600000,
            next_poll: 5000,
            interval: 5,
            settings_write: false,
        };
        for status in [200, 401, 410, 500] {
            assert!(pending_poll_response(&mut pending, status, &json!({}), 12345).is_none());
            assert_eq!(pending.next_poll, 5000);
        }
    }
    #[test]
    fn missing_secure_item_is_unlinked_but_locked_or_unavailable_storage_is_not() {
        assert!(
            decode_secret(Err(keyring::Error::NoEntry))
                .unwrap()
                .token
                .is_none()
        );
        for error in [
            keyring::Error::NoStorageAccess(Box::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "private locked store",
            ))),
            keyring::Error::PlatformFailure(Box::new(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "private unavailable service",
            ))),
        ] {
            assert_eq!(
                decode_secret(Err(error)).err().unwrap(),
                "Booking secure storage is unavailable."
            );
        }
        assert!(decode_secret(Ok("private malformed secret".into())).is_err());
    }
    #[test]
    fn unapproved_pkce_state_is_memory_only_and_scoped_to_each_profile() {
        let first = std::path::PathBuf::from(format!("/profile/{}", Uuid::new_v4()));
        let second = std::path::PathBuf::from(format!("/profile/{}", Uuid::new_v4()));
        let mut pending = pending_connections().lock().unwrap();
        pending.insert(
            first.clone(),
            Pending {
                device_code: "private-device-code".into(),
                verifier: "private-verifier".into(),
                expires_at: now() + 600000,
                next_poll: now() + 5000,
                interval: 5,
                settings_write: false,
            },
        );
        assert!(pending.get(&first).is_some());
        assert!(pending.get(&second).is_none());
        assert!(
            !serde_json::to_string(&Secrets::default())
                .unwrap()
                .contains("private")
        );
        pending.remove(&first);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_booking_uses_dedicated_os_collection_without_opening_it() {
        let entry = secret_entry().unwrap();
        let credential = entry
            .get_credential()
            .downcast_ref::<keyring::secret_service::SsCredential>()
            .unwrap();
        assert_eq!(
            credential.attributes.get("target").map(String::as_str),
            Some(crate::profile::LINUX_SECRET_COLLECTION)
        );
        assert_eq!(
            credential.attributes.get("service").map(String::as_str),
            Some(super::super::KEYRING_SERVICE)
        );
        assert_eq!(
            credential.attributes.get("username").map(String::as_str),
            Some(secret_id().as_str())
        );
    }
    #[test]
    fn isolated_stores_never_share_the_real_scheduling_credential() {
        use std::path::Path;
        assert_eq!(secret_id_for(None), SECRET_ID);
        if let Some(directory) = crate::default_store_directory() {
            assert_eq!(secret_id_for(Some(&directory.join("mail.sqlite"))), SECRET_ID);
        }
        let demo = secret_id_for(Some(Path::new("/repo/dist/demo/mail.sqlite")));
        assert!(demo.starts_with("booking.desktop.v1.store-") && demo.len() == SECRET_ID.len() + 23);
        assert_eq!(demo, secret_id_for(Some(Path::new("/repo/dist/demo/mail.sqlite"))));
        assert_ne!(demo, secret_id_for(Some(Path::new("/other/mail.sqlite"))));
        let absolute = std::env::current_dir().unwrap().join("relative/mail.sqlite");
        assert_eq!(
            secret_id_for(Some(Path::new("relative/mail.sqlite"))),
            secret_id_for(Some(&absolute))
        );
    }
    #[test]
    fn links_have_fixed_origin_and_email_attribution() {
        let host = "h_abcdefghijklmnopqrstuvwx";
        assert_eq!(
            link(host, "intro").unwrap(),
            format!("{TEST_ORIGIN}/b/{host}/intro?source=email")
        );
        for slug in [
            "//evil.example",
            "intro?token=secret",
            "../intro",
            "intro%2fsecret",
            "Intro",
            "",
        ] {
            assert!(link(host, slug).is_err());
        }
        for host in [
            "h_short",
            "https://evil.example",
            "h_abcdefghijklmnopqrstuvw/",
        ] {
            assert!(link(host, "intro").is_err());
        }
    }
    #[test]
    fn cache_projection_never_copies_provider_credentials_or_capabilities() {
        let data = json!({"meetingTypes":[{"id":"intro","slug":"intro","published":true,"title":"Intro","accessToken":"secret","managementToken":"secret","accountId":"private"}],"bookings":[{"id":"b","title":"Intro","start":10,"end":20,"token":"secret","answer":"private intake","answers":{"topic":"private intake"},"questionSnapshot":[{"label":"private intake"}],"additionalGuests":["private intake"],"tokenRecovery":{"token":"secret"},"managementUrl":"https://secret"}]});
        let projected = json!({"types":types(&data),"bookings":bookings(&data)}).to_string();
        assert!(!projected.contains("secret"));
        assert!(!projected.contains("private intake"));
        assert!(projected.contains("Intro"));
        let host = host_identity(
            &json!({"hostId":"h_abcdefghijklmnopqrstuvwx","name":"Host","accessToken":"secret"}),
        )
        .unwrap();
        assert!(!host.to_string().contains("secret"));
        assert!(host_identity(&json!({"hostId":"../../evil"})).is_err());
    }
    #[test]
    fn cache_is_private_and_round_trips_without_secrets() {
        let path = std::env::temp_dir().join(format!("email-booking-test-{}", Uuid::new_v4()));
        let cache = Cache {
            host: json!({"host_id":"h_abcdefghijklmnopqrstuvwx"}),
            meeting_types: types(&json!({"meetingTypes":[{"id":"intro","slug":"intro"}]})),
            ..Cache::default()
        };
        save_cache(&path, &cache).unwrap();
        assert_eq!(load_cache(&path).unwrap().meeting_types.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_file(&path).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/missing/booking-cache-target", &path).unwrap();
            assert!(save_cache(&path, &cache).is_err());
            assert!(load_cache(&path).is_err());
            fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn expiry_and_http_errors_are_safe() {
        assert!(
            bearer(&Secrets {
                token: Some("secret".into()),
                expires_at: 1,
                host_id: None,
                settings_write: false,
            })
            .is_err()
        );
        assert!(
            bearer(&Secrets {
                token: Some("secret\r\nInjected".into()),
                expires_at: u64::MAX,
                host_id: None,
                settings_write: false,
            })
            .is_err()
        );
        for status in [401, 403, 429, 500] {
            let message = status_error(status);
            assert!(!message.contains("secret"));
            assert!(!message.contains(TEST_ORIGIN));
        }
    }
    #[test]
    fn analytics_keeps_aggregate_counts_and_email_source_only() {
        let data = json!({"period":{"from":"2026-10-12","to":"2026-11-10","timeZone":"UTC","days":30,"token":"secret"},"totals":{"pageViews":42,"confirmed":4,"accessToken":"secret"},"sources":{"email":3,"website":1,"direct":2,"https://private.example":99},"series":[{"day":"2026-11-10","confirmed":2,"guestEmail":"secret"}],"trackingStartedAt":10,"token":"secret"});
        let result = analytics(&data);
        assert_eq!(result["sources"]["email"], 3);
        assert_eq!(result["totals"]["pageViews"], 42);
        assert!(!result.to_string().contains("secret"));
        assert!(!result.to_string().contains("private.example"));
    }
    #[test]
    fn issued_credentials_require_the_complete_device_scope() {
        let mut data = json!({"accessToken":"safe-token","tokenType":"Bearer","scope":DEVICE_SCOPE,"expiresAt":now()+60000});
        assert_eq!(issued_credential(&data, false).unwrap().0, "safe-token");
        data["scope"] = json!("booking:read");
        assert!(issued_credential(&data, false).is_err());
        data["scope"] = json!(DEVICE_SCOPE);
        data["expiresAt"] = json!(1);
        assert!(issued_credential(&data, false).is_err());
    }
    #[test]
    fn rejected_credentials_are_invalidated_without_clearing_a_newer_connection() {
        let mut value = Secrets {
            token: Some("current".into()),
            expires_at: now() + 60000,
            host_id: Some("host".into()),
            settings_write: false,
        };
        assert!(!clear_rejected_token(&mut value, "older"));
        assert!(bearer(&value).is_ok());
        assert_eq!(value.host_id.as_deref(), Some("host"));
        assert!(clear_rejected_token(&mut value, "current"));
        assert!(bearer(&value).is_err());
        assert_eq!(value.expires_at, 0);
        assert_eq!(value.host_id, None);
    }
    #[test]
    fn verification_handoff_allows_only_the_exact_origin_and_user_code_fragment() {
        let code = "ABCD-EFGH-JKLM";
        let expected = format!("{TEST_ORIGIN}/device#code={code}");
        assert_eq!(
            verification_url(&json!({"verificationUrl":expected}), code).unwrap(),
            expected
        );
        for value in [
            format!("{TEST_ORIGIN}/device?code={code}"),
            format!("{TEST_ORIGIN}/device#code=ABCD-EFGH-NPQR"),
            format!("{TEST_ORIGIN}/device#code={code}&secret=bad"),
            format!("https://evil.example/device#code={code}"),
            format!("{TEST_ORIGIN}/device"),
        ] {
            assert!(verification_url(&json!({"verificationUrl":value}), code).is_err());
        }
        assert!(
            verification_url(
                &json!({"verificationUrl":format!("{TEST_ORIGIN}/device#code=bad")}),
                "bad"
            )
            .is_err()
        );
    }
}
