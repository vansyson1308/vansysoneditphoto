//! `setup`, `mcp register | unregister | status` and `doctor` as an agent runs them: one binary,
//! plain arguments, JSON out. Each run gets its own home folder, so nothing touches the
//! developer's real client configs.

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

fn run(home: &Path, args: &[&str]) -> (i32, Value, String) {
    let o = Command::new(crate::binary())
        .args(args)
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
