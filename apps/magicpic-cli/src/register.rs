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
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::args::{Client, RegisterArgs, StatusArgs, UnregisterArgs};
use crate::output::print_json_ascii;
use crate::pending::{self, Pending};
use crate::procs::{self, Proc, ProcessTable};
use crate::setup::{Config, absolute, home_dir, magicpic_home, spawn_detached, spawn_unattached, write_atomic};
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

/// Claude Desktop's processes (main and helpers) in `table` now; `None` when the list could not
/// be read.
pub(crate) fn claude_desktop_processes(table: &mut dyn ProcessTable) -> Option<Vec<Proc>> {
    table.snapshot().map(|list| procs::desktop(&list, procs::OS))
}

/// Whether Claude Desktop runs: `None` when the process list could not be read.
pub(crate) fn claude_desktop_running() -> Option<bool> {
    claude_desktop_processes(procs::table(false).as_mut()).map(|app| !app.is_empty())
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
/// folder; it starts through its application user model id. `exe` is the main program seen
/// while it ran, when known. Under test ([`procs::FAKE_ENV`]) it adds a main process to the
/// test's process list instead.
pub(crate) fn relaunch(exe: Option<&Path>) -> Result<String> {
    if let Some(fake) = procs::FakeTable::from_env() {
        fake.start_desktop().map_err(|e| CliError::Other(format!("the test process list: {e}")))?;
        return Ok("the test process list".into());
    }
    if cfg!(windows) {
        let text = exe.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(p) = exe.filter(|_| !text.to_ascii_lowercase().contains("\\windowsapps\\")) {
            spawn_detached(Command::new(p).stdout(Stdio::null()).stderr(Stdio::null()))
                .map_err(|e| CliError::Other(format!("could not start {}: {e}", p.display())))?;
            return Ok(p.to_string_lossy().into_owned());
        }
        let family = msix_family(&text).unwrap_or_else(|| DEFAULT_MSIX_FAMILY.into());
        let target = format!(r"shell:AppsFolder\{family}!Claude");
        spawn_detached(Command::new("explorer.exe").arg(&target).stdout(Stdio::null()).stderr(Stdio::null()))
            .map_err(|e| CliError::Other(format!("could not start Claude ({target}): {e}")))?;
        Ok(target)
    } else if cfg!(target_os = "macos") {
        Command::new("open")
            .args(["-a", "Claude"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| CliError::Other(format!("open -a Claude: {e}")))?;
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

/// How often the helper looks at the process list.
const POLL: Duration = Duration::from_secs(1);
/// After the last Claude Desktop process is gone: Claude writes its config as it quits, and an
/// Electron app may start a new helper in between. The list must still be empty after this.
pub(crate) const GRACE: Duration = Duration::from_secs(3);
/// How long the helper waits for a new main process after opening Claude Desktop again.
const RESTART_CHECK: Duration = Duration::from_secs(60);
/// `--restart`'s default wait.
pub(crate) const DEFAULT_WAIT_MINUTES: u64 = 60;

/// Time, for the waiting helper; a parameter so tests run it without waiting.
pub(crate) trait Clock {
    /// Time since the helper started.
    fn elapsed(&self) -> Duration;
    fn sleep(&mut self, d: Duration);
    /// Seconds since the Unix epoch, comparable with [`Proc::start`].
    fn unix_now(&self) -> u64;
}

struct RealClock(Instant);

impl Clock for RealClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn unix_now(&self) -> u64 {
        procs::unix_now()
    }
}

/// How waiting for Claude Desktop to quit ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Waited {
    /// No Claude Desktop process was left, before and after the grace period.
    Quit,
    TimedOut,
    /// A newer `--restart` took over, or the registration was written meanwhile.
    Superseded,
}

/// Wait until every Claude Desktop process is gone and none is back after [`GRACE`]. A list
/// that could not be read counts as "still running". `seen` gets the main program's path.
pub(crate) fn wait_for_quit(
    table: &mut dyn ProcessTable,
    clock: &mut dyn Clock,
    timeout: Duration,
    still_mine: &mut dyn FnMut() -> bool,
    seen: &mut Option<PathBuf>,
    log: &mut dyn FnMut(&str),
) -> Waited {
    let mut unreadable_logged = false;
    loop {
        if !still_mine() {
            return Waited::Superseded;
        }
        match claude_desktop_processes(table) {
            None => {
                if !unreadable_logged {
                    unreadable_logged = true;
                    log("could not read the process list: counting Claude Desktop as running");
                }
            }
            Some(app) if app.is_empty() => {
                clock.sleep(GRACE);
                match claude_desktop_processes(table) {
                    Some(again) if again.is_empty() => return Waited::Quit,
                    Some(_) => log("a Claude Desktop process appeared again during the grace period: still waiting"),
                    None => {}
                }
            }
            Some(app) => {
                if let Some(exe) = procs::main_processes(&app).into_iter().find_map(|p| p.exe) {
                    *seen = Some(exe);
                }
            }
        }
        if clock.elapsed() >= timeout {
            return Waited::TimedOut;
        }
        clock.sleep(POLL);
    }
}

/// The new Claude Desktop main process, if one started at or after `since` within `wait`.
pub(crate) fn wait_for_start(table: &mut dyn ProcessTable, clock: &mut dyn Clock, since: u64, wait: Duration) -> Option<Proc> {
    let from = clock.elapsed();
    loop {
        if let Some(app) = claude_desktop_processes(table)
            // Start times have whole seconds, and may be rounded down.
            && let Some(p) = procs::main_processes(&app).into_iter().find(|p| p.start.saturating_add(1) >= since)
        {
            return Some(p);
        }
        if clock.elapsed().saturating_sub(from) >= wait {
            return None;
        }
        clock.sleep(POLL);
    }
}

/// How the helper's run ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Written, and a new Claude Desktop main process started afterwards.
    Reopened,
    /// Written while Claude Desktop was closed, but no new main process was seen.
    WrittenUnconfirmed,
    GaveUp,
    Superseded,
    Failed(String),
}

/// What the helper needs; parameters so tests drive it with a fake process list and clock.
pub(crate) struct Helper<'a> {
    pub(crate) table: &'a mut dyn ProcessTable,
    pub(crate) clock: &'a mut dyn Clock,
    pub(crate) timeout: Duration,
    pub(crate) still_mine: &'a mut dyn FnMut() -> bool,
    pub(crate) write: &'a mut dyn FnMut() -> Result<Value>,
    pub(crate) reopen: &'a mut dyn FnMut(Option<&Path>) -> Result<String>,
    pub(crate) log: &'a mut dyn FnMut(&str),
}

/// The detached half of `--restart`: wait until Claude Desktop has quit completely, write,
/// open it again, and check that it really started again.
pub(crate) fn run_helper(h: Helper<'_>) -> Outcome {
    let mut seen = None;
    let minutes = h.timeout.as_secs() / 60;
    match wait_for_quit(h.table, h.clock, h.timeout, h.still_mine, &mut seen, h.log) {
        Waited::Quit => {}
        Waited::TimedOut => {
            (h.log)(&format!(
                "Claude Desktop was still running after {} (some process of it never quit): nothing was written",
                if minutes > 0 { format!("{minutes} min") } else { format!("{}s", h.timeout.as_secs()) }
            ));
            return Outcome::GaveUp;
        }
        Waited::Superseded => {
            (h.log)("another `mcp register --restart` took over, or the registration was written meanwhile: this helper stops");
            return Outcome::Superseded;
        }
    }
    match (h.write)() {
        Ok(written) => (h.log)(&format!("every Claude Desktop process has quit; wrote {written}")),
        Err(e) => {
            (h.log)(&format!("error: {e}"));
            return Outcome::Failed(e.to_string());
        }
    }
    let since = h.clock.unix_now();
    match (h.reopen)(seen.as_deref()) {
        Ok(how) => match wait_for_start(h.table, h.clock, since, RESTART_CHECK) {
            Some(p) => {
                (h.log)(&format!("reopened Claude Desktop ({how}): new main process {} started", p.pid));
                Outcome::Reopened
            }
            None => {
                (h.log)(&format!(
                    "wrote, but could not confirm a restart: no new Claude Desktop process within {}s of `{how}`; open Claude Desktop yourself",
                    RESTART_CHECK.as_secs()
                ));
                Outcome::WrittenUnconfirmed
            }
        },
        Err(e) => {
            (h.log)(&format!("wrote, but could not confirm a restart: could not open Claude Desktop ({e}); open it yourself"));
            Outcome::WrittenUnconfirmed
        }
    }
}

/// How long `--restart` waits.
fn wait_limit(a: &crate::args::ClientArgs) -> Duration {
    match (a.wait_minutes, a.timeout) {
        (Some(m), _) => Duration::from_secs(m.saturating_mul(60)),
        (None, Some(s)) => Duration::from_secs(s),
        (None, None) => Duration::from_secs(DEFAULT_WAIT_MINUTES * 60),
    }
}

fn rfc3339(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The helper as it runs detached, with the system's processes and clock.
fn helper_main(a: &crate::args::ClientArgs, paths: &[PathBuf], entries: Option<&[Entry]>) -> Result {
    let home = magicpic_home()?;
    let log_path = home.join("register.log");
    let timeout = wait_limit(a);
    let id = a.pending_id.clone();
    // Record our pid, so `mcp status` and `doctor` can tell whether we still wait.
    if let Some(id) = &id
        && let Some(mut p) = pending::load(&home).filter(|p| &p.id == id)
    {
        p.helper_pid = Some(std::process::id());
        let _ = pending::save(&home, &p);
    }
    log_line(&log_path, &format!("waiting up to {} min for every Claude Desktop process to quit", timeout.as_secs().div_ceil(60)));
    let mut table = procs::table(false);
    let mut clock = RealClock(Instant::now());
    let mut still_mine = || match &id {
        Some(id) => pending::load(&home).is_some_and(|p| &p.id == id),
        None => true,
    };
    let mut write = || write_all(Client::ClaudeDesktop, paths, entries).map(Value::Array);
    let mut reopen = |exe: Option<&Path>| relaunch(exe);
    let mut log = |t: &str| log_line(&log_path, t);
    let outcome = run_helper(Helper {
        table: table.as_mut(),
        clock: &mut clock,
        timeout,
        still_mine: &mut still_mine,
        write: &mut write,
        reopen: &mut reopen,
        log: &mut log,
    });
    let update = |state: &str, message: Option<String>| {
        if let Some(mut p) = pending::load(&home).filter(|p| Some(&p.id) == id.as_ref()) {
            p.state = state.into();
            p.message = message;
            let _ = pending::save(&home, &p);
        }
    };
    match outcome {
        Outcome::Reopened | Outcome::WrittenUnconfirmed => {
            if pending::load(&home).is_some_and(|p| Some(&p.id) == id.as_ref()) {
                pending::clear(&home);
            }
        }
        Outcome::GaveUp => update("gave-up", None),
        Outcome::Failed(m) => update("error", Some(m)),
        Outcome::Superseded => {}
    }
    Ok(())
}

/// Shared by register and unregister for Claude Desktop: write now, refuse, or hand off to the
/// helper. `None` in the helper itself, which prints nothing (its output is the log).
fn desktop_write(a: &crate::args::ClientArgs, paths: &[PathBuf], entries: Option<&[Entry]>, verb: &str, args_for_child: Vec<String>) -> Result<Option<Value>> {
    let home = magicpic_home()?;
    if a.detached_wait {
        helper_main(a, paths, entries)?;
        return Ok(None);
    }
    // Unknown (the process list could not be read) counts as running.
    let running = claude_desktop_running();
    if running == Some(false) || a.force {
        let written = write_all(Client::ClaudeDesktop, paths, entries)?;
        if a.config.is_none() {
            pending::clear(&home);
        }
        let mut v = json!({"status": "written", "files": written, "claudeDesktopRunning": running});
        if a.restart && running == Some(false) {
            v["relaunched"] = json!(relaunch(None).ok());
        }
        return Ok(Some(v));
    }
    if !a.restart {
        let why = if running.is_none() { "MagicPic could not read the process list, so Claude Desktop may be running" } else { "Claude Desktop is running" };
        return Err(CliError::Other(format!(
            "{why}, and it rewrites its config when it quits, so an entry written now would be lost. Nothing was written. \
             Run this again with --restart (it waits for every Claude Desktop process to quit, writes the file and opens Claude again), or quit Claude first"
        )));
    }
    std::fs::create_dir_all(&home).map_err(|e| CliError::Other(format!("{}: {e}", home.display())))?;
    let log = home.join("register.log");
    let limit = wait_limit(a);
    let now = chrono::Utc::now();
    let deadline = now + chrono::Duration::from_std(limit).unwrap_or(chrono::Duration::hours(1));
    let record = Pending {
        id: pending::new_id(),
        verb: verb.into(),
        state: "waiting".into(),
        since: rfc3339(now),
        deadline: rfc3339(deadline),
        helper_pid: None,
        message: None,
        log: log.clone(),
    };
    pending::save(&home, &record)?;
    let mut args = args_for_child;
    args.extend(["--detached-wait".to_string(), "--pending-id".into(), record.id.clone()]);
    // Started fully detached and holding none of this process's handles, so a caller reading our
    // output to its end gets it as soon as we exit, not when the helper does.
    let pid = match spawn_unattached(&crate::setup::current_cli(), &args, None, Some(&log)) {
        Ok(pid) => pid,
        Err(e) => {
            pending::clear(&home);
            return Err(CliError::Other(format!("could not start the waiting helper: {e}")));
        }
    };
    Ok(Some(json!({
        "status": "waiting",
        "helperPid": pid,
        "log": log,
        "pending": pending::path(&home),
        "waitMinutes": limit.as_secs().div_ceil(60),
        "deadline": record.deadline,
        "message": "Claude Desktop is running. Ask the person to quit it completely now (tray or menu-bar icon > Quit, not just the window). \
                    When every Claude Desktop process is gone, MagicPic writes the config and opens Claude again. \
                    If it does not open within a minute, the person opens it. Then check with `magicpic-cli doctor`, not the log.",
    })))
}

/// The child's command line: everything after `magicpic-cli`, minus flags it re-adds.
fn child_args(verb: &str, a: &crate::args::ClientArgs, extra: &[String]) -> Vec<String> {
    let mut v = vec!["mcp".to_string(), verb.to_string(), "--client".into(), a.client.name().into(), "--restart".into()];
    if let Some(c) = &a.config {
        v.push("--config".into());
        v.push(absolute(c).to_string_lossy().into_owned());
    }
    if let Some(m) = a.wait_minutes {
        v.push("--wait-minutes".into());
        v.push(m.to_string());
    } else if let Some(t) = a.timeout {
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
    // Remember a deliberate --no-start-app, so `doctor` doesn't call the missing flag outdated.
    if !a.client.detached_wait
        && let Some(mut recorded) = Config::load(&home)?
        && recorded.start_app == a.no_start_app
    {
        recorded.start_app = !a.no_start_app;
        recorded.save(&home)?;
    }
    let paths = config_paths(a.client.client, a.client.config.as_deref())?;
    let report = match a.client.client {
        Client::ClaudeDesktop => match desktop_write(&a.client, &paths, Some(&entries), "register", child_args("register", &a.client, &extra))? {
            Some(v) => v,
            None => return Ok(()),
        },
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
    print_json_ascii(out, &report)
}

/// Point `client`'s registration at this install (`setup --migrate`). Claude Desktop is left
/// alone while it runs: the fix says what to run instead.
pub(crate) fn repoint(client: Client, config: &Config) -> Result<Value> {
    let entries = entries(config, config.start_app);
    let paths = config_paths(client, None)?;
    if client == Client::ClaudeDesktop && claude_desktop_running() != Some(false) {
        return Ok(json!({
            "status": "not written",
            "why": "Claude Desktop is running, and would overwrite the change when it quits",
            "fix": "magicpic-cli mcp register --client claude-desktop --restart, then `magicpic-cli setup --migrate` again once Claude Desktop has restarted",
        }));
    }
    let written = write_all(client, &paths, Some(&entries))?;
    if client == Client::ClaudeDesktop
        && let Ok(home) = magicpic_home()
    {
        pending::clear(&home);
    }
    Ok(json!({"status": "written", "files": written}))
}

/// `mcp unregister`.
pub(crate) fn unregister(a: &UnregisterArgs, out: &mut dyn Write) -> Result {
    let paths = config_paths(a.client.client, a.client.config.as_deref())?;
    let mut report = match a.client.client {
        Client::ClaudeDesktop => match desktop_write(&a.client, &paths, None, "unregister", child_args("unregister", &a.client, &[]))? {
            Some(v) => v,
            None => return Ok(()),
        },
        client => json!({"status": "written", "files": write_all(client, &paths, None)?}),
    };
    report["client"] = json!(a.client.client.name());
    report["removed"] = json!(SERVERS);
    print_json_ascii(out, &report)
}

/// `path` spelled so two spellings of one file compare equal: absolute, without `\\?\`, links
/// resolved when it exists, and on Windows in one letter case.
pub(crate) fn same_file_key(path: &Path) -> String {
    let p = absolute(path);
    let p = std::fs::canonicalize(&p).map(|c| crate::setup::strip_verbatim(&c)).unwrap_or(p);
    let text = p.to_string_lossy().into_owned();
    if cfg!(windows) { text.replace('/', "\\").to_lowercase() } else { text }
}

/// The first line `<program> --version` prints, within a few seconds.
pub(crate) fn program_version(program: &Path) -> Option<String> {
    let mut child = Command::new(program).arg("--version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let t = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t.elapsed() < Duration::from_secs(10) => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut text).ok()?;
    text.lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())
}

/// This CLI's own `--version` line.
pub(crate) fn own_version() -> String {
    format!("magicpic-cli {}", magicpic_engine::build_info::long_version())
}

/// What is wrong with one registered server, beyond missing paths: it runs another
/// `magicpic-cli` than this one (an older install, a source build in `target/`), or
/// `magicpic-app` lacks `--start-app-if-needed` (written by an older version).
pub(crate) fn entry_problems(
    client: Client,
    name: &str,
    entry: &Value,
    cli: &Path,
    start_app: bool,
    versions: &mut dyn FnMut(&Path) -> Option<String>,
) -> Vec<Value> {
    let mut out = Vec::new();
    let fix = match client {
        Client::ClaudeDesktop => "magicpic-cli mcp register --client claude-desktop --restart".to_string(),
        c => format!("magicpic-cli mcp register --client {}", c.name()),
    };
    if let Some(cmd) = entry.get("command").and_then(Value::as_str)
        && same_file_key(Path::new(cmd)) != same_file_key(cli)
    {
        let theirs = versions(Path::new(cmd)).map(|v| format!(" ({v})")).unwrap_or_default();
        out.push(json!({
            "problem": format!("{} runs `{name}` with {cmd}{theirs} instead of {}, the CLI of this install ({})", client.name(), cli.display(), own_version()),
            "fix": fix,
        }));
    }
    let args: Vec<&str> = entry.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if name == SERVERS[1] && start_app && !args.contains(&"--start-app-if-needed") {
        out.push(json!({
            "problem": format!("{}: `magicpic-app` has no --start-app-if-needed (registered by an older version), so it can't start the app itself", client.name()),
            "fix": format!("{fix} (or add --no-start-app if that was on purpose)"),
        }));
    }
    out
}

/// One client's registration, as `mcp status` and `doctor` report it, with its `problems`.
pub(crate) fn client_status(client: Client, overridden: Option<&Path>) -> Value {
    let paths = match config_paths(client, overridden) {
        Ok(p) => p,
        Err(e) => return json!({"client": client.name(), "error": e.to_string(), "problems": []}),
    };
    let home = magicpic_home().ok();
    let start_app = home.as_deref().and_then(|h| Config::load(h).ok().flatten()).is_none_or(|c| c.start_app);
    let cli = crate::setup::current_cli();
    let mut versions: std::collections::BTreeMap<String, Option<String>> = Default::default();
    let mut version_of = |p: &Path| versions.entry(same_file_key(p)).or_insert_with(|| program_version(p)).clone();
    let mut problems = Vec::new();
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
                                let found = entry_problems(client, &name, &e, &cli, start_app, &mut version_of);
                                let current = !found.iter().any(|p| p["problem"].as_str().is_some_and(|t| t.contains(" instead of ")));
                                for p in found {
                                    if !problems.contains(&p) {
                                        problems.push(p);
                                    }
                                }
                                json!({"registered": true, "command": e["command"], "args": e["args"], "missingPaths": missing_paths(&e), "thisCli": current})
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
        v["running"] = json!(claude_desktop_running());
        if let Some(p) = home.as_deref().and_then(|h| pending::report(h, procs::pid_alive)) {
            problems.insert(0, json!({"problem": p["problem"], "fix": p["fix"]}));
            v["pending"] = p;
        }
    }
    v["problems"] = Value::Array(problems);
    v
}

/// `mcp status`.
pub(crate) fn status(a: &StatusArgs, out: &mut dyn Write) -> Result {
    let clients: Vec<Client> = match a.client {
        Some(c) => vec![c],
        None => vec![Client::ClaudeDesktop, Client::ClaudeCode, Client::Codex],
    };
    let report: Vec<Value> = clients.into_iter().map(|c| client_status(c, a.config.as_deref())).collect();
    print_json_ascii(out, &json!({"clients": report}))
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
            start_app: true,
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
    fn the_store_package_family_comes_from_the_install_folder() {
        assert_eq!(msix_family(r"C:\Program Files\WindowsApps\Claude_1.2.3.0_x64__pzs8sxrjxfjjc\app\claude.exe").as_deref(), Some("Claude_pzs8sxrjxfjjc"));
        assert_eq!(msix_family(r"C:\Users\a\AppData\Local\AnthropicClaude\claude.exe"), None);
        assert_eq!(msix_family(r"C:\Program Files\WindowsApps\Claude"), None);
    }

    // -- The `--restart` helper, with a scripted process list and a clock that never waits --

    use std::cell::Cell;
    use std::rc::Rc;

    struct FakeClock {
        now: Rc<Cell<u64>>,
    }

    const EPOCH: u64 = 1_800_000_000;

    impl Clock for FakeClock {
        fn elapsed(&self) -> Duration {
            Duration::from_secs(self.now.get())
        }
        fn sleep(&mut self, d: Duration) {
            self.now.set(self.now.get() + d.as_secs().max(1));
        }
        fn unix_now(&self) -> u64 {
            EPOCH + self.now.get()
        }
    }

    /// The process list at each second of the fake clock.
    struct Script {
        now: Rc<Cell<u64>>,
        at: Box<dyn Fn(u64) -> Option<Vec<Proc>>>,
    }

    impl ProcessTable for Script {
        fn snapshot(&mut self) -> Option<Vec<Proc>> {
            (self.at)(self.now.get())
        }
    }

    const STORE: &str = r"C:\Program Files\WindowsApps\Claude_1.2.3.0_x64__pzs8sxrjxfjjc\app\claude.exe";

    /// A Claude Desktop process on this system (the rules are the platform's own).
    fn claude(pid: u32, parent: u32, start: u64) -> Proc {
        let (name, exe) = match procs::OS {
            procs::Os::Windows => ("claude.exe", STORE),
            procs::Os::Macos => ("Claude", "/Applications/Claude.app/Contents/MacOS/Claude"),
            procs::Os::Linux => ("claude-desktop", "/usr/bin/claude-desktop"),
        };
        Proc { pid, parent: Some(parent), name: name.into(), exe: Some(exe.into()), cmd: Vec::new(), start }
    }

    fn other(pid: u32) -> Proc {
        Proc { pid, parent: Some(1), name: "explorer.exe".into(), exe: Some(r"C:\Windows\explorer.exe".into()), cmd: Vec::new(), start: 1 }
    }

    /// Run the helper over `at`; returns the outcome, the second each write happened, how often
    /// it reopened Claude, and the log.
    fn drive(timeout: u64, at: impl Fn(u64, Option<u64>) -> Option<Vec<Proc>> + 'static, mine: bool) -> (Outcome, Vec<u64>, u32, Vec<String>) {
        let now = Rc::new(Cell::new(0));
        let reopened_at: Rc<Cell<Option<u64>>> = Rc::new(Cell::new(None));
        let r = reopened_at.clone();
        let mut table = Script { now: now.clone(), at: Box::new(move |t| at(t, r.get())) };
        let mut clock = FakeClock { now: now.clone() };
        let writes = Rc::new(std::cell::RefCell::new(Vec::new()));
        let (w, n) = (writes.clone(), now.clone());
        let mut write = move || {
            w.borrow_mut().push(n.get());
            Ok(json!([]))
        };
        let reopens = Rc::new(Cell::new(0));
        let (ro, n2) = (reopens.clone(), now.clone());
        let mut reopen = move |_: Option<&Path>| {
            ro.set(ro.get() + 1);
            reopened_at.set(Some(n2.get()));
            Ok("test".to_string())
        };
        let mut still_mine = move || mine;
        let lines = Rc::new(std::cell::RefCell::new(Vec::new()));
        let l = lines.clone();
        let mut log = move |t: &str| l.borrow_mut().push(t.to_string());
        let outcome = run_helper(Helper {
            table: &mut table,
            clock: &mut clock,
            timeout: Duration::from_secs(timeout),
            still_mine: &mut still_mine,
            write: &mut write,
            reopen: &mut reopen,
            log: &mut log,
        });
        let written = writes.borrow().clone();
        let log = lines.borrow().clone();
        (outcome, written, reopens.get(), log)
    }

    #[test]
    fn helpers_that_exit_while_the_main_process_stays_write_nothing() {
        // The main process runs throughout; renderer and GPU helpers exit after 5 s, one more
        // comes and goes. Before the fix, an empty or failed probe ended the wait.
        let (outcome, written, reopens, log) = drive(
            120,
            |t, _| {
                let mut list = vec![other(4), claude(20, 4, 10)];
                if t < 5 {
                    list.extend([claude(21, 20, 11), claude(22, 20, 11)]);
                }
                if (40..45).contains(&t) {
                    list.push(claude(23, 20, EPOCH + 40));
                }
                Some(list)
            },
            true,
        );
        assert_eq!(outcome, Outcome::GaveUp);
        assert!(written.is_empty() && reopens == 0, "nothing written, Claude not reopened: {log:?}");
        assert!(log.last().unwrap().contains("nothing was written"), "{log:?}");
    }

    #[test]
    fn a_process_list_that_cannot_be_read_is_not_a_quit() {
        let (outcome, written, _, log) = drive(30, |_, _| None, true);
        assert_eq!(outcome, Outcome::GaveUp);
        assert!(written.is_empty());
        assert_eq!(log.iter().filter(|l| l.contains("could not read the process list")).count(), 1, "said once: {log:?}");
    }

    #[test]
    fn it_writes_only_after_every_process_is_gone_through_the_grace_period_then_checks_the_restart() {
        // Gone for one second at t=10 (a flicker shorter than the grace period), really gone
        // from t=30. After the reopen a new main process starts.
        let (outcome, written, reopens, log) = drive(
            600,
            |t, reopened| {
                if let Some(r) = reopened {
                    return Some(vec![other(4), claude(90, 4, EPOCH + r)]);
                }
                let gone = t == 10 || t >= 30;
                Some(if gone { vec![other(4)] } else { vec![other(4), claude(20, 4, 1), claude(21, 20, 2)] })
            },
            true,
        );
        assert_eq!(outcome, Outcome::Reopened, "{log:?}");
        assert_eq!(written.len(), 1);
        assert!(written[0] >= 30 + GRACE.as_secs(), "written at {}s, after the grace period: {log:?}", written[0]);
        assert_eq!(reopens, 1);
        assert!(log.iter().any(|l| l.contains("grace period")), "{log:?}");
        assert!(log.last().unwrap().starts_with("reopened Claude Desktop"), "{log:?}");
    }

    #[test]
    fn a_reopen_that_starts_no_new_main_process_is_not_called_a_restart() {
        let (outcome, written, reopens, log) = drive(600, |t, _| Some(if t < 3 { vec![claude(20, 4, 1)] } else { vec![other(4)] }), true);
        assert_eq!(outcome, Outcome::WrittenUnconfirmed);
        assert_eq!((written.len(), reopens), (1, 1));
        assert!(log.last().unwrap().starts_with("wrote, but could not confirm a restart"), "{log:?}");
    }

    #[test]
    fn a_superseded_helper_writes_nothing() {
        let (outcome, written, reopens, _) = drive(600, |_, _| Some(vec![other(4)]), false);
        assert_eq!(outcome, Outcome::Superseded);
        assert!(written.is_empty() && reopens == 0);
    }
}
