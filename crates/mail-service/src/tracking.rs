use super::{
    MailService,
    booking::{ORIGIN, tracking_request},
};
use mail_core::{EmailTracking, Message, TrackingImageEvent};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::HashMap;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn host(data: &Value) -> Result<&str, String> {
    data.get("hostId")
        .and_then(Value::as_str)
        .filter(|s| {
            s.len() == 26
                && s.starts_with("h_")
                && s[2..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .ok_or_else(|| "Tracking service returned an invalid account.".into())
}
/// A Scheduling tracking pixel: `/api/tracking/pixel/<43 base64url>.gif` with no
/// credentials, query or fragment. Matching the exact shape, on any origin,
/// strips other builds' pixels without touching ordinary images.
fn is_pixel(url: &reqwest::Url) -> bool {
    let token = url
        .path()
        .strip_prefix("/api/tracking/pixel/")
        .and_then(|v| v.strip_suffix(".gif"));
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && token.is_some_and(|s| {
            s.len() == 43
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}
// Registration and pixel insertion exist only in builds whose profile enables
// open tracking (`email_open_tracking`, see build.rs). Tests keep the pure
// helpers to check pixel validation and the reader's own-pixel stripping.
#[cfg(any(email_open_tracking, test))]
fn pixel_url(data: &Value) -> Result<&str, String> {
    let value = data
        .get("pixelUrl")
        .and_then(Value::as_str)
        .ok_or("Tracking service returned an invalid image address.")?;
    let url = reqwest::Url::parse(value)
        .map_err(|_| "Tracking service returned an invalid image address.")?;
    // Pixels derive from the profile's Scheduling origin; none is accepted without one.
    if Some(url.origin().ascii_serialization().as_str()) != ORIGIN || !is_pixel(&url) {
        return Err("Tracking service returned an invalid image address.".into());
    }
    Ok(value)
}
#[cfg(email_open_tracking)]
fn register(record: &EmailTracking) -> Result<Value, String> {
    let data = tracking_request(
        Method::POST,
        "/api/device/tracking",
        Some(json!({"id":record.tracking_id})),
    )?;
    if data.get("id").and_then(Value::as_str) != Some(record.tracking_id.as_str()) {
        return Err("Tracking service returned an invalid message identifier.".into());
    }
    if record
        .host_id
        .as_deref()
        .is_some_and(|id| Some(id) != data.get("hostId").and_then(Value::as_str))
    {
        return Err(
            "Reconnect the original tracking account or disable tracking for this draft.".into(),
        );
    }
    host(&data)?;
    pixel_url(&data)?;
    Ok(data)
}
#[cfg(any(email_open_tracking, test))]
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
#[cfg(any(email_open_tracking, test))]
fn html_with_pixel(plain: &str, html: Option<&str>, url: &str) -> String {
    let body = html
        .map(strip_tracking_pixels)
        .unwrap_or_else(|| format!("<div>{}</div>", escape(plain).replace('\n', "<br>\n")));
    format!("{body}<img src=\"{url}\" width=\"1\" height=\"1\" alt=\"\" style=\"display:none\" />")
}
pub(super) fn strip_tracking_pixels(html: &str) -> String {
    // Called after HTML sanitization in the shared native/MCP reader; no image
    // requests are made here. Own Sent previews must not inflate activity.
    let images =
        regex::Regex::new(r#"(?i)<img\b[^>]*\bsrc="([^"]*)"[^>]*>"#).expect("image pattern");
    images
        .replace_all(html, |caps: &regex::Captures<'_>| {
            // Any origin: a build without (or with another) Scheduling service
            // must not load pixels in mail that a different build sent.
            if reqwest::Url::parse(&caps[1]).ok().is_some_and(|url| is_pixel(&url)) {
                String::new()
            } else {
                caps[0].to_owned()
            }
        })
        .into_owned()
}

/// Open tracking comes from the distribution profile's `OPEN_TRACKING` key
/// (`development` only; a distribution profile cannot enable it). Without it
/// no shell, scheduler or MCP send can register a record or add a pixel, and
/// saved activity stays readable. Tests use the public behavior regardless of
/// the developer's profile, as `booking::ORIGIN` does.
#[cfg(not(test))]
pub(crate) const OPEN_TRACKING: bool = super::profile::OPEN_TRACKING;
const UNAVAILABLE: &str = "Open tracking is not available in this build.";
#[cfg(test)]
pub(crate) const OPEN_TRACKING: bool = false;

impl MailService {
    pub(super) fn tracking_get(&self, id: &str) -> Result<Value, String> {
        // Without a Scheduling service a stale enabled record has no effect.
        if ORIGIN.is_none() {
            return Ok(json!({"enabled":false,"available":false}));
        }
        let record = self.store.email_tracking(id).map_err(|e| e.to_string())?;
        Ok(json!({"enabled":record.is_some_and(|r|r.enabled),"available":OPEN_TRACKING}))
    }
    /// Whether this build can track opens, and whether saved activity exists
    /// for the shells to offer. One indexed query; no record is loaded.
    pub(super) fn tracking_capability(&self) -> Result<Value, String> {
        // Without a Scheduling service there is nothing to show or refresh.
        let saved = ORIGIN.is_some() && self.store.has_tracked_sent_mail().map_err(|e| e.to_string())?;
        Ok(json!({"available":OPEN_TRACKING,"saved_activity":saved}))
    }
    pub(super) fn tracking_set(&self, id: &str, enabled: bool) -> Result<Value, String> {
        if !enabled {
            self.store
                .set_email_tracking(id, false, None)
                .map_err(|e| e.to_string())?;
            return Ok(json!({"enabled":false,"available":OPEN_TRACKING}));
        }
        if ORIGIN.is_none() {
            return Err(super::booking::UNCONFIGURED.into());
        }
        if !OPEN_TRACKING {
            return Err(UNAVAILABLE.into());
        }
        self.register_tracking(id)
    }
    #[cfg(email_open_tracking)]
    fn register_tracking(&self, id: &str) -> Result<Value, String> {
        let record = self
            .store
            .prepare_email_tracking(id)
            .map_err(|e| e.to_string())?;
        let data = register(&record)?;
        self.store
            .set_email_tracking(id, true, Some(host(&data)?))
            .map_err(|e| e.to_string())?;
        self.store
            .cache_tracking_snapshot(
                &record.tracking_id,
                host(&data)?,
                record.pixel_requests,
                data["expiresAt"].as_i64(),
                None,
                false,
                None,
            )
            .map_err(|e| e.to_string())?;
        Ok(json!({"enabled":true,"available":true}))
    }
    #[cfg(not(email_open_tracking))]
    fn register_tracking(&self, _id: &str) -> Result<Value, String> {
        Err(UNAVAILABLE.into())
    }
    pub(super) fn tracking_outgoing_html(&self, draft: &Message) -> Result<Option<String>, String> {
        // No Scheduling service: send unchanged rather than fail on a stale record.
        if ORIGIN.is_none() {
            return Ok(draft.body_html.clone());
        }
        let Some(record) = self
            .store
            .email_tracking(&draft.id)
            .map_err(|e| e.to_string())?
            .filter(|r| r.enabled)
        else {
            return Ok(draft.body_html.clone());
        };
        if !OPEN_TRACKING {
            // A development build marked this draft. Send it without a pixel;
            // mark_sent turns tracking off once the provider accepts it.
            return Ok(draft.body_html.clone());
        }
        self.pixel_body(draft, &record)
    }
    #[cfg(email_open_tracking)]
    fn pixel_body(&self, draft: &Message, record: &EmailTracking) -> Result<Option<String>, String> {
        let data = register(record)?;
        Ok(Some(html_with_pixel(
            &draft.body_plain,
            draft.body_html.as_deref(),
            pixel_url(&data)?,
        )))
    }
    #[cfg(not(email_open_tracking))]
    fn pixel_body(&self, draft: &Message, _record: &EmailTracking) -> Result<Option<String>, String> {
        Ok(draft.body_html.clone())
    }
    /// Record provider acceptance. A build without open tracking sent the draft
    /// without a pixel, so the same transaction turns its tracking off; a failed
    /// send never reaches this and changes nothing. Returns a warning when a
    /// draft that had tracking on was sent without it.
    pub(super) fn mark_sent(&self, id: &str) -> mail_core::Result<Option<String>> {
        if OPEN_TRACKING {
            return self.store.mark_draft_sent(id).map(|_| None);
        }
        Ok(self.store.mark_draft_sent_untracked(id)?.then(|| {
            "Sent without open tracking: this build does not include it.".to_string()
        }))
    }
    pub(super) fn tracking_activity(
        &self,
        id: Option<&str>,
        refresh: bool,
    ) -> Result<Value, String> {
        let records: Vec<_> = if let Some(id) = id {
            match self.store.email_tracking(id).map_err(|e| e.to_string())? {
                Some(record) if record.enabled => {
                    if self
                        .store
                        .tracking_message(&record.message_id)
                        .map_err(|e| e.to_string())?
                        .is_some_and(|m| m.is_sent && !m.is_draft)
                    {
                        vec![record]
                    } else {
                        vec![]
                    }
                }
                _ => vec![],
            }
        } else {
            self.store
                .email_tracking_history()
                .map_err(|e| e.to_string())?
        };
        if refresh && !records.is_empty() {
            let path = if records.len() == 1 {
                format!("/api/device/tracking?id={}", records[0].tracking_id)
            } else {
                "/api/device/tracking".into()
            };
            let data = tracking_request(Method::GET, &path, None)?;
            let owner = host(&data)?;
            for item in data
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(200)
            {
                let Some(remote_id) = item.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if !records
                    .iter()
                    .any(|r| r.tracking_id == remote_id && r.host_id.as_deref() == Some(owner))
                {
                    continue;
                }
                let Some(count) = item.get("pixelRequests").and_then(Value::as_u64) else {
                    continue;
                };
                self.store
                    .update_email_tracking_activity(
                        remote_id,
                        owner,
                        count,
                        item.get("firstRequestedAt").and_then(Value::as_i64),
                        item.get("lastRequestedAt").and_then(Value::as_i64),
                    )
                    .map_err(|e| e.to_string())?;
                let events: Option<Vec<TrackingImageEvent>> = item
                    .get("events")
                    .map(|events| {
                        events
                            .as_array()
                            .ok_or_else(|| "Invalid tracking history".to_string())?
                            .iter()
                            .map(|event| {
                                Ok(TrackingImageEvent {
                                    at_ms: event["at"].as_i64().ok_or("Invalid tracking time")?,
                                    count: event["count"]
                                        .as_u64()
                                        .ok_or("Invalid tracking count")?,
                                })
                            })
                            .collect::<Result<Vec<_>, String>>()
                    })
                    .transpose()?;
                self.store
                    .cache_tracking_snapshot(
                        remote_id,
                        owner,
                        count,
                        item["expiresAt"].as_i64(),
                        events.as_deref(),
                        item["timelineComplete"].as_bool().unwrap_or(false),
                        Some(now_ms()),
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
        let mut followups = HashMap::new();
        let mut items = Vec::new();
        for original in records {
            let Some(record) = self
                .store
                .email_tracking(&original.message_id)
                .map_err(|e| e.to_string())?
            else {
                continue;
            };
            let Some(message) = self
                .store
                .tracking_message(&record.message_id)
                .map_err(|e| e.to_string())?
            else {
                continue;
            };
            if message.is_draft || !message.is_sent {
                continue;
            }
            if !followups.contains_key(&message.account_id) {
                followups.insert(
                    message.account_id.clone(),
                    self.store
                        .list_followups(Some(&message.account_id))
                        .map_err(|e| e.to_string())?,
                );
            }
            let followup = followups[&message.account_id]
                .iter()
                .find(|f| f.thread_id == message.thread_id);
            let detail = self
                .store
                .tracking_detail(&record.tracking_id)
                .map_err(|e| e.to_string())?;
            let sent_at = detail.sent_at_ms.unwrap_or(message.received_at);
            let (reply, reply_scan_limited) = self
                .store
                .tracking_reply(&record.message_id, sent_at)
                .map_err(|e| e.to_string())?;
            let mut timeline = vec![json!({"kind":"sent","at_ms":sent_at})];
            for event in &detail.events {
                timeline.push(json!({"kind":"image_request","at_ms":event.at_ms,"count":event.count,"approximate":true}));
            }
            if let Some((reply_id, at)) = &reply {
                timeline.push(json!({"kind":"reply_received","at_ms":at,"message_id":reply_id}));
            }
            timeline.sort_by_key(|event| event["at_ms"].as_i64().unwrap_or(0));
            let reply_at = reply.as_ref().map(|(_, at)| *at);
            let last = record
                .last_requested_at
                .unwrap_or(sent_at)
                .max(reply_at.unwrap_or(sent_at))
                .max(sent_at);
            items.push(json!({"message_id":record.message_id,"thread_id":message.thread_id,"account_id":message.account_id,
                "subject":message.subject,"recipient":message.to.join(", "),"sent_at_ms":sent_at,"sent_time_source":if detail.sent_at_ms.is_some(){"provider_acceptance"}else{"cached_message"},
                "last_activity_at_ms":last,"pixel_requests":record.pixel_requests,"first_requested_at":record.first_requested_at,"last_requested_at":record.last_requested_at,
                "tracking_status":match detail.expires_at {Some(at) if at<=now_ms()=>"expired",Some(_)=>"active",None=>"unknown"},"expires_at_ms":detail.expires_at,
                "reply_received":reply.is_some(),"reply_message_id":reply.as_ref().map(|(id,_)|id),"reply_received_at_ms":reply_at,"reply_coverage":"cached","reply_scan_limited":reply_scan_limited,"reply_baseline_available":self.store.tracking_reply_baseline_available(&record.message_id).map_err(|e|e.to_string())?,
                "followup":followup,"timeline":timeline,"timeline_complete":detail.timeline_complete && detail.remote_count==record.pixel_requests,"refreshed_at_ms":detail.refreshed_at_ms}));
        }
        items.sort_by_key(|item| {
            std::cmp::Reverse(item["last_activity_at_ms"].as_i64().unwrap_or(0))
        });
        let refreshed = if items
            .iter()
            .all(|item| item["refreshed_at_ms"].as_i64().is_some())
        {
            items
                .iter()
                .filter_map(|item| item["refreshed_at_ms"].as_i64())
                .min()
        } else {
            None
        };
        Ok(
            json!({"items":items,"cached":!refresh,"refreshed_at_ms":refreshed,"reply_coverage":"cached"}),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guarded_service_reminder_refuses_a_reply_arriving_after_activity_was_read() {
        let root = std::env::temp_dir().join(format!("tracking-guard-{}", uuid::Uuid::new_v4()));
        let service = MailService::open(root.join("mail.sqlite")).unwrap();
        service.store.seed_demo().unwrap();
        let draft = service
            .store
            .compose_draft("demo:personal", "recipient@example.test", "Subject", "Body")
            .unwrap();
        service.store.prepare_email_tracking(&draft.id).unwrap();
        service
            .store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        service.store.mark_draft_sent(&draft.id).unwrap();
        let before = service.tracking_activity(Some(&draft.id), false).unwrap();
        assert_eq!(before["items"][0]["reply_received"], false);
        assert_eq!(before["items"][0]["reply_baseline_available"], true);
        let mut reply = draft.clone();
        reply.id = "service-racing-reply".into();
        reply.sender = "recipient@example.test".into();
        reply.received_at = now_ms() + 1;
        reply.is_draft = false;
        reply.is_sent = false;
        service.store.upsert_message(&reply).unwrap();
        let request = json!({"op":"set_followup","thread_id":draft.thread_id,"due_at_ms":now_ms()+86_400_000,"only_if_no_reply":true,"sent_message_id":draft.id});
        let error = service.request_json(&request.to_string()).unwrap_err();
        assert!(error.contains("A reply has arrived. Refresh activity before setting a reminder."));
        assert!(
            service
                .store
                .list_followups(Some("demo:personal"))
                .unwrap()
                .is_empty()
        );
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn activity_contains_only_local_context_real_history_and_cached_reply_state() {
        let root = std::env::temp_dir().join(format!("tracking-context-{}", uuid::Uuid::new_v4()));
        let service = MailService::open(root.join("mail.sqlite")).unwrap();
        service.store.seed_demo().unwrap();
        let draft = service
            .store
            .compose_draft(
                "demo:personal",
                "recipient@example.test",
                "Local private subject",
                "Local private body",
            )
            .unwrap();
        let record = service.store.prepare_email_tracking(&draft.id).unwrap();
        service
            .store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        assert!(
            service.tracking_activity(Some(&draft.id), false).unwrap()["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        service.store.mark_draft_sent(&draft.id).unwrap();
        let at = service
            .store
            .tracking_detail(&record.tracking_id)
            .unwrap()
            .sent_at_ms
            .unwrap();
        service
            .store
            .set_followup(&draft.thread_id, now_ms() + 86_400_000, true)
            .unwrap();
        let first = service.tracking_activity(Some(&draft.id), false).unwrap();
        let item = &first["items"][0];
        assert_eq!(item["subject"], "Local private subject");
        assert_eq!(item["thread_id"], draft.thread_id);
        assert_eq!(item["recipient"], "recipient@example.test");
        assert_eq!(item["reply_received"], false);
        assert_eq!(item["followup"]["status"], "waiting");
        assert_eq!(item["tracking_status"], "unknown");
        assert_eq!(item["timeline_complete"], false);
        assert_eq!(item["timeline"], json!([{"kind":"sent","at_ms":at}]));
        assert!(first["refreshed_at_ms"].is_null());
        let mut reply = draft.clone();
        reply.id = "context-reply".into();
        reply.sender = "recipient@example.test".into();
        reply.received_at = now_ms() + 100;
        reply.is_draft = false;
        reply.is_sent = false;
        service.store.upsert_message(&reply).unwrap();
        let activity = service.tracking_activity(Some(&draft.id), false).unwrap();
        let item = &activity["items"][0];
        assert_eq!(item["reply_received"], true);
        assert_eq!(item["followup"]["status"], "replied");
        assert_eq!(item["reply_message_id"], reply.id);
        assert_eq!(item["timeline"][1]["kind"], "reply_received");
        assert!(!activity.to_string().contains("Local private body"));
        assert!(
            service.tracking_activity(Some("unknown"), false).unwrap()["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn notification_poll_without_opted_in_sent_mail_needs_no_connection() {
        let root = std::env::temp_dir().join(format!("email-open-alerts-{}", uuid::Uuid::new_v4()));
        let service = MailService::open(root.join("mail.sqlite")).unwrap();
        service.store.seed_demo().unwrap();
        let draft = service
            .store
            .compose_draft("demo:personal", "reader@example.test", "Subject", "Body")
            .unwrap();
        let record = service.store.prepare_email_tracking(&draft.id).unwrap();
        service
            .store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        service
            .store
            .set_email_tracking(&draft.id, false, None)
            .unwrap();
        service.store.mark_draft_sent(&draft.id).unwrap();
        // Even a cached image count cannot opt a message into tracking.
        service
            .store
            .update_email_tracking_activity(&record.tracking_id, "host", 10, Some(1), Some(2))
            .unwrap();
        assert_eq!(
            service
                .store
                .email_tracking(&draft.id)
                .unwrap()
                .unwrap()
                .pixel_requests,
            10
        );
        for enabled in [false, true] {
            let result = service
                .request_json(
                    &json!({"op":"tracking_notifications","enabled":enabled,"session_since":0})
                        .to_string(),
                )
                .unwrap();
            assert_eq!(result["items"], json!([]));
        }
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tracking_is_local_off_by_default_and_disabled_without_credentials() {
        let root = std::env::temp_dir().join(format!("email-tracking-{}", uuid::Uuid::new_v4()));
        let service = MailService::open(root.join("mail.sqlite")).unwrap();
        service.store.seed_demo().unwrap();
        let draft = service
            .store
            .compose_draft(
                "demo:personal",
                "recipient@example.com",
                "Subject",
                "Private <body>",
            )
            .unwrap();
        assert_eq!(service.tracking_get(&draft.id).unwrap()["enabled"], false);
        assert_eq!(
            service.tracking_set(&draft.id, false).unwrap()["enabled"],
            false
        );
        assert!(service.tracking_outgoing_html(&draft).unwrap().is_none());
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
    use super::super::booking::TEST_ORIGIN;
    #[test]
    fn builds_without_open_tracking_never_register_or_add_a_pixel() {
        let root = std::env::temp_dir().join(format!("email-untracked-{}", uuid::Uuid::new_v4()));
        let service = MailService::open(root.join("mail.sqlite")).unwrap();
        service.store.seed_demo().unwrap();
        let draft = service
            .store
            .compose_draft("demo:personal", "recipient@example.com", "Subject", "Body")
            .unwrap();
        assert!(service.tracking_set(&draft.id, true).is_err());
        assert_eq!(service.tracking_get(&draft.id).unwrap()["available"], false);
        assert_eq!(service.tracking_capability().unwrap(), json!({"available":false,"saved_activity":false}));
        // A draft a development build marked for tracking sends without a pixel.
        service.store.prepare_email_tracking(&draft.id).unwrap();
        service
            .store
            .set_email_tracking(&draft.id, true, Some("host"))
            .unwrap();
        assert_eq!(service.tracking_get(&draft.id).unwrap()["enabled"], true);
        assert_eq!(service.tracking_outgoing_html(&draft).unwrap(), draft.body_html);
        // Building the outgoing body changes nothing: a failed send keeps state.
        assert_eq!(service.tracking_get(&draft.id).unwrap()["enabled"], true);
        assert!(service.mark_sent(&draft.id).unwrap().is_some_and(|w| w.contains("without open tracking")));
        assert_eq!(service.tracking_get(&draft.id).unwrap()["enabled"], false);
        assert!(service.store.email_tracking_history().unwrap().is_empty());
        assert_eq!(service.tracking_capability().unwrap()["saved_activity"], false);
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pixel_is_only_fixed_origin_and_plain_text_is_escaped() {
        let url = format!("{TEST_ORIGIN}/api/tracking/pixel/{}.gif", "a".repeat(43));
        assert_eq!(pixel_url(&json!({"pixelUrl":url})).unwrap(), url);
        for value in [
            "file:///etc/passwd",
            "https://evil.example/api/tracking/pixel/abc.gif",
            "https://scheduling.example.test.evil.example/api/tracking/pixel/abc.gif",
        ] {
            assert!(pixel_url(&json!({"pixelUrl":value})).is_err());
        }
        let html = html_with_pixel("Private <script>\n& text", None, &url);
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert_eq!(html.matches("<img ").count(), 1);
        let safe = strip_tracking_pixels(&html);
        assert!(!safe.contains("/api/tracking/pixel/"));
        assert!(safe.contains("&lt;script&gt;"));
        assert!(
            strip_tracking_pixels("<img src=\"https://example.com/logo.png\">")
                .contains("logo.png")
        );
        let other = format!(
            "<img src=\"https://other-scheduling.example/api/tracking/pixel/{}.gif\">",
            "a".repeat(43)
        );
        assert!(!strip_tracking_pixels(&other).contains("/api/tracking/pixel/"));
        for image in [
            "https://cdn.example/api/tracking/pixel/banner.png",
            "https://cdn.example/api/tracking/pixel/short.gif",
        ] {
            let html = format!("<img src=\"{image}\">");
            assert!(strip_tracking_pixels(&html).contains(image), "{image}");
        }
    }
    #[test]
    fn shared_reader_removes_own_pixels_without_changing_stored_outgoing_content() {
        let root =
            std::env::temp_dir().join(format!("email-tracking-reader-{}", uuid::Uuid::new_v4()));
        let service = MailService::open(root.join("mail.sqlite")).unwrap();
        service.store.seed_demo().unwrap();
        let mut draft = service
            .store
            .compose_draft("demo:personal", "recipient@example.com", "Subject", "Body")
            .unwrap();
        let url = format!("{TEST_ORIGIN}/api/tracking/pixel/{}.gif", "a".repeat(43));
        draft.body_html = Some(html_with_pixel(
            "Body",
            Some("<p>Body</p><img src=\"https://example.com/logo.png\">"),
            &url,
        ));
        service.store.upsert_message(&draft).unwrap();
        let rendered = service
            .request_json(&json!({"op":"render_message","message_id":draft.id}).to_string())
            .unwrap();
        let safe = rendered["body_html_safe"].as_str().unwrap();
        assert!(!safe.contains("/api/tracking/pixel/"));
        assert!(safe.contains("logo.png"));
        assert!(
            service
                .store
                .get_message(&draft.id)
                .unwrap()
                .unwrap()
                .body_html
                .unwrap()
                .contains("/api/tracking/pixel/")
        );
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
}
