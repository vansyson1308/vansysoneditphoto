# Setting MagicPic up for a person (instructions for an AI agent)

You are an AI agent (Claude Code, Codex, Claude Desktop with a shell, or similar) on a person's
computer. They want to **use** MagicPic: edit photos by talking to you. They are not asking you to
develop it. `AGENTS.md` and `docs/development.md` are for contributors; you do not need them.

This page is the **quick path**: a prebuilt release, four commands, no Rust toolchain. Two more
pages complete it:

- [`agent-setup-source.md`](agent-setup-source.md): build from source, when there is no release
  for this machine or the download fails.
- [`agent-troubleshooting.md`](agent-troubleshooting.md): what to do when a step's output is not
  what this page says it should be.

Raw links, for an agent that fetches pages:
`https://raw.githubusercontent.com/vansyson1308/vansysoneditphoto/main/docs/agent-setup-source.md`
and `…/docs/agent-troubleshooting.md`.

## What done looks like

1. `magicpic` (the app) and `magicpic-cli` (the command line and MCP server) are in
   `~/.magicpic/bin`.
2. One photo folder exists, and MagicPic may read and write only inside it.
3. Two MCP servers are registered with the person's agent client:
   - **`magicpic`** edits files in the photo folder, without opening a window.
   - **`magicpic-app`** drives the MagicPic window on screen (bridge mode), so the person watches
     the edits happen. It starts the app itself when it is not running.
4. `magicpic-cli doctor` reports `"ok": true`.
5. You told the person what you did, and what to say after restarting you.

## Rules

- **Ask the person before** you install system software, use administrator rights, or change
  anything outside `~/.magicpic`, the photo folder and their agent client's MCP configuration.
  The quick path needs none of the first two.
- Grant MagicPic **one** photo folder. Never the whole disk, the home folder or a system folder.
- The control token in `~/.magicpic/control.token` is a password for the open app. Never print
  it, paste it into chat or put it in a repository. No command on this page prints it.
- Do not upload the person's photos anywhere.
- **Do not write JSON, TOML or PowerShell scripts by hand.** Every file this setup needs is
  written by `magicpic-cli`. Run the commands below with plain arguments.
- Check every step's output against the expected output below before going on. If it differs,
  look the message up in [`agent-troubleshooting.md`](agent-troubleshooting.md). If it is not
  there, stop and tell the person what failed.
- Speak to the person in their language.

`~` means the home folder (`%USERPROFILE%` on Windows). Every command prints JSON with
**absolute** paths: use those paths in the later steps, not ones you build yourself.

## 1. Install

**Windows** (any shell; Windows PowerShell 5.1, including Constrained Language Mode, is fine):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://github.com/vansyson1308/vansysoneditphoto/releases/latest/download/install.ps1 | iex"
```

**macOS and Linux:**

```sh
curl -fsSL https://github.com/vansyson1308/vansysoneditphoto/releases/latest/download/install.sh | sh
```

Options, only if the person asks: another photo folder (`sh -s -- --photos "/path"`; on Windows
set `$env:MAGICPIC_PHOTOS='D:\Photos'` before `irm`), another port (`--port 7879` /
`MAGICPIC_PORT`), a desktop shortcut (`--desktop-shortcut` / `MAGICPIC_DESKTOP_SHORTCUT=1`), a
fixed version (`--version v0.24.0` / `MAGICPIC_VERSION`).

The installer downloads the archive for this machine and `SHA256SUMS`, refuses an archive whose
checksum does not match, copies both programs into `~/.magicpic/bin`, and runs
`magicpic-cli setup`. Run again later, it **updates in place**, also while MagicPic and the MCP
servers run: it asks a running app to quit (and installs nothing while a document has unsaved
changes, saying which), and renames a program that is still in use to `<name>.old-<time>.exe`
before putting the new one in (deleted on a later run). Expected: progress lines ending in `checksum ok (…)` and
`installed …`, then this JSON (Windows paths shown; yours differ):

```json
{
  "app": "C:\\Users\\Ann\\.magicpic\\bin\\magicpic.exe",
  "appExists": true,
  "cli": "C:\\Users\\Ann\\.magicpic\\bin\\magicpic-cli.exe",
  "config": "C:\\Users\\Ann\\.magicpic\\config.json",
  "desktopShortcut": null,
  "home": "C:\\Users\\Ann\\.magicpic",
  "launcher": "C:\\Users\\Ann\\.magicpic\\start-magicpic.cmd",
  "next": ["magicpic-cli mcp register --client <claude-desktop|claude-code|codex>", "magicpic-cli doctor"],
  "ok": true,
  "photos": "C:\\Users\\Ann\\Pictures\\MagicPic",
  "port": 7878,
  "tokenFile": "C:\\Users\\Ann\\.magicpic\\control.token",
  "legacy": [],
  "version": "0.23.0"
}
```

Check `"ok": true`. Below, `<cli>` means the `cli` path from this output. Non-ASCII characters
(a user name such as `Sơn`) come escaped (`S\u01a1n`), so Windows PowerShell's console code page
can't garble them; any JSON reader turns them back.

`"legacy"` lists what an older setup guide left behind: a `magicpic-cli` installed with `cargo
install` (`~/.cargo/bin`) and the old launcher `start-magicpic.ps1`. If it is not empty, ask the
person, then run `<cli> setup --migrate`: it points registrations that still run the old program
at this one, then removes what nothing uses any more (and says why it kept the rest).

On a fast connection this takes well under a minute: the archives are tens of megabytes, and no
compiler runs. If the installer says **`no MagicPic release at … (HTTP 404)`**, no release has been published yet
(or not that version): follow [`agent-setup-source.md`](agent-setup-source.md), then come back to
step 2.

The binaries are not code-signed yet. Windows SmartScreen or Smart App Control, or macOS
Gatekeeper, may stop them: see [Unsigned binaries](agent-troubleshooting.md#unsigned-binaries).

## 2. Open the app

```sh
<cli> launch
```

Expected, within about 30 seconds:

```json
{"running": true, "started": true, "pid": 12345, "port": 7878, "log": "…/.magicpic/app.log", "seconds": 2.1}
```

`"started": false` means it was already running; that is fine. The window may show "Switched to
CPU rendering" on a machine without a usable GPU; the person can click **Keep Using CPU**. The
launcher in the setup output (`start-magicpic.cmd` or `.sh`) runs the same command, for the
person to use later.

## 3. Register the MCP servers

Pick the client you are running in (ask the person if unsure): `claude-code`, `codex` or
`claude-desktop` (the chat app). One command writes both servers into that client's own
configuration, keeps every other server and setting, makes a backup next to the file first, and
writes UTF-8 without a byte order mark.

```sh
<cli> mcp register --client claude-code
```

Expected:

```json
{
  "client": "claude-code",
  "files": [{"backup": "…/.claude.json.magicpic-backup-20261112T101500Z", "path": "…/.claude.json"}],
  "next": "restart Claude Code (or run /mcp) so it loads the servers; `claude mcp list` shows them",
  "servers": [{"name": "magicpic", "command": "…", "args": ["mcp", "--automation-read-root", "…", "--automation-write-root", "…"]},
              {"name": "magicpic-app", "command": "…", "args": ["mcp", "--bridge", "127.0.0.1:7878", "--control-token-file", "…", "--start-app-if-needed"]}],
  "status": "written"
}
```

**Codex** is the same with `--client codex`; it also sets `tool_timeout_sec = 300` for both
servers, since Codex's 60-second default is too short for some filters on large photos.

**Claude Desktop rewrites its configuration while it runs and when it quits**, so an entry
written while it is open is lost. So:

```sh
<cli> mcp register --client claude-desktop --restart
```

- If Claude Desktop is not running, it writes the file at once (`"status": "written"`) and opens
  Claude Desktop.
- If it is running (it is, when you are Claude Desktop), the command returns at once with
  `"status": "waiting"` and a `message`; nothing is written yet. A helper keeps running in the
  background. It waits (up to 60 minutes; `--wait-minutes N` changes that) until **every** Claude
  Desktop process is gone, not just one of its windows or helper processes, checks again 3
  seconds later, then writes the file, opens Claude Desktop again (the Microsoft Store build
  through its app id) and checks that a new Claude Desktop process really started. Tell the
  person what step 5 says.
- If the helper gives up (the person never quit Claude Desktop), the registration stays
  **pending**: `mcp status` and `doctor` report it with the command that fixes it.
- Without `--restart`, while it runs, nothing is written and the command says why (exit 1).

`<cli> mcp status` shows, for every client, whether both servers are registered, whether every
path they name still exists (`"pathsOk": true`), and its `problems`: a pending Claude Desktop
registration, servers that run another `magicpic-cli` than this one (an older install), or a
`magicpic-app` entry from an older version without `--start-app-if-needed`.
`<cli> mcp unregister --client <client>` removes them again.

## 4. Verify

```sh
<cli> doctor
```

Expected (shortened):

```json
{
  "ok": true,
  "cli": {"version": "0.23.0", "path": "…"},
  "app": {"exists": true, "launcherExists": true, "path": "…"},
  "control": {"port": 7878, "listening": true, "tokenFileExists": true, "authenticated": true, "documents": 0},
  "roots": [{"path": "…/Pictures/MagicPic", "exists": true, "writable": true, "read": true, "write": true}],
  "servers": [{"name": "magicpic", "initialize": true, "tools": 29}, {"name": "magicpic-app", "initialize": true, "tools": 29}],
  "clients": [{"client": "claude-code", "registered": true, "pathsOk": true, "…": "…"}],
  "problems": []
}
```

`doctor` starts each registered server and checks that it answers `initialize` and `tools/list`,
and authenticates to the open app with the token file, without printing it. It also reports what
`mcp status` reports, the MagicPic MCP server processes running right now (`runningServers`,
with `"thisCli": false` for one started from an older binary: the client still shows the old
tools until it restarts), and an older install left behind. When `"ok"` is false, every entry of
`problems` names the problem and the `fix` command. Run `doctor` again whenever something fails
later, too: it, not `register.log`, says whether a registration took effect.

## 5. Tell the person

Report, briefly and in their language:

- that MagicPic is installed in `~/.magicpic`, where the photo folder is, and that the app window
  is open;
- that they must **restart you** so the two servers load (Claude Code: quit and reopen, or `/mcp`;
  Codex: quit and reopen; Claude Desktop: see below);
- **when you are running inside Claude Desktop** and `mcp register` answered `"waiting"`, say
  this, in their language, as the last thing before they act: *"Quit Claude Desktop from the tray
  icon now (right-click the Claude icon by the clock, then Quit). It opens again by itself. If it
  doesn't open again within a minute, open it yourself and tell me."* Quitting ends this
  conversation. In the next one, run `<cli> doctor` and check that `problems` is empty, that
  `claude-desktop` is registered with `"thisCli": true`, and that every entry of `runningServers`
  has `"thisCli": true`. Don't rely on `register.log`;
- how to open the app later: the launcher (or the desktop shortcut, if they asked for one). The
  `magicpic-app` server also starts it when needed;
- what to say after the restart. For example: *"Mở ảnh mới nhất trong thư mục MagicPic trong app
  và cho tôi xem."* ("Open the newest photo in the MagicPic folder in the app and show it to
  me."). More example requests, in Vietnamese: [`docs/vi/art-of-seeing-va-mcp.md`](vi/art-of-seeing-va-mcp.md)
  and [`docs/vi/cai-dat-va-ket-noi.md`](vi/cai-dat-va-ket-noi.md);
- that to update later, they paste the same setup prompt again: the installer replaces the
  programs (also while they run), and `setup` and `mcp register` keep their settings. After an
  update, `doctor` says whether a client still runs the old servers.

## After the restart: using MagicPic

The servers' own instructions explain the tools. The ones agents need most:

- `files_list` (on `magicpic`) finds a photo; `doc_open` opens it and returns its `index` and `id`.
  To work in the open window, open the file with `magicpic-app`'s `doc_open`, passing the
  `absolutePath` from `files_list`.
- **With several photos open, pass `document`** (an index, or `{"id": n}`) to `command_run`,
  `command_batch`, `doc_inspect`, `render_preview`, `doc_histogram`, `doc_save` and `doc_export`.
  In the app its tab comes to the front first. `doc_select` switches tabs. Every `command_run`
  reply has a second block naming the document it edited and its revision.
- Look before and after each edit (`render_preview`). To see one layer without changing it, pass
  `layer` (its id from `doc_inspect`): `solo: true` alone, `false` with the layers below.
- Choose Levels or Threshold values from `doc_histogram` (bins and p50/p90/p99/p99.9 on the 0–255
  scale), not by trial.
- Select layers with `command_run layer.select {"layer": id}` or `{"name": "Stars"}`; undo with
  `edit.undo` (check `doc_inspect`'s history first).
- Prefer adjustment layers and masks; use `command_batch` with `atomic: true` for several steps.
- Ready-made dreamy elements: `filter.render.starfield`, `filter.render.starburst`,
  `filter.render.bokeh`, `filter.render.lensFlare {"ghosts": false}`, and the Color Lookup look
  `dreamy` (`layer.newAdjustmentLayer.colorLookup {"lut": "dreamy"}`). `actions_play` applies one
  recipe to several open photos at once, with per-photo `overrides`.
- `doc_export {"quality": 92}` works in both servers and changes no preference.
- `session_list` is short; `verbose: true` gives everything. `command_list {"brief": true}`
  lists ids only.

`docs/mcp.md` documents every tool.

## Follow-ups (not done yet)

- **Code signing and notarization** for the Windows and macOS binaries, so SmartScreen, Smart App
  Control and Gatekeeper accept them without a prompt. It needs the publisher's certificates.
- A real `.lnk` desktop shortcut on Windows (the shortcut is a `.cmd` file for now).
- Prebuilt archives for Linux on ARM and Windows on ARM (Windows ARM64 runs the x64 build).
- A Windows toast or macOS notification when the `--restart` helper gives up or has restarted
  Claude Desktop. Until then `mcp status` and `doctor` report a pending registration.
