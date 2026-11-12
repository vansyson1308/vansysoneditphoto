//! `magicpic-cli mcp register | unregister | status`: put MagicPic's two MCP servers into an
//! agent client's own configuration, without the agent writing JSON or TOML by hand.
//!
//! Only the `magicpic` and `magicpic-app` entries are touched. The file is backed up first, read
//! as it is (a byte order mark is tolerated), merged, and written back atomically as UTF-8
//! without a byte order mark. A file that is not valid JSON/TOML is never overwritten.
//!
//! Claude Desktop rewrites its config while it runs and when it quits, so an entry written then
//! is lost. Register refuses while it runs, and `--restart` hands the work to a detached copy of
//! this program that waits for the app to quit, writes the file, and opens the app again.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::args::{Client, RegisterArgs, StatusArgs, UnregisterArgs};
use crate::output::{JsonStyle, print_json};
use crate::setup::{Config, absolute, home_dir, magicpic_home, spawn_detached, write_atomic};
use crate::{CliError, Result};

pub(crate) const SERVERS: [&str; 2] = ["magicpic", "magicpic-app"];

/// One MCP server entry: the program and its arguments.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub(crate) name: &'static str,
    pub(crate) command: PathBuf,
    pub(crate) args: Vec<String>,
}

/// The two entries for `config`: headless on the photo folder, and the bridge to the open app.
pub(crate) fn entries(config: &Config, start_app: bool) -> [Entry; 2] {
    let photos = config.photos.to_string_lossy().into_owned();
    let mut bridge = vec![
        "mcp".to_string(),
        "--bridge".into(),
        format!("127.0.0.1:{}", config.port),
        "--control-token-file".into(),
        config.token_file.to_string_lossy().into_owned(),
    ];
    if start_app {
        bridge.push("--start-app-if-needed".into());
    }
    [
        Entry {
            name: SERVERS[0],
            command: config.cli.clone(),
            args: vec!["mcp".into(), "--automation-read-root".into(), photos.clone(), "--automation-write-root".into(), photos],
        },
        Entry { name: SERVERS[1], command: config.cli.clone(), args: bridge },
    ]
}

// ---------------------------------------------------------------------------
// Where each client keeps its servers
// ---------------------------------------------------------------------------

/// Claude Desktop's config files. On Windows the Microsoft Store (MSIX) build may read a
/// redirected copy under its package folder, so every existing candidate is written.
fn claude_desktop_paths() -> Result<Vec<PathBuf>> {
    if cfg!(windows) {
        let mut paths = Vec::new();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let packages = PathBuf::from(local).join("Packages");
            if let Ok(dir) = std::fs::read_dir(&packages) {
                let mut found: Vec<PathBuf> = dir
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with("Claude_"))
                    .map(|e| e.path().join("LocalCache").join("Roaming").join("Claude"))
                    .filter(|p| p.is_dir())
                    .map(|p| p.join("claude_desktop_config.json"))
                    .collect();
                found.sort();
                paths.extend(found);
            }
        }
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home_dir()?.join("AppData").join("Roaming"));
        paths.push(appdata.join("Claude").join("claude_desktop_config.json"));
        Ok(paths)
    } else if cfg!(target_os = "macos") {
        Ok(vec![home_dir()?.join("Library").join("Application Support").join("Claude").join("claude_desktop_config.json")])
    } else {
        let base = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or(home_dir()?.join(".config"));
        Ok(vec![base.join("Claude").join("claude_desktop_config.json")])
    }
}

/// The files `client` reads, most specific first. `--config` replaces them.
pub(crate) fn config_paths(client: Client, overridden: Option<&Path>) -> Result<Vec<PathBuf>> {
    if let Some(p) = overridden {
        return Ok(vec![absolute(p)]);
    }
    Ok(match client {
        Client::ClaudeDesktop => claude_desktop_paths()?,
        // User scope: the top-level `mcpServers` of `~/.claude.json` (or `$CLAUDE_CONFIG_DIR`).
        Client::ClaudeCode => vec![match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir).join(".claude.json"),
            None => home_dir()?.join(".claude.json"),
        }],
        Client::Codex => vec![match std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir).join("config.toml"),
            None => home_dir()?.join(".codex").join("config.toml"),
        }],
    })
}

/// The files to write: the existing candidates, or the last (canonical) one when none exists.
fn write_targets(paths: &[PathBuf]) -> Vec<PathBuf> {
    let existing: Vec<PathBuf> = paths.iter().filter(|p| p.is_file()).cloned().collect();
    if existing.is_empty() { paths.last().cloned().into_iter().collect() } else { existing }
}

// ---------------------------------------------------------------------------
// Reading and merging
// ---------------------------------------------------------------------------

fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
            String::from_utf8(bytes.to_vec()).map(Some).map_err(|_| CliError::Other(format!("{} is not UTF-8 text; it was left unchanged", path.display())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CliError::Other(format!("{}: {e}", path.display()))),
    }
}

fn parse_json(path: &Path, text: Option<&str>) -> Result<Map<String, Value>> {
    match text.map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(Map::new()),
        Some(t) => match serde_json::from_str::<Value>(t) {
            Ok(Value::Object(m)) => Ok(m),
            Ok(_) => Err(CliError::Other(format!("{} does not hold a JSON object; it was left unchanged", path.display()))),
            Err(e) => Err(CliError::Other(format!("{} is not valid JSON ({e}); it was left unchanged: fix or remove it first", path.display()))),
        },
    }
}

fn json_entry(client: Client, e: &Entry) -> Value {
    let mut v = json!({"command": e.command, "args": e.args});
    if client == Client::ClaudeCode {
        v["type"] = json!("stdio");
        v["env"] = json!({});
    }
    v
}

/// `text` (a JSON client config) with `entries` set (or, when `None`, both removed).
pub(crate) fn merge_json(client: Client, path: &Path, text: Option<&str>, entries: Option<&[Entry]>) -> Result<String> {
    let mut root = parse_json(path, text)?;
    let servers = root.entry("mcpServers").or_insert_with(|| json!({}));
    let Value::Object(servers) = servers else {
        return Err(CliError::Other(format!("`mcpServers` in {} is not an object; it was left unchanged", path.display())));
    };
    match entries {
        Some(entries) => {
            for e in entries {
                servers.insert(e.name.into(), json_entry(client, e));
            }
        }
        None => {
            for name in SERVERS {
                servers.remove(name);
            }
        }
    }
    let mut out = serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| CliError::Other(e.to_string()))?;
    out.push('\n');
    Ok(out)
}

/// `text` (Codex's `config.toml`) with `entries` set or removed, keeping its comments and layout.
pub(crate) fn merge_toml(path: &Path, text: Option<&str>, entries: Option<&[Entry]>) -> Result<String> {
    use toml_edit::{Array, DocumentMut, Item, Table, value};
    let mut doc: DocumentMut = text
        .unwrap_or("")
        .parse()
        .map_err(|e| CliError::Other(format!("{} is not valid TOML ({e}); it was left unchanged: fix or remove it first", path.display())))?;
    if !doc.contains_key("mcp_servers") {
        let mut t = Table::new();
        t.set_implicit(true);
        doc.insert("mcp_servers", Item::Table(t));
    }
    let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_mut) else {
        return Err(CliError::Other(format!("`mcp_servers` in {} is not a table; it was left unchanged", path.display())));
    };
    match entries {
        Some(entries) => {
            for e in entries {
                let mut t = Table::new();
                t.insert("command", value(e.command.to_string_lossy().into_owned()));
                let mut args = Array::new();
                for a in &e.args {
                    args.push(a.as_str());
                }
                t.insert("args", value(args));
                // Codex's 60 s default is too short for some filters on large photos.
                t.insert("tool_timeout_sec", value(300));
                servers.insert(e.name, Item::Table(t));
            }
        }
        None => {
            for name in SERVERS {
                servers.remove(name);
            }
        }
    }
    Ok(doc.to_string())
}

fn merge(client: Client, path: &Path, text: Option<&str>, entries: Option<&[Entry]>) -> Result<String> {
    match client {
        Client::Codex => merge_toml(path, text, entries),
        _ => merge_json(client, path, text, entries),
    }
}

/// What `client`'s file says about our two servers: `{name: {command, args} | null}`.
pub(crate) fn read_entries(client: Client, path: &Path, text: &str) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    match client {
        Client::Codex => {
            let doc: toml_edit::DocumentMut = text.parse().map_err(|e| CliError::Other(format!("{} is not valid TOML ({e})", path.display())))?;
            for name in SERVERS {
                let t = doc.get("mcp_servers").and_then(|s| s.get(name));
                let entry = t.map(|t| {
                    let command = t.get("command").and_then(|c| c.as_str()).unwrap_or("");
                    let args: Vec<&str> = t.get("args").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
                    json!({"command": command, "args": args})
                });
                out.insert(name.into(), entry.unwrap_or(Value::Null));
            }
        }
        _ => {
            let root = parse_json(path, Some(text))?;
            for name in SERVERS {
                let e = root.get("mcpServers").and_then(|s| s.get(name)).map(|e| json!({"command": e.get("command"), "args": e.get("args")}));
                out.insert(name.into(), e.unwrap_or(Value::Null));
            }
        }
    }
    Ok(out)
}

/// The paths an entry names that are missing: its program, and the folder after a root flag or
/// the token file's folder.
pub(crate) fn missing_paths(entry: &Value) -> Vec<String> {
    let mut missing = Vec::new();
    if let Some(cmd) = entry.get("command").and_then(Value::as_str)
        && !Path::new(cmd).is_file()
    {
        missing.push(cmd.to_string());
    }
    let args: Vec<&str> = entry.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    for pair in args.windows(2) {
        let check = match pair[0] {
            "--automation-read-root" | "--automation-write-root" => Some(PathBuf::from(pair[1])),
            // The app writes the token when it starts; its folder must exist.
            "--control-token-file" => Path::new(pair[1]).parent().map(Path::to_path_buf),
            _ => None,
        };
        if let Some(p) = check
            && !p.exists()
        {
            let p = p.to_string_lossy().into_owned();
            if !missing.contains(&p) {
                missing.push(p);
            }
        }
    }
    missing
}

/// A copy of `path` before it is changed: `<name>.magicpic-backup-<UTC time>`.
fn backup(path: &Path) -> Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let name = format!("{}.magicpic-backup-{stamp}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let target = path.with_file_name(name);
    std::fs::copy(path, &target).map_err(|e| CliError::Other(format!("backing up {}: {e}", path.display())))?;
    Ok(Some(target))
}

/// Merge `entries` (or remove ours) into every target file. Returns `[{path, backup}]`.
fn write_all(client: Client, paths: &[PathBuf], entries: Option<&[Entry]>) -> Result<Vec<Value>> {
    let targets = write_targets(paths);
    // Everything is merged before anything is written, so one bad file changes nothing.
    let mut planned = Vec::new();
    for path in &targets {
        let text = read_text(path)?;
        planned.push((path.clone(), merge(client, path, text.as_deref(), entries)?));
    }
    let mut written = Vec::new();
    for (path, text) in planned {
        let saved = backup(&path)?;
        write_atomic(&path, text.as_bytes())?;
        written.push(json!({"path": path, "backup": saved}));
    }
    Ok(written)
}

// ---------------------------------------------------------------------------
// Claude Desktop: running, waiting, relaunching
// ---------------------------------------------------------------------------

/// A running Claude Desktop, with its program path when the system tells it.
#[derive(Debug, Clone)]
pub(crate) struct Running {
    pub(crate) path: Option<PathBuf>,
}

/// Whether a `claude.exe` at `path` is the desktop app rather than the Claude Code CLI, which
/// has the same name on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_desktop_exe(path: &str) -> bool {
    let p = path.to_ascii_lowercase().replace('/', "\\");
    if p.is_empty() {
        return true;
    }
    !(p.contains("\\.local\\") || p.contains("node_modules") || p.contains("\\npm\\") || p.contains("\\.bun\\"))
        && (p.contains("\\windowsapps\\claude_") || p.contains("anthropicclaude") || p.contains("\\programs\\claude\\") || p.ends_with("\\claude\\claude.exe"))
}

/// The running Claude Desktop processes.
pub(crate) fn claude_desktop_running() -> Vec<Running> {
    #[cfg(windows)]
    {
        // A fixed one-liner: property reads work in Constrained Language Mode.
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-Process -Name claude -ErrorAction SilentlyContinue | ForEach-Object { '[' + $_.Path + ']' }",
            ])
            .output();
        let Ok(out) = out else { return Vec::new() };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.trim().strip_prefix('[').and_then(|l| l.strip_suffix(']')))
            .filter(|p| is_desktop_exe(p))
            .map(|p| Running { path: (!p.is_empty()).then(|| PathBuf::from(p)) })
            .collect()
    }
    #[cfg(not(windows))]
    {
        let name = if cfg!(target_os = "macos") { "Claude" } else { "claude-desktop" };
        match Command::new("pgrep").args(["-x", name]).output() {
            Ok(out) if out.status.success() => vec![Running { path: None }],
            _ => Vec::new(),
        }
    }
}

/// The MSIX package family (`Claude_pzs8sxrjxfjjc`) from an install folder such as
/// `C:\Program Files\WindowsApps\Claude_1.0.0.0_x64__pzs8sxrjxfjjc\app\claude.exe`.
pub(crate) fn msix_family(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    let at = lower.find("\\windowsapps\\")? + "\\windowsapps\\".len();
    let folder = path.get(at..)?.split('\\').next()?;
    let (name, rest) = folder.split_once('_')?;
    let publisher = rest.rsplit("__").next().filter(|p| !p.is_empty() && *p != rest)?;
    Some(format!("{name}_{publisher}"))
}

/// The Store build's family when the process path is unknown.
const DEFAULT_MSIX_FAMILY: &str = "Claude_pzs8sxrjxfjjc";

/// Open Claude Desktop the way the system does. A Store (MSIX) app can't be run from its
/// folder; it starts through its application user model id.
pub(crate) fn relaunch(was: Option<&Running>) -> Result<String> {
    if cfg!(windows) {
        let path = was.and_then(|r| r.path.clone());
        let text = path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(p) = path.as_ref().filter(|_| !text.to_ascii_lowercase().contains("\\windowsapps\\")) {
            spawn_detached(&mut Command::new(p)).map_err(|e| CliError::Other(format!("could not start {}: {e}", p.display())))?;
            return Ok(p.to_string_lossy().into_owned());
        }
        let family = msix_family(&text).unwrap_or_else(|| DEFAULT_MSIX_FAMILY.into());
        let target = format!(r"shell:AppsFolder\{family}!Claude");
        spawn_detached(Command::new("explorer.exe").arg(&target)).map_err(|e| CliError::Other(format!("could not start Claude ({target}): {e}")))?;
        Ok(target)
    } else if cfg!(target_os = "macos") {
        Command::new("open").args(["-a", "Claude"]).status().map_err(|e| CliError::Other(format!("open -a Claude: {e}")))?;
        Ok("open -a Claude".into())
    } else {
        Err(CliError::Other("start Claude Desktop again yourself".into()))
    }
}

fn log_line(log: &Path, text: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        let _ = writeln!(f, "{} {text}", chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ"));
    }
}

/// The detached half of `--restart`: wait for Claude Desktop to quit, write, open it again.
fn wait_write_relaunch(paths: &[PathBuf], entries: Option<&[Entry]>, timeout: Duration, log: &Path) -> Result {
    let was = claude_desktop_running();
    log_line(log, &format!("waiting up to {}s for Claude Desktop to quit", timeout.as_secs()));
    let t = Instant::now();
    while !claude_desktop_running().is_empty() {
        if t.elapsed() > timeout {
            log_line(log, "Claude Desktop is still running: nothing was written");
            return Err(CliError::Other("Claude Desktop did not quit in time; nothing was written".into()));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    // Claude writes its config as it quits: let that finish before ours.
    std::thread::sleep(Duration::from_secs(2));
    match write_all(Client::ClaudeDesktop, paths, entries) {
        Ok(written) => log_line(log, &format!("wrote {}", Value::Array(written))),
        Err(e) => {
            log_line(log, &format!("error: {e}"));
            return Err(e);
        }
    }
    match relaunch(was.first()) {
        Ok(how) => log_line(log, &format!("reopened Claude Desktop ({how})")),
        Err(e) => log_line(log, &format!("could not reopen Claude Desktop: {e}")),
    }
    Ok(())
}

/// Shared by register and unregister for Claude Desktop: write now, refuse, or hand off.
fn desktop_write(a: &crate::args::ClientArgs, paths: &[PathBuf], entries: Option<&[Entry]>, args_for_child: Vec<String>) -> Result<Value> {
    let running = claude_desktop_running();
    let log = magicpic_home()?.join("register.log");
    if a.detached_wait {
        wait_write_relaunch(paths, entries, Duration::from_secs(a.timeout.unwrap_or(600)), &log)?;
        return Ok(json!({"status": "written"}));
    }
    if running.is_empty() || a.force {
        let written = write_all(Client::ClaudeDesktop, paths, entries)?;
        let mut v = json!({"status": "written", "files": written, "claudeDesktopRunning": !running.is_empty()});
        if a.restart && running.is_empty() {
            v["relaunched"] = json!(relaunch(None).ok());
        }
        return Ok(v);
    }
    if !a.restart {
        return Err(CliError::Other(
            "Claude Desktop is running and rewrites its config when it quits, so an entry written now would be lost. Nothing was written. \
             Run this again with --restart (it waits for Claude to quit, writes the file and opens Claude again), or quit Claude first"
                .into(),
        ));
    }
    std::fs::create_dir_all(magicpic_home()?).map_err(|e| CliError::Other(e.to_string()))?;
    let mut child = Command::new(crate::setup::current_cli());
    child.args(args_for_child).arg("--detached-wait");
    let pid = spawn_detached(&mut child).map_err(|e| CliError::Other(format!("could not start the waiting helper: {e}")))?.id();
    Ok(json!({
        "status": "waiting",
        "helperPid": pid,
        "log": log,
        "message": "Claude Desktop is running. Ask the person to quit it completely (tray or menu-bar icon › Quit, not just close the window). \
                    MagicPic then writes the config and opens Claude again within a few seconds; the servers are there in the new window.",
    }))
}

/// The child's command line: everything after `magicpic-cli`, minus flags it re-adds.
fn child_args(verb: &str, a: &crate::args::ClientArgs, extra: &[String]) -> Vec<String> {
    let mut v = vec!["mcp".to_string(), verb.to_string(), "--client".into(), a.client.name().into(), "--restart".into()];
    if let Some(c) = &a.config {
        v.push("--config".into());
        v.push(absolute(c).to_string_lossy().into_owned());
    }
    if let Some(t) = a.timeout {
        v.push("--timeout".into());
        v.push(t.to_string());
    }
    v.extend(extra.iter().cloned());
    v
}

/// `mcp register`.
pub(crate) fn register(a: &RegisterArgs, out: &mut dyn Write) -> Result {
    let home = magicpic_home()?;
    let mut config = Config::load_or_default(&home)?;
    let mut extra = Vec::new();
    if let Some(p) = &a.photos {
        config.photos = absolute(p);
        extra.extend(["--photos".to_string(), config.photos.to_string_lossy().into_owned()]);
    }
    if let Some(port) = a.port {
        config.port = port;
        extra.extend(["--port".to_string(), port.to_string()]);
    }
    if a.no_start_app {
        extra.push("--no-start-app".into());
    }
    let entries = entries(&config, !a.no_start_app);
    if !config.cli.is_file() {
        return Err(CliError::Other(format!("{} does not exist: run `magicpic-cli setup` with the installed CLI", config.cli.display())));
    }
    let paths = config_paths(a.client.client, a.client.config.as_deref())?;
    let report = match a.client.client {
        Client::ClaudeDesktop => desktop_write(&a.client, &paths, Some(&entries), child_args("register", &a.client, &extra))?,
        client => json!({"status": "written", "files": write_all(client, &paths, Some(&entries))?}),
    };
    let mut report = report;
    report["client"] = json!(a.client.client.name());
    report["servers"] = Value::Array(entries.iter().map(|e| json!({"name": e.name, "command": e.command, "args": e.args})).collect());
    report["next"] = json!(match a.client.client {
        Client::ClaudeDesktop => "the servers load when Claude Desktop starts; then run `magicpic-cli doctor`",
        Client::ClaudeCode => "restart Claude Code (or run /mcp) so it loads the servers; `claude mcp list` shows them",
        Client::Codex => "restart Codex so it loads the servers; `codex mcp list` shows them",
    });
    print_json(out, &report, JsonStyle::Pretty)
}

/// `mcp unregister`.
pub(crate) fn unregister(a: &UnregisterArgs, out: &mut dyn Write) -> Result {
    let paths = config_paths(a.client.client, a.client.config.as_deref())?;
    let mut report = match a.client.client {
        Client::ClaudeDesktop => desktop_write(&a.client, &paths, None, child_args("unregister", &a.client, &[]))?,
        client => json!({"status": "written", "files": write_all(client, &paths, None)?}),
    };
    report["client"] = json!(a.client.client.name());
    report["removed"] = json!(SERVERS);
    print_json(out, &report, JsonStyle::Pretty)
}

/// One client's registration, as `mcp status` and `doctor` report it.
pub(crate) fn client_status(client: Client, overridden: Option<&Path>) -> Value {
    let paths = match config_paths(client, overridden) {
        Ok(p) => p,
        Err(e) => return json!({"client": client.name(), "error": e.to_string()}),
    };
    let mut files = Vec::new();
    for path in &paths {
        let mut f = json!({"path": path, "exists": path.is_file()});
        match read_text(path) {
            Ok(Some(text)) => match read_entries(client, path, &text) {
                Ok(found) => {
                    let servers: Map<String, Value> = found
                        .into_iter()
                        .map(|(name, e)| {
                            let v = if e.is_null() {
                                json!({"registered": false})
                            } else {
                                json!({"registered": true, "command": e["command"], "args": e["args"], "missingPaths": missing_paths(&e)})
                            };
                            (name, v)
                        })
                        .collect();
                    f["servers"] = Value::Object(servers);
                }
                Err(e) => f["error"] = json!(e.to_string()),
            },
            Ok(None) => {}
            Err(e) => f["error"] = json!(e.to_string()),
        }
        files.push(f);
    }
    let registered = files.iter().any(|f| SERVERS.iter().all(|n| f["servers"][n]["registered"] == true));
    let healthy = files
        .iter()
        .any(|f| SERVERS.iter().all(|n| f["servers"][n]["registered"] == true && f["servers"][n]["missingPaths"].as_array().is_some_and(Vec::is_empty)));
    let mut v = json!({"client": client.name(), "registered": registered, "pathsOk": healthy, "files": files});
    if client == Client::ClaudeDesktop && overridden.is_none() {
        v["running"] = json!(!claude_desktop_running().is_empty());
    }
    v
}

/// `mcp status`.
pub(crate) fn status(a: &StatusArgs, out: &mut dyn Write) -> Result {
    let clients: Vec<Client> = match a.client {
        Some(c) => vec![c],
        None => vec![Client::ClaudeDesktop, Client::ClaudeCode, Client::Codex],
    };
    let report: Vec<Value> = clients.into_iter().map(|c| client_status(c, a.config.as_deref())).collect();
    print_json(out, &json!({"clients": report}), JsonStyle::Pretty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            version: 1,
            cli: "/opt/m/magicpic-cli".into(),
            app: "/opt/m/magicpic".into(),
            photos: "/home/a/Pictures/MagicPic".into(),
            port: 7878,
            token_file: "/home/a/.magicpic/control.token".into(),
            launcher: "/home/a/.magicpic/start-magicpic.sh".into(),
        }
    }

    #[test]
    fn json_merge_keeps_other_servers_and_keys() {
        let existing =
            r#"{"globalShortcut":"Ctrl+Space","mcpServers":{"other":{"command":"x","args":["y"]},"magicpic":{"command":"old"}},"preferences":{"a":1}}"#;
        let e = entries(&config(), true);
        let merged = merge_json(Client::ClaudeDesktop, Path::new("c.json"), Some(existing), Some(&e)).unwrap();
        let v: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["globalShortcut"], "Ctrl+Space");
        assert_eq!(v["preferences"], json!({"a": 1}));
        assert_eq!(v["mcpServers"]["other"], json!({"command": "x", "args": ["y"]}));
        assert_eq!(v["mcpServers"]["magicpic"]["command"], "/opt/m/magicpic-cli");
        assert_eq!(
            v["mcpServers"]["magicpic-app"]["args"],
            json!(["mcp", "--bridge", "127.0.0.1:7878", "--control-token-file", "/home/a/.magicpic/control.token", "--start-app-if-needed"])
        );
        assert!(!merged.starts_with('\u{feff}') && merged.ends_with('\n'));

        let removed = merge_json(Client::ClaudeDesktop, Path::new("c.json"), Some(&merged), None).unwrap();
        let v: Value = serde_json::from_str(&removed).unwrap();
        assert_eq!(v["mcpServers"], json!({"other": {"command": "x", "args": ["y"]}}));
        assert_eq!(v["globalShortcut"], "Ctrl+Space");
    }

    #[test]
    fn claude_code_entries_are_stdio() {
        let e = entries(&config(), false);
        let merged = merge_json(Client::ClaudeCode, Path::new(".claude.json"), Some(r#"{"numStartups":3}"#), Some(&e)).unwrap();
        let v: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["numStartups"], 3);
        assert_eq!(v["mcpServers"]["magicpic"]["type"], "stdio");
        assert!(!v["mcpServers"]["magicpic-app"]["args"].as_array().unwrap().contains(&json!("--start-app-if-needed")));
    }

    #[test]
    fn invalid_files_are_refused_not_replaced() {
        let e = entries(&config(), true);
        for bad in ["{not json", "[1,2]", r#"{"mcpServers": 3}"#] {
            assert!(merge_json(Client::ClaudeDesktop, Path::new("c.json"), Some(bad), Some(&e)).is_err(), "{bad}");
        }
        assert!(merge_toml(Path::new("c.toml"), Some("[x\n"), Some(&e)).is_err());
        assert!(merge_toml(Path::new("c.toml"), Some("mcp_servers = 3\n"), Some(&e)).is_err());
    }

    #[test]
    fn toml_merge_keeps_comments_and_other_servers() {
        let existing = "# my settings\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"x\" # keep\n";
        let e = entries(&config(), true);
        let merged = merge_toml(Path::new("c.toml"), Some(existing), Some(&e)).unwrap();
        assert!(merged.starts_with("# my settings\nmodel = \"o3\""), "{merged}");
        assert!(merged.contains("command = \"x\" # keep"), "{merged}");
        let doc: toml_edit::DocumentMut = merged.parse().unwrap();
        assert_eq!(doc["mcp_servers"]["magicpic"]["tool_timeout_sec"].as_integer(), Some(300));
        assert_eq!(doc["mcp_servers"]["magicpic-app"]["command"].as_str(), Some("/opt/m/magicpic-cli"));
        let found = read_entries(Client::Codex, Path::new("c.toml"), &merged).unwrap();
        assert_eq!(found["magicpic"]["args"][0], "mcp");
        let removed = merge_toml(Path::new("c.toml"), Some(&merged), None).unwrap();
        let doc: toml_edit::DocumentMut = removed.parse().unwrap();
        assert!(doc["mcp_servers"].get("magicpic").is_none() && doc["mcp_servers"].get("other").is_some());
    }

    #[test]
    fn write_all_backs_up_merges_and_writes_without_bom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude_desktop_config.json");
        // PowerShell's `Set-Content -Encoding UTF8` wrote a BOM before: still readable.
        std::fs::write(&path, b"\xEF\xBB\xBF{\"mcpServers\":{\"keep\":{\"command\":\"k\"}}}").unwrap();
        let e = entries(&config(), true);
        let written = write_all(Client::ClaudeDesktop, std::slice::from_ref(&path), Some(&e)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.starts_with(b"\xEF\xBB\xBF"), "no byte order mark");
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["mcpServers"]["keep"]["command"], "k");
        assert!(v["mcpServers"]["magicpic"].is_object());
        let backup = PathBuf::from(written[0]["backup"].as_str().unwrap());
        assert!(backup.file_name().unwrap().to_string_lossy().starts_with("claude_desktop_config.json.magicpic-backup-"));
        assert!(std::fs::read(&backup).unwrap().starts_with(b"\xEF\xBB\xBF{"), "the backup is the original, byte for byte");

        // A file that isn't JSON is left alone, and no backup is made of it.
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{oops").unwrap();
        assert!(write_all(Client::ClaudeDesktop, std::slice::from_ref(&broken), Some(&e)).is_err());
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), "{oops");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3, "config, its backup and the broken file");
    }

    #[test]
    fn only_existing_candidates_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.json"), dir.path().join("b.json"));
        assert_eq!(write_targets(&[a.clone(), b.clone()]), vec![b.clone()], "none exists: the canonical last one");
        std::fs::write(&a, "{}").unwrap();
        assert_eq!(write_targets(&[a.clone(), b.clone()]), vec![a.clone()]);
        std::fs::write(&b, "{}").unwrap();
        assert_eq!(write_targets(&[a.clone(), b.clone()]), vec![a, b]);
    }

    #[test]
    fn status_reports_missing_paths() {
        let entry = json!({"command": "/nope/magicpic-cli", "args": ["mcp", "--automation-read-root", "/nope/photos", "--control-token-file", "/nope/dir/control.token"]});
        assert_eq!(missing_paths(&entry), ["/nope/magicpic-cli", "/nope/photos", "/nope/dir"]);
        let dir = tempfile::tempdir().unwrap();
        let ok = json!({"command": std::env::current_exe().unwrap(), "args": ["--automation-read-root", dir.path()]});
        assert!(missing_paths(&ok).is_empty());
    }

    #[test]
    fn windows_process_paths_tell_the_desktop_app_from_claude_code() {
        assert!(is_desktop_exe(r"C:\Program Files\WindowsApps\Claude_1.2.3.0_x64__pzs8sxrjxfjjc\app\claude.exe"));
        assert!(is_desktop_exe(r"C:\Users\a\AppData\Local\AnthropicClaude\app-0.9.0\claude.exe"));
        assert!(!is_desktop_exe(r"C:\Users\a\.local\bin\claude.exe"));
        assert!(!is_desktop_exe(r"C:\Users\a\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\claude.exe"));
        assert_eq!(msix_family(r"C:\Program Files\WindowsApps\Claude_1.2.3.0_x64__pzs8sxrjxfjjc\app\claude.exe").as_deref(), Some("Claude_pzs8sxrjxfjjc"));
        assert_eq!(msix_family(r"C:\Users\a\AppData\Local\AnthropicClaude\claude.exe"), None);
        assert_eq!(msix_family(r"C:\Program Files\WindowsApps\Claude"), None);
    }
}
