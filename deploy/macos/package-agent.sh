#!/usr/bin/env bash
# Build the Apple Silicon macOS release asset for the agent:
#   dist/ebbtop-agent-macos-arm64.tar.gz
#
# Members are exactly: BUILD, SHA256SUMS, ebbtop-agent, ebbtop-agent.plist,
# install-agent.sh. Nothing else is packaged: no agent.env, no token, no key,
# no path, no hostname, no bind address, no Apple signing identity.
#
# Runs on the Mac that built the binary (native aarch64). The tree must be
# clean and the binary must be a Mach-O arm64 executable built for this same
# source tree; the member list, the BUILD stamp and the tarball sha256 are
# printed. The tarball is reproducible: members are sorted, every mtime is
# the commit time and tar ids are 0/0.
#
# Usage: bash deploy/macos/package-agent.sh
#   EBBTOP_AGENT_BIN  package this binary instead of the cargo release build
#                     (the test harness uses it to prove the Mach-O refusals)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

NAME=ebbtop-agent-macos-arm64
TARGET_TRIPLE=aarch64-apple-darwin
DIST="$ROOT/dist"
BIN="${EBBTOP_AGENT_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET_TRIPLE/release/ebbtop-agent}"
MEMBERS=(BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh THIRD-PARTY-NOTICES)
PAYLOAD=(BUILD ebbtop-agent ebbtop-agent.plist THIRD-PARTY-NOTICES)

log() { echo "package-agent: $*"; }
die() { echo "package-agent: $*" >&2; exit 1; }

# --- source must be a committed tree ---------------------------------------
command -v git >/dev/null 2>&1 || die "git is required to record the commit in BUILD"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "$ROOT is not a git work tree"
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  git status --short --untracked-files=no >&2
  die "refusing to package a dirty tree: commit the changes above first"
fi
COMMIT="$(git rev-parse HEAD)"
EPOCH="$(git log -1 --format=%ct)"

[ -d deploy/macos ] || die "deploy/macos is missing"
[ -f deploy/macos/ebbtop-agent.plist.in ] || die "deploy/macos/ebbtop-agent.plist.in is missing"
[ -f deploy/macos/install-agent.sh ] || die "deploy/macos/install-agent.sh is missing"

# --- host must be able to produce an arm64 Mach-O --------------------------
if [[ "$(uname -s)" != Darwin ]]; then
  die "this package builds a Darwin binary: run it on macOS (uname -s reports $(uname -s))"
fi

# --- binary ----------------------------------------------------------------
if [[ ! -f "$BIN" ]]; then
  [[ -n "${EBBTOP_AGENT_BIN:-}" ]] && die "EBBTOP_AGENT_BIN=$BIN does not exist"
  command -v cargo >/dev/null 2>&1 || die "no binary at $BIN and cargo is not on PATH"
  log "building $BIN"
  ( cd "$ROOT" && cargo build --release --locked --target "$TARGET_TRIPLE" \
      -p ebbtop-agent --no-default-features ) >&2
fi
[ -x "$BIN" ] || die "$BIN is not executable"

# A Mach-O file begins with one of the 32-bit magics. Check that, then ask
# the platform's own tool for the CPU type and the minimum macOS version.
magic="$(head -c 4 "$BIN" | od -An -tx1 | tr -d ' \n')"
case "$magic" in
  cffaedfe|cafebabe|cefaedfe) ;;
  *) die "refusing to package $BIN: not a Mach-O file (magic $magic)" ;;
esac
command -v file >/dev/null 2>&1 || die "file(1) is required for the Mach-O check"
FILE_OUT="$(file -b "$BIN")"
case "$FILE_OUT" in
  *arm64*) ;;
  *) die "wrong CPU type for $TARGET_TRIPLE: $FILE_OUT" ;;
esac
command -v lipo >/dev/null 2>&1 || die "lipo is required for the architecture check"
ARCHS="$(lipo -archs "$BIN" 2>/dev/null || true)"
[[ "$ARCHS" == arm64 ]] || die "binary architectures are '$ARCHS', expected exactly 'arm64'"
command -v otool >/dev/null 2>&1 || die "otool is required for the deployment-target check"
MINOS="$(otool -l "$BIN" | awk '/LC_BUILD_VERSION|LC_VERSION_MIN_MACOSX/{f=1} f&&/minos|version/{print $2; exit}')"
[[ -n "$MINOS" ]] || die "cannot read the minimum macOS version of $BIN (otool -l)"

VERSION="$("$BIN" --version | awk '{print $2}')"
[[ -n "$VERSION" ]] || die "refusing to package $BIN: --version printed no version"

# --- stage ----------------------------------------------------------------
mkdir -p "$DIST"
stage="$(mktemp -d "$DIST/.stage.XXXXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
install -m 755 "$BIN" "$stage/ebbtop-agent"
install -m 755 "$ROOT/deploy/macos/install-agent.sh" "$stage/install-agent.sh"

# The plist template is packaged as-is: the installer renders the two paths
# and the port at install time, so one archive serves any user and any home.
install -m 644 "$ROOT/deploy/macos/ebbtop-agent.plist.in" "$stage/ebbtop-agent.plist"
# License attribution for the crates compiled into the binary (cargo-about;
# regenerate in the same commit as any dependency change).
install -m 644 "$ROOT/THIRD-PARTY-NOTICES" "$stage/THIRD-PARTY-NOTICES"

# BUILD is self-describing metadata: version and commit, nothing host-specific.
{
  echo "name=ebbtop-agent"
  echo "version=$VERSION"
  echo "commit=$COMMIT"
  echo "target=$TARGET_TRIPLE"
  echo "min_macos=$MINOS"
} > "$stage/BUILD"
chmod 644 "$stage/BUILD"

( cd "$stage" && shasum -a 256 "${PAYLOAD[@]}" > SHA256SUMS && shasum -a 256 --strict -c SHA256SUMS ) \
  || die "staged checksums do not verify"

# --- archive --------------------------------------------------------------
archive="$DIST/$NAME.tar.gz"
out="$(mktemp "$DIST/.$NAME.XXXXXXXX")"
# BSD tar (macOS) has no --sort/--owner; COPYFILE_DISABLE stops it from
# adding ._ AppleDouble members, which would break the exact member set the
# installer and bootstrap verify.
( cd "$stage" && COPYFILE_DISABLE=1 tar --format ustar \
    --uid 0 --gid 0 --uname root --gname wheel \
    -czf "$out" "${MEMBERS[@]}" ) || die "cannot write $archive"
chmod 644 "$out"
mv -f -- "$out" "$archive"

# --- prove the archive is what we claim -----------------------------------
# The member-set check lives in deploy/macos/member-check.sh: it is also run
# directly by the fixture under a real Bash 3.2, so the macOS-stock-Bash claim
# is proven by executing that file rather than by parsing this one.
# shellcheck source=member-check.sh
. "$ROOT/deploy/macos/member-check.sh"
member_l="$(member_set_matches "$archive" "${MEMBERS[@]}")" \
  || die "archive member set mismatch: $member_l"
grep -qx "commit=$COMMIT" <(tar -xzOf "$archive" BUILD) || die "BUILD in the archive lost the commit"
grep -qx "version=$VERSION" <(tar -xzOf "$archive" BUILD) || die "BUILD in the archive lost the version"
if tar -tzf "$archive" | grep -Eq '(\.env$|\.token$|\.pem$|\.key$|/id_|\._|/\.DS_Store$)'; then
  die "archive carries a secret-shaped or macOS metadata member"
fi
# The plist that ships must not assign a credential or a bind address; it
# names paths and the port only.
if tar -xzOf "$archive" ebbtop-agent.plist | grep -Eq 'EBBTOP_TOKEN=|EBBTOP_BIND=|SOCKTOP_TOKEN='; then
  die "the packaged plist assigns a credential or a bind address"
fi
if tar -xzOf "$archive" ebbtop-agent.plist | grep -Eq '<string>[0-9a-fA-F]{32,}</string>'; then
  die "the packaged plist carries a credential-shaped value"
fi

size="$(stat -f %z "$archive")"
digest="$(shasum -a 256 "$archive" | cut -d' ' -f1)"
# The release assembly reads this sidecar, and so does a user who wants to
# verify a download without the API. Bare `<digest>  <name>` so
# `sha256sum -c`/`shasum -c` both accept it from the download directory.
printf '%s  %s\n' "$digest" "$NAME.tar.gz" > "$archive.sha256"
chmod 644 "$archive.sha256"

echo "package-agent: wrote $archive"
echo "package-agent: size $size bytes"
echo "package-agent: sha256 $digest"
echo "package-agent: min macOS $MINOS"
echo "package-agent: version $VERSION commit $COMMIT target $TARGET_TRIPLE"
echo "package-agent: members: ${MEMBERS[*]}"

# --- evidence --------------------------------------------------------------
# The workflow uploads dist/macos-evidence/ so the native run leaves a
# reconstructible record next to the artifact: the commit, the archive digest,
# the member list, the BUILD stamp and the Mach-O facts the packaging checked.
EVIDENCE="$DIST/macos-evidence"
mkdir -p "$EVIDENCE"
{
  echo "commit=$COMMIT"
  echo "version=$VERSION"
  echo "target=$TARGET_TRIPLE"
  echo "min_macos=$MINOS"
  echo "archive=$NAME.tar.gz"
  echo "size=$size"
  echo "sha256=$digest"
  echo "uname=$(uname -a)"
  echo "sw_vers=$(sw_vers -productVersion 2>/dev/null || echo unknown)"
  echo "rustc=$(rustc --version)"
  echo "cargo=$(cargo --version)"
} > "$EVIDENCE/build.txt"
tar -tzf "$archive" > "$EVIDENCE/members.txt"
tar -xzOf "$archive" BUILD > "$EVIDENCE/BUILD"
file -b "$BIN" > "$EVIDENCE/macho.txt"
lipo -archs "$BIN" >> "$EVIDENCE/macho.txt"
otool -l "$BIN" | awk '/LC_BUILD_VERSION|LC_VERSION_MIN_MACOSX/{f=1} f&&/minos|version/{print "minos " $2; exit}' \
  >> "$EVIDENCE/macho.txt"
echo "package-agent: evidence in $EVIDENCE"
