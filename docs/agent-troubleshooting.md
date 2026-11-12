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
it starts a detached helper that waits for it to quit (up to 10 minutes, `--timeout` to change),
waits two seconds for its last write, merges the entries, and opens it again. The person only has
to quit Claude Desktop completely (tray or menu-bar icon › Quit). Check
`~/.magicpic/register.log`:

```text
2026-11-12T10:15:02Z waiting up to 600s for Claude Desktop to quit
2026-11-12T10:15:40Z wrote [{"backup":"…magicpic-backup-…","path":"…claude_desktop_config.json"}]
2026-11-12T10:15:40Z reopened Claude Desktop (shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude)
```

`magicpic-cli mcp status --client claude-desktop` then shows `"registered": true` and
`"running"`. `--force` writes while it runs (the change may be lost); the person can also quit
Claude Desktop first and run `mcp register --client claude-desktop` without `--restart`.

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
