//! Explicit local export. Never initialize MailStore/MailService or credentials.
use rusqlite::{Connection, limits::Limit};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const MAX_DATABASE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_REPORT_BYTES: usize = 16 * 1024;

pub(crate) fn run(args: &[String]) -> Result<(), &'static str> {
    let (database, output) = paths(args)?;
    let report = collect(&database);
    write_report(&output, &report)
}

fn paths(args: &[String]) -> Result<(PathBuf, PathBuf), &'static str> {
    let (mut database, mut output) = (None, None);
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        let slot = match argument.as_str() {
            "--db" if database.is_none() => &mut database,
            "--diagnostics" if output.is_none() => &mut output,
            _ => return Err("diagnostics_invalid_arguments"),
        };
        let path = args
            .next()
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .ok_or("diagnostics_missing_path")?;
        *slot = Some(PathBuf::from(path));
    }
    Ok((
        database.ok_or("diagnostics_missing_database")?,
        output.ok_or("diagnostics_missing_output")?,
    ))
}

fn collect(path: &Path) -> Value {
    let database = inspect(path).unwrap_or_else(
        |code| json!({"inspection":"not_completed","code":code,"integrity":"not_checked"}),
    );
    // Only the project's source-fingerprint format is exportable. Free-form
    // build environment values can contain private names or credentials.
    let build = option_env!("EMAIL_BUILD_ID")
        .filter(|value| {
            value.strip_prefix("dev-").is_some_and(|hash| {
                hash.len() == 12 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        })
        .unwrap_or("unknown");
    json!({"schemaVersion":1,"product":"Email","component":"mail-mcp","version":env!("CARGO_PKG_VERSION"),"build":build,
        "platform":{"os":std::env::consts::OS,"architecture":std::env::consts::ARCH},
        "localOnly":true,"database":database})
}

fn sidecar_present(path: &Path) -> Result<bool, &'static str> {
    for suffix in ["-wal", "-journal"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        match fs::symlink_metadata(PathBuf::from(name)) {
            Ok(metadata) if !metadata.is_file() || metadata.len() > 0 => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("database_metadata_unavailable"),
        }
    }
    Ok(false)
}

fn inspect(path: &Path) -> Result<Value, &'static str> {
    if sidecar_present(path)? {
        return Err("database_active_journal");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path).map_err(|_| "database_unavailable")?;
    let before = file
        .metadata()
        .map_err(|_| "database_metadata_unavailable")?;
    if !before.is_file() {
        return Err("database_not_regular");
    }
    if before.len() > MAX_DATABASE_BYTES {
        return Err("database_size_limit");
    }
    if before.len() < 100 {
        return Err("database_invalid_header");
    }
    let mut header = [0u8; 100];
    file.read_exact(&mut header)
        .map_err(|_| "database_read_failed")?;
    if &header[..16] != b"SQLite format 3\0"
        || ![1, 2].contains(&header[18])
        || ![1, 2].contains(&header[19])
    {
        return Err("database_invalid_header");
    }
    // A checkpoint file can retain WAL-mode header bytes after clean shutdown.
    // Normalize only the in-memory copy: SQLite deserialization cannot use WAL.
    header[18] = 1;
    header[19] = 1;
    let mut connection = Connection::open_in_memory().map_err(|_| "inspection_unavailable")?;
    let reader = Cursor::new(header).chain((&mut file).take(before.len() - 100));
    connection
        .deserialize_read_exact(rusqlite::MAIN_DB, reader, before.len() as usize, true)
        .map_err(|_| "database_snapshot_failed")?;
    let after = file
        .metadata()
        .map_err(|_| "database_metadata_unavailable")?;
    if before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || sidecar_present(path)?
    {
        return Err("database_changed");
    }
    connection
        .set_limit(Limit::SQLITE_LIMIT_LENGTH, 4 * 1024 * 1024)
        .map_err(|_| "inspection_unavailable")?;
    connection
        .set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 1024 * 1024)
        .map_err(|_| "inspection_unavailable")?;
    connection
        .set_limit(Limit::SQLITE_LIMIT_SCHEMA, 2048)
        .map_err(|_| "inspection_unavailable")?;
    let started = Instant::now();
    let calls = Arc::new(AtomicUsize::new(0));
    connection
        .progress_handler(
            1000,
            Some(move || {
                calls.fetch_add(1, Ordering::Relaxed) >= 5000
                    || started.elapsed() > Duration::from_secs(2)
            }),
        )
        .map_err(|_| "inspection_unavailable")?;
    connection
        .execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON; PRAGMA temp_store=MEMORY;")
        .map_err(|_| "database_schema_unavailable")?;
    let integrity = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
        .map_err(|_| "database_check_incomplete")?;
    if integrity != "ok" {
        return Err("database_integrity_failed");
    }
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| "database_schema_unavailable")?;
    let mut counts = BTreeMap::new();
    // Fixed queries only. Never return schema text, identifiers, provider data,
    // error text, account names, message content or arbitrary status strings.
    for (key, table, predicate) in [
        ("accounts", "accounts", "1"),
        ("threads", "threads", "1"),
        ("messages", "messages", "1"),
        ("drafts", "messages", "is_draft=1"),
        ("draftAttachments", "draft_attachments", "1"),
        ("outboxScheduled", "outbox", "status='scheduled'"),
        ("outboxUndoable", "outbox", "status='undoable'"),
        ("outboxSending", "outbox", "status='sending'"),
        ("outboxFailed", "outbox", "status='failed'"),
        (
            "outboxOther",
            "outbox",
            "status NOT IN ('scheduled','undoable','sending','failed')",
        ),
        ("pendingActions", "actions", "status='pending'"),
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(|_| "database_schema_unavailable")?;
        let value = if exists {
            Some(
                connection
                    .query_row(
                        &format!("SELECT COUNT(*) FROM {table} WHERE {predicate}"),
                        [],
                        |row| row.get::<_, u64>(0),
                    )
                    .map_err(|_| "database_counts_incomplete")?,
            )
        } else {
            None
        };
        counts.insert(key, value);
    }
    Ok(
        json!({"inspection":"checkpoint_snapshot","code":"ok","integrity":"ok","userVersion":user_version,"counts":counts}),
    )
}

fn write_report(path: &Path, report: &Value) -> Result<(), &'static str> {
    let bytes =
        serde_json::to_vec_pretty(report).map_err(|_| "diagnostics_serialization_failed")?;
    if bytes.len() > MAX_REPORT_BYTES {
        return Err("diagnostics_report_size_limit");
    }
    #[cfg(not(unix))]
    {
        let _ = (path, bytes);
        return Err("diagnostics_private_output_unsupported");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    "diagnostics_output_exists"
                } else {
                    "diagnostics_output_unavailable"
                }
            })?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "diagnostics_output_permissions_failed")?;
        file.write_all(&bytes)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|_| "diagnostics_output_write_failed")?;
        Ok(())
    }
}
