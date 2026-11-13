//! Bounded local recipient completion, excluding received Bcc and message bodies.
use crate::{MailError, MailStore, RecipientHeaders, Result, parse_outgoing_recipients};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::HashMap;
const RECENT_MESSAGES: i64 = 5_000;
const MAX_MATCHED_RECORDS: usize = 512;
const MAX_HEADER_BYTES: usize = 16_384;
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecipientSuggestion {
    pub email: String,
    pub display_name: String,
    /// Strictly parsed/escaped mailbox suitable for inserting as one recipient.
    pub formatted: String,
}
struct Candidate {
    suggestion: RecipientSuggestion,
    rank: u8,
    last_seen: i64,
    occurrences: usize,
}
impl MailStore {
    /// Recent correspondents in the selected sending account; no provider I/O.
    /// Reads at most 5,000 indexed metadata rows, parses at most 512 matching
    /// records. This deliberately does not claim full address-book coverage.
    pub fn suggest_recipients(
        &self,
        account_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecipientSuggestion>> {
        if query.len() > 128 || query.chars().any(char::is_control) {
            return Err(MailError::InvalidAction(
                "Recipient query must be at most 128 bytes and contain no control characters"
                    .into(),
            ));
        }
        let query = query.trim().to_lowercase();
        let db = self.lock()?;
        if db
            .query_row("SELECT 1 FROM accounts WHERE id=?1", [account_id], |_| {
                Ok(())
            })
            .optional()?
            .is_none()
        {
            return Err(MailError::UnknownAccount(account_id.into()));
        }
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = db.prepare("SELECT sender,to_json,recipient_headers_json,(is_draft AND id LIKE 'local:%'),received_at FROM messages WHERE account_id=?1 ORDER BY received_at DESC,id LIMIT ?2")?;
        let mut rows = statement.query(params![account_id, RECENT_MESSAGES])?;
        let mut candidates: HashMap<String, Candidate> = HashMap::new();
        let mut matched_records = 0;
        while let Some(row) = rows.next()? {
            let sender: String = row.get(0)?;
            let to_json: String = row.get(1)?;
            let headers_json: Option<String> = row.get(2)?;
            let is_draft: bool = row.get(3)?;
            let received_at: i64 = row.get(4)?;
            let mut addresses = vec![sender];
            if to_json.len() <= MAX_HEADER_BYTES {
                addresses.extend(serde_json::from_str::<Vec<String>>(&to_json).unwrap_or_default());
            }
            if let Some(raw) = headers_json.filter(|raw| raw.len() <= MAX_HEADER_BYTES) {
                if let Ok(headers) = serde_json::from_str::<RecipientHeaders>(&raw) {
                    addresses.extend(headers.cc);
                    // Only explicitly saved local draft Bcc is eligible.
                    if is_draft {
                        addresses.extend(headers.bcc);
                    }
                }
            }
            let matching: Vec<_> = addresses
                .into_iter()
                .filter(|raw| raw.len() <= MAX_HEADER_BYTES && raw.to_lowercase().contains(&query))
                .collect();
            if matching.is_empty() {
                continue;
            }
            matched_records += 1;
            for raw in matching {
                let Ok(formatted) = parse_outgoing_recipients(&raw) else {
                    continue;
                };
                for formatted in formatted {
                    let Ok(parsed) = mailparse::addrparse(&formatted) else {
                        continue;
                    };
                    let Some(mailparse::MailAddr::Single(single)) = parsed.first() else {
                        continue;
                    };
                    let email = single.addr.to_lowercase();
                    let display_name = single.display_name.clone().unwrap_or_default();
                    let name = display_name.to_lowercase();
                    let rank = if email == query {
                        0
                    } else if email.starts_with(&query) {
                        1
                    } else if name.starts_with(&query)
                        || name.split_whitespace().any(|part| part.starts_with(&query))
                    {
                        2
                    } else if email.contains(&query) || name.contains(&query) {
                        3
                    } else {
                        continue;
                    };
                    let suggestion = RecipientSuggestion {
                        email: email.clone(),
                        display_name,
                        formatted,
                    };
                    candidates
                        .entry(email)
                        .and_modify(|candidate| {
                            candidate.occurrences += 1;
                            candidate.last_seen = candidate.last_seen.max(received_at);
                            candidate.rank = candidate.rank.min(rank);
                            if candidate.suggestion.display_name.is_empty()
                                && !suggestion.display_name.is_empty()
                            {
                                candidate.suggestion = suggestion.clone();
                            }
                        })
                        .or_insert(Candidate {
                            suggestion,
                            rank,
                            last_seen: received_at,
                            occurrences: 1,
                        });
                }
            }
            if matched_records >= MAX_MATCHED_RECORDS {
                break;
            }
        }
        let mut results: Vec<_> = candidates.into_values().collect();
        results.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then_with(|| b.last_seen.cmp(&a.last_seen))
                .then_with(|| b.occurrences.cmp(&a.occurrences))
                .then_with(|| a.suggestion.email.cmp(&b.suggestion.email))
        });
        Ok(results
            .into_iter()
            .take(limit.min(20))
            .map(|candidate| candidate.suggestion)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn damaged_json_and_oversized_headers_do_not_break_completion() {
        let store = MailStore::open_in_memory().unwrap();
        store.seed_demo().unwrap();
        store.lock().unwrap().execute("UPDATE messages SET sender='Valid <valid@example.org>',to_json='broken-json',recipient_headers_json='broken-json' WHERE id='demo:1'",[]).unwrap();
        assert_eq!(
            store
                .suggest_recipients("demo:personal", "valid", 6)
                .unwrap()
                .len(),
            1
        );
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE messages SET sender=?1 WHERE id='demo:1'",
                [format!(
                    "{}<valid@example.org>",
                    "x".repeat(MAX_HEADER_BYTES)
                )],
            )
            .unwrap();
        assert!(
            store
                .suggest_recipients("demo:personal", "valid", 6)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn recent_lookup_uses_account_time_index_without_sorting_entire_mailbox() {
        let store = MailStore::open_in_memory().unwrap();
        let db = store.lock().unwrap();
        let mut q = db.prepare("EXPLAIN QUERY PLAN SELECT sender,to_json,recipient_headers_json,(is_draft AND id LIKE 'local:%'),received_at FROM messages WHERE account_id=?1 ORDER BY received_at DESC,id LIMIT 5000").unwrap();
        let plan = q
            .query_map(["account"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
            .join(" ");
        assert!(
            plan.contains("messages_account_recent_recipients"),
            "{plan}"
        );
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }
}
