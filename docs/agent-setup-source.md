# Building MagicPic from source (fallback for the agent setup)

Use this page only when the quick path in [`agent-setup.md`](agent-setup.md) cannot install a
release: the download fails with 404 (no release published yet, or none for this processor), or
the person asks for a source build. The [Rules](agent-setup.md#rules) there apply here too.

It is slow: on a 4-core Linux machine the build took about 15 minutes, and a Windows 10 laptop
(GTX 1650) took over 19 minutes for the app alone, with 3.4 GB in `target/`. A tool call may time
out first: run the build in the background and check on it.

## 1. Prerequisites

Check what is there: `git --version`, `cargo --version`, `rustc --version` (Rust **1.95 or
newer**). Install only what is missing, after the person agrees.

**Windows** (the third installs the C++ build tools Rust needs and takes a while):

```powershell
winget install --id Git.Git -e
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

A shell that was open before the install does not see the new tools: open a new one.

**macOS:** `xcode-select --install`, then Rust from <https://rustup.rs>.

**Linux:** Rust from <https://rustup.rs>, plus the windowing and GTK development packages listed
under *Prerequisites* in `docs/development.md` (Debian and Ubuntu: `sudo apt install
libxkbcommon-dev libwayland-dev libx11-dev libxrandr-dev libxi-dev libgl1-mesa-dev
libgtk-3-dev`).

If `rustc --version` is older than 1.95, run `rustup update stable`.

## 2. Get the source

A shallow clone is enough and much smaller than the full history:

```sh
git clone --depth 1 https://github.com/vansyson1308/vansysoneditphoto ~/magicpic
```

On Windows use `"$HOME\magicpic"` as the target. If `~/magicpic` exists and `git remote get-url
origin` is this repository, this is an update: run `git pull --ff-only` in it (`git fetch --depth
1` then `git reset --hard origin/main` only if the person agrees, since it drops local changes).
If it exists but is something else, ask the person where to put the checkout.

## 3. Build both programs in one go

**Updating?** On Windows a running program's file can't be replaced, and an older setup ran the
app straight from `target/release`. Ask the running app to quit first:

```sh
~/.magicpic/bin/magicpic-cli quit-app
```

(Windows: `& "$HOME\.magicpic\bin\magicpic-cli.exe" quit-app`.) Expected: `{"ok": true,
"running": false, "stopped": true}` (or `"stopped": false` when it was not running). While a
document has unsaved changes it refuses (exit 1, `"unsaved": ["Beach.psd"]`) and the app keeps
running: ask the person to save or close those, then run it again. A CLI older than this command
says `unrecognized subcommand`: then ask the person to quit MagicPic (File > Exit).

In the checkout:

```sh
cargo build --release --locked -p magicpic -p magicpic-cli
```

One build shares the dependencies of both programs (the older guide's separate `cargo install`
compiled them twice). Expected at the end: `Finished `release` profile [optimized] target(s)`.

On Windows 11, *An Application Control policy has blocked this file (os error 4551)* is Smart App
Control or a managed policy blocking the build: see
[Smart App Control](agent-troubleshooting.md#unsigned-binaries). Do not try to switch it off.

## 4. Install the programs out of `target/`

So the person can delete the build folder later, the new CLI copies both programs into
`~/.magicpic/bin`. In the checkout:

```sh
target/release/magicpic-cli install --from target/release
```

(Windows: `.\target\release\magicpic-cli.exe install --from target\release`.) Don't copy them
by hand: on Windows, `Copy-Item` over a running `magicpic-cli.exe` fails with *The process cannot
access the file because it is being used by another process*. Agent clients keep it running as
their MCP servers. `install` does this instead:

- It asks a running app to quit, and changes nothing while a document has unsaved changes.
- It copies both programs in beside the old ones.
- It renames a program that is still running to `<name>.old-<time>.exe` and puts the new one in
  its place. The renamed copy is deleted on a later run.

Expected:

```json
{
  "ok": true,
  "bin": "C:\\Users\\Ann\\.magicpic\\bin",
  "installed": ["…\\magicpic.exe", "…\\magicpic-cli.exe"],
  "appStopped": false,
  "oldCopiesInUse": ["…\\magicpic-cli.old-20261113T165633Z.exe"],
  "note": "an old copy is still running (an agent client's MCP servers): …",
  "next": ["…\\magicpic-cli.exe setup", "…\\magicpic-cli.exe doctor"]
}
```

`oldCopiesInUse` is empty when nothing ran. If it is not, the agent client still runs the old
servers until it restarts. `doctor` shows that under `runningServers`.

## 5. Setup, then continue with the quick path

```sh
~/.magicpic/bin/magicpic-cli setup
```

(Windows: `& "$HOME\.magicpic\bin\magicpic-cli.exe" setup`.) It finds the app next to itself,
creates the photo folder and the launcher, and prints the same JSON as the installer (expected
output: [agent-setup.md, step 1](agent-setup.md#1-install)). Then go on with
[step 2](agent-setup.md#2-open-the-app) of the quick path.

**After an update:**

- **`legacy` is not empty** (for example `~/.cargo/bin/magicpic-cli.exe` from an older guide's
  `cargo install`, or `~/.magicpic/start-magicpic.ps1`): ask the person, then run
  `~/.magicpic/bin/magicpic-cli setup --migrate`. By hand it is `cargo uninstall magicpic-cli`
  and deleting the old launcher. Do that only once no client is registered with the old program,
  or its servers stop working.
- **A client was registered before:** run `mcp register` for it again, so it runs
  `~/.magicpic/bin/magicpic-cli`. For Claude Desktop use `--restart` (step 3 of the quick path).
  Then check with `doctor`: no `problems`, and `"thisCli": true` for every registered server and
  every entry of `runningServers`.

Afterwards the person may free the space with `cargo clean` in the checkout: nothing registered
points into `target/`. Keep the checkout if they want to update by pulling and rebuilding.
