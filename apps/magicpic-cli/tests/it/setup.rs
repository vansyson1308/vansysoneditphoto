//! `setup`, `mcp register | unregister | status` and `doctor` as an agent runs them: one binary,
//! plain arguments, JSON out. Each run gets its own home folder, so nothing touches the
//! developer's real client configs.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The CLI with `home` as every home folder, so nothing touches the developer's real configs.
fn cli(home: &Path) -> Command {
    let mut c = Command::new(crate::binary());
    c.env("HOME", home)
        .env("USERPROFILE", home)
        .env("APPDATA", home.join("AppData/Roaming"))
        .env("LOCALAPPDATA", home.join("AppData/Local"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("MAGICPIC_HOME", home.join(".magicpic"))
        .env("CARGO_HOME", home.join(".cargo"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("MAGICPIC_CONTROL_PORT")
        .env_remove("MAGICPIC_TEST_PROCESSES")
        .stdin(Stdio::null());
    c
}

fn run(home: &Path, args: &[&str]) -> (i32, Value, String) {
    let o = Command::new(crate::binary())
        .args(args)
        .env("CARGO_HOME", home.join(".cargo"))
        .env_remove("MAGICPIC_TEST_PROCESSES")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("APPDATA", home.join("AppData/Roaming"))
        .env("LOCALAPPDATA", home.join("AppData/Local"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("MAGICPIC_HOME", home.join(".magicpic"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("MAGICPIC_CONTROL_PORT")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    let json = serde_json::from_str(&stdout).unwrap_or(Value::Null);
    (o.status.code().unwrap_or(-1), json, String::from_utf8_lossy(&o.stderr).into_owned())
}

#[test]
fn setup_register_status_unregister_and_doctor() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let app = h.join(if cfg!(windows) { "magicpic.exe" } else { "magicpic" });
    std::fs::write(&app, b"").unwrap();
    let photos = h.join("My Photos");

    let (code, setup, err) = run(h, &["setup", "--app", app.to_str().unwrap(), "--photos", photos.to_str().unwrap(), "--port", "7899"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(setup["ok"], true, "{setup}");
    assert!(photos.is_dir(), "the photo folder was created");
    let launcher = Path::new(setup["launcher"].as_str().unwrap());
    assert!(launcher.is_file());
    assert!(std::fs::read_to_string(launcher).unwrap().contains(" launch"), "the launcher runs `magicpic-cli launch`");
    let config: Value = serde_json::from_str(&std::fs::read_to_string(h.join(".magicpic/config.json")).unwrap()).unwrap();
    assert_eq!(config["port"], 7899);
    assert_eq!(Path::new(config["photos"].as_str().unwrap()), photos);

    // Claude Code and Codex: written at once. A pre-existing config keeps its other entries.
    std::fs::write(h.join(".claude.json"), "\u{feff}{\"numStartups\":4,\"mcpServers\":{\"keep\":{\"command\":\"k\"}}}").unwrap();
    for client in ["claude-code", "codex"] {
        let (code, reply, err) = run(h, &["mcp", "register", "--client", client]);
        assert_eq!(code, 0, "{client}: {err}");
        assert_eq!(reply["status"], "written", "{reply}");
    }
    let claude_code = std::fs::read(h.join(".claude.json")).unwrap();
    assert!(!claude_code.starts_with(b"\xEF\xBB\xBF"), "no byte order mark");
    let claude_code: Value = serde_json::from_slice(&claude_code).unwrap();
    assert_eq!(claude_code["numStartups"], 4);
    assert_eq!(claude_code["mcpServers"]["keep"]["command"], "k");
    assert!(claude_code["mcpServers"]["magicpic-app"]["args"].as_array().unwrap().contains(&Value::from("127.0.0.1:7899")));
    assert!(std::fs::read_dir(h).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".claude.json.magicpic-backup-")));
    assert!(std::fs::read_to_string(h.join(".codex/config.toml")).unwrap().contains("tool_timeout_sec = 300"));

    // Claude Desktop, with an explicit file (the real app is not running in a test).
    let desktop = h.join("claude_desktop_config.json");
    let (code, reply, err) = run(h, &["mcp", "register", "--client", "claude-desktop", "--config", desktop.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(reply["status"], "written", "{reply}");

    let (code, status, err) = run(h, &["mcp", "status", "--client", "claude-code"]);
    assert_eq!(code, 0, "{err}");
    let c = &status["clients"][0];
    assert_eq!((&c["registered"], &c["pathsOk"]), (&Value::Bool(true), &Value::Bool(true)), "{status}");

    // The photo folder goes away: status says which path is missing.
    std::fs::remove_dir(&photos).unwrap();
    let (_, status, _) = run(h, &["mcp", "status", "--client", "claude-code"]);
    assert_eq!(status["clients"][0]["pathsOk"], false, "{status}");
    std::fs::create_dir(&photos).unwrap();

    // Doctor: nothing listens on 7899, so it reports that (exit 1), but both servers initialize
    // and no token is printed.
    let (code, doctor, _) = run(h, &["doctor", "--timeout", "60"]);
    assert_eq!(code, 1);
    assert_eq!(doctor["control"]["listening"], false, "{doctor}");
    let servers = doctor["servers"].as_array().unwrap();
    assert!(!servers.is_empty() && servers.iter().all(|s| s["initialize"] == true), "{doctor}");
    assert!(doctor["problems"].as_array().unwrap().iter().any(|p| p["problem"].as_str().unwrap().contains("not running")), "{doctor}");

    let (code, _, err) = run(h, &["mcp", "unregister", "--client", "claude-code"]);
    assert_eq!(code, 0, "{err}");
    let claude_code: Value = serde_json::from_str(&std::fs::read_to_string(h.join(".claude.json")).unwrap()).unwrap();
    assert!(claude_code["mcpServers"].get("magicpic").is_none() && claude_code["mcpServers"]["keep"].is_object(), "{claude_code}");
}

#[test]
fn a_config_that_is_not_json_is_never_replaced() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let file = h.join("broken.json");
    std::fs::write(&file, "{\"mcpServers\": ").unwrap();
    let (code, _, err) = run(h, &["mcp", "register", "--client", "claude-code", "--config", file.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.contains("not valid JSON") && err.contains("left unchanged"), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"mcpServers\": ");
}

/// `cli` with its output parsed, under a test process list.
fn run_with(home: &Path, procs: Option<&Path>, args: &[&str]) -> (i32, Value, String) {
    let mut c = cli(home);
    if let Some(p) = procs {
        c.env("MAGICPIC_TEST_PROCESSES", p);
    }
    let o = c.args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    (o.status.code().unwrap_or(-1), serde_json::from_str(&stdout).unwrap_or(Value::Null), String::from_utf8_lossy(&o.stderr).into_owned())
}

/// A process list in which Claude Desktop runs: a main process and two helpers, named as on
/// this system.
fn claude_running() -> String {
    let (name, exe) = if cfg!(windows) {
        ("claude.exe", r"C:\Program Files\WindowsApps\Claude_1.2.3.0_x64__pzs8sxrjxfjjc\app\claude.exe")
    } else if cfg!(target_os = "macos") {
        ("Claude", "/Applications/Claude.app/Contents/MacOS/Claude")
    } else {
        ("claude-desktop", "/usr/bin/claude-desktop")
    };
    json!([
        {"pid": 20, "parent": 4, "name": name, "exe": exe, "start": 100},
        {"pid": 21, "parent": 20, "name": name, "exe": exe, "start": 101},
        {"pid": 22, "parent": 20, "name": name, "exe": exe, "start": 101},
    ])
    .to_string()
}

fn until(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let t = Instant::now();
    while !ok() {
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out after {secs}s waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn set_up(h: &Path) -> PathBuf {
    let app = h.join(if cfg!(windows) { "magicpic.exe" } else { "magicpic" });
    std::fs::write(&app, b"").unwrap();
    let photos = h.join("Photos");
    let (code, _, err) = run(h, &["setup", "--app", app.to_str().unwrap(), "--photos", photos.to_str().unwrap(), "--port", "7897"]);
    assert_eq!(code, 0, "{err}");
    photos
}

fn pending(h: &Path) -> Option<Value> {
    std::fs::read_to_string(h.join(".magicpic/register-pending.json")).ok().and_then(|t| serde_json::from_str(&t).ok())
}

/// Bug: the helper `--restart` starts inherited this process's output pipe on Windows, so a
/// caller reading our output to the end (`... | Out-String`) hung until Claude Desktop quit.
/// And: it wrote while Claude Desktop still ran.
#[test]
fn register_restart_returns_at_once_and_writes_only_once_claude_desktop_has_quit() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    set_up(h);
    let procs = h.join("procs.json");
    std::fs::write(&procs, claude_running()).unwrap();
    let desktop = h.join("claude_desktop_config.json");

    let t = Instant::now();
    let child = cli(h)
        .env("MAGICPIC_TEST_PROCESSES", &procs)
        .args(["mcp", "register", "--client", "claude-desktop", "--config", desktop.to_str().unwrap(), "--restart", "--wait-minutes", "5"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Read to the end of the output, as a caller capturing it does.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = rx.recv_timeout(Duration::from_secs(30)).expect("the output reached its end while the helper still waits").unwrap();
    assert!(t.elapsed() < Duration::from_secs(30));
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    assert_eq!(reply["status"], "waiting", "{reply} {}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stdout.is_ascii(), "machine-facing output is ASCII");
    assert_eq!(pending(h).unwrap()["state"], "waiting");

    // The helper runs and records itself; while Claude Desktop runs it writes nothing.
    until("the helper's pid", 20, || pending(h).is_some_and(|p| p["helperPid"].is_u64()));
    let (_, status, _) = run_with(h, Some(&procs), &["mcp", "status", "--client", "claude-desktop"]);
    let c = &status["clients"][0];
    assert_eq!(c["pending"]["state"], "waiting", "{status}");
    assert_eq!(c["pending"]["helperRunning"], true, "{status}");
    std::thread::sleep(Duration::from_secs(4));
    assert!(!desktop.exists(), "nothing is written while Claude Desktop runs");

    // Claude Desktop quits: written after the grace period, reopened, and checked.
    std::fs::write(&procs, "[]").unwrap();
    until("the config to be written", 30, || desktop.is_file());
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&desktop).unwrap()).unwrap();
    assert!(v["mcpServers"]["magicpic-app"].is_object(), "{v}");
    until("the pending registration to clear", 30, || pending(h).is_none());
    let log = std::fs::read_to_string(h.join(".magicpic/register.log")).unwrap();
    assert!(log.contains("reopened Claude Desktop") && log.contains("new main process"), "{log}");
}

/// Bug: a helper that gave up left no trace but its log.
#[test]
fn a_helper_that_gives_up_leaves_a_pending_registration_that_status_and_doctor_report() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    set_up(h);
    let procs = h.join("procs.json");
    std::fs::write(&procs, claude_running()).unwrap();
    let (code, reply, err) = run_with(h, Some(&procs), &["mcp", "register", "--client", "claude-desktop", "--restart", "--timeout", "2"]);
    assert_eq!((code, &reply["status"]), (0, &json!("waiting")), "{reply} {err}");
    until("the helper to give up", 40, || pending(h).is_some_and(|p| p["state"] == "gave-up"));

    let (_, status, _) = run_with(h, Some(&procs), &["mcp", "status", "--client", "claude-desktop"]);
    let problem = &status["clients"][0]["problems"][0];
    assert!(problem["problem"].as_str().unwrap().contains("Claude Desktop was never restarted"), "{status}");
    assert!(problem["fix"].as_str().unwrap().contains("mcp register --client claude-desktop --restart"), "{status}");
    let (code, doctor, _) = run_with(h, Some(&procs), &["doctor", "--timeout", "60"]);
    assert_eq!(code, 1);
    assert!(doctor["problems"].as_array().unwrap().iter().any(|p| p["problem"].as_str().unwrap().contains("never restarted")), "{doctor}");

    // Registered later while Claude Desktop is closed: nothing is pending any more.
    std::fs::write(&procs, "[]").unwrap();
    let (code, reply, err) = run_with(h, Some(&procs), &["mcp", "register", "--client", "claude-desktop"]);
    assert_eq!((code, &reply["status"]), (0, &json!("written")), "{reply} {err}");
    assert!(pending(h).is_none());
}

/// Bug: doctor said "registered" while the client ran an older magicpic-cli.
#[test]
fn doctor_flags_a_registration_that_runs_another_cli_or_an_old_flag_set() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let photos = set_up(h);
    let old = h.join("old").join(if cfg!(windows) { "magicpic-cli.exe" } else { "magicpic-cli" });
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::copy(crate::binary(), &old).unwrap();
    let photos = photos.to_str().unwrap();
    let token = h.join(".magicpic/control.token");
    std::fs::write(
        h.join(".claude.json"),
        json!({"mcpServers": {
            "magicpic": {"type": "stdio", "command": old, "args": ["mcp", "--automation-read-root", photos, "--automation-write-root", photos]},
            "magicpic-app": {"type": "stdio", "command": old, "args": ["mcp", "--bridge", "127.0.0.1:7897", "--control-token-file", token]},
        }})
        .to_string(),
    )
    .unwrap();

    let (_, status, _) = run(h, &["mcp", "status", "--client", "claude-code"]);
    let c = &status["clients"][0];
    assert_eq!((&c["registered"], &c["pathsOk"]), (&json!(true), &json!(true)), "{status}");
    assert_eq!(c["files"][0]["servers"]["magicpic"]["thisCli"], false, "{status}");
    let texts: Vec<String> = c["problems"].as_array().unwrap().iter().map(|p| p["problem"].as_str().unwrap().to_string()).collect();
    assert!(texts.iter().any(|t| t.contains(old.to_str().unwrap()) && t.contains("(magicpic-cli ") && t.contains(" instead of ")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("--start-app-if-needed")), "{texts:?}");
    let (_, doctor, _) = run(h, &["doctor", "--timeout", "60"]);
    let problems = doctor["problems"].as_array().unwrap();
    let fix = problems.iter().find(|p| p["problem"].as_str().unwrap().contains(" instead of ")).map(|p| p["fix"].clone());
    assert_eq!(fix, Some(json!("magicpic-cli mcp register --client claude-code")), "{doctor}");

    // Registered again with this CLI: both problems are gone.
    let (code, _, err) = run(h, &["mcp", "register", "--client", "claude-code"]);
    assert_eq!(code, 0, "{err}");
    let (_, status, _) = run(h, &["mcp", "status", "--client", "claude-code"]);
    assert_eq!(status["clients"][0]["problems"], json!([]), "{status}");
}

/// Bug: copying over a running magicpic-cli.exe failed ("being used by another process").
#[test]
fn install_replaces_programs_while_they_run() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    let new = h.join("new");
    std::fs::create_dir_all(&new).unwrap();
    for name in ["magicpic", "magicpic-cli"] {
        std::fs::copy(crate::binary(), new.join(if cfg!(windows) { format!("{name}.exe") } else { name.to_string() })).unwrap();
    }
    let (code, first, err) = run(h, &["install", "--from", new.to_str().unwrap(), "--keep-app-running"]);
    assert_eq!(code, 0, "{err}");
    let installed = PathBuf::from(first["cli"].as_str().unwrap());
    assert!(installed.is_file(), "{first}");

    // An MCP server runs from the installed CLI, as an agent client keeps it running.
    let mut server_from_bin = Command::new(&installed).arg("mcp").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let (code, second, err) = run(h, &["install", "--from", new.to_str().unwrap(), "--keep-app-running"]);
    assert_eq!(code, 0, "replacing a running program: {err}");
    assert_eq!(second["ok"], true, "{second}");
    if cfg!(windows) {
        assert_eq!(second["oldCopiesInUse"].as_array().unwrap().len(), 1, "{second}");
    }
    let _ = server_from_bin.kill();
    let _ = server_from_bin.wait();
    // Windows releases an ended program's file a moment after the process is gone.
    std::thread::sleep(Duration::from_secs(1));
    // The next install deletes the old copy that is no longer running.
    let (code, third, err) = run(h, &["install", "--from", new.to_str().unwrap(), "--keep-app-running"]);
    assert_eq!(code, 0, "{err}");
    assert!(third["oldCopiesInUse"].as_array().unwrap().is_empty(), "{third}");
    let left: Vec<String> = std::fs::read_dir(installed.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(left.len(), 2, "{left:?}");
}

#[test]
fn setup_lists_an_older_install_and_migrate_removes_what_nothing_uses() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    set_up(h);
    let cargo_cli = h.join(".cargo/bin").join(if cfg!(windows) { "magicpic-cli.exe" } else { "magicpic-cli" });
    std::fs::create_dir_all(cargo_cli.parent().unwrap()).unwrap();
    std::fs::write(&cargo_cli, b"old").unwrap();
    let ps1 = h.join(".magicpic/start-magicpic.ps1");
    std::fs::write(&ps1, "Start-Process magicpic").unwrap();
    std::fs::write(
        h.join(".claude.json"),
        json!({"mcpServers": {"magicpic": {"command": cargo_cli, "args": ["mcp"]}, "magicpic-app": {"command": cargo_cli, "args": ["mcp"]}}}).to_string(),
    )
    .unwrap();

    let (code, setup, err) = run(h, &["setup"]);
    assert_eq!(code, 0, "{err}");
    let legacy: Vec<&str> = setup["legacy"].as_array().unwrap().iter().map(|l| l["path"].as_str().unwrap()).collect();
    assert_eq!(legacy.len(), 2, "{setup}");
    assert!(cargo_cli.is_file() && ps1.is_file(), "setup alone removes nothing");

    let (code, setup, err) = run(h, &["setup", "--migrate"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(setup["migrated"]["repointed"][0]["client"], "claude-code", "{setup}");
    assert!(!cargo_cli.exists() && !ps1.exists(), "{setup}");
    assert_eq!(setup["legacy"], json!([]), "{setup}");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(h.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(v["mcpServers"]["magicpic"]["command"].as_str().map(PathBuf::from), Some(setup["cli"].as_str().map(PathBuf::from).unwrap()), "{v}");
}
