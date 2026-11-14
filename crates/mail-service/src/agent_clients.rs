//! "Connect an agent" in Agent access: find the Claude Code and Codex CLIs,
//! check whether Email's MCP helper is registered in them, and register it,
//! so people with a packaged app never need `scripts/connect-mcp.sh`.
//!
//! These are desktop-only operations. mail-mcp's access policy has no rule
//! for them, so an agent can't call them (they fail closed there).
//!
//! The registration runs the same commands as `scripts/connect-mcp.sh`, under
//! the profile's default name (`MCP_SERVER_NAME`), and never replaces an
//! existing registration. It passes every mail permission flag, so the
//! per-account grants in Agent access are the one place the user decides:
//! every account stays off until the user turns it on there, and then only
//! reading is on until they check more. Scheduling settings
//! (`--allow-booking-settings`) are left out: they aren't tied to an account,
//! have no grant in the app, and have their own browser approval.
use crate::profile;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Flags an in-app registration passes after `--db PATH`.
pub(crate) const REGISTRATION_FLAGS: [&str; 5] = [
    "--allow-draft",
    "--allow-triage",
    "--allow-send",
    "--allow-sync",
    "--allow-calendar-write",
];
const THIS_COMPUTER: &str = if cfg!(target_os = "macos") { "this Mac" } else { "this computer" };
const GET_TIMEOUT: Duration = Duration::from_secs(20);
const ADD_TIMEOUT: Duration = Duration::from_secs(60);

struct Client {
    id: &'static str,
    name: &'static str,
    binary: &'static str,
}

const CLIENTS: [Client; 2] = [
    Client { id: "claude", name: "Claude Code", binary: "claude" },
    Client { id: "codex", name: "Codex", binary: "codex" },
];

/// Where the CLIs are looked for. Apps started from the Dock or a desktop
/// launcher don't get the login shell's PATH, so the usual install locations
/// are added to whatever PATH the app has. `EMAIL_AGENT_CLI_PATH` replaces the
/// whole list; it exists for tests, which must never find the real CLIs.
fn search_dirs() -> Vec<PathBuf> {
    if let Some(dirs) = std::env::var_os("EMAIL_AGENT_CLI_PATH") {
        return std::env::split_paths(&dirs).collect();
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = home() {
        for relative in [
            ".local/bin",
            ".claude/local",
            "bin",
            ".npm-global/bin",
            ".bun/bin",
            ".volta/bin",
            "Library/pnpm",
            ".local/share/pnpm",
        ] {
            dirs.push(home.join(relative));
        }
        // nvm keeps node and globally installed CLIs per node version.
        if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            let mut versions: Vec<PathBuf> = entries.flatten().map(|e| e.path().join("bin")).collect();
            versions.sort();
            dirs.extend(versions.into_iter().rev());
        }
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/snap/bin"] {
        dirs.push(dir.into());
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| dir.is_absolute() && seen.insert(dir.clone()));
    dirs
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute())
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn find(binary: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|dir| dir.join(binary)).find(|path| executable(path))
}

struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Run a CLI with a time limit. Its own directory and the search list go on
/// PATH, so an npm-installed CLI finds `node` even from a GUI app.
fn run(cli: &Path, args: &[OsString], dirs: &[PathBuf], timeout: Duration) -> Result<Output, String> {
    let mut path: Vec<PathBuf> = cli.parent().map(Path::to_path_buf).into_iter().collect();
    path.extend(dirs.iter().cloned());
    if let Some(existing) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&existing));
    }
    let name = cli.file_name().and_then(|n| n.to_str()).unwrap_or("the agent").to_string();
    let mut child = Command::new(cli)
        .args(args)
        .env("PATH", std::env::join_paths(path).map_err(|e| e.to_string())?)
        // Claude Code keeps project-scoped servers per directory; run from home
        // so `mcp get` sees the same servers a terminal there would.
        .current_dir(home().unwrap_or_else(|| PathBuf::from("/")))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start {name}: {e}"))?;
    // Read on threads, and stop waiting shortly after the CLI exits: a child it
    // left running (a server it started to check) could hold the pipe open.
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            let _ = sender.send(text);
        });
        receiver
    };
    let stdout = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let stderr = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{name} didn't answer within {} seconds.", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let grace = Duration::from_secs(2);
    Ok(Output {
        success: status.success(),
        stdout: stdout.recv_timeout(grace).unwrap_or_default(),
        stderr: stderr.recv_timeout(grace).unwrap_or_default(),
    })
}

/// The helper binary agents start, and why it's missing when it is.
/// `hint` comes from the shell (the app bundle's `Contents/Helpers/mail-mcp`
/// on macOS); `EMAIL_MCP_HELPER` serves development builds; then the
/// directory of the running executable; on Linux also PATH and the usual
/// install locations, where the separately downloaded helper lives.
fn helper(hint: Option<&str>, dirs: &[PathBuf]) -> Result<PathBuf, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.extend(hint.filter(|h| !h.is_empty()).map(PathBuf::from));
    candidates.extend(std::env::var_os("EMAIL_MCP_HELPER").filter(|h| !h.is_empty()).map(PathBuf::from));
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join("mail-mcp"));
        // Where the Linux packages install it: /usr/lib/email-linux/mail-mcp.
        if !cfg!(target_os = "macos") {
            candidates.push(dir.join("../lib/email-linux/mail-mcp"));
        }
    }
    if !cfg!(target_os = "macos") {
        candidates.extend(dirs.iter().map(|dir| dir.join("mail-mcp")));
    }
    let Some(found) = candidates.into_iter().find(|c| c.is_absolute() && executable(c)) else {
        return Err(if cfg!(target_os = "macos") {
            "This copy of Email doesn't include the mail-mcp helper, so it can't connect agents. Release builds include it; in a source checkout, use scripts/connect-mcp.sh.".into()
        } else {
            "To connect agents, install mail-mcp (it comes with the Email packages, or as a download with each release) in ~/.local/bin, then reopen Agent access.".into()
        });
    };
    // Register a clean path, not ".../bin/../lib/...".
    let found = if found.components().any(|c| c == std::path::Component::ParentDir) {
        std::fs::canonicalize(&found).unwrap_or(found)
    } else {
        found
    };
    // A helper built for another distribution profile uses other credentials
    // and data, so it couldn't use this app's accounts.
    let marker = format!("{}.v", profile::KEYCHAIN_NAMESPACE);
    let bytes = std::fs::read(&found).map_err(|e| format!("Couldn't read {}: {e}", found.display()))?;
    if !bytes.windows(marker.len()).any(|window| window == marker.as_bytes()) {
        return Err(format!(
            "The mail-mcp at {} was built for a different version of Email, so it can't use this app's accounts.",
            found.display()
        ));
    }
    Ok(found)
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|dir| dir.join(path)).unwrap_or_else(|_| path.to_path_buf())
    }
}

fn server_args(helper: &Path, db: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![helper.into(), "--db".into(), db.into()];
    args.extend(REGISTRATION_FLAGS.iter().map(OsString::from));
    args
}

/// `codex mcp add NAME -- …` or `claude mcp add --scope user --transport stdio NAME -- …`,
/// exactly as `scripts/connect-mcp.sh` runs them.
fn add_args(client: &str, name: &str, helper: &Path, db: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = match client {
        "claude" => ["mcp", "add", "--scope", "user", "--transport", "stdio", name, "--"]
            .map(OsString::from)
            .to_vec(),
        _ => ["mcp", "add", name, "--"].map(OsString::from).to_vec(),
    };
    args.extend(server_args(helper, db));
    args
}

fn get_args(name: &str) -> Vec<OsString> {
    ["mcp", "get", name].map(OsString::from).to_vec()
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// The command and an `mcpServers` JSON snippet for any other MCP client.
fn manual(name: &str, helper: &Path, db: &Path) -> Value {
    let args: Vec<String> = server_args(helper, db)
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let command = args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
    let config = serde_json::to_string_pretty(&json!({
        "mcpServers": {name: {"command": args[0], "args": args[1..]}}
    }))
    .unwrap_or_default();
    json!({
        "command": command,
        "json": config,
        "copy_text": format!("Command:\n{command}\n\nJSON config:\n{config}\n"),
    })
}

/// One client's row: found or not, and whether `NAME` is registered in it.
fn client_state(client: &Client, name: &str, helper: Option<&Path>, dirs: &[PathBuf]) -> Value {
    let Some(cli) = find(client.binary, dirs) else {
        return json!({
            "id": client.id,
            "name": client.name,
            "installed": false,
            "connected": false,
            "subtitle": "Not installed",
        });
    };
    let (connected, same_helper, error) = match run(&cli, &get_args(name), dirs, GET_TIMEOUT) {
        Ok(output) => {
            // Only say "another copy" when the registration names a mail-mcp
            // that isn't this one; other output formats stay unknown.
            let same = helper.and_then(|h| {
                if output.stdout.contains(&*h.to_string_lossy()) {
                    Some(true)
                } else if output.stdout.contains("mail-mcp") {
                    Some(false)
                } else {
                    None
                }
            });
            (output.success, output.success.then_some(same).flatten(), None)
        }
        Err(error) => (false, None, Some(error)),
    };
    // The row's sub line; a found client that's ready to connect needs none.
    let subtitle = match (connected, same_helper, &error) {
        (_, _, Some(_)) => "Couldn't check whether it's connected".to_string(),
        (true, Some(false), _) => "Connected to another copy of mail-mcp".to_string(),
        _ => String::new(),
    };
    json!({
        "id": client.id,
        "name": client.name,
        "installed": true,
        "path": cli,
        "connected": connected,
        "same_helper": same_helper,
        "subtitle": subtitle,
        "error": error,
    })
}

fn overview_in(db: &Path, hint: Option<&str>, dirs: &[PathBuf]) -> Value {
    let db = absolute(db);
    let name = profile::MCP_SERVER_NAME;
    let helper = helper(hint, dirs);
    let found = helper.as_ref().ok().map(PathBuf::as_path);
    let clients: Vec<Value> = std::thread::scope(|scope| {
        let rows: Vec<_> = CLIENTS
            .iter()
            .map(|client| scope.spawn(move || client_state(client, name, found, dirs)))
            .collect();
        rows.into_iter().map(|row| row.join().unwrap_or(Value::Null)).collect()
    });
    json!({
        "server_name": name,
        "db_path": db,
        "flags": REGISTRATION_FLAGS,
        "helper": found,
        "helper_problem": helper.as_ref().err(),
        "clients": clients,
        "manual": found.map(|helper| manual(name, helper, &db)),
    })
}

fn connect_in(db: &Path, hint: Option<&str>, client_id: &str, dirs: &[PathBuf]) -> Result<Value, String> {
    let client = CLIENTS.iter().find(|c| c.id == client_id).ok_or_else(|| format!("Unknown agent: {client_id}"))?;
    let db = absolute(db);
    let name = profile::MCP_SERVER_NAME;
    let helper = helper(hint, dirs)?;
    let cli = find(client.binary, dirs)
        .ok_or_else(|| format!("{} isn't installed on {THIS_COMPUTER}.", client.name))?;
    // Like the script, never replace an existing registration.
    let existing = run(&cli, &get_args(name), dirs, GET_TIMEOUT)?;
    let already = existing.success;
    if !already {
        let added = run(&cli, &add_args(client.id, name, &helper, &db), dirs, ADD_TIMEOUT)?;
        if !added.success {
            let reason = [added.stderr.trim(), added.stdout.trim()]
                .into_iter()
                .find(|text| !text.is_empty())
                .and_then(|text| text.lines().last())
                .unwrap_or("it gave no reason")
                .chars()
                .take(300)
                .collect::<String>();
            return Err(format!("{} couldn't add Email: {reason}", client.name));
        }
    }
    let mut state = client_state(client, name, Some(&helper), dirs);
    state["already_registered"] = json!(already);
    Ok(state)
}

/// Every client's state, the helper, and the manual configuration.
pub(crate) fn overview(db: &Path, hint: Option<&str>) -> Value {
    overview_in(db, hint, &search_dirs())
}

/// Register the helper in one client. Returns that client's new state.
pub(crate) fn connect(db: &Path, hint: Option<&str>, client: &str) -> Result<Value, String> {
    connect_in(db, hint, client, &search_dirs())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A fake CLI: `mcp get NAME` succeeds once `mcp add` recorded NAME, and
    /// every call is logged. Nothing real is ever run or changed.
    fn fake_cli(dir: &Path, binary: &str) {
        let script = format!(
            r#"#!/bin/sh
state="{dir}/{binary}.registered"
printf '%s\n' "$*" >> "{dir}/{binary}.log"
if [ "$1 $2" = "mcp get" ]; then
  if [ -f "$state" ] && grep -qx "$3" "$state"; then echo "$3:"; cat "{dir}/{binary}.command"; exit 0; fi
  echo "No MCP server found with name: $3" >&2; exit 1
fi
if [ "$1 $2" = "mcp add" ]; then
  name="$3"; [ "$3" = "--scope" ] && name="$7"
  echo "$name" >> "$state"; printf 'Command: %s\n' "$*" > "{dir}/{binary}.command"; exit 0
fi
exit 2
"#,
            dir = dir.display()
        );
        let path = dir.join(binary);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn fake_helper(dir: &Path, profile_marker: bool) -> PathBuf {
        let path = dir.join("mail-mcp");
        let marker = if profile_marker { format!("{}.v4", profile::KEYCHAIN_NAMESPACE) } else { "other.v4".into() };
        std::fs::write(&path, format!("#!/bin/sh\n# {marker}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("email-agent-clients-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_clients_and_registers_like_the_script() {
        let dir = temp();
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        fake_cli(&bin, "claude");
        let helper = fake_helper(&dir, true);
        let hint = helper.to_str();
        let db = dir.join("mail.sqlite");
        let dirs = vec![bin.clone()];
        let name = profile::MCP_SERVER_NAME;

        let state = overview_in(&db, hint, &dirs);
        assert_eq!(state["server_name"], name);
        assert_eq!(state["helper"], json!(helper));
        assert!(state["helper_problem"].is_null());
        assert_eq!(state["clients"][0]["id"], "claude");
        assert_eq!(state["clients"][0]["installed"], true);
        assert_eq!(state["clients"][0]["connected"], false);
        assert_eq!(state["clients"][1]["installed"], false);
        assert_eq!(state["clients"][1]["subtitle"], "Not installed");
        let copy = state["manual"]["copy_text"].as_str().unwrap();
        assert!(copy.contains("--allow-calendar-write") && copy.contains("\"mcpServers\""), "{copy}");
        assert!(!copy.contains("booking"), "{copy}");

        let connected = connect_in(&db, hint, "claude", &dirs).unwrap();
        assert_eq!(connected["connected"], true);
        assert_eq!(connected["same_helper"], true);
        assert_eq!(connected["already_registered"], false);
        let log = std::fs::read_to_string(bin.join("claude.log")).unwrap();
        let add = log.lines().find(|l| l.starts_with("mcp add")).unwrap();
        assert_eq!(
            add,
            format!(
                "mcp add --scope user --transport stdio {name} -- {} --db {} --allow-draft --allow-triage --allow-send --allow-sync --allow-calendar-write",
                helper.display(),
                db.display()
            )
        );
        // A second Connect leaves the registration alone.
        let again = connect_in(&db, hint, "claude", &dirs).unwrap();
        assert_eq!(again["already_registered"], true);
        let log = std::fs::read_to_string(bin.join("claude.log")).unwrap();
        assert_eq!(log.lines().filter(|l| l.starts_with("mcp add")).count(), 1);
        assert!(connect_in(&db, hint, "codex", &dirs).unwrap_err().contains("isn't installed"));
        assert!(connect_in(&db, hint, "other", &dirs).is_err());
        // The same name registered with another copy of the helper.
        let other = dir.join("elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        let other_helper = fake_helper(&other, true);
        let state = overview_in(&db, other_helper.to_str(), &dirs);
        assert_eq!(state["clients"][0]["connected"], true);
        assert_eq!(state["clients"][0]["same_helper"], false);
        assert_eq!(state["clients"][0]["subtitle"], "Connected to another copy of mail-mcp");

        // Codex uses its own add syntax.
        fake_cli(&bin, "codex");
        connect_in(&db, hint, "codex", &dirs).unwrap();
        let log = std::fs::read_to_string(bin.join("codex.log")).unwrap();
        assert!(log.lines().any(|l| l == format!("mcp add {name} -- {} --db {} --allow-draft --allow-triage --allow-send --allow-sync --allow-calendar-write", helper.display(), db.display())), "{log}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_helper_for_another_profile_is_refused() {
        let dir = temp();
        let helper = fake_helper(&dir, false);
        let error = helper_for_test(helper.to_str());
        assert!(error.unwrap_err().contains("different version of Email"));
        let state = overview_in(&dir.join("mail.sqlite"), helper.to_str(), &[]);
        assert!(state["manual"].is_null() && state["helper"].is_null());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn helper_for_test(hint: Option<&str>) -> Result<PathBuf, String> {
        helper(hint, &[])
    }

    #[test]
    fn quoting_and_config_survive_spaces() {
        let config = manual("email", Path::new("/Apps/Email.app/Contents/Helpers/mail-mcp"), Path::new("/Users/a/Library/Application Support/Email/mail.sqlite"));
        assert_eq!(
            config["command"],
            "/Apps/Email.app/Contents/Helpers/mail-mcp --db '/Users/a/Library/Application Support/Email/mail.sqlite' --allow-draft --allow-triage --allow-send --allow-sync --allow-calendar-write"
        );
        let parsed: Value = serde_json::from_str(config["json"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["mcpServers"]["email"]["args"][1], "/Users/a/Library/Application Support/Email/mail.sqlite");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
