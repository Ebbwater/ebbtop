#!/usr/bin/env bash
# Exercise the prebuilt Linux agent installer inside a disposable fixture.
#
# Every case runs with HOME pointed at a throwaway directory and with stub
# systemctl / ss / loginctl on PATH, so no service-manager call, no write to
# $HOME/services/ebbtop-agent and no listener on this host is ever touched.
# See README.md for what this harness does and does not prove.
#
# Usage:
#   bash deploy/linux/test-prebuilt-install.sh                    # all cases
#   bash deploy/linux/test-prebuilt-install.sh --no-toolchain-only
#
# One PASS line per case, then a SUMMARY line.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ARCHIVE="${EBBTOP_ARCHIVE:-$ROOT/dist/ebbtop-agent-linux-x86_64.tar.gz}"
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/ebbtop-agent"
MODE=all

case "${1:-}" in
  "") ;;
  --no-toolchain-only) MODE=no-toolchain ;;
  *) echo "usage: $0 [--no-toolchain-only]" >&2; exit 2 ;;
esac

PASSES=0
pass() { PASSES=$((PASSES + 1)); printf 'PASS %s: %s\n' "$1" "$2"; }
fail() { printf 'FAIL %s: %s\n' "$1" "$2" >&2; exit 1; }
note() { printf '  - %s\n' "$*"; }

[ -f "$ARCHIVE" ] || fail setup "no release archive at $ARCHIVE; run deploy/linux/package-agent.sh first"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ebbtop-prebuilt.XXXXXXXX")"
cleanup() { rm -rf -- "$WORK"; }
trap cleanup EXIT

STUB="$WORK/stub"
STUB_ARCH="$WORK/stub-arch"
STATE="$WORK/state"
mkdir -p "$STUB" "$STUB_ARCH" "$STATE"
echo yes > "$STATE/linger"
: > "$STATE/calls.log"
: > "$STATE/ss.log"
export EBBTOP_FIXTURE_STATE="$STATE"

# --- stubs: the only service manager, socket table and login record here ----
cat > "$STUB/systemctl" <<'STUB'
#!/usr/bin/env bash
set -u
STATE="${EBBTOP_FIXTURE_STATE:?}"
printf 'systemctl %s\n' "$*" >> "$STATE/calls.log"
[[ "${1:-}" == "--user" ]] && shift
sub="${1:-}"; shift || true
case "$sub" in
  daemon-reload|enable|disable|stop) exit 0 ;;
  restart)
    # A real `systemctl restart` returns 0 once the job ran; a unit that
    # cannot start is reported by `is-active`, not by restart.
    if [[ -f "$STATE/fail-next-restart" ]]; then
      rm -f "$STATE/fail-next-restart"
      echo failed > "$STATE/service"
    else
      echo active > "$STATE/service"
    fi
    exit 0 ;;
  is-active)
    state="$(cat "$STATE/service" 2>/dev/null || echo inactive)"
    echo "$state"
    [[ "$state" == active ]] && exit 0 || exit 3 ;;
  show)
    echo "argv[]=/lib/systemd/systemd --user ; path=$HOME/services/ebbtop-agent/bin/ebbtop-agent ; ignore_errors=no"
    exit 0 ;;
  *) exit 0 ;;
esac
STUB

cat > "$STUB/ss" <<'STUB'
#!/usr/bin/env bash
set -u
STATE="${EBBTOP_FIXTURE_STATE:?}"
printf 'ss %s\n' "$*" >> "$STATE/ss.log"
port=45976
[[ "$*" =~ :([0-9]+) ]] && port="${BASH_REMATCH[1]}"
addr="$(sed -n 's/^[[:blank:]]*EBBTOP_BIND[[:blank:]]*=[[:blank:]]*//p' "$HOME/services/ebbtop-agent/agent.env" 2>/dev/null \
  | head -n 1 | tr -d '\r' | sed -E 's/[[:blank:]]+$//')"
[[ -n "$addr" ]] || addr=100.64.0.5
if [[ "$(cat "$STATE/service" 2>/dev/null || echo inactive)" == active ]]; then
  echo "LISTEN 0      128    ${addr}:${port}      0.0.0.0:*    users:((\"ebbtop-agent\",pid=4242,fd=7))"
fi
exit 0
STUB

cat > "$STUB/loginctl" <<'STUB'
#!/usr/bin/env bash
set -u
STATE="${EBBTOP_FIXTURE_STATE:?}"
case "$*" in
  *Linger*) cat "$STATE/linger" 2>/dev/null || echo yes ;;
  *) echo "" ;;
esac
exit 0
STUB

# Any attempt to build from source inside the prebuilt path is a hard failure.
for tool in git cargo rustc; do
  cat > "$STUB/$tool" <<STUB
#!/bin/sh
echo "$tool must not be called by the prebuilt installer" >&2
: > "\$EBBTOP_FIXTURE_STATE/tool-called"
exit 127
STUB
done
chmod 755 "$STUB"/*

cat > "$STUB_ARCH/uname" <<'STUB'
#!/bin/sh
echo aarch64
STUB
chmod 755 "$STUB_ARCH/uname"

RUN_PATH="$STUB:/usr/bin:/bin"
ENV_ARGS=()
ADDR=""

set_addr() {
  ADDR="$1"
  ENV_ARGS=(HOME="$FIXHOME" EBBTOP_FIXTURE_STATE="$STATE" PATH="$RUN_PATH" TMPDIR="$WORK")
  [[ -n "$ADDR" ]] && ENV_ARGS+=("EBBTOP_ADDR=$ADDR")
  return 0
}

run_installer() { # $1 = extracted package dir, rest = installer args
  local pkg="$1"; shift
  ( cd "$pkg" && env -u CARGO_HOME -u CARGO_TARGET_DIR -u RUSTUP_HOME -u RUSTUP_TOOLCHAIN \
      -u EBBTOP_ADDR -u EBBTOP_PORT "${ENV_ARGS[@]}" bash ./install-agent.sh "$@" )
}

extract() { mkdir -p "$2"; tar -xzf "$1" -C "$2"; }

new_home() { # $1 = label
  FIXHOME="$WORK/home-$1"
  rm -rf -- "$FIXHOME"
  mkdir -p "$FIXHOME"
}

# The caller's real agent installation, if any: captured once at start and
# compared at the end, so "the fixture never escaped HOME" is observed, not
# assumed. Nothing here is ever written.
REAL_SVC="${HOME:-}/services/ebbtop-agent"
REAL_UNIT="${HOME:-}/.config/systemd/user/ebbtop-agent.service"
REAL_BASELINE=""

real_snapshot() {
  local out=""
  [[ -d "$REAL_SVC" ]] && out+="$(snapshot "$REAL_SVC")"
  [[ -f "$REAL_UNIT" ]] && out+="$(sha256sum "$REAL_UNIT")"
  printf '%s' "$out"
}

snapshot() { # $1 = directory; prints type/mode/size/path plus content hashes
  local dir="$1"
  if [[ ! -d "$dir" ]]; then echo "<absent>"; return 0; fi
  ( cd "$dir" && find . -mindepth 1 -printf '%y %m %s %P\n' | LC_ALL=C sort )
  ( cd "$dir" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum | LC_ALL=C sort )
}

calls() { wc -l < "$STATE/calls.log" | tr -d ' '; }

ss_calls() { wc -l < "$STATE/ss.log" | tr -d ' '; }

expect_no_mutation() { # $1 = case, $2 = snapshot before, $3 = calls before
  local after_calls
  [[ "$(snapshot "$FIXHOME")" == "$2" ]] \
    || fail "$1" "fixture home changed: $(snapshot "$FIXHOME" | diff -u <(printf '%s\n' "$2") - | head -20)"
  after_calls="$(calls)"
  [[ "$after_calls" == "$3" ]] \
    || fail "$1" "service manager was called $(($after_calls - $3)) time(s): $(tail -n +$(( $3 + 1 )) "$STATE/calls.log")"
}

REAL_BASELINE="$(real_snapshot)"
if [[ -n "$REAL_BASELINE" ]]; then
  note "real installation under ${HOME:-/} present; it is compared byte-for-byte at the end"
else
  note "no real installation under ${HOME:-/}; isolation rests on the fixture HOME alone"
fi

# ===========================================================================
# 1. no-toolchain: install from an extracted archive with no git, cargo or
#    rustc reachable, no .git and no Cargo.toml above the package directory.
# ===========================================================================
case_no_toolchain() {
  local name=no-toolchain pkg="$WORK/pkg-a" out="$WORK/no-toolchain.out" dir tok mode
  extract "$ARCHIVE" "$pkg"
  dir="$pkg"
  while :; do
    [[ -e "$dir/.git" ]] && fail "$name" "a .git exists above the extracted package: $dir/.git"
    [[ -e "$dir/Cargo.toml" ]] && fail "$name" "a Cargo.toml exists above the extracted package: $dir/Cargo.toml"
    [[ "$dir" == "/" ]] && break
    dir="$(dirname "$dir")"
  done
  note "package extracted at $pkg with no .git or Cargo.toml at or above it"
  note "PATH=$RUN_PATH (no cargo, no rustc; git is a stub that aborts)"

  new_home a
  set_addr 100.64.0.5
  if ! run_installer "$pkg" --prebuilt > "$out" 2>&1; then
    fail "$name" "installer exited non-zero: $(cat "$out")"
  fi
  grep -q 'verified: SHA256SUMS ok' "$out" || fail "$name" "installer did not report checksum verification"
  grep -qx 'active: active' "$out" || fail "$name" "installer did not report an active unit: $(cat "$out")"
  grep -q 'listen: 100.64.0.5:45976' "$out" || fail "$name" "installer did not report the configured listener"
  grep -q 'linger: yes' "$out" || fail "$name" "installer did not report linger state"
  [[ ! -e "$STATE/tool-called" ]] || fail "$name" "the installer invoked a build tool (git/cargo/rustc)"

  [[ "$(sha256sum < "$pkg/ebbtop-agent")" == "$(sha256sum < "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent")" ]] \
    || fail "$name" "installed binary is not the packaged binary"
  cmp "$pkg/ebbtop-agent.service" "$FIXHOME/.config/systemd/user/ebbtop-agent.service" \
    || fail "$name" "installed unit differs from the packaged unit"
  tok="$(cat "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  [[ "$tok" =~ ^[0-9a-f]{64}$ ]] || fail "$name" "token is not 32 bytes of hex: ${tok:0:8}..."
  mode="$(stat -c %a "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  [[ "$mode" == 600 ]] || fail "$name" "token mode is $mode, not 600"
  mode="$(stat -c %a "$FIXHOME/services/ebbtop-agent/agent.env")"
  [[ "$mode" == 600 ]] || fail "$name" "agent.env mode is $mode, not 600"
  grep -qF "$tok" "$out" && fail "$name" "installer printed the token"
  grep -qx "EBBTOP_BIND=100.64.0.5" "$FIXHOME/services/ebbtop-agent/agent.env" \
    || fail "$name" "agent.env does not carry the configured bind address"
  grep -q '^commit=' "$pkg/BUILD" || fail "$name" "BUILD does not record the commit"
  grep -q '^version=' "$pkg/BUILD" || fail "$name" "BUILD does not record the version"
  ( PATH=/usr/bin:/bin; command -v cargo >/dev/null && exit 1 || exit 0 ) \
    || fail "$name" "cargo is reachable on /usr/bin:/bin"
  pass no-toolchain "installed from $pkg with no git/cargo/rustc, no checkout, token 600, unit and binary from the package"
}

# ===========================================================================
# 2. bare-env: a fresh install demands an explicit bind address, refuses
#    wildcard/loopback addresses, and generates a secret that is not printed.
# ===========================================================================
case_bare_env() {
  local name=bare-env pkg="$WORK/pkg-b" out="$WORK/bare-env.out" before calls_before tok
  extract "$ARCHIVE" "$pkg"
  new_home b
  before="$(snapshot "$FIXHOME")"; calls_before="$(calls)"

  set_addr ""
  if run_installer "$pkg" --prebuilt > "$out" 2>&1; then
    fail "$name" "installer accepted a fresh install without EBBTOP_ADDR"
  fi
  grep -q "needs EBBTOP_ADDR" "$out" || fail "$name" "no explicit EBBTOP_ADDR requirement: $(cat "$out")"
  expect_no_mutation "$name" "$before" "$calls_before"
  note "no EBBTOP_ADDR: exit non-zero, no unit, no service dir, no service-manager call"

  local addr
  for addr in 127.0.0.1 0.0.0.0 ::; do
    set_addr "$addr"
    if run_installer "$pkg" --prebuilt > "$out" 2>&1; then
      fail "$name" "installer accepted bind address $addr"
    fi
    note "bind $addr refused: $(grep -o "refusing bind address[^;]*" "$out" | head -1)"
    expect_no_mutation "$name" "$before" "$calls_before"
  done

  echo no > "$STATE/linger"
  set_addr 100.64.0.5
  if ! run_installer "$pkg" --prebuilt > "$out" 2>&1; then
    fail "$name" "fresh install with a valid address failed: $(cat "$out")"
  fi
  grep -q 'sudo loginctl enable-linger' "$out" \
    || fail "$name" "installer did not print the exact linger command when linger is off: $(cat "$out")"
  tok="$(cat "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  [[ "$tok" =~ ^[0-9a-f]{64}$ ]] || fail "$name" "token is not 32 bytes of hex"
  [[ "$(stat -c %a "$FIXHOME/services/ebbtop-agent/ebbtop.token")" == 600 ]] || fail "$name" "token mode is not 600"
  grep -qF "$tok" "$out" && fail "$name" "installer printed the token"
  echo yes > "$STATE/linger"
  pass bare-env "fresh install needs EBBTOP_ADDR, refuses 127.0.0.1/0.0.0.0/::, writes a 600 hex token without printing it, reports the linger command"
}

# ===========================================================================
# 3. invalid-archive / tampered-member / member-set / wrong-arch: refused
#    before any write and before any service-manager call.
# ===========================================================================
case_rejected_inputs() {
  local pkg="$WORK/pkg-c" out="$WORK/reject.out" before calls_before trunc notgz extra

  extract "$ARCHIVE" "$pkg"
  new_home c
  set_addr 100.64.0.5
  # Populate the fixture with a working install so a mutation would be visible.
  run_installer "$pkg" --prebuilt > "$WORK/reject-baseline.out" 2>&1 \
    || fail invalid-archive "baseline install failed: $(cat "$WORK/reject-baseline.out")"

  # --- invalid-archive: truncated and not-a-gzip at all
  before="$(snapshot "$FIXHOME")"; calls_before="$(calls)"
  trunc="$WORK/truncated.tar.gz"
  head -c $(( $(stat -c %s "$ARCHIVE") / 2 )) "$ARCHIVE" > "$trunc"
  mkdir -p "$WORK/trunc-dir"
  if tar -xzf "$trunc" -C "$WORK/trunc-dir" > "$out" 2>&1; then
    fail invalid-archive "tar accepted a truncated archive"
  fi
  note "truncated archive: tar exited non-zero"
  if [[ -f "$WORK/trunc-dir/install-agent.sh" ]]; then
    if run_installer "$WORK/trunc-dir" --prebuilt > "$out" 2>&1; then
      fail invalid-archive "installer accepted a partially extracted archive"
    fi
    grep -Eq 'member set mismatch|checksum verification failed|malformed SHA256SUMS' "$out" \
      || fail invalid-archive "partial-extraction refusal unclear: $(cat "$out")"
    note "partial extraction refused: $(head -1 "$out")"
  else
    note "partial extraction did not even yield install-agent.sh, so no install can run"
  fi
  notgz="$WORK/notgzip.tar.gz"
  printf 'this is not a gzip stream at all\n' > "$notgz"
  if mkdir -p "$WORK/notgz-dir" && tar -xzf "$notgz" -C "$WORK/notgz-dir" > "$out" 2>&1; then
    fail invalid-archive "tar accepted a non-gzip file"
  fi
  [[ -z "$(ls -A "$WORK/notgz-dir")" ]] || fail invalid-archive "a non-gzip file extracted content"
  note "not-a-gzip: tar exited non-zero and extracted nothing"
  expect_no_mutation invalid-archive "$before" "$calls_before"
  pass invalid-archive "truncated and non-gzip archives never reach a mutating install; fixture home and service-manager log unchanged"

  # --- tampered-member: one payload byte changed after the sums were written
  before="$(snapshot "$FIXHOME")"; calls_before="$(calls)"
  rm -rf "$WORK/pkg-tampered"; cp -a "$pkg" "$WORK/pkg-tampered"
  printf 'x' >> "$WORK/pkg-tampered/ebbtop-agent"
  if run_installer "$WORK/pkg-tampered" --prebuilt > "$out" 2>&1; then
    fail tampered-member "installer accepted a modified ebbtop-agent"
  fi
  grep -qi 'checksum' "$out" || fail tampered-member "refusal did not name the checksum: $(cat "$out")"
  expect_no_mutation tampered-member "$before" "$calls_before"
  pass tampered-member "modified binary refused on checksum (exit non-zero), fixture home byte-identical, no service-manager call"

  # --- member-set: an extra member and a missing member are both refused
  before="$(snapshot "$FIXHOME")"; calls_before="$(calls)"
  rm -rf "$WORK/pkg-extra"; cp -a "$pkg" "$WORK/pkg-extra"
  printf 'not for release\n' > "$WORK/pkg-extra/extra.pem"
  if run_installer "$WORK/pkg-extra" --prebuilt > "$out" 2>&1; then
    fail member-set "installer accepted a package with an extra member"
  fi
  grep -q 'member set mismatch' "$out" || fail member-set "refusal did not name the member set: $(cat "$out")"
  note "extra member extra.pem refused: $(head -1 "$out")"
  rm -rf "$WORK/pkg-missing"; cp -a "$pkg" "$WORK/pkg-missing"
  rm -f "$WORK/pkg-missing/BUILD"
  if run_installer "$WORK/pkg-missing" --prebuilt > "$out" 2>&1; then
    fail member-set "installer accepted a package with a missing BUILD"
  fi
  grep -q 'member set mismatch' "$out" || fail member-set "missing member refusal unclear: $(cat "$out")"
  note "missing BUILD refused: $(head -1 "$out")"
  expect_no_mutation member-set "$before" "$calls_before"
  pass member-set "extra and missing members refused before any write; fixture home byte-identical, no service-manager call"

  # --- wrong-arch: uname -m reports aarch64
  before="$(snapshot "$FIXHOME")"; calls_before="$(calls)"
  local saved_path="$RUN_PATH"
  RUN_PATH="$STUB_ARCH:$STUB:/usr/bin:/bin"
  set_addr 100.64.0.5
  if run_installer "$pkg" --prebuilt > "$out" 2>&1; then
    fail wrong-arch "installer accepted an aarch64 host"
  fi
  grep -q "unsupported architecture 'aarch64'" "$out" || fail wrong-arch "refusal did not name the architecture: $(cat "$out")"
  RUN_PATH="$saved_path"
  set_addr 100.64.0.5
  expect_no_mutation wrong-arch "$before" "$calls_before"
  pass wrong-arch "uname -m aarch64 refused with a stated reason before any write or service-manager call"
}

# ===========================================================================
# 4. token-retained / settings-retained: upgrade keeps the token, the env
#    file and any operator settings byte-identical.
# ===========================================================================
repack_v2() { # $1 = source package dir, $2 = destination archive
  local src="$1" dest="$2" stage="$WORK/repack-stage"
  rm -rf -- "$stage"; mkdir -p "$stage"
  cp -p "$src"/BUILD "$src"/SHA256SUMS "$src"/ebbtop-agent "$src"/ebbtop-agent.service "$src"/install-agent.sh "$stage/"
  sed -i 's/^version=.*/version=0.2.0+fixture-v2/' "$stage/BUILD"
  echo 'variant=fixture-v2' >> "$stage/BUILD"
  # A different binary than v1 (trailing bytes are ignored by the ELF loader),
  # so replacing the installed file is observable.
  printf '\nfixture-v2\n' >> "$stage/ebbtop-agent"
  ( cd "$stage" && sha256sum BUILD ebbtop-agent ebbtop-agent.service > SHA256SUMS )
  tar --format=gnu -czf "$dest" -C "$stage" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.service install-agent.sh
}

case_retention() {
  local pkg="$WORK/pkg-a" v2="$WORK/pkg-v2" out="$WORK/retention.out"
  local before_tok before_mode before_env tok mode env_before
  new_home a2
  set_addr 100.64.0.5
  # fresh install of v1 (the upgrade needs a previous installation)
  run_installer "$pkg" --prebuilt > "$WORK/retention-v1.out" 2>&1 \
    || fail token-retained "baseline install failed: $(cat "$WORK/retention-v1.out")"
  before_tok="$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  before_mode="$(stat -c %a "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  before_env="$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")"

  repack_v2 "$pkg" "$v2"
  extract "$v2" "$WORK/pkg-v2-x"
  set_addr ""   # no EBBTOP_ADDR: an upgrade must not need one
  run_installer "$WORK/pkg-v2-x" --prebuilt > "$out" 2>&1 \
    || fail token-retained "upgrade install failed: $(cat "$out")"
  grep -qx 'version: 0.2.0+fixture-v2' "$out" || fail token-retained "upgrade did not install the v2 package: $(cat "$out")"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent")" == "$(sha256sum < "$WORK/pkg-v2-x/ebbtop-agent")" ]] \
    || fail token-retained "installed binary is not the v2 binary"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent.bak")" == "$(sha256sum < "$pkg/ebbtop-agent")" ]] \
    || fail token-retained ".bak does not hold the v1 binary"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")" == "$before_tok" ]] \
    || fail token-retained "token bytes changed across the upgrade"
  [[ "$(stat -c %a "$FIXHOME/services/ebbtop-agent/ebbtop.token")" == "$before_mode" ]] \
    || fail token-retained "token mode changed across the upgrade"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")" == "$before_env" ]] \
    || fail token-retained "agent.env was rewritten across the upgrade"
  pass token-retained "v1 -> v2 upgrade kept token bytes and mode $before_mode, agent.env untouched, v1 binary kept as .bak"

  # --- settings-retained: a setting appended after install survives an upgrade
  printf 'EBBTOP_OPERATOR_SETTING=keep-me\n' >> "$FIXHOME/services/ebbtop-agent/agent.env"
  chmod 600 "$FIXHOME/services/ebbtop-agent/agent.env"
  env_before="$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")"
  tok="$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  set_addr ""
  run_installer "$pkg" --prebuilt > "$out" 2>&1 \
    || fail settings-retained "reinstall failed: $(cat "$out")"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")" == "$env_before" ]] \
    || fail settings-retained "agent.env changed after an operator setting was appended"
  grep -qx 'EBBTOP_OPERATOR_SETTING=keep-me' "$FIXHOME/services/ebbtop-agent/agent.env" \
    || fail settings-retained "operator setting was lost"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")" == "$tok" ]] \
    || fail settings-retained "token changed after the reinstall"
  mode="$(stat -c %a "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  [[ "$mode" == 600 ]] || fail settings-retained "token mode is $mode after the reinstall"
  pass settings-retained "operator setting appended after install stayed byte-identical and token stayed 600 across a reinstall"
}

# ===========================================================================
# 5. rollback: no .bak is a stated non-zero failure; a failed start restores
#    the previous binary, restarts the unit and verifies the restored binary.
# ===========================================================================
case_rollback() {
  local pkg="$WORK/pkg-a" out="$WORK/rollback.out" before calls_before
  local prev_sha restarts first_restart second_restart

  # --- --rollback with no .bak (fresh fixture home, never installed)
  new_home r
  set_addr ""
  before="$(snapshot "$FIXHOME")"; calls_before="$(calls)"
  if run_installer "$pkg" --rollback > "$out" 2>&1; then
    fail rollback "--rollback succeeded without a .bak: $(cat "$out")"
  fi
  grep -q 'no .bak to roll back to' "$out" || fail rollback "no stated reason without .bak: $(cat "$out")"
  expect_no_mutation rollback "$before" "$calls_before"
  note "--rollback without .bak: exit non-zero, 'no .bak to roll back to', fixture home unchanged"

  # --- failed start: install over an existing binary, unit comes up failed
  new_home a3
  set_addr 100.64.0.5
  run_installer "$pkg" --prebuilt > "$WORK/rollback-v1.out" 2>&1 \
    || fail rollback "baseline install failed: $(cat "$WORK/rollback-v1.out")"
  prev_sha="$(sha256sum < "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent")"
  local env_sha tok_sha
  env_sha="$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")"
  tok_sha="$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  calls_before="$(calls)"

  touch "$STATE/fail-next-restart"
  set_addr ""
  if run_installer "$pkg" --prebuilt > "$out" 2>&1; then
    fail rollback "installer reported success although the unit came up failed: $(cat "$out")"
  fi
  grep -q 'failed to start after install' "$out" || fail rollback "failure was not reported: $(cat "$out")"
  grep -q 'restored previous binary' "$out" || fail rollback "previous binary was not restored: $(cat "$out")"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent")" == "$prev_sha" ]] \
    || fail rollback "restored binary is not the previous binary"
  [[ ! -e "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent.bak" ]] \
    || fail rollback ".bak was not consumed by the restore"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")" == "$env_sha" ]] \
    || fail rollback "agent.env changed during the failed install"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")" == "$tok_sha" ]] \
    || fail rollback "token changed during the failed install"
  [[ "$(cat "$STATE/service")" == active ]] || fail rollback "unit is not active after the restore"
  restarts="$(tail -n +$((calls_before + 1)) "$STATE/calls.log" | grep -c 'systemctl .*restart ' || true)"
  [[ "$restarts" == 2 ]] || fail rollback "expected install restart + restore restart, saw $restarts: $(tail -n +$((calls_before + 1)) "$STATE/calls.log")"
  tail -n +$((calls_before + 1)) "$STATE/calls.log" | grep -q 'show -p ExecStart' \
    || fail rollback "installer did not ask the service manager which binary is loaded"
  if grep -q 'restore could not be verified' "$out"; then
    fail rollback "installer could not verify the restore: $(cat "$out")"
  fi
  if grep -q 'is not the restored binary' "$out"; then
    fail rollback "unit ExecStart does not point at the restored binary: $(cat "$out")"
  fi
  pass rollback "--rollback without .bak exits non-zero with 'no .bak to roll back to'; a failed start restored ${prev_sha:0:16}, restarted the unit twice and left it active"
}

# ===========================================================================
# 6. source-parity: with no arguments and with --rollback the checkout path
#    behaves exactly as the base script did, and an unknown flag fails loudly
#    instead of silently taking the source path.
# ===========================================================================
BASE_COMMIT=cba41e6ca3a989d234a3ea16723c69b5cf17331e

masked_snapshot() { # $1 = home; token values are masked, everything else raw
  local h="$1" f
  ( cd "$h" && find . -mindepth 1 -printf '%y %m %s %P\n' | LC_ALL=C sort )
  ( cd "$h" && find . -type f | LC_ALL=C sort | while IFS= read -r f; do
      case "$f" in
        ./services/ebbtop-agent/agent.env|./services/ebbtop-agent/ebbtop.token)
          printf '%s %s\n' "$f" "$(sed -E 's/[0-9a-f]{64}/<token>/g' "$f" | sha256sum | cut -d' ' -f1)" ;;
        *)
          printf '%s %s\n' "$f" "$(sha256sum < "$f" | cut -d' ' -f1)" ;;
      esac
    done )
}

case_source_parity() {
  local name=source-parity out="$WORK/parity.out" base="$WORK/repo-base" cand="$WORK/repo-cand"
  local base_out="$WORK/parity-base.out" cand_out="$WORK/parity-cand.out"
  command -v git >/dev/null 2>&1 || fail "$name" "git is not on PATH"

  # A minimal checkout for each script: deploy/ plus the built binary in target/.
  for repo in "$base" "$cand"; do
    mkdir -p "$repo/deploy" "$repo/target/release"
    cp -p "$ROOT/deploy/ebbtop-agent.service" "$repo/deploy/"
    install -m 755 "$BIN" "$repo/target/release/ebbtop-agent"
  done
  git -C "$ROOT" show "$BASE_COMMIT:deploy/install-agent.sh" > "$base/deploy/install-agent.sh" \
    || fail "$name" "cannot read the base install-agent.sh from $BASE_COMMIT"
  cp -p "$ROOT/deploy/install-agent.sh" "$cand/deploy/install-agent.sh"

  # cargo is a recording stub for this case: it must be called with the base's
  # exact arguments, and the prebuilt path never calls it at all.
  local saved_path="$RUN_PATH"
  RUN_PATH="$STUB:/usr/bin:/bin"
  # the recording cargo shadows the aborting cargo stub for this case only
  printf '#!/bin/sh\necho "$*" >> "$EBBTOP_FIXTURE_STATE/cargo.log"\nexit 0\n' > "$STUB/cargo"
  chmod 755 "$STUB/cargo"

  run_source() { # $1 = repo, $2 = home, rest = args
    local repo="$1" home="$2"; shift 2
    echo inactive > "$STATE/service"
    ( cd "$repo" && env -u CARGO_HOME -u CARGO_TARGET_DIR -u RUSTUP_HOME \
        HOME="$home" EBBTOP_FIXTURE_STATE="$STATE" EBBTOP_ADDR=100.64.0.5 \
        PATH="$RUN_PATH" TMPDIR="$WORK" bash ./deploy/install-agent.sh "$@" )
  }

  FIXHOME="$WORK/home-parity-base"; rm -rf "$FIXHOME"; mkdir -p "$FIXHOME"
  run_source "$base" "$FIXHOME" > "$base_out" 2>&1 || fail "$name" "base script failed: $(cat "$base_out")"
  local snap_base; snap_base="$(masked_snapshot "$FIXHOME")"

  FIXHOME="$WORK/home-parity-cand"; rm -rf "$FIXHOME"; mkdir -p "$FIXHOME"
  run_source "$cand" "$FIXHOME" > "$cand_out" 2>&1 || fail "$name" "candidate script failed: $(cat "$cand_out")"
  local snap_cand; snap_cand="$(masked_snapshot "$FIXHOME")"

  [[ "$snap_base" == "$snap_cand" ]] \
    || fail "$name" "installed layout differs from base: $(diff -u <(printf '%s\n' "$snap_base") <(printf '%s\n' "$snap_cand") | head -30)"
  diff -u <(sed -E 's/[0-9a-f]{64}/<token>/g; s#repo-(base|cand)#<repo>#g; s#home-parity-(base|cand)#<home>#g' "$base_out") \
          <(sed -E 's/[0-9a-f]{64}/<token>/g; s#repo-(base|cand)#<repo>#g; s#home-parity-(base|cand)#<home>#g' "$cand_out") \
    || fail "$name" "installer output differs from base"
  local cargo_args; cargo_args="$(sort -u "$STATE/cargo.log")"
  [[ "$cargo_args" == "build --release --locked -p ebbtop-agent --no-default-features --features gpu" ]] \
    || fail "$name" "cargo was not called with the base build line: $cargo_args"
  [[ "$(wc -l < "$STATE/cargo.log")" == 2 ]] || fail "$name" "expected one cargo build per script, saw $(wc -l < "$STATE/cargo.log")"
  note "both scripts called: cargo $cargo_args"
  note "both scripts produced the same home layout (tokens masked) and the same output"

  # --rollback parity: same .bak handling and same output in both scripts.
  for home in "$WORK/home-parity-base" "$WORK/home-parity-cand"; do
    cp -p "$home/services/ebbtop-agent/bin/ebbtop-agent" "$home/services/ebbtop-agent/bin/ebbtop-agent.bak"
  done
  run_source "$base" "$WORK/home-parity-base" --rollback > "$base_out" 2>&1 || fail "$name" "base --rollback failed: $(cat "$base_out")"
  run_source "$cand" "$WORK/home-parity-cand" --rollback > "$cand_out" 2>&1 || fail "$name" "candidate --rollback failed: $(cat "$cand_out")"
  diff -u "$base_out" "$cand_out" || fail "$name" "--rollback output differs from base"
  [[ "$(masked_snapshot "$WORK/home-parity-base")" == "$(masked_snapshot "$WORK/home-parity-cand")" ]] \
    || fail "$name" "--rollback layout differs from base"
  note "--rollback: identical output and identical layout in both scripts"

  # --rollback without .bak: same stated failure and non-zero exit.
  rm -f "$WORK/home-parity-base/services/ebbtop-agent/bin/ebbtop-agent.bak" \
        "$WORK/home-parity-cand/services/ebbtop-agent/bin/ebbtop-agent.bak"
  if run_source "$base" "$WORK/home-parity-base" --rollback > "$base_out" 2>&1; then
    fail "$name" "base --rollback succeeded without .bak"
  fi
  if run_source "$cand" "$WORK/home-parity-cand" --rollback > "$cand_out" 2>&1; then
    fail "$name" "candidate --rollback succeeded without .bak"
  fi
  diff -u "$base_out" "$cand_out" || fail "$name" "--rollback without .bak output differs from base"
  grep -qx 'no .bak to roll back to' "$cand_out" || fail "$name" "candidate lost the no-.bak message"

  # unknown flag: loud failure, no build, no write.
  local before calls_before
  before="$(masked_snapshot "$WORK/home-parity-cand")"; calls_before="$(calls)"
  : > "$STATE/cargo.log"
  if run_source "$cand" "$WORK/home-parity-cand" --bogus > "$out" 2>&1; then
    fail "$name" "candidate accepted an unknown flag"
  fi
  grep -q 'unknown option: --bogus' "$out" || fail "$name" "unknown-flag refusal unclear: $(cat "$out")"
  [[ ! -s "$STATE/cargo.log" ]] || fail "$name" "unknown flag still ran cargo"
  [[ "$(masked_snapshot "$WORK/home-parity-cand")" == "$before" ]] \
    || fail "$name" "unknown flag wrote to the fixture home"
  [[ "$(calls)" == "$calls_before" ]] || fail "$name" "unknown flag called the service manager"
  note "unknown flag --bogus: exit non-zero, 'unknown option: --bogus', no build, no write"

  for tool in cargo git; do
    printf '#!/bin/sh\necho "%s must not be called by the prebuilt installer" >&2\n: > "$EBBTOP_FIXTURE_STATE/tool-called"\nexit 127\n' "$tool" > "$STUB/$tool"
  done
  chmod 755 "$STUB/cargo" "$STUB/git"
  RUN_PATH="$saved_path"
  pass "$name" "no-arg and --rollback output/layout match the base script, same cargo build line, no-.bak failure identical; unknown flag fails loudly without building or writing"
}

# ===========================================================================
# 7. bind-assignment: an existing agent.env is read the way systemd reads it,
#    so the address the installer validates is the address the unit loads.
#    Refusals happen after package verification and before any write or call.
# ===========================================================================
BIND_TAIL=""        # realistic non-bind lines, built from the fixture token
BIND_TAIL_CRLF=""

bind_fixture() { # $1 = fixture name; writes $FIXHOME/services/ebbtop-agent/agent.env
  local f="$FIXHOME/services/ebbtop-agent/agent.env"
  local B='EBBTOP_BIND=100.64.0.5' W='EBBTOP_BIND=0.0.0.0'
  case "$1" in
    # --- forms systemd reads as one plain EBBTOP_BIND=100.64.0.5
    plain)                         printf '%s\n%s' "$B" "$BIND_TAIL" ;;
    space-indented)                printf '   %s\n%s' "$B" "$BIND_TAIL" ;;
    tab-indented)                  printf '\t%s\n%s' "$B" "$BIND_TAIL" ;;
    spaced-equals)                 printf 'EBBTOP_BIND = 100.64.0.5\n%s' "$BIND_TAIL" ;;
    trailing-space)                printf '%s   \n%s' "$B" "$BIND_TAIL" ;;
    crlf)                          printf '%s\r\n%s' "$B" "$BIND_TAIL_CRLF" ;;
    comments)                      printf '# %s\n;EBBTOP_BIND=::\n%s\n%s' "$W" "$B" "$BIND_TAIL" ;;
    no-final-newline)              printf '%s%s' "$BIND_TAIL" "$B" ;;
    # --- duplicates and unsupported forms, each refused
    valid-then-wildcard)           printf '%s\n%s\n%s' "$B" "$W" "$BIND_TAIL" ;;
    wildcard-then-valid)           printf '%s\n%s\n%s' "$W" "$B" "$BIND_TAIL" ;;
    space-indented-wildcard)       printf '%s\n   %s\n%s' "$B" "$W" "$BIND_TAIL" ;;
    tab-indented-wildcard)         printf '%s\n\t%s\n%s' "$B" "$W" "$BIND_TAIL" ;;
    indented-key-then-wildcard)    printf '  %s\n%s\n%s' "$B" "$W" "$BIND_TAIL" ;;
    duplicate)                     printf '%s\n%s\n%s' "$B" "$B" "$BIND_TAIL" ;;
    lone-space-indented-wildcard)  printf '   %s\n%s' "$W" "$BIND_TAIL" ;;
    quoted)                        printf 'EBBTOP_BIND="0.0.0.0"\n%s' "$BIND_TAIL" ;;
    backslash-escaped)             printf 'EBBTOP_BIND=0\\.0\\.0\\.0\n%s' "$BIND_TAIL" ;;
    exported)                      printf 'export %s\n%s' "$B" "$BIND_TAIL" ;;
    no-equals)                     printf '%s\nnot an assignment\n%s' "$B" "$BIND_TAIL" ;;
    comments-no-bind)              printf '# %s\n;EBBTOP_BIND=::\n' "$W" ;;
    vt-indented)                   printf '\x0b%s\n%s' "$B" "$BIND_TAIL" ;;
    ff-indented)                   printf '\x0c%s\n%s' "$B" "$BIND_TAIL" ;;
    vt-suffixed-key)               printf 'EBBTOP_BIND\x0b=100.64.0.5\n%s' "$BIND_TAIL" ;;
    ff-suffixed-key)               printf 'EBBTOP_BIND\x0c=100.64.0.5\n%s' "$BIND_TAIL" ;;
    nul-prefixed)                  printf '\x00%s\n%s' "$B" "$BIND_TAIL" ;;
    vt-indented-no-newline)        printf '\x0b%s' "$B" ;;
    nul-on-own-line)               printf '\n\x00\n%s\n%s' "$B" "$BIND_TAIL" ;;
    nbsp-indented)                 printf '\xc2\xa0%s\n%s' "$B" "$BIND_TAIL" ;;
    vt-wildcard-after-plain)       printf '%s\n\x0b%s\n%s' "$B" "$W" "$BIND_TAIL" ;;
    cr-duplicate)                  printf '%s\r%s\r' "$B" "$W" ;;
    crlf-spaced-key-tab-wildcard)  printf 'EBBTOP_BIND = 100.64.0.5\r\n\t%s\r\n' "$W" ;;
    *) fail bind-assignment "unknown fixture '$1'" ;;
  esac > "$f"
}

check_refusal() { # $1 = fixture name, $2 = prose label, $3 = expected message
  local fixture="$1" label="$2" want="$3"
  local before calls_before ss_before calls_after ss_after errs verified
  bind_fixture "$fixture"
  before="$(snapshot "$FIXHOME")"; calls_before="$(calls)"; ss_before="$(ss_calls)"
  if run_installer "$WORK/pkg-bind" --prebuilt > "$WORK/bind.out" 2>&1; then
    fail bind-assignment "$label was accepted: $(cat "$WORK/bind.out")"
  fi
  errs="$(grep -c '^install-agent.sh: ' "$WORK/bind.out" || true)"
  [[ "$errs" == 1 ]] \
    || fail bind-assignment "$label: $errs installer error lines, expected exactly one: $(cat "$WORK/bind.out")"
  grep -qF -- "$want" "$WORK/bind.out" \
    || fail bind-assignment "$label: the refusal did not say '$want': $(cat "$WORK/bind.out")"
  [[ "$(snapshot "$FIXHOME")" == "$before" ]] \
    || fail bind-assignment "$label: the fixture home changed"
  calls_after="$(calls)"; ss_after="$(ss_calls)"
  [[ "$calls_after" == "$calls_before" ]] \
    || fail bind-assignment "$label: the service manager was called $((calls_after - calls_before)) time(s)"
  [[ "$ss_after" == "$ss_before" ]] \
    || fail bind-assignment "$label: ss was called $((ss_after - ss_before)) time(s)"
  verified="$(grep -m1 '^verified: SHA256SUMS ok' "$WORK/bind.out" || true)"
  [[ -n "$verified" ]] \
    || fail bind-assignment "$label: the package was not verified before the refusal"
  note "$label: refused before any write or call - $verified"
}

check_accepted() { # $1 = note text, rest = fixture names
  local label="$1" fixture before_env before_tok
  shift
  for fixture in "$@"; do
    bind_fixture "$fixture"
    before_env="$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")"
    before_tok="$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
    set_addr ""
    run_installer "$WORK/pkg-bind" --prebuilt > "$WORK/bind.out" 2>&1 \
      || fail bind-assignment "$fixture was refused: $(cat "$WORK/bind.out")"
    grep -qx 'active: active' "$WORK/bind.out" \
      || fail bind-assignment "$fixture: unit did not come up active: $(cat "$WORK/bind.out")"
    grep -qx 'listen: 100.64.0.5:45976' "$WORK/bind.out" \
      || fail bind-assignment "$fixture: installer did not report 100.64.0.5:45976: $(cat "$WORK/bind.out")"
    [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")" == "$before_env" ]] \
      || fail bind-assignment "$fixture: agent.env was rewritten"
    [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")" == "$before_tok" ]] \
      || fail bind-assignment "$fixture: token bytes changed"
  done
  note "$label"
}

case_bind_assignment() {
  local name=bind-assignment pkg="$WORK/pkg-bind" out="$WORK/bind.out"
  local before_env before_tok tok
  extract "$ARCHIVE" "$pkg"
  new_home bind
  set_addr 100.64.0.5
  run_installer "$pkg" --prebuilt > "$out" 2>&1 \
    || fail "$name" "baseline install failed: $(cat "$out")"
  [[ "$(grep -c '^EBBTOP_BIND=' "$FIXHOME/services/ebbtop-agent/agent.env")" == 1 ]] \
    || fail "$name" "the installer wrote more than one EBBTOP_BIND line"
  tok="$(cat "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  BIND_TAIL="EBBTOP_AGENT_PROCESSES_TTL_MS=500
EBBTOP_AGENT_METRICS_TTL_MS=100
EBBTOP_TOKEN=$tok
"
  BIND_TAIL_CRLF="EBBTOP_AGENT_PROCESSES_TTL_MS=500"$'\r\n'"EBBTOP_AGENT_METRICS_TTL_MS=100"$'\r\n'"EBBTOP_TOKEN=$tok"$'\r\n'

  # --- an ordinary upgrade: one plain EBBTOP_BIND, nothing rewritten
  before_env="$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")"
  before_tok="$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  set_addr ""
  run_installer "$pkg" --prebuilt > "$out" 2>&1 \
    || fail "$name" "ordinary upgrade failed: $(cat "$out")"
  grep -qx 'listen: 100.64.0.5:45976' "$out" \
    || fail "$name" "ordinary upgrade did not listen on 100.64.0.5:45976: $(cat "$out")"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")" == "$before_env" ]] \
    || fail "$name" "ordinary upgrade rewrote agent.env"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")" == "$before_tok" ]] \
    || fail "$name" "ordinary upgrade changed the token bytes"
  note "ordinary upgrade: one EBBTOP_BIND, exit 0, listen 100.64.0.5:45976, agent.env and token byte-identical"

  # --- forms systemd reads as the private address are accepted
  check_accepted "accepted plain, space-indented, tab-indented, 'KEY = value', trailing-space and CRLF forms as 100.64.0.5, agent.env untouched" \
    plain space-indented tab-indented spaced-equals trailing-space crlf
  check_accepted "accepted a plain bind with no final newline as 100.64.0.5, agent.env untouched" \
    no-final-newline
  check_accepted "commented '# EBBTOP_BIND=0.0.0.0' and ';EBBTOP_BIND=::' ignored, as systemd ignores them" \
    comments

  # --- duplicates, ambiguous and unsupported forms are refused before mutation
  check_refusal valid-then-wildcard              "valid bind followed by a wildcard"              "ambiguous"
  check_refusal wildcard-then-valid              "wildcard followed by a valid bind"              "ambiguous"
  check_refusal space-indented-wildcard          "valid bind followed by a space-indented wildcard" "ambiguous"
  check_refusal tab-indented-wildcard            "valid bind followed by a tab-indented wildcard" "ambiguous"
  check_refusal indented-key-then-wildcard       "indented key followed by a wildcard"            "ambiguous"
  check_refusal duplicate                        "two identical assignments"                      "ambiguous"
  check_refusal lone-space-indented-wildcard     "a lone space-indented wildcard"                 "refusing bind address '0.0.0.0'"
  check_refusal quoted                           "a quoted wildcard"                              "quoted or escaped values"
  check_refusal backslash-escaped                "a backslash-escaped wildcard"                   "quoted or escaped values"
  check_refusal exported                         "an exported assignment"                         "is not a plain KEY=VALUE assignment"
  check_refusal no-equals                        "a non-comment line without '='"                 "non-comment line without '='"
  check_refusal comments-no-bind                 "comments but no bind"                           "has no EBBTOP_BIND"
  check_refusal vt-indented                      "a VT-indented bind"                             "the byte 0x0b is not supported"
  check_refusal ff-indented                      "a FF-indented bind"                             "the byte 0x0c is not supported"
  check_refusal vt-suffixed-key                  "a VT-suffixed key"                              "the byte 0x0b is not supported"
  check_refusal ff-suffixed-key                  "a FF-suffixed key"                              "the byte 0x0c is not supported"
  check_refusal nul-prefixed                     "a NUL-prefixed bind"                            "contains a NUL byte"
  check_refusal vt-indented-no-newline           "a VT-indented bind with no final newline"       "the byte 0x0b is not supported"
  check_refusal nul-on-own-line                  "a NUL on its own line before the bind"          "contains a NUL byte"
  check_refusal nbsp-indented                    "an NBSP-indented bind"                          "the byte 0xc2 is not supported"
  check_refusal vt-wildcard-after-plain          "a VT-indented wildcard after a plain bind"      "the byte 0x0b is not supported"
  check_refusal cr-duplicate                     "a CR-separated duplicate"                       "ambiguous"
  check_refusal crlf-spaced-key-tab-wildcard     "a spaced key and a tab-indented wildcard on CRLF lines" "ambiguous"

  pass "$name" "ordinary upgrade unchanged; plain/indented/tabbed/spaced/CRLF/comment forms (incl. no final newline) read as systemd reads them; duplicate (incl. indented and CR-separated) forms refused; quoted/escaped/non-assignment forms refused; VT/FF-indented or -suffixed keys, NUL bytes and non-ASCII bytes refused - every refusal non-zero with an unchanged fixture, zero service/listener calls"
}

# ===========================================================================
# 8. packaging refusals: dirty tree, wrong ELF machine, non-ELF, and a
#    reproducible tarball. Skipped in --no-toolchain-only mode.
# ===========================================================================
case_packaging() {
  local name=packaging-refusals out="$WORK/pack.out" clone="$WORK/clone" sha_a sha_b armbin
  command -v git >/dev/null 2>&1 || fail "$name" "git is not on PATH"
  [ -x "$BIN" ] || fail "$name" "no release binary at $BIN to test the packaging refusals with"

  # dirty tree
  if ! git clone -q --no-hardlinks "$ROOT" "$clone" 2> "$out"; then
    fail "$name" "cannot clone the candidate for the dirty-tree control: $(cat "$out")"
  fi
  printf '\nuncommitted\n' >> "$clone/README.md"
  if EBBTOP_AGENT_BIN="$BIN" bash "$clone/deploy/linux/package-agent.sh" > "$out" 2>&1; then
    fail "$name" "packager accepted a dirty tree"
  fi
  grep -q 'refusing to package a dirty tree' "$out" || fail "$name" "dirty-tree refusal unclear: $(cat "$out")"
  note "dirty tree refused: $(grep 'refusing to package a dirty tree' "$out")"

  # not an ELF
  printf 'not an elf file\n' > "$WORK/not-an-elf"
  chmod 755 "$WORK/not-an-elf"
  if EBBTOP_AGENT_BIN="$WORK/not-an-elf" bash "$ROOT/deploy/linux/package-agent.sh" > "$out" 2>&1; then
    fail "$name" "packager accepted a non-ELF binary"
  fi
  grep -q 'not an ELF file' "$out" || fail "$name" "non-ELF refusal unclear: $(cat "$out")"
  note "non-ELF refused: $(grep 'not an ELF file' "$out")"

  # wrong ELF machine: e_machine patched from x86-64 (0x3e) to AArch64 (0xb7)
  cp "$BIN" "$WORK/armbin"; armbin="$WORK/armbin"
  printf '\xb7\x00' | dd of="$armbin" bs=1 seek=18 conv=notrunc status=none
  if EBBTOP_AGENT_BIN="$armbin" bash "$ROOT/deploy/linux/package-agent.sh" > "$out" 2>&1; then
    fail "$name" "packager accepted an AArch64 binary"
  fi
  grep -q 'wrong ELF machine' "$out" || fail "$name" "wrong-machine refusal unclear: $(cat "$out")"
  note "wrong ELF machine refused: $(grep 'wrong ELF machine' "$out")"

  # a clean package twice must produce the same tarball sha256
  bash "$ROOT/deploy/linux/package-agent.sh" > "$out" 2>&1 || fail "$name" "clean packaging failed: $(cat "$out")"
  sha_a="$(sed -n 's/^package-agent: sha256 //p' "$out")"
  bash "$ROOT/deploy/linux/package-agent.sh" > "$out" 2>&1 || fail "$name" "second clean packaging failed: $(cat "$out")"
  sha_b="$(sed -n 's/^package-agent: sha256 //p' "$out")"
  [[ -n "$sha_a" && "$sha_a" == "$sha_b" ]] || fail "$name" "tarball is not reproducible: $sha_a vs $sha_b"
  note "two clean runs produced the same tarball sha256 $sha_a"
  pass "$name" "dirty tree, non-ELF and wrong-ELF-machine binaries refused; two clean packaging runs produced the same sha256"
}

case_no_toolchain
if [[ "$MODE" == "all" ]]; then
  case_bare_env
  case_rejected_inputs
  case_retention
  case_rollback
  case_source_parity
  case_bind_assignment
  case_packaging
fi

if [[ -n "$REAL_BASELINE" ]]; then
  if [[ "$(real_snapshot)" != "$REAL_BASELINE" ]]; then
    printf 'FAIL isolation: the real %s or %s changed during the run\n' "$REAL_SVC" "$REAL_UNIT" >&2
    exit 1
  fi
  note "real ${REAL_SVC} and ${REAL_UNIT} are byte-identical before and after the run"
fi

printf 'SUMMARY: %d passed, 0 failed\n' "$PASSES"
