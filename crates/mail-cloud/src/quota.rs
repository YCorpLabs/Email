//! Per-credential Gmail pacing shared by short-lived transport clients.
//! Only an opaque digest is kept; credentials and URLs are never persisted.
use crate::{CloudError, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct Budget {
    next: Option<Instant>,
    cooldown: Option<Instant>,
}
impl Budget {
    pub(crate) fn shared(client_id: &str, credential: &str) -> Arc<Mutex<Self>> {
        static BUDGETS: OnceLock<Mutex<HashMap<[u8; 32], Arc<Mutex<Budget>>>>> = OnceLock::new();
        let key = Sha256::digest(format!("{client_id}\0{credential}")).into();
        BUDGETS
            .get_or_init(Default::default)
            .lock()
            .expect("quota registry")
            .entry(key)
            .or_default()
            .clone()
    }
    fn check(&self, now: Instant) -> Result<()> {
        if let Some(until) = self.cooldown.filter(|until| *until > now) {
            return Err(CloudError::RateLimited(
                until.duration_since(now).as_secs().max(1),
            ));
        }
        Ok(())
    }
    pub(crate) fn reserve(&mut self, cost: u64) -> Result<Duration> {
        let now = Instant::now();
        self.check(now)?;
        let slot = self.next.unwrap_or(now).max(now);
        // 4,800 units/minute leaves headroom below Google's 6,000-unit limit
        // for another connected process and for operations using a different client.
        self.next = Some(slot + Duration::from_micros(cost * 12_500));
        Ok(slot.duration_since(now))
    }
    pub(crate) fn check_after_wait(&self) -> Result<()> {
        self.check(Instant::now())
    }
    pub(crate) fn cool_down(&mut self, seconds: u64) {
        let until = Instant::now() + Duration::from_secs(seconds.clamp(1, 3600));
        self.cooldown = Some(self.cooldown.unwrap_or(until).max(until));
        self.next = Some(self.next.unwrap_or(until).max(until));
    }
}
pub(crate) fn cost(method: &reqwest::Method, url: &url::Url) -> u64 {
    let path = url.path();
    if path.contains("/attachments/") {
        return 20;
    }
    if method == reqwest::Method::GET && (path.contains("/messages/") || path.contains("/threads/"))
    {
        let full = url
            .query_pairs()
            .any(|(k, v)| k == "format" && matches!(v.as_ref(), "full" | "raw" | "html"));
        return if full {
            if path.contains("/threads/") { 120 } else { 60 }
        } else if path.contains("/threads/") {
            40
        } else {
            20
        };
    }
    if path.contains("/history") {
        return 2;
    }
    if path.ends_with("/send") || path.ends_with("/insert") || path.ends_with("/import") {
        return 100;
    }
    if path.ends_with("/batchModify") {
        return 50;
    }
    if path.contains("/settings/") || path.contains("/labels") || path.ends_with("/profile") {
        return 1;
    }
    if path.contains("/threads/") {
        return 10;
    }
    5
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_message_cost_and_shared_clients_preserve_budget() {
        assert_eq!(
            cost(
                &reqwest::Method::GET,
                &url::Url::parse(
                    "https://gmail.googleapis.com/gmail/v1/users/me/messages/abc?format=full"
                )
                .unwrap()
            ),
            60
        );
        assert_eq!(
            cost(
                &reqwest::Method::GET,
                &url::Url::parse(
                    "https://gmail.googleapis.com/gmail/v1/users/me/threads/abc?format=minimal"
                )
                .unwrap()
            ),
            40
        );
        let a = Budget::shared("test-client", "test-credential");
        let b = Budget::shared("test-client", "test-credential");
        assert!(Arc::ptr_eq(&a, &b));
        assert!(a.lock().unwrap().reserve(60).unwrap() < Duration::from_millis(20));
        assert!(b.lock().unwrap().reserve(60).unwrap() > Duration::from_millis(700));
        a.lock().unwrap().cool_down(60);
        assert!(matches!(
            b.lock().unwrap().reserve(5),
            Err(CloudError::RateLimited(_))
        ));
    }
}
