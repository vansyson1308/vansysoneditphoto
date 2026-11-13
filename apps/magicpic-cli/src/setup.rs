//! `magicpic-cli setup` and `magicpic-cli launch`: every file an agent used to write by hand
//! (folders, launcher, desktop shortcut, the paths other commands reuse) is written here, so the
//! agent runs one binary with plain arguments instead of scripting PowerShell or JSON
//! (`docs/agent-setup.md`).
//!
//! Everything lives in the MagicPic home, `~/.magicpic` (or `MAGICPIC_HOME`):
//!
//! | file | what |
//! |---|---|
//! | `config.json` | the paths below, read by `launch`, `mcp register` and `doctor` |
//! | `bin/` | `magicpic` and `magicpic-cli`, from the installer or a source build |
//! | `start-magicpic.cmd` / `.sh` | the launcher: runs `magicpic-cli launch` |
//! | `control.token` | written by the app when it starts; never read aloud |
//! | `app.log` | the app's output when started by `launch` |

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::args::{LaunchArgs, SetupArgs, SpawnDetachedArgs};
use crate::output::print_json_ascii;
use crate::{CliError, Result};

/// The control port the guide uses.
pub(crate) const DEFAULT_PORT: u16 = 7878;
const CONFIG_FILE: &str = "config.json";
const TOKEN_FILE: &str = "control.token";

/// What `setup` recorded. Unknown fields are kept out; missing ones take today's defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Config {
    #[serde(default = "one")]
    pub(crate) version: u32,
    pub(crate) cli: PathBuf,
    pub(crate) app: PathBuf,
    pub(crate) photos: PathBuf,
    pub(crate) port: u16,
    pub(crate) token_file: PathBuf,
    pub(crate) launcher: PathBuf,
    /// Whether `magicpic-app` may start the app itself (`mcp register --no-start-app` records
    /// false), so `doctor` can tell a deliberate choice from an outdated registration.
    #[serde(default = "yes")]
    pub(crate) start_app: bool,
}

fn one() -> u32 {
    1
}

fn yes() -> bool {
    true
}

/// The person's home folder: `USERPROFILE` on Windows, `HOME` elsewhere.
pub(crate) fn home_dir() -> Result<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
        .ok_or_else(|| CliError::Other(format!("cannot find the home folder: {var} is not set")))
}

/// `~/.magicpic`, or `MAGICPIC_HOME` when set (tests, portable installs).
pub(crate) fn magicpic_home() -> Result<PathBuf> {
    match std::env::var_os("MAGICPIC_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(home_dir()?.join(".magicpic")),
    }
}

pub(crate) fn exe_name(stem: &str) -> String {
    if cfg!(windows) { format!("{stem}.exe") } else { stem.to_string() }
}

/// `path` made absolute against the current folder, without resolving links (and without the
/// Windows `\\?\` prefix `canonicalize` adds, which other programs mis-handle).
pub(crate) fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// This program's own path, as other programs should start it.
pub(crate) fn current_cli() -> PathBuf {
    std::env::current_exe().map(|p| absolute(&strip_verbatim(&p))).unwrap_or_else(|_| PathBuf::from(exe_name("magicpic-cli")))
}

/// `\\?\C:\x` → `C:\x` and `\\?\UNC\host\share` → `\\host\share`; other paths unchanged.
pub(crate) fn strip_verbatim(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else { return path.to_path_buf() };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

impl Config {
    /// Today's defaults for `home`, before any flag.
    pub(crate) fn defaults(home: &Path) -> Result<Config> {
        let cli = current_cli();
        // The app next to this CLI (release archive, `~/.magicpic/bin`, `target/release`), else
        // where the installer puts it.
        let sibling = cli.parent().map(|dir| dir.join(exe_name("magicpic")));
        let app = sibling.filter(|p| p.is_file()).unwrap_or_else(|| home.join("bin").join(exe_name("magicpic")));
        Ok(Config {
            version: 1,
            cli,
            app,
            photos: home_dir()?.join("Pictures").join("MagicPic"),
            port: DEFAULT_PORT,
            token_file: home.join(TOKEN_FILE),
            launcher: home.join(if cfg!(windows) { "start-magicpic.cmd" } else { "start-magicpic.sh" }),
            start_app: true,
        })
    }

    /// Write `config.json` in `home`.
    pub(crate) fn save(&self, home: &Path) -> Result {
        let mut text = serde_json::to_string_pretty(self).map_err(|e| CliError::Other(e.to_string()))?;
        text.push('\n');
        write_atomic(&home.join(CONFIG_FILE), text.as_bytes())
    }

    /// The recorded config, if `setup` ran.
    pub(crate) fn load(home: &Path) -> Result<Option<Config>> {
        let path = home.join(CONFIG_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(CliError::Other(format!("{}: {e}", path.display()))),
        };
        serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map(Some)
            .map_err(|e| CliError::Other(format!("{} is not a MagicPic config ({e}); run `magicpic-cli setup` again", path.display())))
    }

    /// The recorded config, or the defaults when `setup` has not run.
    pub(crate) fn load_or_default(home: &Path) -> Result<Config> {
        match Config::load(home)? {
            Some(c) => Ok(c),
            None => Config::defaults(home),
        }
    }

    /// The app's command line, as the launcher runs it.
    pub(crate) fn app_args(&self) -> Vec<String> {
        let photos = self.photos.to_string_lossy().into_owned();
        vec![
            "--control".into(),
            self.port.to_string(),
            "--control-token-file".into(),
            self.token_file.to_string_lossy().into_owned(),
            "--automation-read-root".into(),
            photos.clone(),
            "--automation-write-root".into(),
            photos,
        ]
    }
}

/// Write `bytes` to `path` through a temporary file and a rename, so a reader never sees half a
/// file. UTF-8 without a byte order mark: the callers pass text.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| CliError::Other(format!("{}: {e}", dir.display())))?;
    }
    magicpic_format::atomic_write(path, bytes).map_err(|e| CliError::Other(format!("{}: {e}", path.display())))
}

/// The launcher's text: it runs `magicpic-cli launch`, which reads `config.json`.
fn launcher_script(cli: &Path) -> String {
    let cli = cli.to_string_lossy();
    if cfg!(windows) {
        // cmd expands `%` even inside quotes.
        format!("@echo off\r\nrem MagicPic launcher, written by `magicpic-cli setup`.\r\n\"{}\" launch %*\r\n", cli.replace('%', "%%"))
    } else {
        format!("#!/bin/sh\n# MagicPic launcher, written by `magicpic-cli setup`.\nexec '{}' launch \"$@\"\n", cli.replace('\'', r"'\''"))
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|e| CliError::Other(format!("{}: {e}", path.display())))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result {
    Ok(())
}

/// The desktop folder: on Windows the registry's (OneDrive moves it), else `~/Desktop`.
fn desktop_dir() -> Result<PathBuf> {
    let home = home_dir()?;
    #[cfg(windows)]
    if let Some(dir) = windows_desktop(&home) {
        return Ok(dir);
    }
    Ok(home.join("Desktop"))
}

/// `HKCU\…\User Shell Folders\Desktop`, read with `reg.exe` (no PowerShell), `%VAR%` expanded.
#[cfg(windows)]
fn windows_desktop(home: &Path) -> Option<PathBuf> {
    let out =
        Command::new("reg").args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders", "/v", "Desktop"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.trim_start().starts_with("Desktop"))?;
    let value = line.split("REG_").nth(1)?.split_once(char::is_whitespace)?.1.trim();
    let expanded =
        expand_percent(
            value,
            |name| if name.eq_ignore_ascii_case("USERPROFILE") { Some(home.to_string_lossy().into_owned()) } else { std::env::var(name).ok() },
        );
    let dir = PathBuf::from(expanded);
    dir.is_dir().then_some(dir)
}

/// `%NAME%` replaced by `lookup(NAME)`; unknown names stay as written.
#[cfg_attr(not(windows), allow(dead_code))]
fn expand_percent(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match lookup(name) {
                    Some(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// A double-clickable shortcut to the launcher on the desktop. Windows: `MagicPic.cmd` (a `.lnk`
/// needs the shell's COM interface; see the follow-ups in `docs/agent-setup.md`). macOS:
/// `MagicPic.command`. Linux: `magicpic.desktop`.
fn write_desktop_shortcut(launcher: &Path, app: &Path) -> Result<PathBuf> {
    let dir = desktop_dir()?;
    let l = launcher.to_string_lossy();
    let (name, text) = if cfg!(windows) {
        ("MagicPic.cmd", format!("@echo off\r\ncall \"{}\"\r\n", l.replace('%', "%%")))
    } else if cfg!(target_os = "macos") {
        ("MagicPic.command", format!("#!/bin/sh\nexec '{}'\n", l.replace('\'', r"'\''")))
    } else {
        (
            "magicpic.desktop",
            format!(
                "[Desktop Entry]\nType=Application\nName=MagicPic\nComment=Open MagicPic for your AI agent\nExec=\"{}\"\nIcon={}\nTerminal=false\n",
                l.replace('"', "\\\""),
                app.to_string_lossy()
            ),
        )
    };
    let path = dir.join(name);
    write_atomic(&path, text.as_bytes())?;
    make_executable(&path)?;
    Ok(path)
}

/// `magicpic-cli setup`: create the folders, the launcher and `config.json`; print the paths.
pub(crate) fn setup(a: &SetupArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result {
    let home = absolute(&magicpic_home()?);
    let mut config = Config::load_or_default(&home)?;
    // This CLI is the one being set up: a copy that moved (reinstall, source build) wins.
    config.cli = current_cli();
    if let Some(p) = &a.photos {
        config.photos = absolute(p);
    }
    if let Some(p) = &a.app {
        config.app = absolute(p);
    } else if !config.app.is_file() {
        config.app = Config::defaults(&home)?.app;
    }
    if let Some(port) = a.port {
        config.port = port;
    }
    config.token_file = home.join(TOKEN_FILE);
    config.launcher = home.join(if cfg!(windows) { "start-magicpic.cmd" } else { "start-magicpic.sh" });
    for dir in [&home, &config.photos] {
        std::fs::create_dir_all(dir).map_err(|e| CliError::Other(format!("{}: {e}", dir.display())))?;
    }
    write_atomic(&config.launcher, launcher_script(&config.cli).as_bytes())?;
    make_executable(&config.launcher)?;
    config.save(&home)?;
    let shortcut = if a.desktop_shortcut { Some(write_desktop_shortcut(&config.launcher, &config.app)?) } else { None };
    let app_exists = config.app.is_file();
    let migrated = a.migrate.then(|| crate::update::migrate(&home, &config));
    let legacy = crate::update::legacy(&home);
    let mut next = vec!["magicpic-cli mcp register --client <claude-desktop|claude-code|codex>".to_string(), "magicpic-cli doctor".to_string()];
    if !legacy.is_empty() {
        next.push("an older install is still here (see `legacy`): ask the person, then run `magicpic-cli setup --migrate`".into());
    }
    let mut report = json!({
        "ok": app_exists,
        "version": env!("CARGO_PKG_VERSION"),
        "home": home,
        "config": home.join(CONFIG_FILE),
        "cli": config.cli,
        "app": config.app,
        "appExists": app_exists,
        "photos": config.photos,
        "port": config.port,
        "tokenFile": config.token_file,
        "launcher": config.launcher,
        "desktopShortcut": shortcut,
        "legacy": legacy,
        "next": next,
    });
    if let Some(m) = migrated {
        report["migrated"] = m;
    }
    if !app_exists {
        report["error"] = json!(format!("the app is not at {}: pass --app <path to magicpic>", config.app.display()));
    }
    if a.start && app_exists {
        report["launch"] = launch_app(&config, Duration::from_secs(a.wait.unwrap_or(30)), err)?;
    }
    print_json_ascii(out, &report)?;
    if app_exists { Ok(()) } else { Err(CliError::Other("setup wrote its files, but the app was not found (see `error`)".into())) }
}

/// Whether something accepts connections on `127.0.0.1:port`.
pub(crate) fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_millis(500)).is_ok()
}

/// Spawn `program` so it outlives this process and whatever started it (an agent's shell, which
/// a client may kill when it exits).
pub(crate) fn spawn_detached(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // Leave the caller's job object when it allows that, so closing the agent's terminal
        // doesn't close the app; a job that forbids breaking away refuses the flag.
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if let Ok(child) = cmd.spawn() {
            return Ok(child);
        }
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// `program args` with its output appended to `log` (or dropped), started by [`spawn_detached`].
fn start_logged(program: &OsStr, args: &[OsString], cwd: Option<&Path>, log: Option<&Path>) -> std::io::Result<std::process::Child> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    match log {
        Some(path) => {
            let f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            let f2 = f.try_clone()?;
            cmd.stdout(f).stderr(f2);
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    spawn_detached(&mut cmd)
}

/// Start `program args` in the background so that it outlives this process and holds none of
/// its handles, with its output appended to `log` (or dropped). Returns its pid when known.
///
/// On Windows a child inherits every inheritable handle of its parent, whatever its own stdio
/// is set to: Rust starts processes with handle inheritance on, and the pipe a caller gave this
/// program as its output is inheritable. A helper holding that pipe keeps it open, so a caller
/// that reads our output to the end (`... | Out-String`, an agent's tool call) waits until the
/// helper exits, not until we do. So on Windows the shell starts it (`Start-Process`, which never
/// passes handles on) through `magicpic-cli spawn-detached`, which sets up the log and starts it.
pub(crate) fn spawn_unattached(program: &Path, args: &[String], cwd: Option<&Path>, log: Option<&Path>) -> Result<Option<u32>> {
    #[cfg(windows)]
    if let Some(pid) = spawn_through_shell(program, args, cwd, log)? {
        return Ok(pid);
    }
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let child = start_logged(program.as_os_str(), &args, cwd, log).map_err(|e| CliError::Other(format!("could not start {}: {e}", program.display())))?;
    Ok(Some(child.id()))
}

/// Windows PowerShell, by its full path when the system folder is known.
#[cfg(windows)]
fn powershell() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"))
}

/// [`spawn_unattached`] on Windows: `Some(pid)` once started (the pid when it reported back),
/// `None` when PowerShell could not be used (the caller then starts it directly).
#[cfg(windows)]
fn spawn_through_shell(program: &Path, args: &[String], cwd: Option<&Path>, log: Option<&Path>) -> Result<Option<Option<u32>>> {
    let home = magicpic_home()?;
    std::fs::create_dir_all(&home).map_err(|e| CliError::Other(format!("{}: {e}", home.display())))?;
    let pid_file = home.join(format!("spawn-{}.pid", crate::pending::new_id()));
    let mut inner: Vec<String> = vec!["spawn-detached".into(), "--pid-file".into(), pid_file.to_string_lossy().into_owned()];
    if let Some(l) = log {
        inner.extend(["--log".to_string(), l.to_string_lossy().into_owned()]);
    }
    if let Some(d) = cwd {
        inner.extend(["--cwd".to_string(), d.to_string_lossy().into_owned()]);
    }
    inner.push("--".into());
    inner.push(program.to_string_lossy().into_owned());
    inner.extend(args.iter().cloned());
    let script = start_process_script(&current_cli(), &inner);
    use std::os::windows::process::CommandExt;
    // No console window for PowerShell when this process has none (an MCP server, the helper).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = Command::new(powershell())
        .args(["-NoProfile", "-NonInteractive", "-Command", script.as_str()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !matches!(status, Ok(s) if s.success()) {
        return Ok(None);
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10) {
        if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
            let _ = std::fs::remove_file(&pid_file);
            return Ok(Some(Some(pid)));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(Some(None))
}

/// A PowerShell string literal: single quotes, with every kind of single quote PowerShell
/// accepts doubled.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn ps_quote(text: &str) -> String {
    let mut out = String::from("'");
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// One argument as the Windows C runtime reads a command line back.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// The PowerShell that starts `exe args` hidden through the shell. Double quotes never pass
/// through this program's own command line to PowerShell (Windows PowerShell mangles them):
/// PowerShell rebuilds them from `[char]34`. Core cmdlets only, so it runs in Constrained
/// Language Mode.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn start_process_script(exe: &Path, args: &[String]) -> String {
    let line = args.iter().map(|a| quote_windows_arg(a)).collect::<Vec<_>>().join(" ");
    let quoted = line.split('"').map(ps_quote).collect::<Vec<_>>().join(" + $q + ");
    format!("$q = [char]34; Start-Process -FilePath {} -ArgumentList ({quoted}) -WindowStyle Hidden", ps_quote(&exe.to_string_lossy()))
}

/// `magicpic-cli spawn-detached` (hidden): what [`spawn_unattached`] runs on Windows. Starts
/// the program detached, with its output in the log, writes its pid, and exits.
pub(crate) fn spawn_detached_cmd(a: &SpawnDetachedArgs) -> Result {
    let Some((program, rest)) = a.command.split_first() else {
        return Err(CliError::Other("spawn-detached: no program".into()));
    };
    let child = start_logged(program, rest, a.cwd.as_deref(), a.log.as_deref())
        .map_err(|e| CliError::Other(format!("could not start {}: {e}", Path::new(program).display())))?;
    if let Some(p) = &a.pid_file {
        write_atomic(p, child.id().to_string().as_bytes())?;
    }
    Ok(())
}

/// Start the app with the control channel unless it already listens; wait for its token.
pub(crate) fn launch_app(config: &Config, wait: Duration, err: &mut dyn Write) -> Result<Value> {
    if listening(config.port) {
        return Ok(json!({"running": true, "started": false, "port": config.port, "tokenFileExists": config.token_file.is_file()}));
    }
    if !config.app.is_file() {
        return Err(CliError::Other(format!("the MagicPic app is not at {}; run `magicpic-cli setup --app <path>`", config.app.display())));
    }
    let home = config.token_file.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let log_path = home.join("app.log");
    std::fs::File::create(&log_path).map_err(|e| CliError::Other(format!("{}: {e}", log_path.display())))?;
    let pid = spawn_unattached(&config.app, &config.app_args(), config.app.parent(), Some(&log_path))?;
    let _ = writeln!(err, "magicpic-cli: started {}{}", config.app.display(), pid.map(|p| format!(" (pid {p})")).unwrap_or_default());
    let t = Instant::now();
    while t.elapsed() < wait {
        if listening(config.port) && config.token_file.is_file() {
            return Ok(json!({"running": true, "started": true, "pid": pid, "port": config.port, "log": log_path, "seconds": t.elapsed().as_secs_f64()}));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(CliError::Other(format!(
        "started the app, but nothing listens on 127.0.0.1:{} after {}s; see {} (a taken port shows `failed to bind`: run `magicpic-cli setup --port <other>`)",
        config.port,
        wait.as_secs(),
        log_path.display()
    )))
}

/// `magicpic-cli launch`: what the launcher runs.
pub(crate) fn launch(a: &LaunchArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result {
    let home = magicpic_home()?;
    let config = Config::load(&home)?.ok_or_else(|| CliError::Other(format!("{} has no config.json: run `magicpic-cli setup` first", home.display())))?;
    let report = launch_app(&config, Duration::from_secs(a.wait.unwrap_or(30)), err)?;
    print_json_ascii(out, &report)
}

/// The bridge's "app is not running" hint and, with `--start-app-if-needed`, its starter.
pub(crate) fn bridge_hints(client: magicpic_automation::BridgeClient, start: bool) -> magicpic_automation::BridgeClient {
    let config = magicpic_home().ok().and_then(|home| Config::load(&home).ok().flatten());
    let client = match &config {
        Some(c) => client.with_hint(format!("run the launcher {} (or `magicpic-cli launch`)", c.launcher.display())),
        None => client,
    };
    match config {
        Some(c) if start => client
            .with_starter(std::sync::Arc::new(move || launch_app(&c, Duration::from_secs(30), &mut std::io::sink()).map(|_| ()).map_err(|e| e.to_string()))),
        _ => client,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_are_removed() {
        assert_eq!(strip_verbatim(Path::new(r"\\?\C:\Users\a b\x.png")), PathBuf::from(r"C:\Users\a b\x.png"));
        assert_eq!(strip_verbatim(Path::new(r"\\?\UNC\host\share\x")), PathBuf::from(r"\\host\share\x"));
        assert_eq!(strip_verbatim(Path::new("/home/a/x.png")), PathBuf::from("/home/a/x.png"));
        // A verbatim path that is not a drive path keeps its prefix: without it, it means something else.
        assert_eq!(strip_verbatim(Path::new(r"\\?\Volume{1}\x")), PathBuf::from(r"\\?\Volume{1}\x"));
    }

    #[test]
    fn percent_variables_expand_and_unknown_ones_stay() {
        let lookup = |n: &str| (n == "USERPROFILE").then(|| r"C:\Users\Ann".to_string());
        assert_eq!(expand_percent(r"%USERPROFILE%\OneDrive\Desktop", lookup), r"C:\Users\Ann\OneDrive\Desktop");
        assert_eq!(expand_percent("%NOPE%\\x", lookup), "%NOPE%\\x");
        assert_eq!(expand_percent("50%", lookup), "50%");
        assert_eq!(expand_percent("%%", lookup), "%%");
    }

    #[test]
    fn launchers_quote_the_cli_path() {
        let text = launcher_script(Path::new(if cfg!(windows) { r"C:\Users\A 100%\bin\magicpic-cli.exe" } else { "/home/o'neil/bin/magicpic-cli" }));
        if cfg!(windows) {
            assert!(text.contains(r#""C:\Users\A 100%%\bin\magicpic-cli.exe" launch"#), "{text}");
        } else {
            assert!(text.starts_with("#!/bin/sh\n"));
            assert!(text.contains(r"exec '/home/o'\''neil/bin/magicpic-cli' launch"), "{text}");
        }
    }

    #[test]
    fn windows_command_lines_and_powershell_literals_quote_everything() {
        assert_eq!(quote_windows_arg("plain"), "plain");
        assert_eq!(quote_windows_arg(""), r#""""#);
        assert_eq!(quote_windows_arg(r"C:\My Photos\"), r#""C:\My Photos\\""#);
        assert_eq!(quote_windows_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(ps_quote("O'Neil\u{2019}s"), "'O''Neil\u{2019}\u{2019}s'");
        let script = start_process_script(Path::new(r"C:\Users\A B\.magicpic\bin\magicpic-cli.exe"), &["mcp".into(), r"C:\My Photos".into()]);
        assert_eq!(
            script,
            r"$q = [char]34; Start-Process -FilePath 'C:\Users\A B\.magicpic\bin\magicpic-cli.exe' -ArgumentList ('mcp ' + $q + 'C:\My Photos' + $q + '') -WindowStyle Hidden"
        );
        assert!(!script.contains('"'), "no double quote reaches PowerShell's command line");
    }

    #[test]
    fn the_app_command_line_names_one_photo_folder() {
        let c = Config {
            version: 1,
            cli: "cli".into(),
            app: "app".into(),
            photos: "/p/My Photos".into(),
            port: 7879,
            token_file: "/h/control.token".into(),
            launcher: "/h/l".into(),
            start_app: true,
        };
        assert_eq!(
            c.app_args(),
            [
                "--control",
                "7879",
                "--control-token-file",
                "/h/control.token",
                "--automation-read-root",
                "/p/My Photos",
                "--automation-write-root",
                "/p/My Photos"
            ]
        );
    }
}
