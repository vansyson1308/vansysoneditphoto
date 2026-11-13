# MagicPic agent setup: troubleshooting

For agents following [`agent-setup.md`](agent-setup.md). Start with `magicpic-cli doctor`: its
`problems` list names each problem with the command that fixes it. The sections below explain the
ones that need more than one command, and what MagicPic now does about each.

## Claude Desktop forgets the servers

**Symptom:** the `magicpic` entries were added to `claude_desktop_config.json`, Claude Desktop
was restarted, and they are gone.

**Why:** Claude Desktop keeps its configuration in memory and writes it back while it runs and
when it quits, overwriting changes made meanwhile. Only a file edited while it is closed sticks.

**Fix:** `magicpic-cli mcp register --client claude-desktop --restart`. While Claude Desktop runs,
it returns at once (`"status": "waiting"`) and leaves a detached helper that:

1. waits until **every** Claude Desktop process is gone (up to 60 minutes; `--wait-minutes N`
   changes that). Claude Desktop is an Electron app: renderer, GPU and utility processes come and
   go while it stays open, so one of them ending is not a quit. A process list that can't be read
   counts as "still running";
2. checks again 3 seconds later, since Claude writes its config as it quits;
3. merges the entries, opens Claude Desktop again, and checks that a new main process started
   after the write. Only then does it log `reopened`; otherwise it logs `wrote, but could not
   confirm a restart` and the person opens Claude Desktop.

The person only has to quit Claude Desktop completely (tray or menu-bar icon > Quit). Then check
with `magicpic-cli doctor` (not the log): no `problems`, `claude-desktop` registered with
`"thisCli": true`, and every entry of `runningServers` with `"thisCli": true`. The helper's log,
`~/.magicpic/register.log`, reads:

```text
2026-11-13T16:56:33Z waiting up to 60 min for every Claude Desktop process to quit
2026-11-13T17:02:10Z every Claude Desktop process has quit; wrote [{"backup":"…magicpic-backup-…","path":"…claude_desktop_config.json"}]
2026-11-13T17:02:14Z reopened Claude Desktop (shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude): new main process 9312 started
```

`--force` writes while it runs (the change may be lost); the person can also quit Claude Desktop
first and run `mcp register --client claude-desktop` without `--restart`.

If the agent's own shell is a child of Claude Desktop and Windows ends the helper together with
Claude Desktop, nothing is written. Then `doctor` reports that the helper stopped: register again
with `--restart`, or quit Claude Desktop first and register without it.

## "Claude Desktop registration pending"

**Symptom:** `doctor` or `mcp status --client claude-desktop` reports *Claude Desktop registration
pending: Claude Desktop was never restarted …* (or *… the helper that waited … stopped*).

**Why:** the `--restart` helper gave up after its wait (the person never quit Claude Desktop), or
was ended. Nothing was written, so Claude Desktop still has its old servers.
`~/.magicpic/register-pending.json` records it until a registration is written.

**Fix:** what the problem's `fix` says: run `magicpic-cli mcp register --client claude-desktop
--restart` again, then ask the person to quit Claude Desktop completely. While a helper still
waits, the problem says *waiting since …*: the person only has to quit.

## The client still runs an older magicpic-cli

**Symptom:** `doctor` reports *claude-desktop runs `magicpic` with C:\…\.cargo\bin\magicpic-cli.exe
(magicpic-cli 0.23.0 (dev build)) instead of C:\…\.magicpic\bin\magicpic-cli.exe, the CLI of this
install*, or *… MCP server process(es) still run another binary …*, or *`magicpic-app` has no
--start-app-if-needed*.

**Why:** the client's config still names the program an older guide installed, or was written by
an older version. Or the config is right but the client has not restarted, so the servers it
started before still run the old binary and show the old tools.

**Fix:** the problem's `fix`: `magicpic-cli mcp register --client <client>` (Claude Desktop:
`--restart`), run with the CLI of this install, then restart the client. `--no-start-app` keeps
`magicpic-app` from starting the app on purpose; `doctor` then accepts the missing flag.

## "The process cannot access the file because it is being used by another process"

**Symptom:** copying a new `magicpic.exe` or `magicpic-cli.exe` into `~/.magicpic/bin` (or a
`cargo build` that writes `target\release\magicpic.exe`) fails with this message, or with
`os error 32`.

**Why:** Windows does not let a running program's file be replaced. The app runs `magicpic.exe`.
Every agent client that has the servers keeps `magicpic-cli.exe` running, Claude Desktop included,
even with no conversation open.

**Fix:** don't copy by hand. The installer (`install.ps1`) and `magicpic-cli install --from <dir>`
(source builds, [`agent-setup-source.md`](agent-setup-source.md) step 4):

- ask the app to quit over its control channel, or refuse, changing nothing, while a document has
  unsaved changes;
- rename a program that is still in use to `<name>.old-<time>.exe`, which Windows allows for a
  running program, and put the new one in its place;
- delete the old copies on a later run, once nothing runs them.

To stop the app before a source build: `magicpic-cli quit-app`. Restart the agent client
afterwards, so it starts the new servers.

## `?` or `�` in place of characters

**Symptom:** a message or path in a command's output shows `?`, `�?�` or similar in Windows
PowerShell 5.1.

**Why:** Windows PowerShell decodes a native program's output in the console's code page, not as
UTF-8.

**Fix:** update. The setup commands (`setup`, `launch`, `install`, `quit-app`, `mcp register |
unregister | status`, `doctor`) print ASCII only: messages use `>` for menu paths, and JSON escapes
every other character (`S\u01a1n`), which a JSON reader turns back.

## An older install is left behind

**Symptom:** `setup` lists entries under `legacy`, or `doctor` reports *an older MagicPic install is
still here*: `~/.cargo/bin/magicpic-cli(.exe)` from the older guide's `cargo install`, or the old
launcher `~/.magicpic/start-magicpic.ps1` next to the new `start-magicpic.cmd`.

**Fix:** ask the person, then `magicpic-cli setup --migrate`. It re-registers every client that
still runs the old program; Claude Desktop only while it is closed, otherwise it says to use
`mcp register --client claude-desktop --restart` and run `--migrate` again afterwards. It removes
the old programs with `cargo uninstall` (or deletes them) and the old launcher, and keeps anything
still registered or still running, saying why. By hand: `cargo uninstall magicpic-cli`, then
delete `start-magicpic.ps1`.

On Windows the Microsoft Store build may read a redirected copy of the file under
`%LOCALAPPDATA%\Packages\Claude_…\LocalCache\Roaming\Claude\`. `mcp register` writes every copy
that exists (and `%APPDATA%\Claude\claude_desktop_config.json` when none does); `mcp status`
lists them all.

## Opening Claude Desktop again on Windows (MSIX)

The Store build runs from `C:\Program Files\WindowsApps\Claude_<version>_<arch>__<publisher>\`,
which cannot be started directly, and `Get-StartApps` may list a Chrome web app called "Claude"
first. The way that works is its application user model id:

```powershell
explorer.exe shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude
```

`mcp register --restart` does this by itself: it reads the package family from the running
process's folder, and uses `Claude_pzs8sxrjxfjjc` when it cannot. A non-Store install
(`%LOCALAPPDATA%\AnthropicClaude\claude.exe`) is started from its own path; on macOS it runs
`open -a Claude`. Claude Code's own `claude.exe` (under `.local\bin` or `npm`) is never taken for
the desktop app.

## PowerShell in Constrained Language Mode

**Symptom:** `New-Object System.Text.UTF8Encoding`, property setters or `[Console]::OutputEncoding
= …` fail with "Cannot set property. Property setting is supported only on core types in this
language mode."; `$ExecutionContext.SessionState.LanguageMode` says `ConstrainedLanguage`.

**Why:** an application control policy (AppLocker, WDAC, some company images) restricts scripts.
Windows PowerShell 5.1 also writes a BOM with `Set-Content -Encoding UTF8`, drops double quotes
from arguments to native programs, and a `.js` helper fails in a folder whose parent
`package.json` says `"type": "module"`.

**Fix:** none needed: the setup no longer asks you to script anything. `install.ps1` uses only
`curl.exe`, `tar.exe` and core cmdlets, and every file (folders, launcher, `config.json`, client
configs, desktop shortcut) is written by `magicpic-cli`, which you run with plain arguments. If a
path has spaces, put it in double quotes as one argument.

## "parse error: expected value at line 1 column 1" over stdio

**Symptom:** piping JSON-RPC into `magicpic-cli mcp` from Windows PowerShell 5.1 fails on the
first line, then `expect initialized request, but received Notification`.

**Why:** PowerShell 5.1 puts a UTF-8 byte order mark before the first line it pipes into a
native program.

**Fix:** update. The MCP stdio reader now drops leading byte order marks and skips blank lines,
so the BOM-prefixed `initialize` is accepted. Real MCP clients never sent a BOM; this only
matters for hand-made pipes and tests. `magicpic-cli doctor` does the handshake for you.

## "MagicPic app is not running"

**Symptom:** a `magicpic-app` tool answers *MagicPic app is not running on 127.0.0.1:7878 (…):
run the launcher …*.

**Why:** nothing listens on the control port, or the app has not written its token file yet.

**Fix:** run the launcher (`magicpic-cli launch`); the next call connects. The server reads the
token file on its first call, not when it starts, and reads it again if the app restarted with a
new token, so the agent client does not need a restart after the app starts. Servers registered
by `mcp register` also have `--start-app-if-needed`: they start the app themselves with the
settings `setup` recorded (unless registered with `--no-start-app`).

## Unsigned binaries

The release archives are not code-signed yet (planned: see Follow-ups in `agent-setup.md`).

- **Windows SmartScreen** ("Windows protected your PC") applies to files carrying the internet
  zone mark. The installer downloads with `curl.exe` and runs `Unblock-File`, so it should not
  appear; if it does, the person chooses **More info › Run anyway**.
- **Smart App Control** (Windows 11) and managed **application control policies** block unsigned
  programs outright: *An Application Control policy has blocked this file (os error 4551)* or
  `magicpic-cli` failing to start. Do not try to switch them off, and do not suggest it: turning
  Smart App Control off is the person's decision, and on some Windows versions it cannot simply be
  turned back on. Tell them what was blocked. A source build is blocked the same way. Signed
  releases are the real fix.
- **macOS Gatekeeper** checks files with the quarantine flag that browsers set. `install.sh`
  downloads with `curl` (no flag) and clears it anyway. If the person downloaded an archive in a
  browser: `xattr -d com.apple.quarantine ~/.magicpic/bin/magicpic ~/.magicpic/bin/magicpic-cli`.

## The port is taken

**Symptom:** `magicpic-cli launch` says nothing listens after 30 s, and `~/.magicpic/app.log`
contains `failed to bind`; or `doctor` says something listens but the token file is missing.

**Fix:** `magicpic-cli setup --port 7879`, then `magicpic-cli mcp register --client <client>`
again (the bridge entry names the port), then `magicpic-cli launch`.

## The app refuses the token

**Symptom:** `doctor` shows `"authenticated": false`.

**Fix:** quit MagicPic, run the launcher again (it writes a fresh token), and retry. Never copy a
token by hand.

## Manual fallback: registering without `mcp register`

Only when `magicpic-cli` itself cannot run here. Use absolute paths from `setup`'s output.

**Claude Code:**

```sh
claude mcp add --transport stdio --scope user magicpic -- <cli> mcp --automation-read-root <photos> --automation-write-root <photos>
claude mcp add --transport stdio --scope user magicpic-app -- <cli> mcp --bridge 127.0.0.1:7878 --control-token-file <home>/.magicpic/control.token --start-app-if-needed
```

**Codex:** the same with `codex mcp add magicpic -- …` and `codex mcp add magicpic-app -- …`,
then `tool_timeout_sec = 300` under both `[mcp_servers.*]` tables in `~/.codex/config.toml`.

**Claude Desktop:** give the person this to paste under **Settings › Developer › Edit Config**
while you wait, with `\\` doubled in Windows paths; then they quit and reopen the app:

```json
{
  "mcpServers": {
    "magicpic": { "command": "<cli>", "args": ["mcp", "--automation-read-root", "<photos>", "--automation-write-root", "<photos>"] },
    "magicpic-app": { "command": "<cli>", "args": ["mcp", "--bridge", "127.0.0.1:7878", "--control-token-file", "<home>/.magicpic/control.token", "--start-app-if-needed"] }
  }
}
```

**A launcher by hand** (Windows PowerShell, works in Constrained Language Mode):

```powershell
Start-Process -FilePath "$HOME\.magicpic\bin\magicpic.exe" -ArgumentList "--control 7878 --control-token-file `"$HOME\.magicpic\control.token`" --automation-read-root `"$HOME\Pictures\MagicPic`" --automation-write-root `"$HOME\Pictures\MagicPic`""
```
