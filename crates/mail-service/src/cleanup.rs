//! Resumable sender cleanup: one quota-paced provider request per explicit step.
use super::*;
use mail_core::CleanupLease;
enum StepResult {
    Page(Vec<String>, Option<String>),
    Applied,
}
impl MailService {
    pub(super) fn create_sender_cleanup(
        &self,
        account_id: &str,
        sender: &str,
    ) -> Result<Value, String> {
        let account = self.account(account_id)?;
        if account.provider != Provider::Gmail {
            return Err("Provider-wide sender cleanup currently supports Gmail".into());
        }
        self.value(self.store.create_sender_cleanup(account_id, sender))
    }
    pub(super) fn step_sender_cleanup(&self, job_id: &str) -> Result<Value, String> {
        self.cleanup_step_with(job_id, |lease| {
            let Some(_account_lease) =
                SyncLease::acquire(&self.config_path, &lease.job.account_id)?
            else {
                return Err("Account sync is busy; retrying after 5 seconds".into());
            };
            let account = self.account(&lease.job.account_id)?;
            let saved = self.saved_config(&lease.job.account_id)?;
            if account.provider != Provider::Gmail {
                return Err("Sender cleanup requires Gmail".into());
            }
            if lease.job.phase == "apply"
                && self
                    .store
                    .pending_action_count_for_account(&lease.job.account_id)
                    .map_err(|e| e.to_string())?
                    > 0
            {
                let report = self.flush_actions(&account, &saved)?;
                if report.pending > 0 {
                    return Err(format!(
                        "Waiting for earlier mail changes; retrying after {} seconds",
                        report.retry_after_ms.max(5000) / 1000
                    ));
                }
            }
            let client = self.cloud_client(&saved)?;
            let result = if lease.job.phase == "scan" {
                client
                    .gmail_sender_cleanup_page(
                        &account,
                        &lease.job.sender,
                        lease.job.page_token.as_deref(),
                    )
                    .map(|page| StepResult::Page(page.message_ids, page.next_page_token))
                    .map_err(|e| e.to_string())
            } else {
                client
                    .gmail_apply_sender_cleanup(&account, &lease.message_ids)
                    .map(|_| StepResult::Applied)
                    .map_err(|e| e.to_string())
            };
            let _ = self.save_cloud_tokens(&saved, &client);
            result
        })
    }
    fn cleanup_step_with(
        &self,
        job_id: &str,
        execute: impl FnOnce(&CleanupLease) -> Result<StepResult, String>,
    ) -> Result<Value, String> {
        let Some(lease) = self
            .store
            .claim_cleanup_step(job_id)
            .map_err(|e| e.to_string())?
        else {
            return self.value(self.store.sender_cleanup(job_id));
        };
        let result = match execute(&lease) {
            Ok(StepResult::Page(ids, next)) => {
                self.store
                    .record_cleanup_page(&lease, &ids, next.as_deref())
            }
            Ok(StepResult::Applied) => self.store.finish_cleanup_batch(&lease),
            Err(error) => {
                return self.value(self.store.cleanup_step_failed(
                    &lease,
                    &error,
                    cleanup_retry_ms(&error),
                ));
            }
        };
        // Validation/storage errors release the lease into visible retry state too.
        match result {
            Ok(job) => Ok(json!(job)),
            Err(error) => self.value(self.store.cleanup_step_failed(
                &lease,
                &error.to_string(),
                60_000,
            )),
        }
    }
}
fn cleanup_retry_ms(error: &str) -> i64 {
    // Provider error carries its safe cooldown; generic failures retry conservatively.
    let seconds = error
        .split("retrying after ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse::<i64>().ok());
    seconds
        .map(|n| n.saturating_mul(1000))
        .unwrap_or(60_000)
        .clamp(1000, 3_600_000)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scans_require_confirmation_and_remote_failures_keep_snapshot() {
        let root = std::env::temp_dir().join(format!("email-cleanup-service-{}", Uuid::new_v4()));
        let service = MailService::open(&root.join("mail.sqlite")).unwrap();
        service.store.seed_demo().unwrap();
        let job = service
            .store
            .create_sender_cleanup("demo:personal", "news@example.com")
            .unwrap();
        let preview = service
            .cleanup_step_with(&job.id, |_| {
                Ok(StepResult::Page(
                    vec!["demo:personal:gmail-message:abc".into()],
                    None,
                ))
            })
            .unwrap();
        assert_eq!(preview["state"], "ready");
        let paused = service
            .cleanup_step_with(&job.id, |_| panic!("must not mutate without confirmation"))
            .unwrap();
        assert_eq!(paused["completed"], 0);
        service.store.confirm_sender_cleanup(&job.id).unwrap();
        let failed = service
            .cleanup_step_with(&job.id, |_| {
                Err("Gmail rate limit; retrying after 120 seconds".into())
            })
            .unwrap();
        assert_eq!(failed["matched"], 1);
        assert_eq!(failed["completed"], 0);
        assert!(failed["retry_at_ms"].as_i64().unwrap() > now_ms() + 110_000);
        service
            .cleanup_step_with(&job.id, |_| panic!("must respect retry cooldown"))
            .unwrap();
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(test)]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
