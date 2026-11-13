//! Account-scoped cache reads for capability-limited local clients.
use crate::{MailError, MailStore, Result, ThreadSummary};
use rusqlite::OptionalExtension;
use std::collections::BTreeSet;

impl MailStore {
    /// Same structured local-search semantics as `search`, with account filtering before
    /// LIMIT. An empty account set means no results, never unrestricted access.
    pub fn search_for_accounts(
        &self,
        query: &str,
        limit: u32,
        account_ids: &[String],
    ) -> Result<Vec<ThreadSummary>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        self.search_scoped(
            query,
            limit,
            0,
            &crate::SearchScope {
                account_ids: Some(account_ids.to_vec()),
                ..Default::default()
            },
        )
    }

    /// Inspect only the recorded batch and thread ownership, without hydrating
    /// message bodies. Mutation still rechecks undo validity independently.
    pub fn triage_undo_account_ids(&self, operation_id: &str) -> Result<Option<Vec<String>>> {
        let db = self.lock()?;
        let request: Option<String> = db
            .query_row(
                "SELECT request_json FROM triage_batches WHERE operation_id=?1",
                [operation_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(request) = request else {
            return Ok(None);
        };
        let (_, ids): (String, Vec<String>) = serde_json::from_str(&request)?;
        if ids.is_empty() {
            return Err(MailError::InvalidAction(
                "Undo batch has no account ownership".into(),
            ));
        }
        let mut accounts = BTreeSet::new();
        let mut statement = db.prepare("SELECT account_id FROM threads WHERE id=?1")?;
        for id in ids {
            let account: Option<String> = statement.query_row([id], |r| r.get(0)).optional()?;
            let Some(account) = account else {
                return Err(MailError::InvalidAction(
                    "Undo batch account ownership is unavailable".into(),
                ));
            };
            accounts.insert(account);
        }
        Ok(Some(accounts.into_iter().collect()))
    }
}

#[cfg(test)]
mod tests {
    use crate::{Account, MailStore, Provider};
    #[test]
    fn scoped_fts_filters_before_limit_and_treats_sql_as_literal() {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        store
            .upsert_account(&Account {
                id: "other".into(),
                provider: Provider::Gmail,
                email: "x@example.com".into(),
                display_name: "Other".into(),
            })
            .unwrap();
        let mut message = store.get_message("demo:1").unwrap().unwrap();
        // More than both the global cap and adaptive FTS threshold: the allowed
        // older match must survive either query plan.
        for i in 0..1005 {
            message.id = format!("foreign-{i}");
            message.thread_id = message.id.clone();
            message.account_id = "other".into();
            message.received_at += 1;
            store.upsert_message(&message).unwrap();
        }
        assert_eq!(store.search("Friday", 500).unwrap().len(), 500);
        let own = store
            .search_for_accounts("Friday", 20, &["demo:personal".into()])
            .unwrap();
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].id, "demo:1");
        assert!(
            store
                .search_for_accounts("Friday", 20, &[])
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .search_for_accounts("Friday", 20, &["other') OR 1=1 --".into()])
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .search_for_accounts("\" OR *", 20, &["demo:personal".into()])
                .is_err()
        );
    }
    #[test]
    fn undo_ownership_is_recorded_for_old_and_mixed_account_batches() {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        store
            .batch_triage("older", "archive", &["demo:1".into()])
            .unwrap();
        store
            .batch_triage("latest", "archive", &["demo:2".into()])
            .unwrap();
        assert_eq!(
            store.triage_undo_account_ids("older").unwrap(),
            Some(vec!["demo:personal".into()])
        );
        assert!(store.triage_undo_account_ids("missing").unwrap().is_none());
    }
}
