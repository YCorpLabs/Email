//! Build a disposable demo store for recordings: the launch demo mailbox plus
//! an older synthetic history (100,000 conversations by default), then time
//! inbox search through the same `mail-service` request the desktop apps send.
//!
//! ```sh
//! cargo run --release -p mail-service --example demo_store -- \
//!     --store "$(mktemp -d)/mail.sqlite"
//! ```
//!
//! Everything is synthetic (see docs/demo-mailbox.md). The store must be a new
//! file outside the default mail directory; this tool never opens a real store.
use mail_core::MailStore;
use mail_service::{MailService, is_default_store};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Instant;

const USAGE: &str =
    "Usage: demo_store --store /absolute/path/mail.sqlite [--messages N] [--utc-offset-minutes M]
                  [--bench-only] [--all-folders] [--cold] [--query Q ...]

Creates a new store with the launch demo mailbox and N older synthetic
conversations (default 100000), then times inbox search with the request the
desktop apps send. --bench-only times an existing demo store without changing it.
--all-folders searches every folder, as the MCP server does, instead of the inbox.
--cold times each search as the first request on a store whose pages are in
neither SQLite's nor the system's cache: a fresh APFS clone on macOS (the
original's cached pages do not apply to it), or the store after its cached
pages are dropped on Linux. Each sample opens a new service.
--query replaces the default queries (repeat it for several).";

struct Options {
    store: PathBuf,
    messages: usize,
    utc_offset_minutes: i32,
    bench_only: bool,
    all_folders: bool,
    cold: bool,
    queries: Vec<String>,
}

fn options() -> Result<Options, String> {
    let mut args = std::env::args().skip(1);
    let mut store = None;
    let mut messages = 100_000;
    let mut utc_offset_minutes = 0;
    let mut bench_only = false;
    let mut all_folders = false;
    let mut cold = false;
    let mut queries = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--store" => store = args.next().map(PathBuf::from),
            "--messages" => {
                messages = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--messages needs a number")?
            }
            "--utc-offset-minutes" => {
                utc_offset_minutes = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--utc-offset-minutes needs a number")?
            }
            "--bench-only" => bench_only = true,
            "--all-folders" => all_folders = true,
            "--cold" => cold = true,
            "--query" => queries.push(args.next().ok_or("--query needs a value")?),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("Unknown argument {other}\n\n{USAGE}")),
        }
    }
    let store = store.ok_or(USAGE)?;
    if !store.is_absolute() {
        return Err("--store needs an absolute path".into());
    }
    if is_default_store(&store) {
        return Err(
            "Refusing to use the default mail store or its directory. Choose a separate directory."
                .into(),
        );
    }
    if !bench_only && store.exists() {
        return Err(format!(
            "{} already exists. Choose a new path, or pass --bench-only to time it.",
            store.display()
        ));
    }
    if bench_only && !store.exists() {
        return Err(format!("{} does not exist.", store.display()));
    }
    Ok(Options {
        store,
        messages,
        utc_offset_minutes,
        bench_only,
        all_folders,
        cold,
        queries,
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn main() {
    let options = match options() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    if !options.bench_only {
        let started = Instant::now();
        let service = MailService::open(&options.store).unwrap_or_else(|e| fail(&e));
        let summary = service
            .request_json(
                &json!({"op":"seed_demo","set":"launch","utc_offset_minutes":options.utc_offset_minutes})
                    .to_string(),
            )
            .unwrap_or_else(|e| fail(&e));
        drop(service);
        println!(
            "Launch demo: {} conversations, {} messages ({:.1} s)",
            summary["threads"],
            summary["messages"],
            started.elapsed().as_secs_f64()
        );
        let started = Instant::now();
        let store = MailStore::open(&options.store).unwrap_or_else(|e| fail(&e.to_string()));
        let corpus = store
            .seed_demo_search_corpus(options.messages, now_ms())
            .unwrap_or_else(|e| fail(&e.to_string()));
        drop(store);
        println!(
            "Search history: {} conversations, {} messages ({:.1} s)",
            corpus.threads,
            corpus.messages,
            started.elapsed().as_secs_f64()
        );
    }
    let service = MailService::open(&options.store).unwrap_or_else(|e| fail(&e));
    let total: Value = service
        .request_json(r#"{"op":"unread_counts"}"#)
        .unwrap_or(Value::Null);
    let size = ["", "-wal"]
        .iter()
        .filter_map(|suffix| std::fs::metadata(format!("{}{suffix}", options.store.display())).ok())
        .map(|m| m.len())
        .sum::<u64>();
    println!(
        "Store: {} ({:.0} MB), unread {}",
        options.store.display(),
        size as f64 / 1e6,
        total["total"]
    );
    println!(
        "Build: {} (bundled SQLite)",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!(
        "\n{} search through mail-service request_json{} (limit 100).",
        if options.all_folders {
            "All-folder"
        } else {
            "Inbox"
        },
        if options.all_folders {
            ", without an inbox scope"
        } else {
            ", as the apps send it"
        }
    );
    if options.cold {
        println!(
            "Cold: 30 samples, each the first request on a newly opened service with no cached store pages.\n"
        );
        println!("| Query | Results | Cold P50 ms | Cold P95 ms |\n| --- | ---: | ---: | ---: |");
    } else {
        println!(
            "Warm: each query runs 31 times: the first is shown separately, P50/P95 come from the other 30.\n"
        );
        println!(
            "| Query | Results | First ms | P50 ms | P95 ms |\n| --- | ---: | ---: | ---: | ---: |"
        );
    }
    let defaults = [
        "invoice",
        "faktura",
        "Ingrid",
        "Brightwater",
        "Elena",
        "from:ingrid",
        "is:unread subject:invoice",
        "\"launch plan\"",
        "nonexistentterm",
        "",
    ];
    let queries: Vec<String> = if options.queries.is_empty() {
        defaults.iter().map(|q| q.to_string()).collect()
    } else {
        options.queries.clone()
    };
    for query in &queries {
        let query = query.as_str();
        let mut request = json!({
            "op": "search", "query": query, "limit": 100, "offset": 0,
            "unread_only": false, "starred_only": false, "snoozed_only": false,
            "followups_only": false, "is_inbox": true,
        });
        if options.all_folders {
            request.as_object_mut().unwrap().remove("is_inbox");
        }
        let request = request.to_string();
        let label = if query.is_empty() {
            "(inbox listing)"
        } else {
            query
        };
        if options.cold {
            let mut samples = Vec::with_capacity(30);
            let mut hits = 0;
            for n in 0..30 {
                let (ms, rows) = cold_sample(&options.store, n, &request);
                samples.push(ms);
                hits = rows;
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "| `{label}` | {hits} | {:.3} | {:.3} |",
                samples[15], samples[28]
            );
            continue;
        }
        let mut samples = Vec::with_capacity(31);
        let mut hits = 0;
        for _ in 0..31 {
            let started = Instant::now();
            let rows = service.request_json(&request).unwrap_or_else(|e| fail(&e));
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
            hits = rows.as_array().map_or(0, Vec::len);
        }
        let first = samples.remove(0);
        samples.sort_by(f64::total_cmp);
        println!(
            "| `{label}` | {hits} | {first:.3} | {:.3} | {:.3} |",
            samples[15], samples[28]
        );
    }
    println!(
        "\nOpen it again later with:\n  scripts/dev.sh --store {} --no-demo",
        options.store.display()
    );
}

/// One search as the first request on a store with no cached pages; returns
/// the request time in milliseconds and the number of results.
fn cold_sample(store: &Path, n: usize, request: &str) -> (f64, usize) {
    let target = uncached_store(store, n);
    let service = MailService::open(&target).unwrap_or_else(|e| fail(&e));
    let started = Instant::now();
    let rows = service.request_json(request).unwrap_or_else(|e| fail(&e));
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    drop(service);
    if target != store {
        let _ = std::fs::remove_dir_all(target.parent().unwrap());
    }
    (ms, rows.as_array().map_or(0, Vec::len))
}

/// A clone shares the original's blocks on disk but none of its cached pages.
#[cfg(target_os = "macos")]
fn uncached_store(store: &Path, n: usize) -> PathBuf {
    let dir = store
        .parent()
        .unwrap()
        .join(format!("cold-sample-{}-{n}", std::process::id()));
    std::fs::create_dir(&dir).unwrap_or_else(|e| fail(&e.to_string()));
    let target = dir.join("mail.sqlite");
    for suffix in ["", "-wal"] {
        let from = PathBuf::from(format!("{}{suffix}", store.display()));
        if from.exists() {
            let status = std::process::Command::new("/bin/cp")
                .arg("-c")
                .arg(&from)
                .arg(format!("{}{suffix}", target.display()))
                .status()
                .unwrap_or_else(|e| fail(&e.to_string()));
            if !status.success() {
                fail("Could not clone the store (cp -c needs APFS)");
            }
        }
    }
    target
}

/// Linux drops a file's clean cached pages on request, without privileges.
#[cfg(target_os = "linux")]
fn uncached_store(store: &Path, _n: usize) -> PathBuf {
    use std::os::fd::AsRawFd;
    for suffix in ["", "-wal"] {
        let path = format!("{}{suffix}", store.display());
        if let Ok(file) = std::fs::File::open(&path) {
            let _ = file.sync_all();
            // SAFETY: a valid descriptor; the call only drops cached pages.
            unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        }
    }
    store.to_path_buf()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn uncached_store(_store: &Path, _n: usize) -> PathBuf {
    fail("--cold is supported on macOS and Linux");
}

fn fail(error: &str) -> ! {
    eprintln!("{error}");
    std::process::exit(1);
}
