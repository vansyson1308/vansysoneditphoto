//! Updating an install in place, while it may be running: `magicpic-cli install --from <dir>`,
//! `magicpic-cli quit-app`, and finding (and with `setup --migrate`, removing) what an older
//! install left behind.
//!
//! On Windows a running program's file can't be overwritten ("The process cannot access the file
//! because it is being used by another process"), but it can be renamed. The app is asked to
//! quit first (never with unsaved documents); `magicpic-cli`, which an agent client keeps
//! running as its MCP servers, is renamed to `magicpic-cli.old-<time>.exe` and replaced, and the
//! old copies are deleted on a later run. Everything is copied in beside first, so a failure
//! before the swap changes nothing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::args::{Client, InstallArgs, QuitAppArgs};
use crate::output::print_json_ascii;
use crate::procs::ProcessTable;
use crate::register::{SERVERS, client_status, same_file_key};
use crate::setup::{Config, absolute, current_cli, exe_name, home_dir, listening, magicpic_home};
use crate::{CliError, Result};

// ---------------------------------------------------------------------------
// Asking the app to quit
// ---------------------------------------------------------------------------

/// How asking the app to quit went.
#[derive(Debug, PartialEq)]
pub(crate) enum Stop {
    NotRunning,
    Stopped,
    /// Refused: these documents have unsaved changes.
    Unsaved(Vec<String>),
    Failed(String),
}

/// Ask the app on `config`'s port to quit, unless a document has unsaved changes (`app.quit`
/// closes without asking), and wait until it stops listening.
pub(crate) fn stop_app(config: &Config, wait: Duration) -> Stop {
    if !listening(config.port) {
        return Stop::NotRunning;
    }
    let client = match magicpic_automation::BridgeClient::with_token_file(format!("127.0.0.1:{}", config.port), &config.token_file) {
        Ok(c) => c.with_timeout(Duration::from_secs(10)),
        Err(e) => return Stop::Failed(e.to_string()),
    };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return Stop::Failed(e.to_string()),
    };
    let session = match rt.block_on(client.call_read("engine.execute", json!({"command": "session.inspect"}))) {
        Ok(s) => s,
        Err(e) => return Stop::Failed(format!("could not read the app's documents: {e}")),
    };
    let unsaved: Vec<String> = session["documents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["dirty"] == true)
        .map(|d| d["name"].as_str().unwrap_or("untitled").to_string())
        .collect();
    if !unsaved.is_empty() {
        return Stop::Unsaved(unsaved);
    }
    if let Err(e) = rt.block_on(client.call("app.quit", json!({}))) {
        return Stop::Failed(format!("app.quit: {e}"));
    }
    let t = Instant::now();
    while t.elapsed() < wait {
        if !listening(config.port) {
            // The window closes before the process ends: let it release its files.
            std::thread::sleep(Duration::from_millis(500));
            return Stop::Stopped;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Stop::Failed(format!("asked the app to quit, but it still listens on 127.0.0.1:{} after {}s", config.port, wait.as_secs()))
}

fn unsaved_message(names: &[String]) -> String {
    format!(
        "MagicPic has unsaved changes in {}: nothing was changed. Save or close them in MagicPic (or ask the person to), then run this again",
        names.join(", ")
    )
}

/// `magicpic-cli quit-app`.
pub(crate) fn quit_app(a: &QuitAppArgs, out: &mut dyn Write) -> Result {
    let config = Config::load_or_default(&magicpic_home()?)?;
    let (report, result) = match stop_app(&config, Duration::from_secs(a.wait.unwrap_or(20))) {
        Stop::NotRunning => (json!({"ok": true, "running": false, "stopped": false}), Ok(())),
        Stop::Stopped => (json!({"ok": true, "running": false, "stopped": true}), Ok(())),
        Stop::Unsaved(names) => {
            let m = unsaved_message(&names);
            (json!({"ok": false, "running": true, "stopped": false, "unsaved": names, "error": m}), Err(CliError::Other(m)))
        }
        Stop::Failed(e) => (json!({"ok": false, "running": listening(config.port), "stopped": false, "error": e}), Err(CliError::Other(e))),
    };
    print_json_ascii(out, &report)?;
    result
}

// ---------------------------------------------------------------------------
// Replacing the programs
// ---------------------------------------------------------------------------

/// `magicpic-cli.old-<stamp>.exe` beside `magicpic-cli.exe`.
fn old_name(name: &str, stamp: &str) -> String {
    match name.strip_suffix(".exe") {
        Some(stem) => format!("{stem}.old-{stamp}.exe"),
        None => format!("{name}.old-{stamp}"),
    }
}

fn is_old_copy(name: &str) -> bool {
    ["magicpic.old-", "magicpic-cli.old-"].iter().any(|p| name.starts_with(p))
}

/// Delete the old copies a previous install renamed aside. Returns `(deleted, still in use)`.
pub(crate) fn remove_old_copies(bin: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut gone, mut kept) = (Vec::new(), Vec::new());
    let Ok(dir) = std::fs::read_dir(bin) else { return (gone, kept) };
    for entry in dir.flatten() {
        if is_old_copy(&entry.file_name().to_string_lossy()) {
            let p = entry.path();
            match std::fs::remove_file(&p) {
                Ok(()) => gone.push(p),
                Err(_) => kept.push(p),
            }
        }
    }
    (gone, kept)
}

/// One program put in place.
#[derive(Debug)]
pub(crate) struct Placed {
    pub(crate) path: PathBuf,
    /// Where the old copy was moved (when it could not be deleted at once).
    pub(crate) old: Option<PathBuf>,
}

/// Copy `names` from `from` into `bin`. Every file is copied in beside first (`.<name>.new`);
/// then each is swapped in. With `aside` (Windows), an existing file is renamed to its
/// `.old-<stamp>` name first, since a running program can be renamed but not replaced; elsewhere
/// the rename replaces it in one step. Any failure undoes the swaps already made.
pub(crate) fn place(from: &Path, bin: &Path, names: &[String], aside: bool, stamp: &str) -> Result<Vec<Placed>> {
    let fail = |what: String| CliError::Other(what);
    let mut staged = Vec::new();
    for name in names {
        let tmp = bin.join(format!(".{name}.new"));
        if let Err(e) = std::fs::copy(from.join(name), &tmp) {
            for t in &staged {
                let _ = std::fs::remove_file(t);
            }
            let _ = std::fs::remove_file(&tmp);
            return Err(fail(format!("could not copy {} into {}: {e}. Nothing was replaced", from.join(name).display(), bin.display())));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
        }
        staged.push(tmp);
    }
    // (final path, where its old copy went)
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let undo = |done: &[(PathBuf, Option<PathBuf>)], staged: &[PathBuf]| {
        for (target, old) in done.iter().rev() {
            if let Some(old) = old {
                let _ = std::fs::rename(old, target);
            }
        }
        for t in staged {
            let _ = std::fs::remove_file(t);
        }
    };
    for (name, tmp) in names.iter().zip(&staged) {
        let target = bin.join(name);
        let mut old = None;
        if aside && target.exists() {
            let o = bin.join(old_name(name, stamp));
            if let Err(e) = std::fs::rename(&target, &o) {
                undo(&done, &staged);
                return Err(fail(format!(
                    "could not move {} aside ({e}): nothing was replaced. Quit the program using it, then run this again",
                    target.display()
                )));
            }
            old = Some(o);
        }
        if let Err(e) = std::fs::rename(tmp, &target) {
            if let Some(o) = &old {
                let _ = std::fs::rename(o, &target);
            }
            undo(&done, &staged);
            return Err(fail(format!("could not put the new {} in place ({e}): nothing was replaced", target.display())));
        }
        done.push((target, old));
    }
    // An old copy that is not running goes now; a running one on the next install.
    Ok(done
        .into_iter()
        .map(|(path, old)| {
            let old = old.filter(|o| std::fs::remove_file(o).is_err());
            Placed { path, old }
        })
        .collect())
}

/// `magicpic-cli install --from <dir>`.
pub(crate) fn install(a: &InstallArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result {
    let home = absolute(&magicpic_home()?);
    let bin = home.join("bin");
    let from = absolute(&a.from);
    let names = [exe_name("magicpic"), exe_name("magicpic-cli")];
    for n in &names {
        if !from.join(n).is_file() {
            return Err(CliError::Other(format!(
                "{} has no {n}: pass the folder that holds both programs (an unpacked release archive, or target/release after a source build)",
                from.display()
            )));
        }
    }
    if same_file_key(&from) == same_file_key(&bin) {
        return Err(CliError::Other(format!("{} is the install folder itself: pass the folder with the new programs", from.display())));
    }
    std::fs::create_dir_all(&bin).map_err(|e| CliError::Other(format!("{}: {e}", bin.display())))?;
    let (cleaned, still_old) = remove_old_copies(&bin);

    let config = Config::load_or_default(&home)?;
    let stop = if a.keep_app_running { None } else { Some(stop_app(&config, Duration::from_secs(20))) };
    if let Some(Stop::Unsaved(names)) = &stop {
        let m = unsaved_message(names);
        print_json_ascii(out, &json!({"ok": false, "unsaved": names, "error": m}))?;
        return Err(CliError::Other(m));
    }
    if let Some(Stop::Failed(e)) = &stop {
        let _ = writeln!(err, "magicpic-cli: could not ask the app to quit ({e}); a running app keeps its old version until it restarts");
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let placed = place(&from, &bin, &names, cfg!(windows), &stamp)?;
    for f in ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE"] {
        if from.join(f).is_file() {
            let _ = std::fs::copy(from.join(f), home.join(f));
        }
    }
    let in_use: Vec<&PathBuf> = placed.iter().filter_map(|p| p.old.as_ref()).chain(still_old.iter()).collect();
    let cli = bin.join(exe_name("magicpic-cli"));
    let report = json!({
        "ok": true,
        "bin": bin,
        "app": bin.join(exe_name("magicpic")),
        "cli": cli,
        "installed": placed.iter().map(|p| &p.path).collect::<Vec<_>>(),
        "appStopped": stop == Some(Stop::Stopped),
        "appStillRunning": matches!(stop, Some(Stop::Failed(_))) || a.keep_app_running && listening(config.port),
        "oldCopiesRemoved": cleaned,
        "oldCopiesInUse": in_use,
        "note": if in_use.is_empty() { Value::Null } else {
            json!("an old copy is still running (an agent client's MCP servers): it is deleted on a later install; restart the client to load the new servers")
        },
        "next": [format!("{} setup", cli.display()), format!("{} doctor", cli.display())],
    });
    print_json_ascii(out, &report)
}

// ---------------------------------------------------------------------------
// What an older install left behind
// ---------------------------------------------------------------------------

/// The folder `cargo install` puts programs in.
fn cargo_bin() -> Option<PathBuf> {
    match std::env::var_os("CARGO_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir).join("bin")),
        None => home_dir().ok().map(|h| h.join(".cargo").join("bin")),
    }
}

/// Whether cargo's own records list `package` as installed (so `cargo uninstall` applies).
fn cargo_installed(package: &str) -> bool {
    cargo_bin()
        .and_then(|bin| bin.parent().map(|home| home.join(".crates2.json")))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|text| text.contains(&format!("\"{package} ")))
}

/// Programs and files from older setup guides: `cargo install`ed programs in `~/.cargo/bin`
/// (other than the one running) and the PowerShell launcher `start-magicpic.ps1`.
pub(crate) fn legacy(home: &Path) -> Vec<Value> {
    let me = same_file_key(&current_cli());
    let mut found = Vec::new();
    if let Some(dir) = cargo_bin() {
        for stem in ["magicpic-cli", "magicpic"] {
            let p = dir.join(exe_name(stem));
            if p.is_file() && same_file_key(&p) != me {
                found.push(json!({"path": p, "kind": "cargo install", "package": stem, "remove": format!("cargo uninstall {stem}")}));
            }
        }
    }
    let ps1 = home.join("start-magicpic.ps1");
    if ps1.is_file() {
        found.push(json!({"path": ps1, "kind": "old launcher", "remove": format!("delete {}", ps1.display())}));
    }
    found
}

/// Which clients still run `path` as a server.
fn registered_with(statuses: &[Value], path: &Path) -> Vec<String> {
    let key = same_file_key(path);
    let mut out = Vec::new();
    for c in statuses {
        for f in c["files"].as_array().into_iter().flatten() {
            for name in SERVERS {
                if f["servers"][name]["command"].as_str().is_some_and(|cmd| same_file_key(Path::new(cmd)) == key) {
                    let client = c["client"].as_str().unwrap_or("").to_string();
                    if !out.contains(&client) {
                        out.push(client);
                    }
                }
            }
        }
    }
    out
}

/// `setup --migrate`: point registrations that run another magicpic-cli at this one, then
/// remove each legacy item nothing uses any more. The person agreed before the agent ran it.
pub(crate) fn migrate(home: &Path, config: &Config) -> Value {
    let mut repointed = Vec::new();
    for client in [Client::ClaudeCode, Client::Codex, Client::ClaudeDesktop] {
        let status = client_status(client, None);
        let outdated = status["files"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|f| SERVERS.iter().any(|n| f["servers"][n]["registered"] == true && f["servers"][n]["thisCli"] == false));
        if outdated {
            repointed.push(match crate::register::repoint(client, config) {
                Ok(v) => json!({"client": client.name(), "result": v}),
                Err(e) => json!({"client": client.name(), "error": e.to_string()}),
            });
        }
    }
    let statuses: Vec<Value> = [Client::ClaudeDesktop, Client::ClaudeCode, Client::Codex].into_iter().map(|c| client_status(c, None)).collect();
    let processes = crate::procs::SystemTable::new(false).snapshot().unwrap_or_default();
    let items: Vec<Value> = legacy(home)
        .into_iter()
        .map(|item| {
            let path = PathBuf::from(item["path"].as_str().unwrap_or(""));
            let users = registered_with(&statuses, &path);
            if !users.is_empty() {
                return json!({"path": path, "removed": false, "why": format!("still registered with {}: re-register it first (see `repointed`)", users.join(", "))});
            }
            let key = same_file_key(&path);
            if processes.iter().any(|p| p.exe.as_deref().is_some_and(|e| same_file_key(e) == key)) {
                return json!({"path": path, "removed": false, "why": "still running (an agent client's old MCP server): quit or restart that client, then run `magicpic-cli setup --migrate` again"});
            }
            if let Some(package) = item["package"].as_str()
                && cargo_installed(package)
                && Command::new("cargo").args(["uninstall", package]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
            {
                return json!({"path": path, "removed": true, "how": format!("cargo uninstall {package}")});
            }
            match std::fs::remove_file(&path) {
                Ok(()) => json!({"path": path, "removed": true, "how": "deleted"}),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({"path": path, "removed": true, "how": "cargo uninstall"}),
                Err(e) => json!({"path": path, "removed": false, "why": e.to_string()}),
            }
        })
        .collect();
    json!({"repointed": repointed, "removed": items})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        vec!["magicpic".into(), "magicpic-cli".into()]
    }

    fn folder(dir: &Path, content: &str) {
        std::fs::create_dir_all(dir).unwrap();
        for n in names() {
            std::fs::write(dir.join(&n), format!("{content} {n}")).unwrap();
        }
    }

    #[test]
    fn programs_are_replaced_and_old_copies_cleaned_up() {
        let t = tempfile::tempdir().unwrap();
        let (from, bin) = (t.path().join("new"), t.path().join("bin"));
        folder(&from, "new");
        folder(&bin, "old");
        for aside in [false, true] {
            let placed = place(&from, &bin, &names(), aside, "20261113T165633Z").unwrap();
            assert_eq!(placed.len(), 2);
            for n in names() {
                assert_eq!(std::fs::read_to_string(bin.join(&n)).unwrap(), format!("new {n}"));
            }
            // Nothing is running here, so the old copies went at once.
            assert!(placed.iter().all(|p| p.old.is_none()));
            let left: Vec<String> = std::fs::read_dir(&bin).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            assert_eq!(left.len(), 2, "no staged or old files left: {left:?}");
        }
        // An old copy left by an earlier install (it was running then) is removed now.
        std::fs::write(bin.join("magicpic-cli.old-20260101T000000Z.exe"), "old").unwrap();
        let (gone, kept) = remove_old_copies(&bin);
        assert_eq!((gone.len(), kept.len()), (1, 0));
        assert_eq!(old_name("magicpic-cli.exe", "S"), "magicpic-cli.old-S.exe");
        assert_eq!(old_name("magicpic-cli", "S"), "magicpic-cli.old-S");
    }

    #[test]
    fn a_failed_copy_changes_nothing() {
        let t = tempfile::tempdir().unwrap();
        let (from, bin) = (t.path().join("new"), t.path().join("bin"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("magicpic"), "new").unwrap();
        // No new magicpic-cli: the copy fails after the first file was staged.
        folder(&bin, "old");
        assert!(place(&from, &bin, &names(), true, "S").is_err());
        for n in names() {
            assert_eq!(std::fs::read_to_string(bin.join(&n)).unwrap(), format!("old {n}"));
        }
        assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 2, "the staged copy was removed");
    }

    #[test]
    fn older_installs_are_found() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join(".magicpic");
        std::fs::create_dir_all(&home).unwrap();
        assert!(legacy(&home).iter().all(|v| v["kind"] != "old launcher"));
        std::fs::write(home.join("start-magicpic.ps1"), "Start-Process").unwrap();
        let found = legacy(&home);
        assert!(found.iter().any(|v| v["kind"] == "old launcher" && v["remove"].as_str().unwrap().starts_with("delete ")), "{found:?}");

        let statuses = vec![json!({"client": "claude-desktop", "files": [{"servers": {"magicpic": {"command": "/old/magicpic-cli"}}}]})];
        assert_eq!(registered_with(&statuses, Path::new("/old/magicpic-cli")), ["claude-desktop"]);
        assert!(registered_with(&statuses, Path::new("/new/magicpic-cli")).is_empty());
    }
}
