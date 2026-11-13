//! `magicpic-cli doctor`: one JSON report an agent can read after setup and whenever something
//! fails. Every check says what it found and, when it failed, what to run. The control token is
//! used to authenticate but never printed.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::args::{Client, DoctorArgs};
use crate::output::print_json_ascii;
use crate::procs::{self, ProcessTable};
use crate::register::{SERVERS, client_status, entries, own_version, same_file_key};
use crate::setup::{Config, listening, magicpic_home};
use crate::{CliError, Result};

/// A problem and the command or step that fixes it.
fn problem(list: &mut Vec<Value>, what: impl Into<String>, fix: impl Into<String>) {
    list.push(json!({"problem": what.into(), "fix": fix.into()}));
}

/// Start `command args` as an MCP server and run `initialize`, `notifications/initialized` and
/// `tools/list`. Returns the number of tools, or why it failed.
pub(crate) fn handshake(command: &Path, args: &[String], timeout: Duration) -> std::result::Result<usize, String> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", command.display()))?;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut send = |v: Value| writeln!(stdin, "{v}").and_then(|()| stdin.flush()).map_err(|e| format!("writing to the server: {e}"));
        let reply = |id: u64| -> std::result::Result<Value, String> {
            loop {
                let line = rx.recv_timeout(timeout).map_err(|_| format!("no reply to request {id} within {}s", timeout.as_secs()))?;
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v["id"] == id {
                    return match v.get("error") {
                        Some(e) => Err(format!("error reply: {e}")),
                        None => Ok(v),
                    };
                }
            }
        };
        send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "magicpic-doctor", "version": env!("CARGO_PKG_VERSION")}
        }}))?;
        reply(1)?;
        send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))?;
        let tools = reply(2)?;
        Ok(tools["result"]["tools"].as_array().map_or(0, Vec::len))
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Authenticate to the open app and read its session, without printing the token.
fn app_session(config: &Config) -> std::result::Result<Value, String> {
    let client = magicpic_automation::BridgeClient::with_token_file(format!("127.0.0.1:{}", config.port), &config.token_file)
        .map_err(|e| e.to_string())?
        .with_timeout(Duration::from_secs(10));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(client.call_read("engine.execute", json!({"command": "session.inspect"}))).map_err(|e| e.to_string())
}

/// Whether a file can be created in `dir` (and is removed again).
fn writable(dir: &Path) -> bool {
    let probe = dir.join(".magicpic-doctor-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

pub(crate) fn doctor(a: &DoctorArgs, out: &mut dyn Write) -> Result {
    let timeout = Duration::from_secs(a.timeout.unwrap_or(20));
    let home = magicpic_home()?;
    let configured = Config::load(&home)?;
    let config = match &configured {
        Some(c) => c.clone(),
        None => Config::defaults(&home)?,
    };
    let mut problems = Vec::new();
    if configured.is_none() {
        problem(&mut problems, format!("{} has no config.json", home.display()), "magicpic-cli setup");
    }

    let cli = json!({"path": crate::setup::current_cli(), "version": env!("CARGO_PKG_VERSION"), "build": magicpic_engine::build_info::long_version()});
    let app_exists = config.app.is_file();
    if !app_exists {
        problem(
            &mut problems,
            format!("the app is not at {}", config.app.display()),
            "install it (install.ps1 / install.sh) or run `magicpic-cli setup --app <path>`",
        );
    }

    let is_listening = listening(config.port);
    let token_exists = config.token_file.is_file();
    let mut control = json!({"port": config.port, "listening": is_listening, "tokenFile": config.token_file, "tokenFileExists": token_exists});
    if is_listening && token_exists {
        match app_session(&config) {
            Ok(session) => {
                control["authenticated"] = json!(true);
                control["documents"] = json!(session["documents"].as_array().map_or(0, Vec::len));
            }
            Err(e) => {
                control["authenticated"] = json!(false);
                control["error"] = json!(e);
                problem(&mut problems, "the app listens but refused this token", "quit the app, then run the launcher again so it writes a fresh token");
            }
        }
    } else if !is_listening {
        problem(
            &mut problems,
            format!("the app is not running on 127.0.0.1:{}", config.port),
            format!("run the launcher {} (or `magicpic-cli launch`)", config.launcher.display()),
        );
    } else {
        problem(
            &mut problems,
            format!("something listens on {} but the token file {} is missing", config.port, config.token_file.display()),
            "another program may hold the port: run `magicpic-cli setup --port 7879`, then `mcp register` again",
        );
    }

    let photos_exists = config.photos.is_dir();
    let roots = json!([{"path": config.photos, "read": true, "write": true, "exists": photos_exists, "writable": photos_exists && writable(&config.photos)}]);
    if !photos_exists {
        problem(&mut problems, format!("the photo folder {} does not exist", config.photos.display()), "magicpic-cli setup");
    }

    let clients: Vec<Value> = [Client::ClaudeDesktop, Client::ClaudeCode, Client::Codex].into_iter().map(|c| client_status(c, None)).collect();
    if !clients.iter().any(|c| c["registered"] == true) {
        problem(&mut problems, "no agent client has the MagicPic servers", "magicpic-cli mcp register --client <claude-desktop|claude-code|codex>");
    }
    for c in &clients {
        if c["registered"] == true && c["pathsOk"] != true {
            problem(
                &mut problems,
                format!("{}: a registered path no longer exists", c["client"].as_str().unwrap_or("")),
                format!("magicpic-cli mcp register --client {}", c["client"].as_str().unwrap_or("")),
            );
        }
        // A pending Claude Desktop registration, another CLI than this one, an old flag set.
        for p in c["problems"].as_array().into_iter().flatten() {
            if !problems.contains(p) {
                problems.push(p.clone());
            }
        }
    }

    // The MCP servers that run right now: started from an older binary, they keep serving the old
    // tools until their client restarts, whatever the config files say now.
    let me = same_file_key(&crate::setup::current_cli());
    let running: Vec<Value> = procs::SystemTable::new(true)
        .snapshot()
        .map(|list| procs::mcp_servers(&list))
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let mut v = procs::describe(p);
            // Unknown when the system hides the path (another user's process): not counted.
            v["thisCli"] = p.exe.as_deref().map_or(Value::Null, |e| json!(same_file_key(e) == me));
            v
        })
        .collect();
    let outdated: BTreeSet<String> = running.iter().filter(|p| p["thisCli"] == false).filter_map(|p| p["exe"].as_str().map(str::to_string)).collect();
    if !outdated.is_empty() {
        problem(
            &mut problems,
            format!(
                "{} MagicPic MCP server process(es) still run another binary ({}) than this install ({}), so the agent client still sees the old tools",
                running.iter().filter(|p| p["thisCli"] == false).count(),
                outdated.into_iter().collect::<Vec<_>>().join(", "),
                own_version()
            ),
            "restart the agent client so it starts the new servers (Claude Desktop: tray icon > Quit, then open it again)",
        );
    }

    let legacy = crate::update::legacy(&home);
    if !legacy.is_empty() {
        let paths: Vec<&str> = legacy.iter().filter_map(|l| l["path"].as_str()).collect();
        let how: Vec<&str> = legacy.iter().filter_map(|l| l["remove"].as_str()).collect();
        problem(
            &mut problems,
            format!("an older MagicPic install is still here: {}", paths.join(", ")),
            format!("ask the person, then run `magicpic-cli setup --migrate` (or by hand: {})", how.join("; ")),
        );
    }

    // Each distinct registered server, else the ones `register` would write.
    let mut seen = BTreeSet::new();
    let mut to_check: Vec<(String, String, Vec<String>)> = Vec::new();
    for c in &clients {
        for f in c["files"].as_array().into_iter().flatten() {
            for name in SERVERS {
                let s = &f["servers"][name];
                let (Some(cmd), Some(args)) = (s["command"].as_str(), s["args"].as_array()) else { continue };
                let args: Vec<String> = args.iter().filter_map(Value::as_str).map(str::to_string).collect();
                if seen.insert((cmd.to_string(), args.clone())) {
                    to_check.push((name.to_string(), cmd.to_string(), args));
                }
            }
        }
    }
    if to_check.is_empty() {
        for e in entries(&config, false) {
            to_check.push((e.name.to_string(), e.command.to_string_lossy().into_owned(), e.args));
        }
    }
    let servers: Vec<Value> = to_check
        .into_iter()
        .map(|(name, cmd, args)| match handshake(Path::new(&cmd), &args, timeout) {
            Ok(tools) => json!({"name": name, "command": cmd, "initialize": true, "tools": tools}),
            Err(e) => {
                problem(&mut problems, format!("the `{name}` MCP server did not start: {e}"), "magicpic-cli mcp register --client <client>");
                json!({"name": name, "command": cmd, "initialize": false, "error": e})
            }
        })
        .collect();

    let report = json!({
        "ok": problems.is_empty(),
        "home": home,
        "configured": configured.is_some(),
        "cli": cli,
        "app": {"path": config.app, "exists": app_exists, "launcher": config.launcher, "launcherExists": config.launcher.is_file()},
        "control": control,
        "roots": roots,
        "servers": servers,
        "runningServers": running,
        "clients": clients,
        "legacy": legacy,
        "problems": problems,
    });
    print_json_ascii(out, &report)?;
    if report["ok"] == true { Ok(()) } else { Err(CliError::Other("doctor found problems (see `problems`)".into())) }
}
