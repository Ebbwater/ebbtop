#!/usr/bin/env bash
# Assemble a release asset set and print the manifest to attach.
#
# The Windows artifacts are NOT built here. They come from the hosted
# `windows-build` run on the exact release commit, because the MSIX that goes to
# Partner Center and the ZIP that goes to users have to be the bytes WACK
# certified. The Apple Silicon agent archive comes from the `macos-agent` run on
# the same commit for the same reason: only a real arm64 Mac can build, test and
# package it, so only that run's bytes may ship. This script downloads both
# runs' artifacts, refuses anything whose BUILD stamp names a different commit
# or version, builds the one asset neither CI job produces (the Linux agent
# tarball), and writes dist/release/SHA256SUMS over the whole set.
#
# Usage:
#   bash deploy/release/assemble.sh <windows-build-run-id> [macos-agent-run-id] [macos-gui-run-id]
#   EBBTOP_MACOS_RUN=<id> EBBTOP_MACOS_GUI_RUN=<id> bash deploy/release/assemble.sh <windows-run-id>
#
#   The macOS run ids may also be given by environment. If either is missing the
#   assembly REFUSES: a release without the Apple Silicon archive would silently
#   leave the published curl installer unable to serve Macs, and one without the
#   desktop bundle would silently break the macOS in-app updater (whose asset is
#   exactly EbbTop.app.zip).
#
# Environment:
#   EBBTOP_REPO      default Ebbwater/ebbtop
#   SKIP_AGENT=1     reuse an existing dist/release/ebbtop-agent-linux-x86_64.tar.gz
#   SKIP_MAC=1       reuse an existing dist/release/ebbtop-agent-macos-arm64.tar.gz
#   SKIP_MACGUI=1    reuse an existing dist/release/EbbTop.app.zip
#                    (SKIP_* are only for a local dry run; the release path
#                    must not use them)
set -euo pipefail

REPO="${EBBTOP_REPO:-Ebbwater/ebbtop}"
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
RUN_ID="${1:?usage: assemble.sh <windows-build-run-id> [macos-agent-run-id] [macos-gui-run-id]}"
MAC_RUN="${2:-${EBBTOP_MACOS_RUN:-}}"
MAC_GUI_RUN="${3:-${EBBTOP_MACOS_GUI_RUN:-}}"

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/dist/release"
WORK="$(mktemp -d)"
trap 'rm -rf -- "$WORK"' EXIT

log() { echo "assemble: $*"; }
die() { echo "assemble: $*" >&2; exit 1; }

cd "$ROOT"
[[ -n "$VERSION" ]] || die "cannot read the workspace version from Cargo.toml"
[[ -n "$(git status --porcelain --untracked-files=no)" ]] \
  && die "refusing to assemble from a dirty tree"

COMMIT="$(git rev-parse HEAD)"
log "tree $COMMIT, version $VERSION, hosted run $RUN_ID${MAC_RUN:+, macos run $MAC_RUN}"

# --- the hosted run must be the release commit and green ---------------------
RUN="$(gh api "repos/$REPO/actions/runs/$RUN_ID")"
HEAD_SHA="$(jq -r .head_sha <<<"$RUN")"
CONCLUSION="$(jq -r .conclusion <<<"$RUN")"
[[ "$HEAD_SHA" == "$COMMIT" ]] \
  || die "run $RUN_ID was built from $HEAD_SHA, not $COMMIT"
[[ "$CONCLUSION" == "success" ]] \
  || die "run $RUN_ID concluded '$CONCLUSION', not success"

# --- Windows artifacts from the run -----------------------------------------
# Artifact names end in the tag (tag builds) or the run number (dispatch), so
# discover them instead of composing the suffix.
log "downloading the run's artifacts"
gh api "repos/$REPO/actions/runs/$RUN_ID/artifacts" \
  --jq '.artifacts[] | select(.expired == false) | "\(.name)"' > "$WORK/artifacts.txt"
grep -q '^EbbTop-Windows-x64-' "$WORK/artifacts.txt" || die "run $RUN_ID has no Windows artifact"
grep -q '^EbbTop-Store-msix-' "$WORK/artifacts.txt" || die "run $RUN_ID has no MSIX artifact"
grep -q '^EbbTop-WACK-report-' "$WORK/artifacts.txt" || die "run $RUN_ID has no WACK report"

gh run download "$RUN_ID" -R "$REPO" \
  -n "$(grep -m1 '^EbbTop-Windows-x64-' "$WORK/artifacts.txt")" -D "$WORK/win"
gh run download "$RUN_ID" -R "$REPO" \
  -n "$(grep -m1 '^EbbTop-Store-msix-' "$WORK/artifacts.txt")" -D "$WORK/msix"
gh run download "$RUN_ID" -R "$REPO" \
  -n "$(grep -m1 '^EbbTop-WACK-report-' "$WORK/artifacts.txt")" -D "$WORK/wack"

WIN="$(find "$WORK/win" -name BUILD -printf '%h\n' | head -n 1)"
[[ -n "$WIN" ]] || die "no Windows artifact set in run $RUN_ID"
grep -qx "version $VERSION" "$WIN/BUILD" || die "Windows BUILD is not $VERSION: $(tr '\n' ' ' <"$WIN/BUILD")"
grep -qx "commit ${COMMIT:0:7}" "$WIN/BUILD" \
  || die "Windows BUILD names another commit: $(tr '\n' ' ' <"$WIN/BUILD")"

MSIX="$(find "$WORK/msix" -name '*.msix' | head -n 1)"
[[ -n "$MSIX" ]] || die "no MSIX in run $RUN_ID"
grep -qx "commit ${COMMIT:0:7}" "$(dirname "$MSIX")/BUILD" \
  || die "MSIX BUILD names another commit"

WACK="$(find "$WORK/wack" -name 'wack-report.xml' | head -n 1)"
[[ -n "$WACK" ]] || die "no WACK report in run $RUN_ID"
python3 - "$WACK" <<'PY'
import sys, xml.etree.ElementTree as ET
r = ET.parse(sys.argv[1]).getroot()
overall = r.get('OVERALL_RESULT')
tests = r.findall('.//REQUIREMENT/TEST')
fails = [t for t in tests if (t.findtext('RESULT') or '').strip() == 'FAIL']
blocking = [t for t in fails if t.get('OPTIONAL') != 'TRUE']
print(f"assemble: WACK overall={overall} tests={len(tests)} fail={len(fails)} blocking={len(blocking)}")
for t in fails:
    print(f"assemble:   {'BLOCKING' if t.get('OPTIONAL') != 'TRUE' else 'optional'} FAIL: {t.get('NAME')}")
if blocking:
    sys.exit("assemble: WACK has a blocking failure")
PY

# --- the Linux agent archive, built here ------------------------------------
AGENT="$OUT/ebbtop-agent-linux-x86_64.tar.gz"
if [[ "${SKIP_AGENT:-0}" != "1" ]]; then
  log "building the agent archive"
  cargo build --release --locked -p ebbtop-agent --no-default-features --features gpu >&2
  bash deploy/linux/package-agent.sh >&2
  mkdir -p "$OUT"
  cp dist/ebbtop-agent-linux-x86_64.tar.gz "$AGENT"
fi
[[ -f "$AGENT" ]] || die "no agent archive at $AGENT"

# --- the Apple Silicon agent archive, from the macos-agent run --------------
# Only a native arm64 Mac can build this, so it is taken from the run rather
# than produced here, and the BUILD stamp inside it must name THIS commit and
# version. Without it the published curl installer cannot serve macOS at all,
# which is why a missing run id is a refusal rather than a warning.
MAC_AGENT="$OUT/ebbtop-agent-macos-arm64.tar.gz"
if [[ "${SKIP_MAC:-0}" != "1" ]]; then
  [[ -n "$MAC_RUN" ]] \
    || die "no macos-agent run id: pass it as the second argument or set EBBTOP_MACOS_RUN. The Apple Silicon archive must come from a native run on $COMMIT (SKIP_MAC=1 is for a local dry run only)"
  MACRUN="$(gh api "repos/$REPO/actions/runs/$MAC_RUN")" \
    || die "cannot read the macos-agent run $MAC_RUN"
  MAC_SHA="$(jq -r .head_sha <<<"$MACRUN")"
  MAC_CONC="$(jq -r .conclusion <<<"$MACRUN")"
  [[ "$MAC_SHA" == "$COMMIT" ]] \
    || die "macos-agent run $MAC_RUN was built from $MAC_SHA, not $COMMIT"
  [[ "$MAC_CONC" == "success" ]] \
    || die "macos-agent run $MAC_RUN concluded '$MAC_CONC', not success"

  log "downloading the macos-agent artifact"
  gh api "repos/$REPO/actions/runs/$MAC_RUN/artifacts" \
    --jq '.artifacts[] | select(.expired == false) | "\(.name)"' > "$WORK/mac-artifacts.txt"
  MAC_ART="$(grep -m1 '^ebbtop-agent-macos-arm64-' "$WORK/mac-artifacts.txt" || true)"
  [[ -n "$MAC_ART" ]] || die "macos-agent run $MAC_RUN has no ebbtop-agent-macos-arm64 artifact"
  gh run download "$MAC_RUN" -R "$REPO" -n "$MAC_ART" -D "$WORK/mac"
  SRC="$(find "$WORK/mac" -name 'ebbtop-agent-macos-arm64.tar.gz' -type f | head -n 1)"
  [[ -n "$SRC" ]] || die "the macos-agent artifact carries no ebbtop-agent-macos-arm64.tar.gz"

  # The BUILD stamp inside the archive is the authority for which commit and
  # version these bytes are, not the artifact name.
  grep -qx "commit=$COMMIT" <(tar -xzOf "$SRC" BUILD) \
    || die "the Mac archive's BUILD does not name $COMMIT: $(tar -xzOf "$SRC" BUILD | tr '\n' ' ')"
  grep -qx "version=$VERSION" <(tar -xzOf "$SRC" BUILD) \
    || die "the Mac archive's BUILD does not name version $VERSION: $(tar -xzOf "$SRC" BUILD | tr '\n' ' ')"
  grep -qx "target=aarch64-apple-darwin" <(tar -xzOf "$SRC" BUILD) \
    || die "the Mac archive is not aarch64-apple-darwin"

  # Member set and manifest, checked here rather than trusted: the archive is
  # about to be published under our name, and the bootstrap will verify it at
  # install time with the same rules.
  mapfile -t mac_members < <(tar -tzf "$SRC" | LC_ALL=C sort)
  mac_expected="$(printf '%s\n' BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh THIRD-PARTY-NOTICES | LC_ALL=C sort)"
  [[ "$(printf '%s\n' "${mac_members[@]}")" == "$mac_expected" ]] \
    || die "the Mac archive's member set is ${mac_members[*]}"
  EXTRACT="$WORK/mac-x"
  mkdir -p "$EXTRACT"
  tar -xzf "$SRC" -C "$EXTRACT"
  ( cd "$EXTRACT" && sha256sum --strict --quiet -c SHA256SUMS ) \
    || die "the Mac archive's own SHA256SUMS did not verify"
  if grep -Eq '(\.env$|\.token$|\.pem$|\.key$)' <<<"${mac_members[*]}"; then
    die "the Mac archive carries a secret-shaped member"
  fi
  mkdir -p "$OUT"
  install -m 644 "$SRC" "$MAC_AGENT"
  install -m 644 "$SRC.sha256" "$MAC_AGENT.sha256" 2>/dev/null || true
  # The sidecar, when the packaging step produced one, must agree.
  if [[ -f "$MAC_AGENT.sha256" ]]; then
    want="$(cut -d' ' -f1 "$MAC_AGENT.sha256")"
    got="$(sha256sum "$MAC_AGENT" | cut -d' ' -f1)"
    [[ "$want" == "$got" ]] || die "the Mac archive's .sha256 sidecar does not match the archive"
  fi
fi
[[ -f "$MAC_AGENT" ]] || die "no Mac agent archive at $MAC_AGENT"

# --- the macOS desktop bundle, from the macos-gui run ------------------------
# Same rule as the agent archive: only a real Mac can produce it (cargo for
# aarch64-apple-darwin, iconutil, codesign), so only a hosted run's bytes may
# ship. The in-app updater on macOS downloads exactly the asset name
# EbbTop.app.zip and verifies it against SHA256SUMS, so a release without this
# file makes every mac client's update check fail — a missing run id is a
# refusal rather than a warning.
MACGUI_ZIP="$OUT/EbbTop.app.zip"
if [[ "${SKIP_MACGUI:-0}" != "1" ]]; then
  [[ -n "$MAC_GUI_RUN" ]] \
    || die "no macos-gui run id: pass it as the third argument or set EBBTOP_MACOS_GUI_RUN. The desktop bundle must come from a native run on $COMMIT (SKIP_MACGUI=1 is for a local dry run only)"
  MACGUIRUN="$(gh api "repos/$REPO/actions/runs/$MAC_GUI_RUN")" \
    || die "cannot read the macos-gui run $MAC_GUI_RUN"
  MACGUI_SHA="$(jq -r .head_sha <<<"$MACGUIRUN")"
  MACGUI_CONC="$(jq -r .conclusion <<<"$MACGUIRUN")"
  [[ "$MACGUI_SHA" == "$COMMIT" ]] \
    || die "macos-gui run $MAC_GUI_RUN was built from $MACGUI_SHA, not $COMMIT"
  [[ "$MACGUI_CONC" == "success" ]] \
    || die "macos-gui run $MAC_GUI_RUN concluded '$MACGUI_CONC', not success"

  log "downloading the macos-gui artifact"
  gh api "repos/$REPO/actions/runs/$MAC_GUI_RUN/artifacts" \
    --jq '.artifacts[] | select(.expired == false) | "\(.name)"' > "$WORK/macgui-artifacts.txt"
  MACGUI_ART="$(grep -m1 '^EbbTop-app-' "$WORK/macgui-artifacts.txt" || true)"
  [[ -n "$MACGUI_ART" ]] || die "macos-gui run $MAC_GUI_RUN has no EbbTop-app artifact"
  gh run download "$MAC_GUI_RUN" -R "$REPO" -n "$MACGUI_ART" -D "$WORK/macgui"
  MACGUI_SRC="$(find "$WORK/macgui" -name 'EbbTop.app.zip' -type f | head -n 1)"
  [[ -n "$MACGUI_SRC" ]] || die "the macos-gui artifact carries no EbbTop.app.zip"

  # The sidecar from the run must agree with the downloaded bytes.
  MACGUI_SIDECAR="$(find "$WORK/macgui" -name 'EbbTop.app.zip.sha256' -type f | head -n 1)"
  [[ -n "$MACGUI_SIDECAR" ]] || die "the macos-gui artifact carries no EbbTop.app.zip.sha256 sidecar"
  want="$(cut -d' ' -f1 "$MACGUI_SIDECAR")"
  got="$(sha256sum "$MACGUI_SRC" | cut -d' ' -f1)"
  [[ "$want" == "$got" ]] || die "EbbTop.app.zip does not match its .sha256 sidecar"

  # The zip is about to be published under our name: prove its shape here,
  # the same rules the packaging script proved on the Mac.
  unzip -tq "$MACGUI_SRC" > /dev/null || die "EbbTop.app.zip does not unzip cleanly"
  mapfile -t zip_members < <(unzip -Z1 "$MACGUI_SRC")
  [[ "${zip_members[0]}" == "EbbTop.app/"* || "${zip_members[0]}" == "EbbTop.app/" ]] \
    || die "the bundle zip's first member is ${zip_members[0]}, not EbbTop.app/"
  for part in "EbbTop.app/Contents/Info.plist" "EbbTop.app/Contents/MacOS/ebbtop" "EbbTop.app/Contents/Resources/EbbTop.icns"; do
    [[ " ${zip_members[*]} " == *" $part "* ]] \
      || die "the bundle zip is missing $part"
  done
  if grep -Eq '(\.env$|\.token$|\.pem$|\.key$)' <<<"${zip_members[*]}"; then
    die "the bundle zip carries a secret-shaped member"
  fi
  # The binary must be an arm64 Mach-O (magic cffaedfe), read back out of the
  # packed bytes rather than trusted from the run. Extract to a file first:
  # piping the 27 MB payload into `head -c 4` makes unzip die of SIGPIPE on
  # Linux, and with pipefail that kills this script (exit 141).
  EXEBIN="$WORK/packed-ebbtop"
  unzip -p "$MACGUI_SRC" "EbbTop.app/Contents/MacOS/ebbtop" > "$EXEBIN"
  magic="$(head -c 4 "$EXEBIN" | od -An -tx1 | tr -d ' \n')"
  [[ "$magic" == "cffaedfe" ]] || die "the packaged ebbtop binary is not an arm64 Mach-O (magic $magic)"
  # Info.plist must state this release's version; parse the XML rather than
  # grep a substring out of it. A plist dict interleaves <key> and value
  # elements, so walk the child list pairwise — indexing by the key list's
  # enumerate index would land on the next key, not the value.
  python3 - "$MACGUI_SRC" "$VERSION" <<'PY'
import sys, zipfile, xml.etree.ElementTree as ET
src, want = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(src) as z:
    plist = ET.fromstring(z.read("EbbTop.app/Contents/Info.plist"))
def get(key):
    for d in plist.findall("dict"):
        children = list(d)
        for j, el in enumerate(children):
            if el.tag == "key" and el.text == key and j + 1 < len(children):
                return children[j + 1].text
    return None
short = get("CFBundleShortVersionString")
if short != want:
    sys.exit(f"assemble: bundle CFBundleShortVersionString is {short}, not {want}")
print(f"assemble: bundle version {short}, executable {get('CFBundleExecutable')}")
PY

  mkdir -p "$OUT"
  install -m 644 "$MACGUI_SRC" "$MACGUI_ZIP"
  install -m 644 "$MACGUI_SIDECAR" "$MACGUI_ZIP.sha256"
fi
[[ -f "$MACGUI_ZIP" ]] || die "no macOS bundle at $MACGUI_ZIP"

# --- stage the set ----------------------------------------------------------
mkdir -p "$OUT"
install -m 644 "$WIN/ebbtop.exe" "$WIN/ebbtop-probe.exe" \
  "$WIN/install.ps1" "$WIN/setup.ps1" "$WIN/LICENSE-OFL.txt" "$WIN/THIRD-PARTY-NOTICES" \
  "$WIN/EbbTop-Windows-x64.zip" "$WIN/BUILD" "$WIN/VERSIONINFO.txt" "$OUT/"
install -m 755 deploy/bootstrap/ebbtop-install.sh "$OUT/ebbtop-install.sh"
install -m 644 "$MSIX" "$OUT/EbbTop-${VERSION}-x64.msix"
install -m 644 "$(dirname "$MSIX")/EbbTop-${VERSION}-x64.msix.sha256" "$OUT/" 2>/dev/null || true
install -m 644 "$(dirname "$MSIX")/BUILD" "$OUT/BUILD.msix"
install -m 644 "$WACK" "$OUT/wack-report.xml"

# SHA256SUMS covers the user-facing assets, not the reports. Names are bare so
# `sha256sum -c` works from the download directory on any platform. The Mac
# agent archive and the shared bootstrap are in it: a Mac user installs exactly
# the bytes these lines name, from the same release.
( cd "$OUT" && sha256sum \
    ebbtop.exe ebbtop-probe.exe install.ps1 setup.ps1 LICENSE-OFL.txt THIRD-PARTY-NOTICES \
    EbbTop-Windows-x64.zip ebbtop-agent-linux-x86_64.tar.gz \
    ebbtop-agent-macos-arm64.tar.gz EbbTop.app.zip \
    ebbtop-install.sh "EbbTop-${VERSION}-x64.msix" > SHA256SUMS )
( cd "$OUT" && sha256sum --strict --quiet -c SHA256SUMS )

log "asset set in $OUT"
ls -l "$OUT"
log "manifest:"
cat "$OUT/SHA256SUMS"
