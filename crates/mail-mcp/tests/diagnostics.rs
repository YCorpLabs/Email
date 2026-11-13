use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

const PRIVATE: &str = "private-account-secret-abcdef@example.invalid";
fn execute(db: &Path, output: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mail-mcp"))
        .env_clear()
        .args(["--db"])
        .arg(db)
        .args(["--diagnostics"])
        .arg(output)
        .args(extra)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn fixture(db: &Path) {
    let connection = Connection::open(db).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE accounts(id TEXT,email TEXT);
        CREATE TABLE threads(id TEXT,subject TEXT);
        CREATE TABLE messages(id TEXT,subject TEXT,body_plain TEXT,sender TEXT,is_draft INTEGER);
        CREATE TABLE draft_attachments(bytes BLOB);
        CREATE TABLE outbox(status TEXT,last_error TEXT);
        CREATE TABLE actions(status TEXT,action_json TEXT);
        PRAGMA user_version=7;",
        )
        .unwrap();
    connection
        .execute("INSERT INTO accounts VALUES(?1,?1)", [PRIVATE])
        .unwrap();
    connection
        .execute("INSERT INTO threads VALUES(?1,?1)", [PRIVATE])
        .unwrap();
    connection
        .execute("INSERT INTO messages VALUES(?1,?1,?1,?1,1)", [PRIVATE])
        .unwrap();
    connection
        .execute(
            "INSERT INTO draft_attachments VALUES(?1)",
            [PRIVATE.as_bytes()],
        )
        .unwrap();
    connection
        .execute("INSERT INTO outbox VALUES(?1,?1)", [PRIVATE])
        .unwrap();
    connection
        .execute("INSERT INTO actions VALUES('pending',?1)", [PRIVATE])
        .unwrap();
}

#[test]
fn cli_export_is_private_local_readonly_and_excludes_sensitive_values() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("private-user-store.sqlite");
    fixture(&db);
    let config = db.with_extension("accounts.json");
    fs::write(&config, format!("not JSON: {PRIVATE}")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config, fs::Permissions::from_mode(0o640)).unwrap();
    }
    let before = fs::read(&db).unwrap();
    let output = directory.path().join("diagnostics.json");
    let result = execute(&db, &output, &[]);
    assert!(result.status.success(), "{:?}", result.stderr);
    let bytes = fs::read(&output).unwrap();
    assert!(bytes.len() < 16 * 1024);
    let text = String::from_utf8(bytes).unwrap();
    for forbidden in [
        PRIVATE,
        "private-user-store",
        directory.path().to_str().unwrap(),
        "not JSON",
        "CREATE TABLE",
        "last_error",
    ] {
        assert!(!text.contains(forbidden), "leaked {forbidden}");
    }
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(report["localOnly"], true);
    assert_eq!(report["database"]["integrity"], "ok");
    assert_eq!(report["database"]["userVersion"], 7);
    for key in [
        "accounts",
        "messages",
        "threads",
        "drafts",
        "draftAttachments",
        "outboxOther",
        "pendingActions",
    ] {
        assert_eq!(report["database"]["counts"][key], 1, "{key}");
    }
    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "diagnostics must not run migrations or write the cache"
    );
    assert!(!db.with_extension("sqlite-wal").exists());
    assert!(!db.with_extension("sqlite-shm").exists());
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        format!("not JSON: {PRIVATE}")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("Nothing was uploaded")
    );
    assert!(result.stderr.is_empty());
}

#[test]
fn output_creation_never_overwrites_or_follows_a_symlink() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("store.sqlite");
    fixture(&db);
    let existing = directory.path().join("existing.json");
    fs::write(&existing, PRIVATE).unwrap();
    let result = execute(&db, &existing, &[]);
    assert!(!result.status.success());
    assert_eq!(fs::read_to_string(&existing).unwrap(), PRIVATE);
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("diagnostics_output_exists")
    );
    #[cfg(unix)]
    {
        let symlink = directory.path().join("linked.json");
        std::os::unix::fs::symlink(&existing, &symlink).unwrap();
        assert!(!execute(&db, &symlink, &[]).status.success());
        assert_eq!(fs::read_to_string(&existing).unwrap(), PRIVATE);
    }
}

#[test]
fn active_wal_is_not_opened_or_modified_and_missing_database_is_not_created() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("store.sqlite");
    let connection = Connection::open(&db).unwrap();
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE private_table(secret); INSERT INTO private_table VALUES('private-value');").unwrap();
    let wal = directory.path().join("store.sqlite-wal");
    let shm = directory.path().join("store.sqlite-shm");
    let before = (
        fs::read(&db).unwrap(),
        fs::read(&wal).unwrap(),
        fs::read(&shm).unwrap(),
    );
    let output = directory.path().join("wal.json");
    assert!(execute(&db, &output, &[]).status.success());
    let report: Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(report["database"]["code"], "database_active_journal");
    assert!(report["database"].get("counts").is_none());
    assert_eq!(
        (
            fs::read(&db).unwrap(),
            fs::read(&wal).unwrap(),
            fs::read(&shm).unwrap()
        ),
        before
    );
    let absent = directory.path().join("missing.sqlite");
    let output = directory.path().join("missing.json");
    assert!(execute(&absent, &output, &[]).status.success());
    assert!(!absent.exists());
    let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(report["database"]["code"], "database_unavailable");
}

#[test]
fn corrupt_large_and_symlink_databases_fail_closed_with_fixed_codes() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("private-corrupt.sqlite");
    fs::write(&db, PRIVATE.repeat(4)).unwrap();
    let output = directory.path().join("corrupt.json");
    assert!(execute(&db, &output, &[]).status.success());
    let text = fs::read_to_string(&output).unwrap();
    assert!(!text.contains(PRIVATE));
    assert!(!text.contains(directory.path().to_str().unwrap()));
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(report["database"]["code"], "database_invalid_header");
    let large = directory.path().join("large.sqlite");
    fs::File::create(&large)
        .unwrap()
        .set_len(128 * 1024 * 1024 + 1)
        .unwrap();
    let output = directory.path().join("large.json");
    assert!(execute(&large, &output, &[]).status.success());
    let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(report["database"]["code"], "database_size_limit");
    #[cfg(unix)]
    {
        let linked = directory.path().join("linked.sqlite");
        std::os::unix::fs::symlink(&db, &linked).unwrap();
        let output = directory.path().join("linked.json");
        assert!(execute(&linked, &output, &[]).status.success());
        let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(report["database"]["code"], "database_unavailable");
    }
}

#[test]
fn diagnostics_cannot_be_combined_with_mcp_or_provider_permissions() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("missing.sqlite");
    for (index, flag) in [
        "--allow-write",
        "--allow-send",
        "--allow-sync",
        "--seed-demo",
        "--dispatch-due",
        "--account",
    ]
    .iter()
    .enumerate()
    {
        let output = directory.path().join(format!("refused-{index}.json"));
        let result = execute(&db, &output, &[flag]);
        assert!(!result.status.success());
        assert!(!output.exists());
        assert!(!db.exists());
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(error.contains("diagnostics_invalid_arguments"));
        assert!(!error.contains(directory.path().to_str().unwrap()));
    }
}

#[test]
fn malformed_diagnostic_flag_does_not_echo_private_argument_text() {
    let result = Command::new(env!("CARGO_BIN_EXE_mail-mcp"))
        .env_clear()
        .arg(format!("--diagnostics={PRIVATE}"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(error.contains("diagnostics_invalid_arguments"));
    assert!(!error.contains(PRIVATE));
}
