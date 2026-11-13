# Changelog

Notable changes to MagicPic. Releases are cut as described in [`docs/releasing.md`](docs/releasing.md).

## Unreleased

Faster installs and easier driving by AI agents, from an agent's report of setting MagicPic up
on a Windows 10 laptop and editing four photos with it.

### Installing

- Prebuilt archives: the `agent-binaries` workflow builds `magicpic` and `magicpic-cli` for
  windows-x86_64, macos-aarch64, macos-x86_64 and linux-x86_64 on version tags, with a
  `SHA256SUMS` file, tests installing from them on Windows, macOS and Linux, and attaches them
  with `install.sh` and `install.ps1` to the release (a draft when the tag has none). The
  binaries are not code-signed yet.
- `install.sh` / `install.ps1`: download the latest (or a given) release, refuse an archive whose
  checksum does not match, install both programs into `~/.magicpic/bin`, run `magicpic-cli setup`
  and print its JSON. `install.ps1` runs in Windows PowerShell 5.1 in Constrained Language Mode.
- `magicpic-cli setup`: creates the photo folder, `~/.magicpic/config.json`, the launcher and,
  with `--desktop-shortcut`, a desktop shortcut; prints every absolute path as JSON.
  `magicpic-cli launch` starts the app with its control channel, detached, and waits for it.
- `magicpic-cli mcp register | unregister | status --client claude-desktop|claude-code|codex`:
  merges only the `magicpic` and `magicpic-app` entries into the client's own config, after a
  backup, as UTF-8 without a byte order mark. Claude Desktop is refused while it runs unless
  `--restart`, which waits for it to quit, writes, and opens it again (the Store build through its
  app id). `status` checks that every registered path still exists.
- `magicpic-cli doctor`: a JSON health report (versions, the control port and token file, each
  MCP server's `initialize` + `tools/list`, the photo folder, registered clients) with a fix for
  every problem. It never prints the token.
- `docs/agent-setup.md` is now the quick path (release, `setup`, `mcp register`, `doctor`) with
  the expected output of every step; building from source moved to `docs/agent-setup-source.md`
  (shallow clone, one build for both programs, copied out of `target/`), and problems to
  `docs/agent-troubleshooting.md`.

### Updating and re-registering

From testing an update on Windows 10 with the Microsoft Store build of Claude Desktop.

- `mcp register --restart` (and `unregister --restart`) waits until **every** Claude Desktop
  process has quit, not just one of its Electron helpers. It reads the process list natively
  (no PowerShell). A list that can't be read counts as "still running". It checks again 3 s
  later before writing. After opening Claude again, it logs `reopened` only when a new main
  process started after the write, and otherwise `wrote, but could not confirm a restart`.
  Before, one failed or empty probe was enough to write while Claude Desktop still ran.
- The helper waits 60 minutes by default (`--wait-minutes N`; `--timeout` in seconds still
  works). When it gives up, or is ended, the registration stays pending
  (`~/.magicpic/register-pending.json`). `mcp status` and `doctor` report it with the command
  that fixes it.
- `mcp register --restart` returns at once, also when its output is captured
  (`... | Out-String`). On Windows, Rust starts processes with handle inheritance on, so the
  helper inherited the caller's output pipe and kept it open. The helper, and the app that
  `launch` and `--start-app-if-needed` start, are now started through the shell
  (`Start-Process`) and hold none of the caller's handles.
- `mcp status` and `doctor` report a registration that runs another `magicpic-cli` than this
  install (with both versions), and a `magicpic-app` entry without `--start-app-if-needed`
  (`mcp register --no-start-app` is remembered, so a deliberate choice is not flagged). `doctor`
  also lists the MagicPic MCP server processes running now (`runningServers`) and says when any
  was started from another binary.
- `magicpic-cli install --from <dir>` and `install.ps1` replace the programs while they run. They
  ask a running app to quit, and refuse, changing nothing, while it has unsaved changes. They copy
  both programs in beside the old ones, rename one still in use (`magicpic-cli.exe`, which agent
  clients keep running) to `<name>.old-<time>.exe`, and delete old copies on a later run.
  `magicpic-cli quit-app` asks the app to quit on its own.
- `setup` lists what an older guide left behind under `legacy` (`~/.cargo/bin/magicpic-cli`,
  `start-magicpic.ps1`). `setup --migrate` re-points registrations to this install and then
  removes what nothing uses any more. `doctor` reports leftovers.
- The setup commands print ASCII only: menu paths use `>`, and JSON escapes non-ASCII characters
  (`\u203a`). Windows PowerShell 5.1 no longer shows `?` in their messages and paths.
- `install.sh` and `install.ps1` say when no release is published (HTTP 404) and point to the
  source build. `docs/releasing.md` says how to cut the first release.

### MCP

- The stdio reader drops a leading UTF-8 byte order mark (Windows PowerShell 5.1 pipes one).
- Bridge mode (`magicpic-app`) reads the token file on its first call and again after the app
  restarts, so it may start before the app; until then calls say "MagicPic app is not running"
  and how to start it. `--start-app-if-needed` starts it.
- `document` (a tab index or `{"id": n}`) on `command_run`, `command_batch`, `doc_inspect`,
  `doc_save`, `doc_export`, the previews and `doc_histogram`; in the app its tab comes to the
  front first. `doc_select` works in bridge mode; `doc_open` returns `index` and `id`.
- `command_run` adds a second text block naming the document it left active and its revision;
  batch replies carry `document`, `index` and `revision`.
- Bridge `doc_save` / `doc_export` take `quality` (1–100), `tiffLayers` and `pcraftFullComposite`
  for that save only (`app.save` `exportQuality`); preferences are not changed.
- `render_preview` / `doc_render_preview` take `layer` and `solo`: one layer alone, or with the
  layers below, without changing its visibility, blend mode or history (`seeing.view`).
- `doc_histogram` (engine `document.histogram`): full-resolution 256-bin histograms with min,
  max, mean, standard deviation, median and p50/p90/p99/p99.9, of the composite, a layer or a
  region.
- `session_list` is compact unless `verbose: true`; `command_list` takes `brief`, `offset` and
  `limit`.
- `files_list` returns the ordinary Windows spelling in `absolutePath` (`extendedPath` keeps the
  `\\?\` form when it differs).
- `actions_play`: a saved Action or inline steps applied to several documents in one call, with
  per-document overrides; each document is one transaction by default.
- The server instructions cover document targeting, `layer.select`, `edit.undo`, histograms and
  solo views.

### Filters and looks

- `filter.render.starfield`, `filter.render.starburst` (rays that narrow and fade instead of a
  hard plus sign) and `filter.render.bokeh`: seeded, on a new Screen layer by default, one undo
  step.
- `filter.render.lensFlare` takes `ghosts: false` for the core alone, without the reflection rings
  across the frame.
- Color Lookup has a `dreamy` look: violet shadows, pink highlights, lifted blacks.
