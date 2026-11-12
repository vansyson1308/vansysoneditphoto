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

In the checkout:

```sh
cargo build --release --locked -p magicpic -p magicpic-cli
```

One build shares the dependencies of both programs (the older guide's separate `cargo install`
compiled them twice). Expected at the end: `Finished `release` profile [optimized] target(s)`.

On Windows 11, *An Application Control policy has blocked this file (os error 4551)* is Smart App
Control or a managed policy blocking the build: see
[Smart App Control](agent-troubleshooting.md#unsigned-binaries). Do not try to switch it off.

## 4. Copy the programs out of `target/`

So the person can delete the build folder later, copy both programs to `~/.magicpic/bin`. Use the
file tools you have (no script needed):

| From (in the checkout) | To |
|---|---|
| `target/release/magicpic` (Windows: `magicpic.exe`) | `~/.magicpic/bin/` |
| `target/release/magicpic-cli` (Windows: `magicpic-cli.exe`) | `~/.magicpic/bin/` |

For example `mkdir -p ~/.magicpic/bin && cp target/release/magicpic target/release/magicpic-cli
~/.magicpic/bin/`, or on Windows `New-Item -ItemType Directory -Force "$HOME\.magicpic\bin"`
followed by `Copy-Item target\release\magicpic.exe, target\release\magicpic-cli.exe
"$HOME\.magicpic\bin\"` (both work in Constrained Language Mode).

## 5. Setup, then continue with the quick path

```sh
~/.magicpic/bin/magicpic-cli setup
```

(Windows: `& "$HOME\.magicpic\bin\magicpic-cli.exe" setup`.) It finds the app next to itself,
creates the photo folder and the launcher, and prints the same JSON as the installer (expected
output: [agent-setup.md, step 1](agent-setup.md#1-install)). Then go on with
[step 2](agent-setup.md#2-open-the-app) of the quick path.

Afterwards the person may free the space with `cargo clean` in the checkout: nothing registered
points into `target/`. Keep the checkout if they want to update by pulling and rebuilding.
