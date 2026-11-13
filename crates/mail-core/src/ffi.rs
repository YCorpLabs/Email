//! Stable JSON-over-C ABI for native shells. All returned strings are owned by Rust.
use crate::{MailAction, MailStore};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    SeedDemo,
    ListAccounts,
    ListThreads {
        account_id: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
    },
    ListThreadMessages {
        thread_id: String,
    },
    GetLatestMessageForThread {
        thread_id: String,
    },
    Search {
        query: String,
        limit: Option<u32>,
    },
    GetMessage {
        id: String,
    },
    ComposeDraft {
        account_id: String,
        to: String,
        subject: String,
        body_plain: String,
    },
    ApplyAction {
        operation_id: String,
        action: MailAction,
    },
    PendingActions,
}
fn respond(store: &MailStore, request: &str) -> std::result::Result<Value, String> {
    let request: Request = serde_json::from_str(request).map_err(|e| e.to_string())?;
    let result = match request {
        Request::SeedDemo => {
            store.seed_demo().map_err(|e| e.to_string())?;
            Value::Null
        }
        Request::ListAccounts => json!(store.list_accounts().map_err(|e| e.to_string())?),
        Request::ListThreads {
            account_id,
            limit,
            offset,
        } => json!(
            store
                .list_threads(
                    account_id.as_deref(),
                    limit.unwrap_or(50),
                    offset.unwrap_or(0)
                )
                .map_err(|e| e.to_string())?
        ),
        Request::ListThreadMessages { thread_id } => json!(
            store
                .list_thread_messages(&thread_id)
                .map_err(|e| e.to_string())?
        ),
        Request::GetLatestMessageForThread { thread_id } => json!(
            store
                .get_latest_message_for_thread(&thread_id)
                .map_err(|e| e.to_string())?
        ),
        Request::Search { query, limit } => json!(
            store
                .search(&query, limit.unwrap_or(50))
                .map_err(|e| e.to_string())?
        ),
        Request::GetMessage { id } => json!(store.get_message(&id).map_err(|e| e.to_string())?),
        Request::ComposeDraft {
            account_id,
            to,
            subject,
            body_plain,
        } => json!(
            store
                .compose_draft(&account_id, &to, &subject, &body_plain)
                .map_err(|e| e.to_string())?
        ),
        Request::ApplyAction {
            operation_id,
            action,
        } => json!(
            store
                .apply_action(&operation_id, action)
                .map_err(|e| e.to_string())?
        ),
        Request::PendingActions => json!(store.pending_actions().map_err(|e| e.to_string())?),
    };
    Ok(result)
}
fn string_result(value: Value) -> *mut c_char {
    CString::new(value.to_string())
        .expect("JSON is NUL-free")
        .into_raw()
}

/// # Safety
/// `path` must point to a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_store_open(path: *const c_char) -> *mut MailStore {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let path = unsafe { CStr::from_ptr(path) }.to_str().ok()?;
        MailStore::open(path).ok().map(Box::new)
    }));
    result
        .ok()
        .flatten()
        .map(Box::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// `store` must be a pointer returned by `mail_store_open`, not already closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_store_close(store: *mut MailStore) {
    if !store.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(store) })));
    }
}
/// # Safety
/// `store` must be a live store pointer and `request_json` a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_store_request_json(
    store: *mut MailStore,
    request_json: *const c_char,
) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(|| {
        if store.is_null() || request_json.is_null() {
            return json!({"ok":false,"error":"null pointer"});
        }
        let request = match unsafe { CStr::from_ptr(request_json) }.to_str() {
            Ok(s) => s,
            Err(_) => return json!({"ok":false,"error":"request must be UTF-8"}),
        };
        match respond(unsafe { &*store }, request) {
            Ok(data) => json!({"ok":true,"data":data}),
            Err(error) => json!({"ok":false,"error":error}),
        }
    }))
    .unwrap_or_else(|_| json!({"ok":false,"error":"panic in mail core"}));
    string_result(value)
}
/// # Safety
/// `value` must be a pointer returned by `mail_store_request_json`, not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_store_free_string(value: *mut c_char) {
    if !value.is_null() {
        let _ = unsafe { CString::from_raw(value) };
    }
}
