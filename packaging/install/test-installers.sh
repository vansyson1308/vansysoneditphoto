#!/bin/sh
# Test install.sh against archives made by package.sh, without a release or a network.
#
#   test-installers.sh [<dir with magicpic and magicpic-cli>]
#
# With a binary folder (a local `cargo build`), the real CLI runs `setup` and its JSON is checked.
# Without one, stand-in programs are packaged, which tests download, checksum and copy only.
# Checks: a good archive installs both programs and prints JSON; a tampered archive and a
# SHA256SUMS without the archive's line are refused and install nothing.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
case $(uname -s) in Darwin) os=macos ;; *) os=linux ;; esac
case $(uname -m) in arm64 | aarch64) arch=aarch64 ;; *) arch=x86_64 ;; esac
platform=$os-$arch
fail() { echo "FAIL: $*" >&2; exit 1; }

if [ $# -ge 1 ]; then
  bins=$(cd "$1" && pwd)
  real=1
else
  bins=$t/fake
  mkdir -p "$bins"
  printf '#!/bin/sh\necho magicpic "$@"\n' > "$bins/magicpic"
  printf '#!/bin/sh\nprintf "{\\"fake\\":true,\\"home\\":\\"%%s\\"}\\n" "$MAGICPIC_HOME"\n' > "$bins/magicpic-cli"
  real=
fi

"$here/package.sh" "$platform" "$bins" "$t/release" >/dev/null
grep -q "  magicpic-$platform.tar.gz\$" "$t/release/SHA256SUMS" || fail "SHA256SUMS lacks the archive"
(cd "$t/release" && if command -v sha256sum >/dev/null 2>&1; then sha256sum -c SHA256SUMS; else shasum -a 256 -c SHA256SUMS; fi) >/dev/null || fail "SHA256SUMS does not verify"

# 1. A good archive installs both programs; stdout is JSON.
export HOME=$t/home
mkdir -p "$HOME"
MAGICPIC_HOME=$t/home/.magicpic sh "$here/install.sh" --from "$t/release" --photos "$t/home/My Photos" > "$t/out.json" 2> "$t/err.txt" || { cat "$t/err.txt" >&2; fail "install failed"; }
for b in magicpic magicpic-cli; do [ -x "$t/home/.magicpic/bin/$b" ] || fail "$b not installed"; done
python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$t/out.json" || fail "stdout is not JSON: $(cat "$t/out.json")"
if [ -n "$real" ]; then
  python3 - "$t" <<'EOF' || fail "setup JSON"
import json, os, sys
t = sys.argv[1]
d = json.load(open(f"{t}/out.json"))
assert d["ok"] is True, d
assert d["photos"] == f"{t}/home/My Photos" and os.path.isdir(d["photos"]), d
assert d["app"] == f"{t}/home/.magicpic/bin/magicpic", d
assert os.access(d["launcher"], os.X_OK), d
cfg = json.load(open(f"{t}/home/.magicpic/config.json"))
assert cfg["photos"] == d["photos"] and cfg["port"] == 7878, cfg
EOF
fi
grep -q 'checksum ok' "$t/err.txt" || fail "no checksum message"

# 2. A tampered archive is refused and leaves the installed programs alone.
cp "$t/home/.magicpic/bin/magicpic-cli" "$t/before"
printf 'x' >> "$t/release/magicpic-$platform.tar.gz"
if MAGICPIC_HOME=$t/home/.magicpic sh "$here/install.sh" --from "$t/release" --no-setup > /dev/null 2> "$t/err2.txt"; then fail "a tampered archive was installed"; fi
grep -q 'checksum mismatch' "$t/err2.txt" || fail "no mismatch message: $(cat "$t/err2.txt")"
cmp -s "$t/before" "$t/home/.magicpic/bin/magicpic-cli" || fail "the refused archive changed the installed CLI"

# 3. A SHA256SUMS without the archive's line is refused.
: > "$t/release/SHA256SUMS"
if MAGICPIC_HOME=$t/home/.magicpic sh "$here/install.sh" --from "$t/release" --no-setup > /dev/null 2> "$t/err3.txt"; then fail "installed without a checksum"; fi
grep -q 'has no line' "$t/err3.txt" || fail "no missing-line message"

echo "installer tests passed ($platform${real:+, real CLI})"
