# Setting MagicPic up for a person (instructions for an AI agent)

You are an AI agent (Claude Code, Codex or similar) on a person's computer. They want to
**use** MagicPic: edit photos by talking to you. They are not asking you to develop it.
`AGENTS.md` and `docs/development.md` are for contributors; you do not need them, except where
this page points to them.

## What done looks like

1. The command-line server `magicpic-cli` and the desktop app `magicpic` are built from
   <https://github.com/vansyson1308/vansysoneditphoto>.
2. One photo folder exists, and MagicPic may read and write only inside it.
3. Two MCP servers are registered with you:
   - **`magicpic`** edits files in the photo folder, without opening a window.
   - **`magicpic-app`** drives the MagicPic window that is open on screen (bridge mode), so
     the person watches the edits happen.
4. The app window is open, and a launcher opens it again later.
5. You told the person what you did, and what to say after restarting you.

## Rules

- **Ask the person before** you install system software, use administrator rights, or change
  anything outside the folders named on this page.
- Grant MagicPic **one** photo folder. Never the whole disk, the home folder or a system folder.
- The control token in `~/.magicpic/control.token` is a password for the open app. Never print
  it, paste it into chat or put it in a repository. Nothing is exposed beyond `127.0.0.1`.
- Do not upload the person's photos anywhere. Do not commit or push to the repository.
- Check every step's result before going on. If a step fails and this page does not cover it,
  stop and tell the person what failed. Do not work around it in ways this page does not
  describe.
- Speak to the person in their language.

## Paths

Use these unless the person asks for others. `~` means the home folder: `$HOME` in PowerShell,
`$HOME` in sh.

| What | Windows | macOS and Linux |
|---|---|---|
| Source checkout | `$HOME\magicpic` | `~/magicpic` |
| Photo folder | `$HOME\Pictures\MagicPic` | `~/Pictures/MagicPic` |
| Token and launcher | `$HOME\.magicpic\` | `~/.magicpic/` |
| `magicpic-cli` after install | `$HOME\.cargo\bin\magicpic-cli.exe` | `~/.cargo/bin/magicpic-cli` |
| App after build | `$HOME\magicpic\target\release\magicpic.exe` | `~/magicpic/target/release/magicpic` |
| Control port | `7878` on `127.0.0.1` | same |

Use absolute paths everywhere you register or launch something; expand `~` yourself.

## 1. Prerequisites

Check what is there: `git --version`, `cargo --version`, `rustc --version` (Rust **1.95 or
newer**). Install only what is missing, after the person agrees.

**Windows** (PowerShell; the third installs the C++ build tools Rust needs and takes a while):

```powershell
winget install --id Git.Git -e
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

A shell that was open before the install does not see the new tools. Reload `PATH` in it:

```powershell
$env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')
```

**macOS:** `xcode-select --install`, then Rust from <https://rustup.rs>.

**Linux:** Rust from <https://rustup.rs>, plus the windowing and GTK development packages for
the distribution, listed under *Prerequisites* in `docs/development.md` (Debian and Ubuntu:
`sudo apt install libxkbcommon-dev libwayland-dev libx11-dev libxrandr-dev libxi-dev
libgl1-mesa-dev libgtk-3-dev`).

If `rustc --version` is older than 1.95, run `rustup update stable`.

## 2. Get the source

- **No `~/magicpic` yet:**
  `git clone https://github.com/vansyson1308/vansysoneditphoto ~/magicpic` (Windows:
  `git clone https://github.com/vansyson1308/vansysoneditphoto "$HOME\magicpic"`).
- **`~/magicpic` exists** and its `git remote get-url origin` is this repository: this is an
  update. Run `git pull --ff-only` in it. If that refuses because of local changes, stop and ask.
- **It exists but is something else:** ask the person where to put the checkout.

## 3. Build

In the checkout, run both. Each takes several minutes; the first build of the CLI took about
10 minutes on a 4-core machine, and a slower machine or Windows can take 20–40 minutes.

```sh
cargo install --path apps/magicpic-cli --locked --force
cargo build --release -p magicpic
```

Check that both files from the *Paths* table exist, then run
`magicpic-cli --version` with its full path; it prints `magicpic-cli` and a version.

On Windows 11, an error *An Application Control policy has blocked this file (os error 4551)*
is Smart App Control or a managed policy blocking the build. Do not try to switch it off.
Tell the person, and point them to the *Windows source builds* section of
`docs/development.md`.

## 4. Photo folder and launcher

Create the photo folder and `~/.magicpic/`. Then write the launcher. It starts the app with
the control channel on `127.0.0.1:7878`. The first start creates `~/.magicpic/control.token`
by itself, with a random token. Never write a token yourself.

**Windows:** `$HOME\.magicpic\start-magicpic.ps1`. Keep the quotes, because a home folder can
contain spaces.

```powershell
$app = "$HOME\magicpic\target\release\magicpic.exe"
$pictures = "$HOME\Pictures\MagicPic"
$token = "$HOME\.magicpic\control.token"
Start-Process -FilePath $app -ArgumentList "--control 7878 --control-token-file `"$token`" --automation-read-root `"$pictures`" --automation-write-root `"$pictures`""
```

**macOS and Linux:** `~/.magicpic/start-magicpic.sh`; then run `chmod +x` on it.

```sh
#!/bin/sh
exec >"$HOME/.magicpic/app.log" 2>&1
nohup "$HOME/magicpic/target/release/magicpic" --control 7878 \
  --control-token-file "$HOME/.magicpic/control.token" \
  --automation-read-root "$HOME/Pictures/MagicPic" \
  --automation-write-root "$HOME/Pictures/MagicPic" &
```

Run the launcher once now (Windows: `powershell -ExecutionPolicy Bypass -File "$HOME\.magicpic\start-magicpic.ps1"`, since the default policy may refuse to run a script). Then check that the app is up:
- **Windows:** `Test-NetConnection 127.0.0.1 -Port 7878 -InformationLevel Quiet` prints `True`.
- **macOS and Linux:** `~/.magicpic/app.log` contains `control server listening on 127.0.0.1:7878`.

In both cases, `~/.magicpic/control.token` must now exist. If the port is taken (`failed to bind`),
use another port, such as 7879, in the launcher and in step 5.

The window may show a notice "Switched to CPU rendering" on a machine without a usable GPU.
That is fine; the person can click **Keep Using CPU**.

## 5. Register the MCP servers

Register both servers at **user scope**, so they work in every folder. Use the absolute paths
from the *Paths* table. If a server of the same name is already registered (an update),
remove it first.

**Claude Code:**

```sh
claude mcp add --transport stdio --scope user magicpic -- <magicpic-cli> mcp --automation-read-root <photo folder> --automation-write-root <photo folder>
claude mcp add --transport stdio --scope user magicpic-app -- <magicpic-cli> mcp --bridge 127.0.0.1:7878 --control-token-file <home>/.magicpic/control.token
claude mcp list
```

`claude mcp list` should show both as connected.

**Codex:**

```sh
codex mcp add magicpic -- <magicpic-cli> mcp --automation-read-root <photo folder> --automation-write-root <photo folder>
codex mcp add magicpic-app -- <magicpic-cli> mcp --bridge 127.0.0.1:7878 --control-token-file <home>/.magicpic/control.token
codex mcp list
```

Then, in `~/.codex/config.toml`, add `tool_timeout_sec = 300` under both
`[mcp_servers.magicpic]` and `[mcp_servers.magicpic-app]`. Codex's default of 60 seconds is
too short for some filters on large photos.

**Claude Desktop** (the chat app) reads its own file, which you cannot reliably locate or
reload. If the person uses it, give them this to paste under **Settings › Developer › Edit
Config**, with absolute paths and `\\` doubled on Windows. Then they quit and reopen the app.

```json
{
  "mcpServers": {
    "magicpic": { "command": "<magicpic-cli>", "args": ["mcp", "--automation-read-root", "<photo folder>", "--automation-write-root", "<photo folder>"] },
    "magicpic-app": { "command": "<magicpic-cli>", "args": ["mcp", "--bridge", "127.0.0.1:7878", "--control-token-file", "<home>/.magicpic/control.token"] }
  }
}
```

## 6. Tell the person

Report, briefly and in their language:

- where the checkout, the photo folder and the launcher are, and that the app window is open;
- that they must **restart you** (quit and reopen Claude Code or Codex), so that the two
  servers load;
- how to open the app later: run the launcher. On Windows, offer to put a desktop shortcut
  to `powershell -ExecutionPolicy Bypass -File "<launcher>"`, and make it only if they agree;
- what to say after the restart. For example: *"Mở ảnh mới nhất trong thư mục MagicPic trong
  app và cho tôi xem."* ("Open the newest photo in the MagicPic folder in the app and show it
  to me."). With `magicpic-app`, you then work in the open window. With `magicpic`, you work
  on files without a window;
- that to update later, they paste the same setup prompt again. Steps 2–5 then pull, rebuild
  and re-register.

## After the restart: using MagicPic

The server's own instructions explain the tools: `files_list`, `doc_open`, `render_preview`,
`command_list`, `command_run` and `command_batch`, then `doc_save` and `doc_export`. In short:

- Look before and after each edit.
- Prefer adjustment layers and masks.
- Use `command_batch` with `atomic: true` for several steps.
- Save into the photo folder. Missing subfolders are created there.

`docs/mcp.md` documents every tool. `docs/vi/art-of-seeing-va-mcp.md` has example requests in
Vietnamese.
