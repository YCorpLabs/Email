//! Filters: the JSON operations both shells use, the shared model and its
//! download. See docs/filters.md.
//!
//! The model is process-wide: native shells open several `MailService`
//! handles, but they share one loaded copy. It loads when there is mail to
//! sort and is dropped when the backlog is empty.
//! `mail-mcp` never sorts; it only reads and edits filters.

use mail_ai::{Category, Classifier, Email, ModelSpec, download};
use mail_core::{FilterAction, FilterDecision, MailFilter, MailStore, filter_starters};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_BUDGET_MS: u64 = 20_000;
const BATCH: usize = 8;

#[derive(Default)]
struct DownloadState {
    running: bool,
    done: u64,
    total: u64,
    error: Option<String>,
    cancel: Option<Arc<AtomicBool>>,
}

#[derive(Default)]
struct SortState {
    remaining: usize,
    last_sorted_at_ms: Option<i64>,
    error: Option<String>,
}

static ENGINE: Mutex<Option<Classifier>> = Mutex::new(None);
static SORTING: Mutex<()> = Mutex::new(());
static SORT_STATE: Mutex<SortState> = Mutex::new(SortState { remaining: 0, last_sorted_at_ms: None, error: None });
static DOWNLOAD: Mutex<DownloadState> = Mutex::new(DownloadState {
    running: false,
    done: 0,
    total: 0,
    error: None,
    cancel: None,
});

/// Shared by every store of this profile, so a disposable development store
/// does not download its own copy. `EMAIL_MODELS_DIR` overrides it.
pub(crate) fn models_dir(database_path: &std::path::Path) -> PathBuf {
    if cfg!(test) {
        // Tests never see (or load) a model installed for real use.
        return database_path.with_extension("models");
    }
    if let Some(dir) = std::env::var_os("EMAIL_MODELS_DIR") {
        return PathBuf::from(dir);
    }
    crate::store_location::default_store_directory()
        .unwrap_or_else(|| database_path.parent().map(PathBuf::from).unwrap_or_default())
        .join("Models")
}

fn model() -> &'static ModelSpec {
    mail_ai::recommended_model()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub(crate) fn status(store: &MailStore, dir: &std::path::Path) -> Result<Value, String> {
    let filters = store.list_filters().map_err(err)?;
    let counts = store.filter_counts().map_err(err)?;
    let spec = model();
    let installed = download::is_installed(dir, spec);
    let dl = DOWNLOAD.lock().map_err(err)?;
    let downloaded = if dl.running { dl.done } else { download::bytes_present(dir, spec) };
    let sort = SORT_STATE.lock().map_err(err)?;
    let rows: Vec<Value> = filters
        .iter()
        .map(|f| {
            let c = counts.iter().find(|c| c.filter_id == f.id);
            json!({"id":f.id,"name":f.name,"description":f.description,"action":f.action,
                "position":f.position,"unread":c.map_or(0,|c|c.unread),"last_7_days":c.map_or(0,|c|c.last_7_days)})
        })
        .collect();
    Ok(json!({
        "enabled": store.filters_enabled().map_err(err)?,
        "filters": rows,
        "starters": filter_starters(),
        "model": {
            "id": spec.id,
            "model_name": spec.model_name,
            "license": spec.license,
            "download_bytes": spec.download_bytes(),
            "memory_bytes": spec.memory_bytes,
            "installed": installed,
            "downloading": dl.running,
            "downloaded_bytes": downloaded,
            "error": dl.error,
        },
        "accelerator": mail_ai::accelerator(),
        "system_memory_bytes": mail_ai::system_memory_bytes(),
        "sorting": {
            "running": SORTING.try_lock().is_err(),
            "remaining": sort.remaining,
            "last_sorted_at_ms": sort.last_sorted_at_ms,
            "error": sort.error,
        },
    }))
}

/// Download the model, blocking until it finishes, fails or is cancelled.
/// Shells call this off the UI thread and poll `filters_status` for progress.
pub(crate) fn download_model(dir: &std::path::Path) -> Result<Value, String> {
    let spec = model();
    let cancel = {
        let mut dl = DOWNLOAD.lock().map_err(err)?;
        if dl.running {
            return Ok(json!({"already_running": true}));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *dl = DownloadState {
            running: true,
            done: download::bytes_present(dir, spec),
            total: spec.download_bytes(),
            error: None,
            cancel: Some(cancel.clone()),
        };
        cancel
    };
    let result = download::download(dir, spec, &cancel, &|done, total| {
        if let Ok(mut dl) = DOWNLOAD.lock() {
            dl.done = done;
            dl.total = total;
        }
    });
    let mut dl = DOWNLOAD.lock().map_err(err)?;
    dl.running = false;
    dl.cancel = None;
    match result {
        Ok(()) => {
            dl.error = None;
            Ok(json!({"installed": true}))
        }
        Err(mail_ai::AiError::Cancelled) => Ok(json!({"cancelled": true})),
        Err(e) => {
            dl.error = Some(e.to_string());
            Err(e.to_string())
        }
    }
}

pub(crate) fn cancel_download() -> Result<Value, String> {
    let dl = DOWNLOAD.lock().map_err(err)?;
    if let Some(cancel) = &dl.cancel {
        cancel.store(true, Ordering::Relaxed);
    }
    Ok(json!({"cancelled": dl.running}))
}

pub(crate) fn remove_model(dir: &std::path::Path) -> Result<Value, String> {
    if DOWNLOAD.lock().map_err(err)?.running {
        return Err("Wait for the download to stop first".into());
    }
    let _sorting = SORTING.lock().map_err(err)?;
    *ENGINE.lock().map_err(err)? = None;
    download::remove(dir, model(), &[]).map_err(err)?;
    Ok(json!({"removed": true}))
}

fn categories(filters: &[MailFilter]) -> Vec<Category> {
    filters.iter().map(|f| Category { name: f.name.clone(), description: f.description.clone() }).collect()
}

fn decision_json(d: &FilterDecision) -> Value {
    json!({"thread_id": d.thread_id, "filter_id": d.filter_id, "moved": d.moved})
}

/// Sort waiting mail for up to `budget_ms`. Returns what moved and how much
/// is left; shells call again while `remaining` is above zero.
pub(crate) fn wake(store: &MailStore, dir: &std::path::Path, budget_ms: Option<u64>) -> Result<Value, String> {
    if !store.filters_enabled().map_err(err)? {
        return Ok(json!({"enabled": false, "decisions": [], "remaining": 0}));
    }
    let Ok(_sorting) = SORTING.try_lock() else {
        return Ok(json!({"busy": true, "decisions": [], "remaining": SORT_STATE.lock().map_err(err)?.remaining}));
    };
    let budget = Duration::from_millis(budget_ms.unwrap_or(DEFAULT_BUDGET_MS).clamp(1_000, 120_000));
    let started = Instant::now();
    let spec = model();
    let installed = download::is_installed(dir, spec);
    let mut decisions = Vec::new();
    let mut error = None;
    loop {
        let (candidates, settled) = store.filter_candidates(BATCH).map_err(err)?;
        decisions.extend(settled);
        if candidates.is_empty() || !installed {
            break;
        }
        let filters = store.list_filters().map_err(err)?;
        if filters.is_empty() {
            break;
        }
        let cats = categories(&filters);
        let mut engine = ENGINE.lock().map_err(err)?;
        if engine.is_none() {
            match Classifier::load(&download::file_path(dir, &spec.weights), &download::file_path(dir, &spec.tokenizer)) {
                Ok(classifier) => *engine = Some(classifier),
                Err(e) => {
                    error = Some(format!("The sorting model couldn't start: {e}"));
                    break;
                }
            }
        }
        let classifier = engine.as_ref().expect("loaded above");
        let mut out_of_time = false;
        for candidate in &candidates {
            let email = Email { from: candidate.sender.clone(), subject: candidate.subject.clone(), body: candidate.body.clone() };
            match classifier.classify(&cats, &email, candidate.bulk) {
                Ok(verdict) => {
                    // The store applies the thresholds: below them the
                    // conversation stays in the inbox, untagged.
                    let (filter_id, confidence) = match &verdict {
                        Some(v) => (Some(filters[v.index].id.as_str()), v.confidence),
                        None => (None, 1.0),
                    };
                    decisions.push(store.record_model_decision(candidate, filter_id, confidence, "").map_err(err)?);
                }
                Err(e) => {
                    error = Some(format!("Sorting failed: {e}"));
                    break;
                }
            }
            if started.elapsed() >= budget {
                out_of_time = true;
                break;
            }
        }
        if error.is_some() || out_of_time {
            break;
        }
    }
    let remaining = if installed { store.filter_backlog().map_err(err)? } else { 0 };
    if remaining == 0 || error.is_some() {
        // Free the model's memory until there is mail to sort again.
        *ENGINE.lock().map_err(err)? = None;
    }
    {
        let mut state = SORT_STATE.lock().map_err(err)?;
        state.remaining = remaining;
        state.error = error.clone();
        if !decisions.is_empty() {
            state.last_sorted_at_ms = Some(now_ms());
        }
    }
    let moved = decisions.iter().filter(|d| d.moved).count();
    Ok(json!({
        "decisions": decisions.iter().map(decision_json).collect::<Vec<_>>(),
        "moved": moved,
        "remaining": remaining,
        "model_missing": !installed,
        "error": error,
    }))
}

pub(crate) fn upsert(store: &MailStore, id: Option<&str>, name: &str, description: &str, action: FilterAction) -> Result<Value, String> {
    serde_json::to_value(store.upsert_filter(id, name, description, action).map_err(err)?).map_err(err)
}

pub(crate) fn move_to(store: &MailStore, thread_id: &str, filter_id: Option<&str>) -> Result<Value, String> {
    Ok(decision_json(&store.move_thread_to_filter(thread_id, filter_id).map_err(err)?))
}

#[cfg(test)]
mod tests {
    use crate::MailService;
    use serde_json::{Value, json};

    fn call(service: &MailService, request: Value) -> Value {
        service.request_json(&request.to_string()).unwrap_or_else(|e| panic!("{request}: {e}"))
    }

    #[test]
    fn filter_operations_work_without_a_model() {
        let dir = std::env::temp_dir().join(format!("email-filters-{}", uuid::Uuid::new_v4()));
        let service = MailService::open(dir.join("mail.sqlite")).unwrap();
        call(&service, json!({"op":"seed_demo","set":"launch"}));
        let status = call(&service, json!({"op":"filters_status"}));
        assert_eq!(status["enabled"], false);
        assert_eq!(status["model"]["installed"], false);
        assert_eq!(status["starters"].as_array().unwrap().len(), 4);
        assert_eq!(status["model"]["download_bytes"].as_u64().unwrap(), mail_ai::recommended_model().download_bytes());

        let status = call(&service, json!({"op":"filters_setup","starter_keys":["cold_outreach","newsletters"]}));
        assert_eq!(status["enabled"], true);
        let filters = status["filters"].as_array().unwrap();
        assert_eq!(filters.len(), 2);
        let cold = filters[0]["id"].as_str().unwrap().to_owned();
        assert_eq!(filters[0]["action"], "ignore");

        // No model: contacts are settled, nothing is guessed.
        let woke = call(&service, json!({"op":"filters_wake","budget_ms":1000}));
        assert_eq!(woke["model_missing"], true);
        assert_eq!(woke["moved"], 0);

        // Sorting by hand works without the model.
        let inbox = call(&service, json!({"op":"list_threads","limit":200}));
        let pitch = inbox.as_array().unwrap().iter().find(|t| t["id"] == "demo:launch:pitch-seo").unwrap();
        assert!(pitch["filter"].is_null());
        let moved = call(&service, json!({"op":"move_to_filter","thread_id":"demo:launch:pitch-seo","filter_id":cold}));
        assert_eq!(moved["moved"], true);
        let listed = call(&service, json!({"op":"list_filter_threads","filter_id":cold}));
        assert_eq!(listed[0]["filter"]["name"], "Cold outreach");
        assert_eq!(listed[0]["filter"]["source"], "user");
        let inbox = call(&service, json!({"op":"list_threads","limit":200}));
        assert!(inbox.as_array().unwrap().iter().all(|t| t["id"] != "demo:launch:pitch-seo"));
        call(&service, json!({"op":"move_to_filter","thread_id":"demo:launch:pitch-seo","filter_id":null}));
        let inbox = call(&service, json!({"op":"list_threads","limit":200}));
        assert!(inbox.as_array().unwrap().iter().any(|t| t["id"] == "demo:launch:pitch-seo"));

        let receipts = call(&service, json!({"op":"upsert_filter","name":"Receipts","description":"Receipts and invoices","action":"skip_inbox"}));
        let renamed = call(&service, json!({"op":"upsert_filter","id":receipts["id"],"name":"Bills","description":"Receipts, invoices and bills","action":"inbox"}));
        assert_eq!(renamed["name"], "Bills");
        let err = service
            .request_json(&json!({"op":"upsert_filter","name":"Inbox","description":"x","action":"inbox"}).to_string())
            .unwrap_err();
        assert!(err.contains("inbox"), "{err}");
        call(&service, json!({"op":"delete_filter","id":renamed["id"]}));
        let status = call(&service, json!({"op":"filters_set_enabled","enabled":false}));
        assert_eq!(status["enabled"], false);
        assert_eq!(status["filters"].as_array().unwrap().len(), 2);
        let woke = call(&service, json!({"op":"filters_wake"}));
        assert_eq!(woke["enabled"], false);
        let _ = std::fs::remove_dir_all(dir);
    }
}
