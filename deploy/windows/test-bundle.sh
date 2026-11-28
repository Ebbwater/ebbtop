#!/usr/bin/env bash
# Exercise the packager with a directory containing the regular release assets.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf -- "$TMP"' EXIT
sources=(ebbtop.exe ebbtop-probe.exe install.ps1 setup.ps1 LICENSE-OFL.txt THIRD-PARTY-NOTICES BUILD)
members=("${sources[@]}" SHA256SUMS 'Install EbbTop.cmd')
# The license fixtures are the repository's real notices: the zip must ship
# those exact bytes, not placeholders standing in for them.
LICENSE_SRC="$(cd "$ROOT/../.." && pwd)/crates/ebbtop-gui/assets/fonts/LICENSE-OFL.txt"
[[ -f "$LICENSE_SRC" ]] || { printf 'missing %s\n' "$LICENSE_SRC" >&2; exit 1; }
NOTICES_SRC="$(cd "$ROOT/../.." && pwd)/THIRD-PARTY-NOTICES"
[[ -f "$NOTICES_SRC" ]] || { printf 'missing %s\n' "$NOTICES_SRC" >&2; exit 1; }
mkdir "$TMP/dist with spaces" "$TMP/extracted"
cd "$TMP/dist with spaces"
for name in ebbtop.exe ebbtop-probe.exe install.ps1 setup.ps1 BUILD; do
  printf 'fixture for %s\n' "$name" > "$name"
done
cp "$LICENSE_SRC" LICENSE-OFL.txt
cp "$NOTICES_SRC" THIRD-PARTY-NOTICES
sha256sum ebbtop.exe ebbtop-probe.exe install.ps1 setup.ps1 LICENSE-OFL.txt THIRD-PARTY-NOTICES > SHA256SUMS
printf 'never bundle this\n' > stale-file

bash "$ROOT/bundle.sh" "$PWD"
archive=EbbTop-Windows-x64.zip
unzip -Z -1 "$archive" | sort > "$TMP/actual"
printf '%s\n' "${members[@]}" | sort > "$TMP/expected"
diff -u "$TMP/expected" "$TMP/actual"
unzip -q "$archive" -d "$TMP/extracted"
for name in "${sources[@]}" SHA256SUMS; do cmp "$name" "$TMP/extracted/$name"; done
cmp "$ROOT/Install EbbTop.cmd" "$TMP/extracted/Install EbbTop.cmd"
# The embedded fonts' license ships beside the exe, byte for byte: the archive
# member equals the checked-in notice, and it is the OFL text itself.
[[ -n "$(grep -F 'SIL OPEN FONT LICENSE' "$TMP/extracted/LICENSE-OFL.txt")" ]] || {
  printf 'bundled license notice is not the OFL text\n' >&2
  exit 1
}
sha256sum "$archive" > "$TMP/previous"

mkdir "$TMP/without launcher"
cp "$ROOT/bundle.sh" "$TMP/without launcher/"
if bash "$TMP/without launcher/bundle.sh" "$PWD" > "$TMP/refusal" 2>&1; then
  printf 'packager accepted a missing launcher\n' >&2
  exit 1
fi
sha256sum -c "$TMP/previous"
mv BUILD "$TMP/BUILD"
if bash "$ROOT/bundle.sh" "$PWD" > "$TMP/refusal" 2>&1; then
  printf 'packager accepted a missing BUILD\n' >&2
  exit 1
fi
sha256sum -c "$TMP/previous"
mv "$TMP/BUILD" BUILD
mv LICENSE-OFL.txt "$TMP/LICENSE-OFL.txt"
if bash "$ROOT/bundle.sh" "$PWD" > "$TMP/refusal" 2>&1; then
  printf 'packager accepted a missing font license notice\n' >&2
  exit 1
fi
sha256sum -c "$TMP/previous"
mv "$TMP/LICENSE-OFL.txt" LICENSE-OFL.txt
mv THIRD-PARTY-NOTICES "$TMP/THIRD-PARTY-NOTICES"
if bash "$ROOT/bundle.sh" "$PWD" > "$TMP/refusal" 2>&1; then
  printf 'packager accepted a missing third-party notices file\n' >&2
  exit 1
fi
sha256sum -c "$TMP/previous"
mv "$TMP/THIRD-PARTY-NOTICES" THIRD-PARTY-NOTICES

printf 'Expecting checksum refusal for the modified executable\n'
printf 'changed after the checksum was generated\n' >> ebbtop.exe
if bash "$ROOT/bundle.sh" "$PWD"; then
  printf 'packager accepted a mismatched checksum\n' >&2
  exit 1
fi
sha256sum -c "$TMP/previous"
printf 'Windows bundle checks passed (exact OFL notice bytes included)\n'
