#!/bin/sh
# Install MagicPic (app + magicpic-cli) from a GitHub release, for an AI agent or a person.
#
#   curl -fsSL https://github.com/vansyson1308/vansysoneditphoto/releases/latest/download/install.sh | sh
#   curl -fsSL …/install.sh | sh -s -- --version v0.24.0 --photos "$HOME/Pictures/MagicPic"
#
# It downloads the archive for this machine and SHA256SUMS, refuses an archive whose checksum
# does not match, installs both programs into ~/.magicpic/bin (MAGICPIC_HOME overrides
# ~/.magicpic), then runs `magicpic-cli setup`, which creates the photo folder, the launcher and
# config.json. Progress goes to stderr; stdout is one JSON object with the absolute paths.
# No Rust toolchain, no administrator rights, nothing outside ~/.magicpic and the photo folder.
set -eu

repo=vansyson1308/vansysoneditphoto
version=latest
from=
setup_args=
run_setup=1

say() { printf 'magicpic install: %s\n' "$*" >&2; }
die() { say "error: $*"; exit 1; }

while [ $# -gt 0 ]; do
  case $1 in
    --version) version=$2; shift 2 ;;
    --repo) repo=$2; shift 2 ;;
    --from) from=$2; shift 2 ;;                  # a folder with the archives and SHA256SUMS
    --photos | --port) setup_args="$setup_args $1 $(printf '%s' "$2" | sed "s/'/'\\\\''/g; s/^/'/; s/\$/'/")"; shift 2 ;;
    --desktop-shortcut | --start) setup_args="$setup_args $1"; shift ;;
    --no-setup) run_setup=; shift ;;
    -h | --help) sed -n '2,12p' "$0" 2>/dev/null || true; exit 0 ;;
    *) die "unknown option $1" ;;
  esac
done

case $(uname -s) in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) die "unsupported system $(uname -s): on Windows run install.ps1" ;;
esac
case $(uname -m) in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) die "unsupported processor $(uname -m)" ;;
esac
platform=$os-$arch
case $platform in
  macos-aarch64 | macos-x86_64 | linux-x86_64) ;;
  *) die "no prebuilt MagicPic for $platform yet: build from source (docs/agent-setup.md, Fallback)" ;;
esac
archive=magicpic-$platform.tar.gz

home=${MAGICPIC_HOME:-$HOME/.magicpic}
bin=$home/bin
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fetch() {
  if [ -n "$from" ]; then
    cp "$from/$1" "$work/$1" || die "$from/$1 is missing"
    return
  fi
  if [ "$version" = latest ]; then url=https://github.com/$repo/releases/latest/download/$1
  else url=https://github.com/$repo/releases/download/$version/$1; fi
  say "downloading $url"
  no_release="no MagicPic release at $url (HTTP 404): none is published yet, or not that version. Build from source instead: https://github.com/$repo/blob/main/docs/agent-setup-source.md"
  if command -v curl >/dev/null 2>&1; then
    code=$(curl -sSL --retry 3 -w '%{http_code}' -o "$work/$1" "$url") || die "download failed: $url"
    [ "$code" = 404 ] && die "$no_release"
    case $code in 2*) ;; *) die "download failed: $url (HTTP $code)" ;; esac
  elif command -v wget >/dev/null 2>&1; then
    if ! wget -nv -O "$work/$1" "$url" 2> "$work/wget.err"; then
      grep -q 404 "$work/wget.err" 2>/dev/null && die "$no_release"
      die "download failed: $url"
    fi
  else die "needs curl or wget"; fi
}

fetch SHA256SUMS
fetch "$archive"

expected=$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$work/SHA256SUMS")
[ -n "$expected" ] || die "SHA256SUMS has no line for $archive"
if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$work/$archive" | cut -d' ' -f1)
else actual=$(shasum -a 256 "$work/$archive" | cut -d' ' -f1); fi
[ "$actual" = "$expected" ] || die "checksum mismatch for $archive (expected $expected, got $actual): nothing was installed"
say "checksum ok ($actual)"

mkdir -p "$work/x" "$bin"
tar -xzf "$work/$archive" -C "$work/x"
for b in magicpic magicpic-cli; do
  [ -f "$work/x/$b" ] || die "$archive has no $b"
  # Copy beside, then rename: a running copy keeps working and never sees half a file.
  cp "$work/x/$b" "$bin/.$b.new"
  chmod 755 "$bin/.$b.new"
  mv -f "$bin/.$b.new" "$bin/$b"
done
cp "$work/x/LICENSE-MIT" "$work/x/LICENSE-APACHE" "$work/x/NOTICE" "$home/" 2>/dev/null || true
# Unsigned for now: a download made by curl carries no quarantine flag, but clear one anyway so
# Gatekeeper does not stop the app (docs/agent-setup.md, Troubleshooting).
if [ "$os" = macos ] && command -v xattr >/dev/null 2>&1; then
  xattr -d com.apple.quarantine "$bin/magicpic" "$bin/magicpic-cli" 2>/dev/null || true
fi
say "installed $bin/magicpic and $bin/magicpic-cli"

if [ -z "$run_setup" ]; then
  printf '{"ok":true,"bin":"%s","cli":"%s","app":"%s"}\n' "$bin" "$bin/magicpic-cli" "$bin/magicpic"
  exit 0
fi
# The final stdout is setup's JSON: the photo folder, launcher, token file, port and both paths.
eval "set -- $setup_args"
MAGICPIC_HOME=$home exec "$bin/magicpic-cli" setup --app "$bin/magicpic" "$@"
