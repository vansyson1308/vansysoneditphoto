#!/bin/sh
# Bundle built binaries into the archive the installers download, and add its line to SHA256SUMS.
#
#   package.sh <platform> <binary dir> <out dir>
#
# <platform> is windows-x86_64, macos-aarch64, macos-x86_64 or linux-x86_64. The archive is
# magicpic-<platform>.zip (Windows) or .tar.gz, with no version in the name so
# releases/latest/download/<name> always works. It holds magicpic(.exe), magicpic-cli(.exe), the
# licence files and NOTICE at its top level. Used by .github/workflows/agent-binaries.yml and by
# packaging/install/test-installers.sh, so the installers are tested against the real layout.
set -eu

platform=$1
bin_dir=$2
out_dir=$3
root=$(cd "$(dirname "$0")/../.." && pwd)

case $platform in
  windows-*) ext=.exe; archive=magicpic-$platform.zip ;;
  macos-* | linux-*) ext=; archive=magicpic-$platform.tar.gz ;;
  *) echo "package.sh: unknown platform $platform" >&2; exit 2 ;;
esac

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
for b in magicpic magicpic-cli; do
  cp "$bin_dir/$b$ext" "$stage/$b$ext"
  chmod +x "$stage/$b$ext"
done
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$root/NOTICE" "$stage/"

mkdir -p "$out_dir"
out_dir=$(cd "$out_dir" && pwd)
rm -f "$out_dir/$archive"
case $archive in
  *.zip) (cd "$stage" && if command -v zip >/dev/null 2>&1; then zip -q -r "$out_dir/$archive" .; else 7z a -tzip "$out_dir/$archive" . >/dev/null; fi) ;;
  *) tar -C "$stage" -czf "$out_dir/$archive" . ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
  sum=$(sha256sum "$out_dir/$archive" | cut -d' ' -f1)
else
  sum=$(shasum -a 256 "$out_dir/$archive" | cut -d' ' -f1)
fi
# One line per archive, in the format `sha256sum -c` reads.
touch "$out_dir/SHA256SUMS"
grep -v "  $archive\$" "$out_dir/SHA256SUMS" > "$out_dir/SHA256SUMS.tmp" || true
echo "$sum  $archive" >> "$out_dir/SHA256SUMS.tmp"
sort -k2 "$out_dir/SHA256SUMS.tmp" > "$out_dir/SHA256SUMS"
rm -f "$out_dir/SHA256SUMS.tmp"
echo "$out_dir/$archive"
