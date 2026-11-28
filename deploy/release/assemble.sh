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
#   bash deploy/release/assemble.sh <windows-build-run-id> [macos-agent-run-id]
#   EBBTOP_MACOS_RUN=<id> bash deploy/release/assemble.sh <windows-run-id>
#
#   The macOS run id may also be given by environment. If neither is given the
#   assembly REFUSES: a release without the Apple Silicon archive would silently
#   leave the published curl installer unable to serve Macs.
#
# Environment:
#   EBBTOP_REPO      default Ebbwater/ebbtop
#   SKIP_AGENT=1     reuse an existing dist/release/ebbtop-agent-linux-x86_64.tar.gz
#   SKIP_MAC=1       reuse an existing dist/release/ebbtop-agent-macos-arm64.tar.gz
#                    (only for a local dry run; the release path must not use it)
set -euo pipefail

REPO="${EBBTOP_REPO:-Ebbwater/ebbtop}"
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
RUN_ID="${1:?usage: assemble.sh <windows-build-run-id> [macos-agent-run-id]}"
MAC_RUN="${2:-${EBBTOP_MACOS_RUN:-}}"

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
  mac_expected="$(printf '%s\n' BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh | LC_ALL=C sort)"
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
    ebbtop-agent-macos-arm64.tar.gz \
    ebbtop-install.sh "EbbTop-${VERSION}-x64.msix" > SHA256SUMS )
( cd "$OUT" && sha256sum --strict --quiet -c SHA256SUMS )

log "asset set in $OUT"
ls -l "$OUT"
log "manifest:"
cat "$OUT/SHA256SUMS"
