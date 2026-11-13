//! A Claude Desktop registration that `mcp register --restart` handed to its helper and that has
//! not been written yet: `~/.magicpic/register-pending.json`.
//!
//! The helper waits in the background for Claude Desktop to quit, so when it gives up nobody
//! sees it. The file stays behind until a registration is written, and `mcp status` and `doctor`
//! report it as a problem with its fix. Starting a new helper replaces the file, and an older
//! helper that finds someone else's id in it stops.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::setup::write_atomic;

const FILE: &str = "register-pending.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pending {
    /// Which helper owns it.
    pub(crate) id: String,
    /// `register` or `unregister`.
    pub(crate) verb: String,
    /// `waiting`, `gave-up` or `error`.
    pub(crate) state: String,
    pub(crate) since: String,
    pub(crate) deadline: String,
    #[serde(default)]
    pub(crate) helper_pid: Option<u32>,
    #[serde(default)]
    pub(crate) message: Option<String>,
    pub(crate) log: PathBuf,
}

pub(crate) fn path(home: &Path) -> PathBuf {
    home.join(FILE)
}

pub(crate) fn load(home: &Path) -> Option<Pending> {
    let text = std::fs::read_to_string(path(home)).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn save(home: &Path, p: &Pending) -> crate::Result {
    let mut text = serde_json::to_string_pretty(p).map_err(|e| crate::CliError::Other(e.to_string()))?;
    text.push('\n');
    write_atomic(&path(home), text.as_bytes())
}

/// The registration was written: nothing is pending any more.
pub(crate) fn clear(home: &Path) {
    let _ = std::fs::remove_file(path(home));
}

/// A new helper's id: unique enough to tell two helpers apart.
pub(crate) fn new_id() -> String {
    format!("{}-{}", chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ"), std::process::id())
}

pub(crate) const RETRY: &str =
    "run `magicpic-cli mcp register --client claude-desktop --restart` again, then quit Claude Desktop completely (tray icon > Quit); it opens again by itself";

/// What `mcp status` and `doctor` say about it: `{state, since, …, problem, fix}`, or `None`.
/// `helper_alive` tells whether a pid still runs (`None`: unknown).
pub(crate) fn report(home: &Path, helper_alive: impl Fn(u32) -> Option<bool>) -> Option<Value> {
    let p = load(home)?;
    let what = if p.verb == "unregister" { "removal" } else { "registration" };
    let alive = p.helper_pid.and_then(&helper_alive);
    let (problem, fix) = match p.state.as_str() {
        "waiting" if alive != Some(false) => (
            format!("Claude Desktop {what} pending: waiting since {} for Claude Desktop to quit (until {}); nothing is written before that", p.since, p.deadline),
            "ask the person to quit Claude Desktop completely (tray icon > Quit, not just the window); it opens again by itself. Then run `magicpic-cli doctor`".to_string(),
        ),
        "waiting" => (format!("Claude Desktop {what} pending: the helper that waited for Claude Desktop since {} stopped, and nothing was written", p.since), RETRY.to_string()),
        "gave-up" => (format!("Claude Desktop {what} pending: Claude Desktop was never restarted between {} and {}, so nothing was written", p.since, p.deadline), RETRY.to_string()),
        _ => (format!("Claude Desktop {what} pending: writing it failed ({})", p.message.as_deref().unwrap_or("see the log")), format!("fix that, then {RETRY}")),
    };
    Some(json!({
        "state": p.state,
        "verb": p.verb,
        "since": p.since,
        "deadline": p.deadline,
        "helperPid": p.helper_pid,
        "helperRunning": alive,
        "log": p.log,
        "problem": problem,
        "fix": fix,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(state: &str) -> Pending {
        Pending {
            id: "a".into(),
            verb: "register".into(),
            state: state.into(),
            since: "2026-11-13T09:06:00Z".into(),
            deadline: "2026-11-13T10:06:00Z".into(),
            helper_pid: Some(42),
            message: None,
            log: "/h/register.log".into(),
        }
    }

    #[test]
    fn a_helper_that_gave_up_leaves_a_problem_with_its_fix() {
        let dir = tempfile::tempdir().unwrap();
        assert!(report(dir.path(), |_| None).is_none());
        save(dir.path(), &pending("gave-up")).unwrap();
        let r = report(dir.path(), |_| Some(false)).unwrap();
        assert!(r["problem"].as_str().unwrap().contains("never restarted"), "{r}");
        assert!(r["fix"].as_str().unwrap().contains("mcp register --client claude-desktop --restart"), "{r}");
        assert!(r.to_string().is_ascii());

        save(dir.path(), &pending("waiting")).unwrap();
        let r = report(dir.path(), |_| Some(true)).unwrap();
        assert!(r["problem"].as_str().unwrap().contains("waiting since"), "{r}");
        assert!(r["fix"].as_str().unwrap().contains("quit Claude Desktop completely"), "{r}");
        // The helper is gone (killed, or the machine restarted): the same retry.
        let r = report(dir.path(), |_| Some(false)).unwrap();
        assert!(r["problem"].as_str().unwrap().contains("stopped"), "{r}");
        assert_eq!(r["fix"], RETRY);

        clear(dir.path());
        assert!(report(dir.path(), |_| None).is_none());
    }
}
