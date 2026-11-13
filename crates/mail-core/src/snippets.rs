use crate::{MailError, MailStore, Result};
use rusqlite::{OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    pub body: String,
}

impl MailStore {
    pub fn list_snippets(&self) -> Result<Vec<Snippet>> {
        let db = self.lock()?;
        let mut query =
            db.prepare("SELECT id,name,body FROM snippets ORDER BY name COLLATE NOCASE,id")?;
        Ok(query
            .query_map([], |r| {
                Ok(Snippet {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    body: r.get(2)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?)
    }

    pub fn upsert_snippet(&self, id: Option<&str>, name: &str, body: &str) -> Result<Snippet> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err(MailError::InvalidAction(
                "Snippet names must be 1–80 printable characters".into(),
            ));
        }
        if body.trim().is_empty() || body.len() > 65536 || body.contains('\0') {
            return Err(MailError::InvalidAction(
                "Snippet text must be nonempty and at most 64 KiB".into(),
            ));
        }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        if let Some(existing) = tx
            .query_row(
                "SELECT id FROM snippets WHERE name=?1 COLLATE NOCASE",
                [name],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            if Some(existing.as_str()) != id {
                return Err(MailError::InvalidAction(
                    "A snippet with that name already exists".into(),
                ));
            }
        }
        let id = match id {
            Some(id) => {
                if !tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM snippets WHERE id=?1)",
                    [id],
                    |r| r.get::<_, bool>(0),
                )? {
                    return Err(MailError::NotFound(id.into()));
                }
                id.to_owned()
            }
            None => {
                let count: i64 = tx.query_row("SELECT COUNT(*) FROM snippets", [], |r| r.get(0))?;
                if count >= 500 {
                    return Err(MailError::InvalidAction(
                        "Keep at most 500 saved snippets".into(),
                    ));
                }
                uuid::Uuid::new_v4().to_string()
            }
        };
        tx.execute("INSERT INTO snippets(id,name,body) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET name=excluded.name,body=excluded.body",params![id,name,body])?;
        tx.commit()?;
        Ok(Snippet {
            id,
            name: name.into(),
            body: body.into(),
        })
    }

    pub fn delete_snippet(&self, id: &str) -> Result<()> {
        if self
            .lock()?
            .execute("DELETE FROM snippets WHERE id=?1", [id])?
            == 0
        {
            return Err(MailError::NotFound(id.into()));
        }
        Ok(())
    }
}
