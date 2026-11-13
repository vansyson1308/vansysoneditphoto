//! The process list, for `mcp register --restart` (is Claude Desktop still running? did it start
//! again?) and `doctor` (which MCP servers run right now, and from which binary).
//!
//! Electron apps such as Claude Desktop run as many processes: a main one plus renderer, GPU and
//! utility helpers that come and go while the app stays open. Claude Desktop counts as running
//! while **any** of its processes is alive, and as started again only when a new main process
//! (one whose parent is not itself a Claude Desktop process) appears.
//!
//! A list that could not be read is `None`, never "nothing runs": a failed probe must not look
//! like an app that quit.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One process, as much of it as the system tells.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Proc {
    pub(crate) pid: u32,
    #[serde(default)]
    pub(crate) parent: Option<u32>,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) exe: Option<PathBuf>,
    #[serde(default)]
    pub(crate) cmd: Vec<String>,
    /// When it started, in seconds since the Unix epoch.
    #[serde(default)]
    pub(crate) start: u64,
}

/// Where processes come from: the system, or a test's file.
pub(crate) trait ProcessTable {
    /// Every process now, or `None` when the list could not be read.
    fn snapshot(&mut self) -> Option<Vec<Proc>>;
}

/// Test-only: a JSON file (`[Proc, …]`) read in place of the system's process list by
/// `mcp register` and its helper, so tests can play "Claude Desktop is running" without it. The
/// helper's "open Claude again" then adds a new main process to that file instead.
pub(crate) const FAKE_ENV: &str = "MAGICPIC_TEST_PROCESSES";

/// The system's processes, through `sysinfo` (no PowerShell, no `unsafe` here).
pub(crate) struct SystemTable {
    sys: Option<sysinfo::System>,
    with_cmd: bool,
}

impl SystemTable {
    /// `with_cmd` also reads command lines (slower; `doctor` needs them, waiting does not).
    pub(crate) fn new(with_cmd: bool) -> SystemTable {
        SystemTable { sys: None, with_cmd }
    }
}

impl ProcessTable for SystemTable {
    fn snapshot(&mut self) -> Option<Vec<Proc>> {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
        if self.sys.is_none() {
            self.sys = sysinfo::System::new().ok();
        }
        let sys = self.sys.as_mut()?;
        let mut kind = ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet);
        if self.with_cmd {
            kind = kind.with_cmd(UpdateKind::OnlyIfNotSet);
        }
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        let list: Vec<Proc> = sys
            .processes()
            .values()
            .map(|p| Proc {
                pid: p.pid().as_u32(),
                parent: p.parent().map(|pp| pp.as_u32()),
                name: p.name().to_string_lossy().into_owned(),
                exe: p.exe().map(Path::to_path_buf),
                cmd: p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect(),
                start: p.start_time(),
            })
            .collect();
        // A list without this very process was not read properly: say so rather than "nothing runs".
        let me = std::process::id();
        list.iter().any(|p| p.pid == me).then_some(list)
    }
}

/// A test's process list (see [`FAKE_ENV`]). `{"unreadable": true}` plays a failed probe.
pub(crate) struct FakeTable {
    pub(crate) path: PathBuf,
}

impl FakeTable {
    pub(crate) fn from_env() -> Option<FakeTable> {
        std::env::var_os(FAKE_ENV).filter(|v| !v.is_empty()).map(|p| FakeTable { path: PathBuf::from(p) })
    }

    fn read(&self) -> Option<Vec<Proc>> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// What "open Claude Desktop again" does under test: a new main process, started now.
    pub(crate) fn start_desktop(&self) -> std::io::Result<()> {
        let mut list = self.read().unwrap_or_default();
        let pid = list.iter().map(|p| p.pid).max().unwrap_or(1000).saturating_add(1);
        list.push(Proc { pid, parent: Some(1), name: DESKTOP_NAME.into(), exe: Some(PathBuf::from(DESKTOP_EXE)), cmd: Vec::new(), start: unix_now() });
        let text = serde_json::to_string(&list).map_err(std::io::Error::other)?;
        std::fs::write(&self.path, text)
    }
}

impl ProcessTable for FakeTable {
    fn snapshot(&mut self) -> Option<Vec<Proc>> {
        self.read()
    }
}

/// The system's table, or a test's when [`FAKE_ENV`] is set.
pub(crate) fn table(with_cmd: bool) -> Box<dyn ProcessTable> {
    match FakeTable::from_env() {
        Some(fake) => Box::new(fake),
        None => Box::new(SystemTable::new(with_cmd)),
    }
}

/// Whether `pid` is alive, from the system's own list (never the test's): `None` when unknown.
pub(crate) fn pid_alive(pid: u32) -> Option<bool> {
    SystemTable::new(false).snapshot().map(|list| list.iter().any(|p| p.pid == pid))
}

pub(crate) fn unix_now() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Claude Desktop
// ---------------------------------------------------------------------------

/// The system whose naming rules apply; a parameter so tests check every system's rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    Windows,
    Macos,
    Linux,
}

pub(crate) const OS: Os = if cfg!(windows) {
    Os::Windows
} else if cfg!(target_os = "macos") {
    Os::Macos
} else {
    Os::Linux
};

/// How a Claude Desktop main process looks on this system (the fake table's "started again").
const DESKTOP_NAME: &str = match OS {
    Os::Windows => "claude.exe",
    Os::Macos => "Claude",
    Os::Linux => "claude-desktop",
};
const DESKTOP_EXE: &str = match OS {
    Os::Windows => r"C:\Program Files\WindowsApps\Claude_1.0.0.0_x64__pzs8sxrjxfjjc\app\claude.exe",
    Os::Macos => "/Applications/Claude.app/Contents/MacOS/Claude",
    Os::Linux => "/usr/bin/claude-desktop",
};

/// Whether a `claude.exe` at `path` is the desktop app rather than the Claude Code CLI, which
/// has the same name on Windows: the Store package folder (`WindowsApps\Claude_*`) or the
/// non-Store install (`AnthropicClaude`, `Programs\Claude`). An unknown path counts as the app,
/// so a process whose path the system hides keeps the wait going rather than ending it.
pub(crate) fn is_desktop_exe(path: &str) -> bool {
    let p = path.to_ascii_lowercase().replace('/', "\\");
    if p.is_empty() {
        return true;
    }
    !(p.contains("\\.local\\") || p.contains("node_modules") || p.contains("\\npm\\") || p.contains("\\.bun\\"))
        && (p.contains("\\windowsapps\\claude_") || p.contains("anthropicclaude") || p.contains("\\programs\\claude\\") || p.ends_with("\\claude\\claude.exe"))
}

/// Whether `p` is one of Claude Desktop's processes (main or helper) on `os`. macOS: anything
/// running from inside a `Claude.app` bundle (the main program and every `Claude Helper`).
pub(crate) fn is_claude_desktop(p: &Proc, os: Os) -> bool {
    let exe = p.exe.as_ref().map(|e| e.to_string_lossy().into_owned());
    match os {
        Os::Windows => p.name.eq_ignore_ascii_case("claude.exe") && is_desktop_exe(exe.as_deref().unwrap_or("")),
        Os::Macos => match &exe {
            Some(e) => e.contains("/Claude.app/Contents/"),
            None => p.name == "Claude" || p.name.starts_with("Claude Helper"),
        },
        Os::Linux => p.name == "claude-desktop" || exe.as_deref().is_some_and(|e| e.ends_with("/claude-desktop")),
    }
}

/// Claude Desktop's processes in `list`.
pub(crate) fn desktop(list: &[Proc], os: Os) -> Vec<Proc> {
    list.iter().filter(|p| is_claude_desktop(p, os)).cloned().collect()
}

/// The main processes among `app` (one app's processes): those whose parent is not one of them.
pub(crate) fn main_processes(app: &[Proc]) -> Vec<Proc> {
    app.iter().filter(|p| !p.parent.is_some_and(|parent| app.iter().any(|q| q.pid == parent))).cloned().collect()
}

// ---------------------------------------------------------------------------
// MCP servers that run right now (doctor)
// ---------------------------------------------------------------------------

/// `magicpic-cli mcp …` server processes in `list` (not `mcp register` and its helper).
pub(crate) fn mcp_servers(list: &[Proc]) -> Vec<Proc> {
    let me = std::process::id();
    list.iter()
        .filter(|p| p.pid != me)
        .filter(|p| {
            let stem = Path::new(&p.name).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase());
            stem.as_deref() == Some("magicpic-cli")
        })
        .filter(|p| {
            let mut args = p.cmd.iter().skip(1);
            args.next().map(String::as_str) == Some("mcp") && !matches!(args.next().map(String::as_str), Some("register" | "unregister" | "status"))
        })
        .cloned()
        .collect()
}

/// A short JSON view of `p` (no command line: paths only, never a token).
pub(crate) fn describe(p: &Proc) -> Value {
    json!({"pid": p.pid, "exe": p.exe, "started": chrono::DateTime::from_timestamp(i64::try_from(p.start).unwrap_or(0), 0).map(|t| t.to_rfc3339())})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, parent: u32, name: &str, exe: &str) -> Proc {
        Proc { pid, parent: Some(parent), name: name.into(), exe: (!exe.is_empty()).then(|| exe.into()), cmd: Vec::new(), start: 100 }
    }

    #[test]
    fn windows_process_paths_tell_the_desktop_app_from_claude_code() {
        assert!(is_desktop_exe(r"C:\Program Files\WindowsApps\Claude_1.2.3.0_x64__pzs8sxrjxfjjc\app\claude.exe"));
        assert!(is_desktop_exe(r"C:\Users\a\AppData\Local\AnthropicClaude\app-0.9.0\claude.exe"));
        assert!(!is_desktop_exe(r"C:\Users\a\.local\bin\claude.exe"));
        assert!(!is_desktop_exe(r"C:\Users\a\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\claude.exe"));
        // Claude Desktop's own copy of Claude Code is not the app either.
        assert!(!is_desktop_exe(r"C:\Users\a\AppData\Roaming\Claude\claude-code\2.1.0\claude.exe"));
    }

    #[test]
    fn every_electron_process_counts_and_the_main_one_is_the_root() {
        let store = r"C:\Program Files\WindowsApps\Claude_1.2.3.0_x64__pzs8sxrjxfjjc\app\claude.exe";
        let list = vec![
            proc(10, 4, "explorer.exe", r"C:\Windows\explorer.exe"),
            proc(20, 10, "claude.exe", store),
            proc(21, 20, "claude.exe", store),
            // A sandboxed renderer whose path the system hides still counts.
            proc(22, 20, "claude.exe", ""),
            proc(30, 20, "claude.exe", r"C:\Users\a\AppData\Roaming\Claude\claude-code\2.1.0\claude.exe"),
            proc(40, 10, "magicpic-cli.exe", r"C:\Users\a\.magicpic\bin\magicpic-cli.exe"),
        ];
        let app = desktop(&list, Os::Windows);
        assert_eq!(app.iter().map(|p| p.pid).collect::<Vec<_>>(), [20, 21, 22]);
        assert_eq!(main_processes(&app).iter().map(|p| p.pid).collect::<Vec<_>>(), [20]);

        let mac = vec![
            proc(1, 0, "launchd", "/sbin/launchd"),
            proc(5, 1, "Claude", "/Applications/Claude.app/Contents/MacOS/Claude"),
            proc(
                6,
                5,
                "Claude Helper (Renderer)",
                "/Applications/Claude.app/Contents/Frameworks/Claude Helper (Renderer).app/Contents/MacOS/Claude Helper (Renderer)",
            ),
            proc(7, 1, "claude", "/Users/a/.local/bin/claude"),
        ];
        let app = desktop(&mac, Os::Macos);
        assert_eq!(app.iter().map(|p| p.pid).collect::<Vec<_>>(), [5, 6]);
        assert_eq!(main_processes(&app).iter().map(|p| p.pid).collect::<Vec<_>>(), [5]);
    }

    #[test]
    fn mcp_server_processes_are_found_by_program_and_verb() {
        let mut server = proc(50, 20, "magicpic-cli.exe", r"C:\old\magicpic-cli.exe");
        server.cmd = vec!["magicpic-cli.exe".into(), "mcp".into(), "--bridge".into(), "127.0.0.1:7878".into()];
        let mut helper = proc(51, 1, "magicpic-cli.exe", r"C:\new\magicpic-cli.exe");
        helper.cmd = vec!["magicpic-cli.exe".into(), "mcp".into(), "register".into(), "--detached-wait".into()];
        let mut convert = proc(52, 1, "magicpic-cli", "/x/magicpic-cli");
        convert.cmd = vec!["magicpic-cli".into(), "convert".into()];
        assert_eq!(mcp_servers(&[server.clone(), helper, convert]), [server]);
    }

    #[test]
    fn the_system_list_includes_this_process() {
        let list = SystemTable::new(true).snapshot().expect("the process list is readable here");
        let me = list.iter().find(|p| p.pid == std::process::id()).unwrap();
        assert!(me.start > 0);
    }
}
