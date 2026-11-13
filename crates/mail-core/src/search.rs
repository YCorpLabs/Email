//! Bounded, cache-only structured search shared by every client.
use crate::{MailError, MailStore, Result, ThreadSummary, now_ms, split_filter, thread_row};
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    /// None = all accounts; Some(empty) = no accounts. Never broadens an allowlist.
    pub account_ids: Option<Vec<String>>,
    pub is_inbox: Option<bool>,
    pub unread_only: bool,
    pub starred_only: bool,
    pub snoozed_only: bool,
    pub followups_only: bool,
    pub label: Option<String>,
    pub folder: Option<String>,
    pub split_id: Option<String>,
}
#[derive(Debug, PartialEq)]
struct Token {
    value: String,
    literal: bool,
}
fn invalid(message: impl Into<String>) -> MailError {
    MailError::InvalidAction(message.into())
}
fn tokens(query: &str) -> Result<Vec<Token>> {
    if query.len() > 4096 {
        return Err(invalid("Search is limited to 4096 bytes"));
    }
    let mut result = vec![];
    let mut value = String::new();
    let mut quote = false;
    let mut escape = false;
    let mut literal = false;
    let mut active = false;
    for c in query.chars() {
        if c.is_control() && !c.is_whitespace() {
            return Err(invalid("Search contains an unsupported control character"));
        }
        if escape {
            value.push(c);
            escape = false;
            continue;
        }
        if quote && c == '\\' {
            escape = true;
            continue;
        }
        if c == '"' {
            if !active {
                literal = true;
            }
            active = true;
            quote = !quote;
            continue;
        }
        if c.is_whitespace() && !quote {
            if active {
                result.push(Token {
                    value: std::mem::take(&mut value),
                    literal,
                });
                active = false;
                literal = false;
            }
        } else {
            active = true;
            value.push(c);
        }
    }
    if quote || escape {
        return Err(invalid("Close the quoted search value with a double quote"));
    }
    if active {
        result.push(Token { value, literal });
    }
    if result.len() > 64 {
        return Err(invalid("Use at most 64 search terms"));
    }
    Ok(result)
}
fn bind(values: &mut Vec<Value>, value: impl Into<Value>) -> String {
    values.push(value.into());
    format!("?{}", values.len())
}
fn contains(column: &str, value: &str, values: &mut Vec<Value>) -> String {
    let param = bind(values, value.to_owned());
    format!("instr(lower({column}),lower({param}))>0")
}
fn starred() -> &'static str {
    "(EXISTS(SELECT 1 FROM pinned_threads p WHERE p.thread_id=t.id) OR EXISTS(SELECT 1 FROM thread_labels l WHERE l.thread_id=t.id AND upper(l.label)='STARRED'))"
}
fn label(value: &str, values: &mut Vec<Value>) -> String {
    let p = bind(values, value.to_owned());
    format!(
        "EXISTS(SELECT 1 FROM thread_labels l WHERE l.thread_id=t.id AND lower(l.label)=lower({p}))"
    )
}
fn folder(value: &str) -> Result<String> {
    Ok(match value.to_ascii_lowercase().as_str() {
        "inbox" => "t.is_inbox=1".into(),
        "archive" => "(t.is_inbox=0 AND lower(t.folder)='archive')".into(),
        "spam" | "junk" => "lower(t.folder) IN ('spam','junk')".into(),
        "trash" => "lower(t.folder)='trash'".into(),
        "sent" => "EXISTS(SELECT 1 FROM messages sm WHERE sm.thread_id=t.id AND sm.is_sent=1 AND sm.is_draft=0)".into(),
        _ => return Err(invalid("Use in:inbox, in:archive, in:sent, in:spam, or in:trash")),
    })
}
/// UTC midnight, with strict calendar validation rather than SQLite normalization.
fn date_ms(value: &str) -> Result<i64> {
    let bad = || invalid("Use a valid date in YYYY-MM-DD form after before: or after:");
    let b = value.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, c)| i != 4 && i != 7 && !c.is_ascii_digit())
    {
        return Err(bad());
    }
    let year: i64 = value[..4].parse().map_err(|_| bad())?;
    let month: i64 = value[5..7].parse().map_err(|_| bad())?;
    let day: i64 = value[8..].parse().map_err(|_| bad())?;
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
        return Err(bad());
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=max).contains(&day) {
        return Err(bad());
    }
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yo = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    Ok((era * 146097 + yo * 365 + yo / 4 - yo / 100 + doy - 719468) * 86_400_000)
}
impl MailStore {
    /// All predicates are applied before result LIMIT/OFFSET. Empty query lists
    /// the supplied scope; no request reads the network or hydrates message bodies.
    pub fn search_scoped(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
        scope: &SearchScope,
    ) -> Result<Vec<ThreadSummary>> {
        self.search_executing(query, limit, offset, scope, Execution::Auto)
    }

    pub(crate) fn search_executing(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
        scope: &SearchScope,
        execution: Execution,
    ) -> Result<Vec<ThreadSummary>> {
        self.with_filters(query, scope, |db, filters| {
            filters
                .run(db, limit, offset, execution)
                .map(|(rows, _)| rows)
        })
    }

    /// Parses the search and builds its predicates, then hands them to `run`
    /// under the store lock.
    fn with_filters<T>(
        &self,
        query: &str,
        scope: &SearchScope,
        run: impl FnOnce(&mut Connection, Filters) -> Result<T>,
    ) -> Result<T> {
        let terms = tokens(query)?;
        if scope.followups_only {
            self.list_followups(None)?;
        }
        let mut values = vec![];
        let mut predicates = vec!["1".to_string()];
        let mut message_predicates = vec![];
        let mut fts_query = None;
        let mut plain = vec![];
        for token in terms {
            if token.value.is_empty() {
                return Err(invalid("Search terms cannot be empty"));
            }
            let pair = if token.literal {
                None
            } else {
                token.value.split_once(':')
            };
            let known = pair.filter(|(field, _)| {
                matches!(
                    field.to_ascii_lowercase().as_str(),
                    "from"
                        | "to"
                        | "cc"
                        | "subject"
                        | "is"
                        | "in"
                        | "label"
                        | "has"
                        | "before"
                        | "after"
                )
            });
            let Some((field, value)) = known else {
                plain.push(token.value);
                continue;
            };
            if value.is_empty() {
                return Err(invalid(format!("{field}: needs a value")));
            }
            let predicate = match field.to_ascii_lowercase().as_str() {
                "from" => contains("m.sender", value, &mut values),
                "subject" => contains("m.subject", value, &mut values),
                "to" => {
                    let p = contains("j.value", value, &mut values);
                    format!("EXISTS(SELECT 1 FROM json_each(m.to_json) j WHERE {p})")
                }
                "cc" => {
                    let p = contains("j.value", value, &mut values);
                    format!(
                        "EXISTS(SELECT 1 FROM json_each(coalesce(json_extract(m.recipient_headers_json,'$.cc'),'[]')) j WHERE {p})"
                    )
                }
                "label" => label(value, &mut values),
                "in" => folder(value)?,
                "is" => match value.to_ascii_lowercase().as_str() {
                    "unread" => "t.unread_count>0".into(),
                    "read" => "t.unread_count=0".into(),
                    "starred" => starred().into(),
                    _ => return Err(invalid("Use is:unread, is:read, or is:starred")),
                },
                "has" => {
                    if value.eq_ignore_ascii_case("attachment") {
                        "(m.has_attachments=1 OR EXISTS(SELECT 1 FROM draft_attachments d WHERE d.message_id=m.id))".into()
                    } else {
                        return Err(invalid("Use has:attachment"));
                    }
                }
                "before" | "after" => {
                    let p = bind(&mut values, date_ms(value)?);
                    format!(
                        "m.received_at{}{p}",
                        if field.eq_ignore_ascii_case("before") {
                            "<"
                        } else {
                            ">="
                        }
                    )
                }
                _ => unreachable!(),
            };
            if matches!(
                field.to_ascii_lowercase().as_str(),
                "from" | "to" | "cc" | "subject" | "has" | "before" | "after"
            ) {
                message_predicates.push(predicate);
            } else {
                predicates.push(predicate);
            }
        }
        if !plain.is_empty() {
            let fts = plain
                .iter()
                .map(|v| format!("\"{}\"", v.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" AND ");
            let p = bind(&mut values, fts.clone());
            fts_query = Some((fts, p));
        }
        if let Some(accounts) = &scope.account_ids {
            if accounts.is_empty() {
                predicates.push("0".into());
            } else {
                let params = accounts
                    .iter()
                    .map(|id| bind(&mut values, id.clone()))
                    .collect::<Vec<_>>()
                    .join(",");
                predicates.push(format!("t.account_id IN ({params})"));
            }
        }
        if let Some(inbox) = scope.is_inbox {
            let p = bind(&mut values, i64::from(inbox));
            predicates.push(format!("t.is_inbox={p}"));
        }
        if scope.unread_only {
            predicates.push("t.unread_count>0".into());
        }
        if scope.starred_only {
            predicates.push(starred().into());
        }
        if let Some(name) = &scope.label {
            predicates.push(label(name, &mut values));
        }
        if let Some(name) = &scope.folder {
            predicates.push(folder(name)?);
        }
        if scope.followups_only {
            predicates.push("EXISTS(SELECT 1 FROM followups f WHERE f.thread_id=t.id AND f.status IN ('waiting','due'))".into());
        }
        let time = bind(&mut values, now_ms());
        if scope.snoozed_only {
            predicates.push(format!(
                "EXISTS(SELECT 1 FROM snoozes s WHERE s.thread_id=t.id AND s.wake_at_ms>{time})"
            ));
        } else if scope.is_inbox == Some(true)
            || scope.folder.as_deref().is_some_and(|f| {
                f.eq_ignore_ascii_case("inbox") || f.eq_ignore_ascii_case("archive")
            })
        {
            predicates.push(format!(
                "NOT EXISTS(SELECT 1 FROM snoozes s WHERE s.thread_id=t.id AND s.wake_at_ms>{time})"
            ));
        }
        let mut db = self.lock()?;
        if let Some(id) = &scope.split_id {
            let rule: Option<String> = db
                .query_row("SELECT query FROM split_inboxes WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .optional()?;
            let rule = rule.ok_or_else(|| MailError::NotFound(id.clone()))?;
            predicates.push(split_filter(&rule, &mut values)?);
            predicates.push("t.is_inbox=1".into());
            predicates.push(format!(
                "NOT EXISTS(SELECT 1 FROM snoozes s WHERE s.thread_id=t.id AND s.wake_at_ms>{time})"
            ));
        }
        let filters = Filters {
            values,
            predicates,
            message_predicates,
            fts: fts_query,
            followups_order: scope.followups_only,
        };
        run(&mut db, filters)
    }
}

/// How a search with free-text terms reads the full-text index. Every plan
/// returns the same rows in the same order; only the cost differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextPlan {
    /// Start from the full-text matches, keep the conversations in scope and
    /// sort them. Cost grows with the number of matching messages.
    Matches,
    /// Read the matching message IDs once, then walk the scope newest first and
    /// stop after one page. Cost grows with how far the walk must go.
    Walk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Execution {
    Auto,
    #[cfg(test)]
    Plan(TextPlan),
    /// The query plans before November 2026, kept to prove results did not change.
    #[cfg(test)]
    Legacy,
}

/// Which way an automatic search went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// No free-text terms: the scope is walked with its own predicates.
    Scan,
    Matches,
    Walk,
    /// The walk went much further than expected (the scope holds few of the
    /// matches), so the matches were looked up instead.
    WalkThenMatches,
    #[cfg(test)]
    Forced,
}

// Costs in SQLite virtual machine steps, measured on the 100,000-conversation
// demo store (docs/benchmarks/search-100k.md). A step in the walk takes about
// 16 ns there, a step of a match lookup about 45 ns; LOOKUP_STEPS is in walk
// steps, so one looked-up match costs about four walked conversations.
/// Walking one conversation of the scope and checking its messages.
const WALK_STEPS: u64 = 25;
/// Passing over one conversation that a cheap scope check rejects.
const REJECT_STEPS: u64 = 10;
/// Adding one match to the list the walk checks against.
const LIST_STEPS: u64 = 5;
/// Looking up one match: message, conversation, scope and sort.
const LOOKUP_STEPS: u64 = 100;
/// The walk may run this many times its expected length before it is judged.
const EXPLORE_FACTOR: u64 = 4;
/// How often SQLite reports progress during a walk.
const PROGRESS_STEPS: u64 = 1_000;
/// Counting stops here (about 1.5 ms); above it a walk always runs to the end.
const MAX_COUNTED: u64 = 32_768;

const COLUMNS: &str = "t.id,t.account_id,t.subject,t.sender,t.snippet,t.last_message_at,t.unread_count,t.is_inbox,t.folder";

/// The parsed search: thread predicates on `t`, message predicates on `m`
/// (all must hold for the same message as the free-text match), and the FTS5
/// expression with its bound parameter.
#[derive(Clone)]
struct Filters {
    values: Vec<Value>,
    predicates: Vec<String>,
    message_predicates: Vec<String>,
    fts: Option<(String, String)>,
    followups_order: bool,
}

/// A statement and its parameters.
type Built = (String, Vec<Value>);

impl Filters {
    /// Rare terms start from their matches. Common terms walk the scope newest
    /// first; if that walk runs well past its expected length and the rest of
    /// it would cost more than a lookup, the scope holds few matches and they
    /// are looked up instead.
    fn run(
        self,
        db: &mut Connection,
        limit: u32,
        offset: u32,
        execution: Execution,
    ) -> Result<(Vec<ThreadSummary>, Route)> {
        let Some((fts, _)) = self.fts.clone() else {
            return Ok((read(db, self.sql(None, limit, offset))?, Route::Scan));
        };
        match execution {
            Execution::Auto => {}
            #[cfg(test)]
            Execution::Plan(plan) => {
                return Ok((
                    read(db, self.sql(Some(plan), limit, offset))?,
                    Route::Forced,
                ));
            }
            #[cfg(test)]
            Execution::Legacy => {
                let built = self.legacy_sql(db, limit, offset)?;
                return Ok((read(db, built)?, Route::Forced));
            }
        }
        let window = u64::from(offset) + u64::from(limit.min(500));
        // max(rowid) is a single B-tree seek and never below the row count.
        let last_rowid = |table: &str| -> Result<u64> {
            let n: i64 = db.query_row(
                &format!("SELECT coalesce(max(rowid),0) FROM {table}"),
                [],
                |r| r.get(0),
            )?;
            Ok(n.max(0) as u64)
        };
        let (messages, threads) = (last_rowid("messages")?, last_rowid("threads")?);
        // Beyond this many matches, looking them up costs more than walking
        // every conversation, so the walk always wins.
        let whole_walk = threads.saturating_mul(WALK_STEPS);
        let cap = (whole_walk / LOOKUP_STEPS).min(MAX_COUNTED);
        let bound = if self.followups_order {
            // Follow-ups are sorted by due date, so a walk reads every
            // conversation, and the follow-up check rejects most of them cheaply.
            threads.saturating_mul(REJECT_STEPS) / LOOKUP_STEPS
        } else {
            lookup_bound(window, messages)
        };
        let matches = count_matches(db, &fts, bound.max(cap) + 1)?;
        if matches <= bound {
            let rows = read(db, self.sql(Some(TextPlan::Matches), limit, offset))?;
            return Ok((rows, Route::Matches));
        }
        let walk = self.clone().sql(Some(TextPlan::Walk), limit, offset);
        // Above the cap a lookup cannot win; follow-ups cannot stop early.
        if matches > cap || self.followups_order {
            return Ok((read(db, walk)?, Route::Walk));
        }
        let budget = WalkBudget {
            window,
            offset: u64::from(offset),
            explore: EXPLORE_FACTOR.saturating_mul(expected_walk_steps(window, messages, matches)),
            whole_walk,
            lookup: matches.saturating_mul(LOOKUP_STEPS),
        };
        match read_walk(db, walk, budget)? {
            Some(rows) => Ok((rows, Route::Walk)),
            None => {
                let rows = read(db, self.sql(Some(TextPlan::Matches), limit, offset))?;
                Ok((rows, Route::WalkThenMatches))
            }
        }
    }

    fn sql(mut self, plan: Option<TextPlan>, limit: u32, offset: u32) -> Built {
        let limit = bind(&mut self.values, i64::from(limit.min(500)));
        let offset = bind(&mut self.values, i64::from(offset));
        let ordering = if self.followups_order {
            "(SELECT f.due_at_ms FROM followups f WHERE f.thread_id=t.id) ASC,t.id"
        } else {
            "t.last_message_at DESC,t.id"
        };
        let mut predicates = self.predicates;
        let mut message_predicates = self.message_predicates;
        let sql = match (plan, &self.fts) {
            (Some(TextPlan::Matches), Some((_, param))) => {
                // CROSS JOIN fixes the join order: the match set is the outer
                // loop, so SQLite cannot prefer a scope index and probe each
                // conversation instead.
                let message_filter = message_predicates
                    .iter()
                    .map(|p| format!(" AND {p}"))
                    .collect::<String>();
                format!(
                    "WITH hits(id) AS MATERIALIZED (SELECT DISTINCT m.thread_id FROM message_search CROSS JOIN messages m ON m.rowid=message_search.rowid WHERE message_search MATCH {param}{message_filter}) \
                     SELECT {COLUMNS} FROM hits CROSS JOIN threads t ON t.id=hits.id WHERE {} ORDER BY {ordering} LIMIT {limit} OFFSET {offset}",
                    predicates.join(" AND ")
                )
            }
            (Some(TextPlan::Walk), Some((_, param))) => {
                // The IN list is built once per statement. The unary plus
                // keeps SQLite from driving the message lookup from that list.
                message_predicates.push(format!(
                    "+m.rowid IN (SELECT rowid FROM message_search WHERE message_search MATCH {param})"
                ));
                predicates.push(format!(
                    "EXISTS(SELECT 1 FROM messages m WHERE m.thread_id=t.id AND {})",
                    message_predicates.join(" AND ")
                ));
                format!(
                    "SELECT {COLUMNS} FROM threads t WHERE {} ORDER BY {ordering} LIMIT {limit} OFFSET {offset}",
                    predicates.join(" AND ")
                )
            }
            _ => {
                if !message_predicates.is_empty() {
                    predicates.push(format!(
                        "EXISTS(SELECT 1 FROM messages m WHERE m.thread_id=t.id AND {})",
                        message_predicates.join(" AND ")
                    ));
                }
                format!(
                    "SELECT {COLUMNS} FROM threads t WHERE {} ORDER BY {ordering} LIMIT {limit} OFFSET {offset}",
                    predicates.join(" AND ")
                )
            }
        };
        (sql, self.values)
    }

    /// The previous assembly, unchanged, for comparison tests only.
    #[cfg(test)]
    fn legacy_sql(self, db: &Connection, limit: u32, offset: u32) -> Result<Built> {
        let Filters {
            mut values,
            mut predicates,
            mut message_predicates,
            fts: fts_query,
            followups_order,
        } = self;
        let mut selective_fts = false;
        if let Some((fts, param)) = &fts_query {
            let count:i64=db.query_row("SELECT count(*) FROM (SELECT rowid FROM message_search WHERE message_search MATCH ?1 LIMIT 1001)",[fts],|r|r.get(0))?;
            selective_fts = count <= 1000;
            message_predicates.push(if selective_fts {format!("m.rowid IN (SELECT rowid FROM message_search WHERE message_search MATCH {param})")}
                else {format!("EXISTS(SELECT 1 FROM message_search WHERE rowid=m.rowid AND message_search MATCH {param})")});
        }
        let limit = bind(&mut values, i64::from(limit.min(500)));
        let offset = bind(&mut values, i64::from(offset));
        let ordering = if followups_order {
            "(SELECT f.due_at_ms FROM followups f WHERE f.thread_id=t.id) ASC,t.id"
        } else {
            "t.last_message_at DESC,t.id"
        };
        let sql = if selective_fts {
            predicates.extend(message_predicates);
            format!(
                "SELECT DISTINCT {COLUMNS} FROM messages m JOIN threads t ON t.id=m.thread_id WHERE {} ORDER BY {ordering} LIMIT {limit} OFFSET {offset}",
                predicates.join(" AND ")
            )
        } else {
            if !message_predicates.is_empty() {
                predicates.push(format!(
                    "EXISTS(SELECT 1 FROM messages m WHERE m.thread_id=t.id AND {})",
                    message_predicates.join(" AND ")
                ));
            }
            format!(
                "SELECT {COLUMNS} FROM threads t WHERE {} ORDER BY {ordering} LIMIT {limit} OFFSET {offset}",
                predicates.join(" AND ")
            )
        };
        Ok((sql, values))
    }
}

/// The most matches for which looking them up beats walking. A walk finds
/// `window` conversations after about `window * messages / matches` steps if
/// matches are spread through the mailbox, so lookups win while
/// `matches^2 <= window * messages * WALK_STEPS / LOOKUP_STEPS`.
fn lookup_bound(window: u64, messages: u64) -> u64 {
    ((window.max(1) as f64 * messages.max(1) as f64 * WALK_STEPS as f64 / LOOKUP_STEPS as f64)
        .sqrt()) as u64
}

/// Steps a walk should need to fill `window` if matches are spread evenly.
fn expected_walk_steps(window: u64, messages: u64, matches: u64) -> u64 {
    let walked = (window as u128 * messages.max(1) as u128 / matches.max(1) as u128) as u64;
    walked
        .saturating_mul(WALK_STEPS)
        .saturating_add(matches.saturating_mul(LIST_STEPS))
}

/// Matching messages, counted up to `cap` from row IDs only, never message text.
fn count_matches(db: &Connection, fts: &str, cap: u64) -> Result<u64> {
    let cap = i64::try_from(cap).unwrap_or(i64::MAX);
    let count: i64 = db.query_row(
        "SELECT count(*) FROM (SELECT rowid FROM message_search WHERE message_search MATCH ?1 LIMIT ?2)",
        rusqlite::params![fts, cap],
        |r| r.get(0),
    )?;
    Ok(count.max(0) as u64)
}

fn read(db: &Connection, (sql, values): Built) -> Result<Vec<ThreadSummary>> {
    let mut statement = db.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(values))?;
    let mut out = vec![];
    while let Some(row) = rows.next()? {
        out.push(thread_row(db, row)?);
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy)]
struct WalkBudget {
    window: u64,
    offset: u64,
    /// Steps before the walk is judged.
    explore: u64,
    /// A walk of every conversation, in steps, assuming cheap scope filters.
    whole_walk: u64,
    /// Looking up all matches, in walk steps.
    lookup: u64,
}

impl WalkBudget {
    /// Steps the rest of the walk should take: at the rate it found
    /// conversations so far, and never more than the rest of a whole walk.
    fn remaining(&self, steps: u64, found: u64) -> u64 {
        let rest_of_scope = self.whole_walk.saturating_sub(steps);
        if found == 0 {
            return rest_of_scope;
        }
        let at_rate = steps as u128 * self.window.saturating_sub(found) as u128 / found as u128;
        (at_rate.min(rest_of_scope as u128)) as u64
    }

    fn should_stop(&self, steps: u64, found: u64) -> bool {
        steps >= self.explore && self.remaining(steps, found) > self.lookup
    }
}

#[derive(Default)]
struct Progress {
    steps: AtomicU64,
    delivered: AtomicU64,
    /// Steps of other statements (labels, snooze, pin per row) are not counted.
    paused: AtomicBool,
    stopped: AtomicBool,
    /// Set when the walk is over, so a handler left behind never interrupts.
    finished: AtomicBool,
}

impl Progress {
    /// Called by SQLite every PROGRESS_STEPS steps; true interrupts the walk.
    fn tick(&self, budget: &WalkBudget) -> bool {
        if self.paused.load(Ordering::Relaxed) || self.finished.load(Ordering::Relaxed) {
            return false;
        }
        let steps = self.steps.fetch_add(PROGRESS_STEPS, Ordering::Relaxed) + PROGRESS_STEPS;
        // Offset rows are skipped inside SQLite; they count once a row arrives.
        let found = match self.delivered.load(Ordering::Relaxed) {
            0 => 0,
            delivered => budget.offset + delivered,
        };
        let stop = budget.should_stop(steps, found);
        if stop {
            self.stopped.store(true, Ordering::Relaxed);
        }
        stop
    }
}

struct FinishOnDrop(Arc<Progress>);

impl Drop for FinishOnDrop {
    fn drop(&mut self) {
        self.0.finished.store(true, Ordering::Relaxed);
    }
}

/// Runs a walk, stopping it once the rest would cost more than a lookup.
/// Returns None if it was stopped.
fn read_walk(
    db: &mut Connection,
    built: Built,
    budget: WalkBudget,
) -> Result<Option<Vec<ThreadSummary>>> {
    let progress = Arc::new(Progress::default());
    let handler = Arc::clone(&progress);
    db.progress_handler(PROGRESS_STEPS as i32, Some(move || handler.tick(&budget)))?;
    let finish = FinishOnDrop(Arc::clone(&progress));
    let result = (|| {
        let db: &Connection = db;
        let (sql, values) = built;
        let mut statement = db.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(values))?;
        let mut out = vec![];
        while let Some(row) = rows.next()? {
            progress.delivered.fetch_add(1, Ordering::Relaxed);
            progress.paused.store(true, Ordering::Relaxed);
            let summary = thread_row(db, row);
            progress.paused.store(false, Ordering::Relaxed);
            out.push(summary?);
        }
        Ok(out)
    })();
    drop(finish);
    db.progress_handler(0, None::<fn() -> bool>)?;
    match result {
        Ok(rows) => Ok(Some(rows)),
        Err(_) if progress.stopped.load(Ordering::Relaxed) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calendar_is_strict() {
        assert_eq!(date_ms("1970-01-01").unwrap(), 0);
        assert_eq!(date_ms("2000-01-01").unwrap(), 946684800000);
        assert!(date_ms("2025-02-29").is_err());
        assert!(date_ms("2024-02-29").is_ok());
        assert!(date_ms("2026-13-01").is_err());
    }
    #[test]
    fn lexer_preserves_phrases() {
        assert_eq!(
            tokens("from:\"Maya Chen\" \"design review\"").unwrap(),
            vec![
                Token {
                    value: "from:Maya Chen".into(),
                    literal: false
                },
                Token {
                    value: "design review".into(),
                    literal: true
                }
            ]
        );
        assert!(tokens("subject:\"oops").is_err());
    }
}

/// The plans must return exactly what the previous plans returned: same rows,
/// order and pages. These tests run every plan on the launch demo store with
/// its older search history and compare them with the previous assembly.
#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::demo::{DEMO_PERSONAL_ACCOUNT, DEMO_WORK_ACCOUNT};
    use std::time::Instant;

    /// The launch demo plus `history` older conversations, with some of that
    /// history archived, labelled, starred, pinned, snoozed or followed up so
    /// every scope has hits and misses.
    fn corpus(history: usize) -> MailStore {
        let store = MailStore::open_in_memory().unwrap();
        // Seeded at the real time so snoozes and follow-ups stay in the future.
        let now = now_ms();
        store.seed_launch_demo(now, 60).unwrap();
        store.seed_demo_search_corpus(history, now).unwrap();
        store
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TEMP VIEW bulk AS SELECT id, CAST(substr(id,11) AS INTEGER) AS n, subject FROM threads WHERE id LIKE 'demo:bulk:%';
                 UPDATE threads SET is_inbox=0, folder='Archive' WHERE id IN (SELECT id FROM bulk WHERE n % 5 = 1);
                 UPDATE threads SET is_inbox=0, folder='Spam' WHERE id IN (SELECT id FROM bulk WHERE n % 89 = 2);
                 INSERT OR IGNORE INTO thread_labels(thread_id,label) SELECT id,'Receipts' FROM bulk WHERE subject LIKE '%nvoice%' OR subject LIKE '%aktura%' OR subject LIKE '%eceipt%';
                 INSERT OR IGNORE INTO thread_labels(thread_id,label) SELECT id,'STARRED' FROM bulk WHERE n % 41 = 0;
                 INSERT OR IGNORE INTO pinned_threads(thread_id) SELECT id FROM bulk WHERE n % 97 = 0;
                 INSERT OR REPLACE INTO snoozes(thread_id,wake_at_ms) SELECT id, 4102444800000 FROM bulk WHERE n % 53 = 0;
                 DROP VIEW bulk;",
            )
            .unwrap();
        for n in (7..history).step_by(history.max(40) / 40) {
            // Same due time for pairs, so the id tie-break is exercised.
            let due = now + 86_400_000 * (1 + (n as i64 / 2) % 9);
            store
                .set_followup(&format!("demo:bulk:{n}"), due, false)
                .unwrap();
        }
        store
    }

    fn scopes(store: &MailStore) -> Vec<(String, SearchScope)> {
        let inbox = SearchScope {
            is_inbox: Some(true),
            ..Default::default()
        };
        let mut out = vec![
            ("inbox".to_string(), inbox.clone()),
            ("all mail".into(), SearchScope::default()),
            (
                "not inbox".into(),
                SearchScope {
                    is_inbox: Some(false),
                    ..Default::default()
                },
            ),
            (
                "work inbox".into(),
                SearchScope {
                    account_ids: Some(vec![DEMO_WORK_ACCOUNT.into()]),
                    ..inbox.clone()
                },
            ),
            (
                "personal, all mail".into(),
                SearchScope {
                    account_ids: Some(vec![DEMO_PERSONAL_ACCOUNT.into()]),
                    ..Default::default()
                },
            ),
            (
                "unread inbox".into(),
                SearchScope {
                    unread_only: true,
                    ..inbox.clone()
                },
            ),
            (
                "starred".into(),
                SearchScope {
                    starred_only: true,
                    ..Default::default()
                },
            ),
            (
                "snoozed".into(),
                SearchScope {
                    snoozed_only: true,
                    ..Default::default()
                },
            ),
            (
                "follow-ups".into(),
                SearchScope {
                    followups_only: true,
                    ..Default::default()
                },
            ),
            (
                "label Receipts".into(),
                SearchScope {
                    label: Some("Receipts".into()),
                    ..Default::default()
                },
            ),
            (
                "label work".into(),
                SearchScope {
                    label: Some("work".into()),
                    ..Default::default()
                },
            ),
            (
                "archive".into(),
                SearchScope {
                    folder: Some("archive".into()),
                    ..Default::default()
                },
            ),
            (
                "sent".into(),
                SearchScope {
                    folder: Some("sent".into()),
                    ..Default::default()
                },
            ),
        ];
        for split in store.list_splits().unwrap() {
            out.push((
                format!("split {}", split.name),
                SearchScope {
                    split_id: Some(split.id),
                    is_inbox: Some(true),
                    ..Default::default()
                },
            ));
        }
        out
    }

    const QUERIES: &[&str] = &[
        // Common, rare and absent terms, and what typing produces on the way.
        "invoice",
        "Invoice",
        "faktura",
        "Ingrid",
        "Brightwater",
        "Elena",
        "the",
        "Alex",
        "nonexistentterm",
        "i",
        "inv",
        "invoi",
        // Phrases and several terms.
        "\"launch plan\"",
        "launch plan",
        "\"plan launch\"",
        "invoice Cirrusbay",
        "Elena pricing",
        "\"Fossekraft Strøm\"",
        // Operators alone and with text.
        "from:ingrid",
        "from:ingrid review",
        "invoice from:cirrusbay",
        "subject:invoice",
        "is:unread invoice",
        "is:unread subject:invoice",
        "is:starred",
        "is:read faktura",
        "label:Receipts faktura",
        "label:work launch",
        "in:archive invoice",
        "in:sent thanks",
        "in:spam the",
        "has:attachment",
        "has:attachment invoice",
        "after:2025-01-01 invoice",
        "before:2025-01-01 faktura",
        "to:alex@fernhill.example invoice",
        "cc:nobody invoice",
        // Norwegian, with and without diacritics.
        "Påminnelse",
        "paminnelse",
        "Strøm",
        "strom",
        "Beløp",
        "beløp",
        "fiskesuppe",
        "hytta",
        "eFaktura",
        "Bønne",
        "borettslaget",
        // Punctuation the tokenizer splits.
        "CB-2024",
        "https://console.cirrusbay.test/billing",
        "",
    ];

    const PAGES: &[(u32, u32)] = &[(100, 0), (100, 100), (37, 250), (500, 0)];

    fn run(
        store: &MailStore,
        query: &str,
        limit: u32,
        offset: u32,
        scope: &SearchScope,
        execution: Execution,
    ) -> Vec<ThreadSummary> {
        store
            .search_executing(query, limit, offset, scope, execution)
            .unwrap_or_else(|e| panic!("{query:?} {execution:?}: {e}"))
    }

    fn assert_plans_match_legacy(store: &MailStore, pages: &[(u32, u32)]) -> usize {
        let mut compared = 0;
        let mut nonempty = 0;
        for (name, scope) in scopes(store) {
            for query in QUERIES {
                for &(limit, offset) in pages {
                    let legacy = run(store, query, limit, offset, &scope, Execution::Legacy);
                    for execution in [
                        Execution::Auto,
                        Execution::Plan(TextPlan::Matches),
                        Execution::Plan(TextPlan::Walk),
                    ] {
                        let rows = run(store, query, limit, offset, &scope, execution);
                        assert!(
                            rows == legacy,
                            "{query:?} in {name}, limit {limit} offset {offset}, {execution:?}: {} rows, previous plan {} rows",
                            rows.len(),
                            legacy.len()
                        );
                        compared += 1;
                    }
                    nonempty += usize::from(!legacy.is_empty());
                }
            }
        }
        let searches = compared / 3;
        println!("{searches} searches, {nonempty} with results");
        assert!(nonempty * 4 > searches, "too few searches had results");
        compared
    }

    #[test]
    fn every_plan_returns_the_previous_results_and_pages() {
        let store = corpus(3_000);
        let compared = assert_plans_match_legacy(&store, PAGES);
        assert!(compared > 5_000);
    }

    fn route(
        store: &MailStore,
        query: &str,
        limit: u32,
        offset: u32,
        scope: &SearchScope,
    ) -> Route {
        store
            .with_filters(query, scope, |db, filters| {
                filters
                    .run(db, limit, offset, Execution::Auto)
                    .map(|(_, route)| route)
            })
            .unwrap()
    }

    #[test]
    fn the_route_follows_the_number_of_matches_and_the_walk() {
        let store = corpus(3_000);
        let inbox = SearchScope {
            is_inbox: Some(true),
            ..Default::default()
        };
        let all = SearchScope::default();
        assert_eq!(route(&store, "from:ingrid", 100, 0, &inbox), Route::Scan);
        assert_eq!(
            route(&store, "nonexistentterm", 100, 0, &inbox),
            Route::Matches
        );
        assert_eq!(route(&store, "Brightwater", 100, 0, &inbox), Route::Matches);
        assert_eq!(
            route(&store, "\"launch plan\"", 100, 0, &all),
            Route::Matches
        );
        assert_eq!(route(&store, "invoice", 100, 0, &inbox), Route::Walk);
        assert_eq!(route(&store, "the", 100, 0, &all), Route::Walk);
        // Deep pages make the walk longer, so lookups win sooner.
        assert_eq!(
            route(&store, "invoice", 500, 10_000, &inbox),
            Route::Matches
        );
    }

    /// On a larger store, a common term that a split or view barely holds makes
    /// the walk give up and look the matches up, with the same results.
    #[test]
    fn a_walk_through_a_scope_without_matches_switches_to_lookups() {
        let store = corpus(20_000);
        let promotions = store
            .list_splits()
            .unwrap()
            .into_iter()
            .find(|s| s.name == "Promotions")
            .unwrap();
        let split = SearchScope {
            split_id: Some(promotions.id),
            is_inbox: Some(true),
            ..Default::default()
        };
        let starred = SearchScope {
            starred_only: true,
            ..Default::default()
        };
        for (query, scope) in [("invoice", &split), ("invoice from:cirrusbay", &starred)] {
            assert_eq!(
                route(&store, query, 100, 0, scope),
                Route::WalkThenMatches,
                "{query}"
            );
            assert_eq!(
                run(&store, query, 100, 0, scope, Execution::Auto),
                run(&store, query, 100, 0, scope, Execution::Legacy),
                "{query}"
            );
        }
        // A dense term in the inbox finishes its walk.
        let inbox = SearchScope {
            is_inbox: Some(true),
            ..Default::default()
        };
        assert_eq!(route(&store, "the", 100, 0, &inbox), Route::Walk);
    }

    #[test]
    fn a_stopped_walk_leaves_no_progress_handler_behind() {
        let store = corpus(3_000);
        let scope = SearchScope {
            snoozed_only: true,
            ..Default::default()
        };
        let stopped = store
            .with_filters("invoice", &scope, |db, filters| {
                let walk = filters.sql(Some(TextPlan::Walk), 100, 0);
                let budget = WalkBudget {
                    window: 100,
                    offset: 0,
                    explore: 2 * PROGRESS_STEPS,
                    whole_walk: u64::MAX,
                    lookup: 0,
                };
                read_walk(db, walk, budget)
            })
            .unwrap();
        assert!(stopped.is_none(), "the walk should stop");
        // A long statement afterwards runs to the end.
        let db = store.lock().unwrap();
        let n: i64 = db
            .query_row(
                "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<2000000) SELECT count(*) FROM c",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2_000_000);
    }

    fn query_plan(store: &MailStore, query: &str, scope: &SearchScope, plan: TextPlan) -> String {
        store
            .with_filters(query, scope, |db, filters| {
                let (sql, values) = filters.sql(Some(plan), 100, 0);
                let mut statement = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
                let rows = statement
                    .query_map(params_from_iter(values), |r| r.get::<_, String>(3))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows.join("\n"))
            })
            .unwrap()
    }

    #[test]
    fn query_plans_start_from_matches_or_walk_once() {
        let store = corpus(500);
        let inbox = SearchScope {
            is_inbox: Some(true),
            ..Default::default()
        };
        for scope in [&inbox, &SearchScope::default()] {
            // Rare terms: the match set drives, conversations are looked up by id.
            let plan = query_plan(&store, "launch plan from:maya", scope, TextPlan::Matches);
            assert!(plan.contains("SCAN message_search VIRTUAL TABLE"), "{plan}");
            assert!(
                plan.contains("SEARCH m USING INTEGER PRIMARY KEY (rowid=?)"),
                "{plan}"
            );
            assert!(plan.contains("SCAN hits"), "{plan}");
            assert!(
                plan.contains("SEARCH t USING INDEX sqlite_autoindex_threads_1 (id=?)"),
                "{plan}"
            );
            assert!(!plan.contains("SCAN t"), "{plan}");
            assert!(!plan.contains("threads_inbox_time"), "{plan}");
            // Common terms: one walk newest first; the match list is built once
            // and each conversation's messages are checked against it.
            let plan = query_plan(&store, "invoice", scope, TextPlan::Walk);
            assert!(
                plan.contains("INDEX messages_thread (thread_id=?)"),
                "{plan}"
            );
            assert!(plan.contains("LIST SUBQUERY"), "{plan}");
            assert!(plan.contains("SCAN message_search VIRTUAL TABLE"), "{plan}");
            assert!(!plan.contains("INTEGER PRIMARY KEY"), "{plan}");
            // Only ties on the timestamp are sorted, never the whole result.
            assert!(!plan.contains("USE TEMP B-TREE FOR ORDER BY"), "{plan}");
        }
        let plan = query_plan(&store, "invoice", &inbox, TextPlan::Walk);
        assert!(
            plan.contains("SEARCH t USING INDEX threads_inbox_time (is_inbox=?)"),
            "{plan}"
        );
    }

    /// On the 100,000-conversation search store, every plan against the
    /// previous one. Run in release:
    /// `cargo test --release -p mail-core --lib plans_match_legacy_on_100k -- --ignored --nocapture`
    #[test]
    #[ignore = "seeds 100,000 conversations; run in release"]
    fn plans_match_legacy_on_100k() {
        let started = Instant::now();
        let store = corpus(100_000);
        println!("seeded in {:.1} s", started.elapsed().as_secs_f64());
        let started = Instant::now();
        let compared = assert_plans_match_legacy(&store, &[(100, 0), (37, 250)]);
        println!(
            "{compared} searches matched the previous plan in {:.1} s",
            started.elapsed().as_secs_f64()
        );
    }

    /// Engine time per plan in every scope on 100,000 conversations (median
    /// of 9, warm). Rows where the chosen plan is slower than the previous one
    /// are marked. Run in release:
    /// `cargo test --release -p mail-core --lib plan_timings_on_100k -- --ignored --nocapture`
    #[test]
    #[ignore = "seeds 100,000 conversations; run in release"]
    fn plan_timings_on_100k() {
        let store = corpus(100_000);
        println!(
            "| Query | Scope | Previous ms | Matches ms | Walk ms | Chosen ms | Chosen plan |"
        );
        println!("| --- | --- | ---: | ---: | ---: | ---: | --- |");
        let (mut previous_total, mut chosen_total) = (0.0, 0.0);
        for (name, scope) in scopes(&store) {
            for query in [
                "invoice",
                "faktura",
                "Brightwater",
                "the",
                "Alex",
                "\"launch plan\"",
                "nonexistentterm",
                "invoice from:cirrusbay",
                "hytta",
                "from:ingrid",
            ] {
                let time = |execution| {
                    let mut samples: Vec<f64> = (0..9)
                        .map(|_| {
                            let started = Instant::now();
                            run(&store, query, 100, 0, &scope, execution);
                            started.elapsed().as_secs_f64() * 1000.0
                        })
                        .collect();
                    samples.sort_by(f64::total_cmp);
                    samples[4]
                };
                let chosen = route(&store, query, 100, 0, &scope);
                let (previous, auto) = (time(Execution::Legacy), time(Execution::Auto));
                previous_total += previous;
                chosen_total += auto;
                println!(
                    "| `{query}` | {name} | {previous:.2} | {:.2} | {:.2} | {auto:.2}{} | {chosen:?} |",
                    time(Execution::Plan(TextPlan::Matches)),
                    time(Execution::Plan(TextPlan::Walk)),
                    if auto > previous * 1.25 + 0.5 {
                        " (slower)"
                    } else {
                        ""
                    },
                );
            }
        }
        println!("total: previous {previous_total:.0} ms, chosen {chosen_total:.0} ms");
    }
}
