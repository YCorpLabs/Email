//! On-device mail sorting for Email.
//!
//! A small decision model (see [`sorter`]) reads a filter's description and
//! an email together and returns the probability that the email belongs
//! there. It never generates text, so message content can only move a score:
//! an email can at most land in the wrong filter, never instruct the app.
//! It runs on the CPU through `candle`, in tens of milliseconds per email.
//!
//! The model file is downloaded on request from a pinned location and
//! verified with SHA-256 before use (see [`catalog`] and [`download`]).

pub mod sorter;

pub mod catalog;
pub mod download;

use std::path::Path;

pub use sorter::Sorter;

pub use catalog::{ModelSpec, catalog, find_model, recommended_model, system_memory_bytes};

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("model: {0}")]
    Model(#[from] candle_core::Error),
    #[error("tokenizer: {0}")]
    Tokenizer(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Download(String),
    #[error("download cancelled")]
    Cancelled,
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, AiError>;

/// A filter as the model sees it: its name and plain-language description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct Email {
    pub from: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct Verdict {
    /// Index into the filters passed to [`Classifier::classify`].
    pub index: usize,
    /// Probability that the email belongs in that filter, 0–1.
    pub confidence: f32,
}

/// Which hardware runs the model in this build. The sorter is small enough
/// that the CPU is fast on every supported computer.
pub fn accelerator() -> &'static str {
    "CPU"
}

/// Characters of the email the model reads: sender, subject and the start
/// of the body.
const EMAIL_CHARS: usize = 1200;
const BULK_MARKER: &str = "[Bulk mail with an unsubscribe link]";

pub struct Classifier {
    sorter: Sorter,
}

impl Classifier {
    pub fn load(weights: &Path, tokenizer: &Path) -> Result<Self> {
        Ok(Self { sorter: Sorter::load(weights, tokenizer)? })
    }

    /// Scores the email against each filter's description and returns the
    /// best match, or `None` when there are no filters. The caller decides
    /// whether the confidence is high enough to act on.
    pub fn classify(&self, filters: &[Category], email: &Email, bulk: bool) -> Result<Option<Verdict>> {
        if filters.is_empty() {
            return Ok(None);
        }
        let descriptions: Vec<String> = filters.iter().map(|f| f.description.chars().take(400).collect()).collect();
        let scores = self.sorter.score(&descriptions, &email_text(email, bulk))?;
        Ok(scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, &confidence)| Verdict { index, confidence }))
    }
}

/// The text the model reads, in the format it was trained on.
fn email_text(email: &Email, bulk: bool) -> String {
    let head = format!(
        "From: {}\nSubject: {}\n\n",
        email.from.trim().chars().take(200).collect::<String>(),
        email.subject.trim().chars().take(300).collect::<String>()
    );
    let marker = if bulk { format!("\n{BULK_MARKER}") } else { String::new() };
    let room = EMAIL_CHARS.saturating_sub(head.chars().count() + marker.chars().count());
    let body: String = email.body.trim().chars().take(room).collect();
    format!("{head}{body}{marker}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_text_keeps_the_bulk_marker_when_the_body_is_long() {
        let email = Email { from: "A <a@b.c>".into(), subject: "Hi".into(), body: "x".repeat(5000) };
        let text = email_text(&email, true);
        assert!(text.starts_with("From: A <a@b.c>\nSubject: Hi\n\nxxx"));
        assert!(text.ends_with(BULK_MARKER));
        assert_eq!(text.chars().count(), EMAIL_CHARS);
        assert!(!email_text(&email, false).contains(BULK_MARKER));
    }
}
