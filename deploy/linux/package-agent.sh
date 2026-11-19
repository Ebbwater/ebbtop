#!/usr/bin/env bash
# Build the x86-64 Linux release asset for the agent:
#   dist/ebbtop-agent-linux-x86_64.tar.gz
#
# Members are exactly: ebbtop-agent, install-agent.sh, ebbtop-agent.service,
# BUILD, SHA256SUMS. Nothing else is packaged: no agent.env, no token, no key,
# no path, no hostname, no bind address.
#
# The tree must be clean and the binary must be a Linux x86-64 ELF. Both the
# member list and the tarball sha256 are printed. The tarball is reproducible:
# members are sorted, all mtimes are the commit time and tar ids are 0/0.
#
# Usage: bash deploy/linux/package-agent.sh
#   EBBTOP_AGENT_BIN  package this binary instead of the cargo release build
#                     (used by the test harness to prove the ELF refusals)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

NAME=ebbtop-agent-linux-x86_64
TARGET_TRIPLE=x86_64-unknown-linux-gnu
DIST="$ROOT/dist"
BIN="${EBBTOP_AGENT_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/release/ebbtop-agent}"
MEMBERS=(BUILD SHA256SUMS ebbtop-agent ebbtop-agent.service install-agent.sh)
PAYLOAD=(BUILD ebbtop-agent ebbtop-agent.service)

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

[ -d deploy/linux ] || die "deploy/linux is missing"

# --- binary ----------------------------------------------------------------
if [[ ! -f "$BIN" ]]; then
  [[ -n "${EBBTOP_AGENT_BIN:-}" ]] && die "EBBTOP_AGENT_BIN=$BIN does not exist"
  command -v cargo >/dev/null 2>&1 || die "no binary at $BIN and cargo is not on PATH"
  log "building $BIN"
  ( cd "$ROOT" && cargo build --release --locked -p ebbtop-agent --no-default-features --features gpu ) >&2
fi
[ -x "$BIN" ] || die "$BIN is not executable"

is_elf() { [ "$(head -c 4 "$1" | od -An -tx1 | tr -d ' \n')" = "7f454c46" ]; }
is_elf "$BIN" || die "refusing to package $BIN: not an ELF file"

command -v readelf >/dev/null 2>&1 || die "readelf (binutils) is required for the ELF checks"
ELF_CLASS="$(readelf -h "$BIN" | sed -n 's/^ *Class: *//p')"
ELF_MACHINE="$(readelf -h "$BIN" | sed -n 's/^ *Machine: *//p')"
INTERP="$(readelf -l "$BIN" | sed -n 's/.*interpreter: *\([^]]*\)].*/\1/p' | head -n 1)"
[[ "$ELF_CLASS" == ELF64 ]] || die "wrong ELF class for $TARGET_TRIPLE: $ELF_CLASS"
[[ "$ELF_MACHINE" == "Advanced Micro Devices X86-64" ]] \
  || die "wrong ELF machine for $TARGET_TRIPLE: $ELF_MACHINE"
[[ "$INTERP" == *"/ld-linux-x86-64.so.2" ]] \
  || die "target mismatch for $TARGET_TRIPLE: interpreter '${INTERP:-none}'"

command -v objdump >/dev/null 2>&1 || die "objdump (binutils) is required for the GLIBC floor"
GLIBC_FLOOR="$(objdump -T "$BIN" | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -n 1)"
[[ -n "$GLIBC_FLOOR" ]] || die "cannot measure the GLIBC_* symbol version floor of $BIN"

VERSION="$("$BIN" --version | awk '{print $2}')"
[[ -n "$VERSION" ]] || die "refusing to package $BIN: --version printed no version"

# --- stage ----------------------------------------------------------------
mkdir -p "$DIST"
stage="$(mktemp -d "$DIST/.stage.XXXXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
install -m 755 "$BIN" "$stage/ebbtop-agent"
install -m 755 "$ROOT/deploy/install-agent.sh" "$stage/install-agent.sh"
install -m 644 "$ROOT/deploy/ebbtop-agent.service" "$stage/ebbtop-agent.service"

# BUILD is self-describing metadata: version and commit, nothing host-specific.
{
  echo "name=ebbtop-agent"
  echo "version=$VERSION"
  echo "commit=$COMMIT"
  echo "target=$TARGET_TRIPLE"
  echo "glibc_floor=${GLIBC_FLOOR#GLIBC_}"
} > "$stage/BUILD"
chmod 644 "$stage/BUILD"

( cd "$stage" && sha256sum "${PAYLOAD[@]}" > SHA256SUMS && sha256sum --strict --quiet -c SHA256SUMS ) \
  || die "staged checksums do not verify"

# --- archive --------------------------------------------------------------
archive="$DIST/$NAME.tar.gz"
out="$(mktemp "$DIST/.$NAME.XXXXXXXX")"
tar --sort=name --mtime="@$EPOCH" --owner=0 --group=0 --numeric-owner \
    --format=gnu -czf "$out" -C "$stage" "${MEMBERS[@]}"
chmod 644 "$out"
mv -f -- "$out" "$archive"

# --- prove the archive is what we claim -----------------------------------
mapfile -t actual < <(tar -tzf "$archive" | LC_ALL=C sort)
mapfile -t expected < <(printf '%s\n' "${MEMBERS[@]}" | LC_ALL=C sort)
[[ "${actual[*]}" == "${expected[*]}" ]] \
  || die "archive member set mismatch: ${actual[*]}"
[[ "$(tar -xzOf "$archive" SHA256SUMS | grep -c 'ebbtop-agent$')" == "1" ]] \
  || die "SHA256SUMS in the archive does not name exactly one ebbtop-agent binary"
if tar -tzf "$archive" | grep -Eq '(\.env$|\.token$|\.pem$|\.key$|/id_)'; then
  die "archive carries a secret-shaped member"
fi
tar -xzOf "$archive" BUILD | grep -qx "commit=$COMMIT" || die "BUILD in the archive lost the commit"
tar -xzOf "$archive" BUILD | grep -qx "version=$VERSION" || die "BUILD in the archive lost the version"

size="$(stat -c %s "$archive")"
digest="$(sha256sum "$archive" | cut -d' ' -f1)"
echo "package-agent: wrote $archive"
echo "package-agent: size $size bytes"
echo "package-agent: sha256 $digest"
echo "package-agent: glibc floor $GLIBC_FLOOR (measured with objdump -T)"
echo "package-agent: version $VERSION commit $COMMIT target $TARGET_TRIPLE"
echo "package-agent: members: ${MEMBERS[*]}"
