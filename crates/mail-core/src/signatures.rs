//! Local signature preferences. A saved identity is not authorization to send.
//! The delivery service must still validate From against the provider's aliases.
use crate::{MailError, MailStore, Result};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const MAX_SIGNATURE_BYTES: usize = 16 * 1024;
const MAX_SIGNATURES_PER_ACCOUNT: i64 = 256;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct IdentitySignature {
    pub account_id: String,
    pub email: String,
    pub body: String,
    pub for_new: bool,
    pub for_reply: bool,
}

pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS identity_signatures(
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            email TEXT NOT NULL,
            body TEXT NOT NULL,
            for_new INTEGER NOT NULL,
            for_reply INTEGER NOT NULL,
            PRIMARY KEY(account_id,email)
        );",
    )?;
    Ok(())
}

fn normalized_email(email: &str) -> Result<String> {
    if email.chars().any(char::is_control) {
        return Err(MailError::InvalidAction(
            "Signature identity must be one plain email address".into(),
        ));
    }
    let email = email.trim();
    if email.len() > 254
        || email.chars().any(char::is_control)
        || email_address::EmailAddress::parse_with_options(
            email,
            email_address::Options::default().without_display_text(),
        )
        .is_err()
    {
        return Err(MailError::InvalidAction(
            "Signature identity must be one plain email address".into(),
        ));
    }
    Ok(email.to_lowercase())
}

fn check_account(connection: &Connection, account_id: &str) -> Result<()> {
    if !connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1)",
        [account_id],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(MailError::UnknownAccount(account_id.into()));
    }
    Ok(())
}

fn read_signature(row: &rusqlite::Row<'_>) -> rusqlite::Result<IdentitySignature> {
    Ok(IdentitySignature {
        account_id: row.get(0)?,
        email: row.get(1)?,
        body: row.get(2)?,
        for_new: row.get(3)?,
        for_reply: row.get(4)?,
    })
}

impl MailStore {
    pub fn get_signature(
        &self,
        account_id: &str,
        email: &str,
    ) -> Result<Option<IdentitySignature>> {
        let email = normalized_email(email)?;
        let db = self.lock()?;
        check_account(&db, account_id)?;
        Ok(db
            .query_row(
                "SELECT account_id,email,body,for_new,for_reply FROM identity_signatures
             WHERE account_id=?1 AND email=?2",
                params![account_id, email],
                read_signature,
            )
            .optional()?)
    }

    /// Saves local formatting preferences only; this does not configure a send-as
    /// alias, verify ownership, or authorize provider delivery from this address.
    pub fn upsert_signature(
        &self,
        account_id: &str,
        email: &str,
        body: &str,
        for_new: bool,
        for_reply: bool,
    ) -> Result<IdentitySignature> {
        let email = normalized_email(email)?;
        if body.trim().is_empty()
            || body.len() > MAX_SIGNATURE_BYTES
            || body
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
        {
            return Err(MailError::InvalidAction(
                "Signature text must be nonempty, at most 16 KiB, and contain no control characters other than line breaks or tabs".into(),
            ));
        }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        check_account(&tx, account_id)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM identity_signatures WHERE account_id=?1 AND email=?2)",
            params![account_id, email],
            |row| row.get(0),
        )?;
        if !exists {
            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM identity_signatures WHERE account_id=?1",
                [account_id],
                |row| row.get(0),
            )?;
            if count >= MAX_SIGNATURES_PER_ACCOUNT {
                return Err(MailError::InvalidAction(
                    "Keep at most 256 signature identities per account".into(),
                ));
            }
        }
        tx.execute(
            "INSERT INTO identity_signatures(account_id,email,body,for_new,for_reply)
             VALUES(?1,?2,?3,?4,?5) ON CONFLICT(account_id,email) DO UPDATE SET
             body=excluded.body,for_new=excluded.for_new,for_reply=excluded.for_reply",
            params![account_id, email, body, for_new, for_reply],
        )?;
        tx.commit()?;
        Ok(IdentitySignature {
            account_id: account_id.into(),
            email,
            body: body.into(),
            for_new,
            for_reply,
        })
    }

    /// Idempotent deletion lets a disabled/absent signature be cleared safely.
    pub fn delete_signature(&self, account_id: &str, email: &str) -> Result<()> {
        let email = normalized_email(email)?;
        let db = self.lock()?;
        check_account(&db, account_id)?;
        db.execute(
            "DELETE FROM identity_signatures WHERE account_id=?1 AND email=?2",
            params![account_id, email],
        )?;
        Ok(())
    }
}
