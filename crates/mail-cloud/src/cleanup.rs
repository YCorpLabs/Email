//! One page or mutation per call; the service owns durable progress and confirmation.
use crate::{Account, CloudClient, CloudError, CloudProvider, Result, local_id, remote_id};
use serde_json::{Value, json};

pub struct GmailCleanupPage {
    pub message_ids: Vec<String>,
    pub next_page_token: Option<String>,
}
const BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me/messages";
fn query(sender: &str) -> Result<String> {
    if mail_core::parse_outgoing_recipients(sender).is_err()
        || !sender
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+-/=?^_`{|}~@".contains(c))
    {
        return Err(CloudError::InvalidHeader);
    }
    Ok(format!("from:\"{sender}\" -in:spam -in:trash"))
}
impl CloudClient {
    pub fn gmail_sender_cleanup_page(
        &self,
        account: &Account,
        sender: &str,
        page_token: Option<&str>,
    ) -> Result<GmailCleanupPage> {
        if self.provider() != CloudProvider::Gmail || account.provider != mail_core::Provider::Gmail
        {
            return Err(CloudError::ProviderMismatch);
        }
        let mut url = url::Url::parse(BASE)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("q", &query(sender)?)
                .append_pair("labelIds", "UNREAD")
                .append_pair("maxResults", "500")
                .append_pair("includeSpamTrash", "false");
            if let Some(token) = page_token {
                pairs.append_pair("pageToken", token);
            }
        }
        parse_page(account, self.get(url.as_str())?)
    }
    pub fn gmail_apply_sender_cleanup(
        &self,
        account: &Account,
        message_ids: &[String],
    ) -> Result<()> {
        if self.provider() != CloudProvider::Gmail || account.provider != mail_core::Provider::Gmail
        {
            return Err(CloudError::ProviderMismatch);
        }
        if message_ids.len() > 500 {
            return Err(CloudError::Response("cleanup batch exceeds 500 messages"));
        }
        if message_ids.is_empty() {
            return Ok(());
        }
        let remote = message_ids
            .iter()
            .map(|id| remote_id(account, "gmail-message", id))
            .collect::<Result<Vec<_>>>()?;
        self.post(
            &format!("{BASE}/batchModify"),
            &json!({"ids":remote,"removeLabelIds":["UNREAD"]}),
        )?;
        Ok(())
    }
}
fn parse_page(account: &Account, value: Value) -> Result<GmailCleanupPage> {
    let mut ids = vec![];
    if let Some(messages) = value.get("messages") {
        let messages = messages
            .as_array()
            .ok_or(CloudError::Response("cleanup messages must be an array"))?;
        if messages.len() > 500 {
            return Err(CloudError::Response("cleanup page too large"));
        }
        for item in messages {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| {
                    !id.is_empty()
                        && id.len() <= 256
                        && id
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                })
                .ok_or(CloudError::Response("invalid cleanup message ID"))?;
            ids.push(local_id(account, "gmail-message", id));
        }
    }
    let next = match value.get("nextPageToken") {
        None | Some(Value::Null) => None,
        Some(Value::String(v)) if !v.is_empty() && v.len() <= 8192 => Some(v.clone()),
        _ => return Err(CloudError::Response("invalid cleanup page token")),
    };
    Ok(GmailCleanupPage {
        message_ids: ids,
        next_page_token: next,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn account() -> Account {
        Account {
            id: "a".into(),
            provider: mail_core::Provider::Gmail,
            email: "me@example.com".into(),
            display_name: "Me".into(),
        }
    }
    #[test]
    fn sender_query_cannot_inject_search_operators() {
        assert_eq!(
            query("news@example.com").unwrap(),
            "from:\"news@example.com\" -in:spam -in:trash"
        );
        for input in [
            "a@example.com OR is:unread",
            "a@example.com\"",
            "a@example.com\n",
            "A <a@example.com>",
        ] {
            assert!(query(input).is_err());
        }
    }
    #[test]
    fn pages_are_account_scoped_and_malformed_pages_fail() {
        let page = parse_page(
            &account(),
            json!({"messages":[{"id":"123abc"}],"nextPageToken":"next"}),
        )
        .unwrap();
        assert_eq!(page.message_ids, vec!["a:gmail-message:123abc"]);
        assert_eq!(page.next_page_token.as_deref(), Some("next"));
        assert!(parse_page(&account(), json!({"messages":[{}]})).is_err());
        assert!(parse_page(&account(), json!({"messages":[{"id":"../other"}]})).is_err());
        assert!(parse_page(&account(), json!({"messages":{}})).is_err());
        assert!(parse_page(&account(), json!({"nextPageToken":12})).is_err());
    }
}
